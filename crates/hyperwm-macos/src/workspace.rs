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

use std::ptr::NonNull;

use accessibility_sys::pid_t;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_app_kit::{NSRunningApplication, NSWorkspace, NSWorkspaceApplicationKey};
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

/// pids of every currently running application (per `NSWorkspace`, which
/// -- unlike raw process enumeration -- is already scoped to actual
/// user-facing applications, not arbitrary background processes),
/// regardless of whether any of them currently has an open window. See
/// the module doc for why daemon startup needs this instead of (or
/// alongside) `ax::all_app_pids`.
#[must_use]
pub fn all_running_app_pids() -> Vec<pid_t> {
    // `.to_vec()`, not `.iter()`: the latter needs the "NSEnumerator"
    // feature, which nothing else here pulls in (see screen.rs's
    // `to_vec()` use for the same reason).
    NSWorkspace::sharedWorkspace()
        .runningApplications()
        .to_vec()
        .into_iter()
        .map(|app| app.processIdentifier())
        .collect()
}
