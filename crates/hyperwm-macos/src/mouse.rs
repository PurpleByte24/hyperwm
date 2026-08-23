//! Left-mouse-button state watcher: a `CGEventTap` at `kCGHIDEventTap`,
//! `ListenOnly` (never intercepts or modifies events -- purely
//! observational, so it can never interfere with ordinary mouse use
//! anywhere on the system), watching `LeftMouseDown`/`LeftMouseUp` as a
//! plain down/up flag.
//!
//! Used by hyperwm-daemon's drift correction (architecture.md §3.10) to
//! pause corrective writes while the user is actively dragging or
//! resizing a window by hand, then sweep for drift once the button is
//! released. Deliberately no drag-distance thresholds, no per-window
//! tracking, no state machine beyond the flag itself -- same "plain
//! modifier, no hold-duration timing" philosophy as `hyperkey.rs`
//! (architecture.md §2.2), just applied to the mouse button instead of
//! the hyperkey.

use std::fmt;

use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use core_graphics::event::{
    CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    CallbackResult,
};

/// The `CGEventTap` could not be created -- in practice, missing Input
/// Monitoring permission (same requirement `hyperkey::install` has).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TapCreationError;

impl fmt::Display for TapCreationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to create the mouse-button CGEventTap (Input Monitoring permission missing?)"
        )
    }
}

impl std::error::Error for TapCreationError {}

/// Keeps the `CGEventTap` alive and enabled for as long as this is held;
/// dropping it disables and releases the tap (via `CGEventTap`'s own
/// `Drop`).
pub struct MouseButtonWatcher {
    _tap: CGEventTap<'static>,
}

/// Creates the listen-only tap and attaches it to the *current* thread's
/// `CFRunLoop` in `kCFRunLoopCommonModes`, enabled immediately. Does not
/// block -- same calling convention as `hyperkey::install`.
///
/// `on_down_changed(true)` fires on left-mouse-down, `on_down_changed(false)`
/// on left-mouse-up. `ListenOnly` means every event still reaches its
/// normal destination unmodified -- this tap can only observe, never
/// swallow or alter a click/drag.
///
/// # Errors
///
/// Returns `Err` if the tap could not be created, which in practice means
/// Input Monitoring permission is missing.
pub fn install(
    on_down_changed: impl Fn(bool) + Send + 'static,
) -> Result<MouseButtonWatcher, TapCreationError> {
    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        vec![CGEventType::LeftMouseDown, CGEventType::LeftMouseUp],
        move |_proxy, event_type, _event| {
            match event_type {
                CGEventType::LeftMouseDown => on_down_changed(true),
                CGEventType::LeftMouseUp => on_down_changed(false),
                _ => {}
            }
            CallbackResult::Keep
        },
    )
    .map_err(|()| TapCreationError)?;

    let source = tap
        .mach_port()
        .create_runloop_source(0)
        .map_err(|()| TapCreationError)?;
    let run_loop = CFRunLoop::get_current();
    run_loop.add_source(&source, unsafe { kCFRunLoopCommonModes });
    tap.enable();

    Ok(MouseButtonWatcher { _tap: tap })
}
