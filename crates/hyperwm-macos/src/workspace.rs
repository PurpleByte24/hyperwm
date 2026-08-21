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
