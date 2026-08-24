//! New-app-launch detection, so the daemon can attach an `AXObserver` to an
//! app that didn't exist yet at startup (architecture.md §3.10's window
//! lifecycle observation must cover windows from apps launched after the
//! daemon starts, not just ones already running).
//!
//! `AXObserver` is scoped to an already-known pid (`AXObserverCreate`), so
//! there's nothing to attach a `kAXWindowCreatedNotification` watcher to
//! until the app exists. `NSWorkspace`'s launch notification is the public,
//! event-driven way to learn that -- confirmed with the user alongside
//! `screen.rs`'s NSScreen use, since it's the same "AppKit, not literally
//! AX + CGEventTap" tradeoff.
//!
//! [`all_running_app_pids`] covers a gap the launch notification and
//! `ax::all_app_pids` (`CGWindowListCopyWindowInfo`, scoped to apps with an
//! on-screen window right now) leave open: an app that's already running
//! but has zero windows open at daemon startup (Finder with no Finder
//! windows open is the common case -- it's essentially always running)
//! never appears in either -- not `ax::all_app_pids` (no window to be
//! found), not the launch notification (it didn't just launch). Its
//! *first* window, opened any time after, would then never be observed at
//! all: nothing ever created an `AXObserver` for its pid. Enumerating
//! every running application (not just ones with windows) at startup and
//! adopting each closes this.
//!
//! The notification block runs on whichever thread posts it; in practice
//! that's the main thread's run loop in the common modes, which is also
//! where this daemon pumps its `CGEventTap` and `AXObserver` sources, so no
//! extra run-loop wiring is needed here (unlike `hyperkey`/`ax::observer`,
//! which each own a `CFRunLoopSource` that must be explicitly attached).

use std::ffi::c_void;
use std::ptr::NonNull;

use accessibility_sys::pid_t;
use block2::RcBlock;
use core_foundation::base::TCFType;
use core_foundation::date::CFDate;
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop, CFRunLoopTimer};
use core_foundation_sys::runloop::CFRunLoopTimerInvalidate;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{
    NSApplicationActivationPolicy, NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey,
};
use objc2_foundation::{NSNotification, NSObjectProtocol};

/// Keeps the `NSWorkspace` launch observer registered for as long as this
/// is alive; drops the registration on `Drop`.
pub struct AppLaunchWatcher {
    observer: Retained<ProtocolObject<dyn NSObjectProtocol>>,
}

impl Drop for AppLaunchWatcher {
    fn drop(&mut self) {
        unsafe {
            NSWorkspace::sharedWorkspace()
                .notificationCenter()
                .removeObserver(self.observer.as_ref());
        }
    }
}

/// Registers `on_launch` to fire (with the new process's pid) whenever an
/// application launches, from now until the returned [`AppLaunchWatcher`]
/// is dropped.
#[must_use]
pub fn watch_app_launches(on_launch: impl Fn(pid_t) + 'static) -> AppLaunchWatcher {
    let block = RcBlock::new(move |notification: NonNull<NSNotification>| {
        let notification = unsafe { notification.as_ref() };
        let Some(user_info) = notification.userInfo() else {
            return;
        };
        let key: &objc2_foundation::NSString = unsafe { NSWorkspaceApplicationKey };
        let Some(app) = user_info.objectForKey(key.as_ref()) else {
            return;
        };
        // Safety: `NSWorkspaceApplicationKey`'s documented value type for
        // this notification is `NSRunningApplication`.
        let app: Retained<NSRunningApplication> = unsafe { Retained::cast_unchecked(app) };
        on_launch(app.processIdentifier());
    });

    let center = NSWorkspace::sharedWorkspace().notificationCenter();
    let observer = unsafe {
        center.addObserverForName_object_queue_usingBlock(
            Some(objc2_app_kit::NSWorkspaceDidLaunchApplicationNotification),
            None,
            None,
            &block,
        )
    };
    AppLaunchWatcher { observer }
}

/// How often [`watch_space_changes`] re-checks the on-screen window set.
/// Small enough that a Space switch feels immediate to a human; large
/// enough not to matter as a background cost.
const SPACE_POLL_INTERVAL_SECS: f64 = 0.5;

type PollCallback = Box<dyn FnMut() + 'static>;

/// Keeps the Space-change poll timer installed on the run loop for as long
/// as this is alive; invalidates the timer and reclaims the boxed callback
/// on `Drop`. See [`watch_space_changes`] for why this polls instead of
/// observing `NSWorkspaceActiveSpaceDidChangeNotification` directly.
pub struct SpaceChangeWatcher {
    timer: CFRunLoopTimer,
    // Leaked and reclaimed in `Drop`; `CFRunLoopTimerCreate`'s `info` is a
    // raw pointer, so the callback can't live as a normal owned field the
    // trampoline could borrow from -- same reasoning as
    // `ax::observer::WindowObserver`'s `callback` field.
    callback: *mut PollCallback,
}

