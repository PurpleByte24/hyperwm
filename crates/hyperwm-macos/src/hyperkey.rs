//! Hyperkey watcher (architecture.md §2.2): a `CGEventTap` at
//! `kCGHIDEventTap` watching exactly one configured keycode as a plain
//! down/up flag -- key down means hyper-active becomes true, key up means
//! it becomes false. Deliberately no hold-duration timing and no state
//! machine: each event is handled independently, nothing is remembered
//! between calls.
//!
//! This module only watches and reports. Swallowing non-matching key
//! events while hyper-active, and dispatching them to the Event Router, is
//! wired up starting in build unit 5 -- every event is passed through
//! unchanged here.

use std::fmt;

use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
use core_graphics::event::{
    CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
    CallbackResult, EventField,
};

use crate::keycode::CGKeyCode;

/// The `CGEventTap` could not be created. In practice this means Input
/// Monitoring permission is missing or was revoked after startup --
/// [`crate::permissions::check`] should be used to give the user a clearer
/// diagnosis before calling [`watch`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TapCreationError;

impl fmt::Display for TapCreationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to create the CGEventTap (Input Monitoring permission missing?)"
        )
    }
}

impl std::error::Error for TapCreationError {}

/// Creates the event tap watching `watch_keycode`, enables it on the
/// current thread's `CFRunLoop`, and then runs that run loop forever --
/// this call does not return under normal operation.
///
/// `on_change(true)` fires on key down of `watch_keycode`, `on_change(false)`
/// on key up. Every event (matching or not) is passed through unmodified;
/// this build unit only observes.
///
/// # Errors
///
/// Returns `Err` if the tap could not be created, which in practice means
/// Input Monitoring permission is missing. Check
/// [`crate::permissions::check`] first so that case has already been
/// explained to the user.
pub fn watch(
    watch_keycode: CGKeyCode,
    on_change: impl Fn(bool) + Send + 'static,
) -> Result<(), TapCreationError> {
    let watch_keycode = i64::from(watch_keycode);

    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        vec![CGEventType::KeyDown, CGEventType::KeyUp],
        move |_proxy, event_type, event| {
            let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
            if keycode == watch_keycode {
                match event_type {
                    CGEventType::KeyDown => on_change(true),
                    CGEventType::KeyUp => on_change(false),
                    _ => {}
                }
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
    CFRunLoop::run_current();
    Ok(())
}
