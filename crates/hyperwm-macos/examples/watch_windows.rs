//! Manual verification harness for the `AXObserver` half of build unit 4:
//! proves window creation/destruction/move/resize notifications actually
//! fire and can be logged, ahead of unit 5 wiring them into
//! `hyperwm-core`'s tree.
//!
//! Usage:
//!
//! ```text
//! cargo run -p hyperwm-macos --example watch_windows -- "<app name>"
//! ```
//!
//! Watches that app's focused window for move/resize/destroy, and the
//! app itself for new-window creation. While this runs, drag/resize the
//! window, open a new window in the app, or close it, and watch the log
//! lines appear. Ctrl-C to stop.

use std::process::ExitCode;

use accessibility_sys::{
    error_string, kAXFocusedWindowAttribute, kAXTitleAttribute, kAXUIElementDestroyedNotification,
    kAXWindowCreatedNotification, kAXWindowMovedNotification, kAXWindowResizedNotification,
};
use hyperwm_macos::ax::{self, AXUIElement, WindowObserver};
use hyperwm_macos::permissions;

fn main() -> ExitCode {
    let Some(app_name) = std::env::args().nth(1) else {
        eprintln!("usage: watch_windows <app name>");
        return ExitCode::FAILURE;
    };

    let status = permissions::check();
    if !status.accessibility {
        eprintln!(
            "missing Accessibility permission; grant it, then re-run:\n{}",
            status.instructions()
        );
        return ExitCode::FAILURE;
    }

    let Some(pid) = ax::pid_for_app_name(&app_name) else {
        eprintln!("no on-screen window is owned by an app named \"{app_name}\"");
        return ExitCode::FAILURE;
    };
    println!("watching \"{app_name}\" (pid {pid}) -- drag/resize/close its focused window, or \
               open a new one, to see notifications below; Ctrl-C to stop");

    let app = AXUIElement::application(pid);

    let observer = match WindowObserver::new(pid, |event| {
        let title = event
            .element
            .string_attribute(kAXTitleAttribute)
            .unwrap_or_else(|_| "<unavailable -- window may already be gone>".to_string());
        println!("[{}] {title:?}", event.name);
    }) {
        Ok(observer) => observer,
        Err(err) => {
            eprintln!("failed to create AXObserver: {err}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(err) = observer.watch(&app, kAXWindowCreatedNotification) {
        eprintln!(
            "couldn't watch for new windows: {}",
            error_string(err)
        );
    }

    match app.element_attribute(kAXFocusedWindowAttribute) {
        Ok(window) => {
            for notification in [
                kAXWindowMovedNotification,
                kAXWindowResizedNotification,
                kAXUIElementDestroyedNotification,
            ] {
                if let Err(err) = observer.watch(&window, notification) {
                    eprintln!(
                        "couldn't watch {notification} on the focused window: {}",
                        error_string(err)
                    );
                }
            }
        }
        Err(err) => {
            eprintln!(
                "couldn't get the app's focused window to watch move/resize/destroy on it: {}",
                error_string(err)
            );
        }
    }

    observer.run();
}