impl Drop for SpaceChangeWatcher {
    fn drop(&mut self) {
        unsafe {
            CFRunLoopTimerInvalidate(self.timer.as_concrete_TypeRef());
            drop(Box::from_raw(self.callback));
        }
    }
}

/// Polls for a Space switch by calling `on_change` (with no arguments)
/// every [`SPACE_POLL_INTERVAL_SECS`], from now until the returned
/// [`SpaceChangeWatcher`] is dropped. Unlike [`watch_app_launches`],
/// `on_change` fires on every tick regardless of whether the Space
/// actually changed -- `DaemonState::handle_space_change` already
/// recognizes an unchanged on-screen window set as a no-op cheaply, so
/// there's no need to duplicate that comparison here.
///
/// architecture.md §3.11 originally specified
/// `NSWorkspace.activeSpaceDidChangeNotification` for this (event-driven,
/// no polling latency). Confirmed twice independently -- once by an
/// earlier session, once by directly registering this notification the
/// same way [`watch_app_launches`] registers
/// `NSWorkspaceDidLaunchApplicationNotification` (which *does* fire
/// reliably in this same process) and testing live: switching Spaces
/// several times produced zero callback invocations. So this specific
/// notification does not reach this process, for reasons that don't
/// generalize from the launch notification working -- not a bundling
/// theory we've since ruled out as the cause, just an empirical dead end.
/// Polling needs no bundling and is fully public API, so it's used here
/// instead, at the cost of up to [`SPACE_POLL_INTERVAL_SECS`] of latency
/// recognizing a switch (never a correctness issue -- `handle_space_change`
/// never writes to AX either way, so a slightly-late passive adopt/no-op
/// is harmless).
#[must_use]
pub fn watch_space_changes(on_change: impl FnMut() + 'static) -> SpaceChangeWatcher {
    let callback: *mut PollCallback = Box::into_raw(Box::new(Box::new(on_change)));

    // `CFRunLoopTimerCreate` copies this struct itself, but not what
    // `info` points to -- with no `retain`/`release` callbacks registered,
    // CF never manages `callback`'s lifetime, so `SpaceChangeWatcher::drop`
    // must (see its doc comment).
    let mut context = core_foundation_sys::runloop::CFRunLoopTimerContext {
        version: 0,
        info: callback.cast(),
        retain: None,
        release: None,
        copyDescription: None,
    };

    let now = CFDate::now().abs_time();
    let timer = CFRunLoopTimer::new(
        now + SPACE_POLL_INTERVAL_SECS,
        SPACE_POLL_INTERVAL_SECS,
        0,
        0,
        poll_trampoline,
        &mut context,
    );
    let run_loop = CFRunLoop::get_current();
    run_loop.add_timer(&timer, unsafe { kCFRunLoopCommonModes });

    SpaceChangeWatcher { timer, callback }
}

extern "C" fn poll_trampoline(
    _timer: core_foundation_sys::runloop::CFRunLoopTimerRef,
    info: *mut c_void,
) {
    // Safety: `info` is always the `callback` pointer `watch_space_changes`
    // handed to `CFRunLoopTimer::new`, which stays valid for the timer's
    // lifetime (freed only in `SpaceChangeWatcher::drop`, after which the
    // timer has already been invalidated and can't fire again).
    let callback = unsafe { &mut *(info.cast::<PollCallback>()) };
    callback();
}

/// pids of every currently running application with a UI presence --
/// `NSApplicationActivationPolicy::Regular` (Dock icon, normal windows)
/// or `Accessory` (menu-bar-style, but can still show windows/panels) --
/// regardless of whether any of them currently has an open window. See
/// the module doc for why daemon startup needs this instead of (or
/// alongside) `ax::all_app_pids`.
///
/// Excludes `Prohibited` (pure background processes: XPC services, login
/// items, helper/agent processes) -- `NSWorkspace.runningApplications` is
/// already scoped to actual applications rather than arbitrary processes,
/// but still includes plenty of these with zero UI and no AX support at
/// all. Adopting them wastes an `AXObserverCreate` call each (observed in
/// manual testing: a couple dozen `kAXErrorCannotComplete`/
/// `kAXErrorAPIDisabled`/etc. lines at startup, one per such pid) for
/// something that can never have a window worth tiling.
#[must_use]
pub fn all_running_app_pids() -> Vec<pid_t> {
    // `.to_vec()`, not `.iter()`: the latter needs the "NSEnumerator"
    // feature, which nothing else here pulls in (see screen.rs's
    // `to_vec()` use for the same reason).
    NSWorkspace::sharedWorkspace()
        .runningApplications()
        .to_vec()
        .into_iter()
        .filter(|app| app.activationPolicy() != NSApplicationActivationPolicy::Prohibited)
        .map(|app| app.processIdentifier())
        .collect()
}
