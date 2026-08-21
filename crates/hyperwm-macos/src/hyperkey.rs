//! Hyperkey watcher (architecture.md §2.2): a `CGEventTap` at
//! `kCGHIDEventTap` watching exactly one configured keycode as a plain
//! down/up flag -- key down means hyper-active becomes true, key up means
//! it becomes false. Deliberately no hold-duration timing and no state
//! machine: each event is handled independently, nothing is remembered
//! between calls.
//!
//! While hyper-active, every other key event is intercepted here and
//! reported to the caller instead of reaching the focused application
//! (architecture.md §2.2) -- this module owns the swallow/pass-through
//! decision (it's the only thing that can, since only the tap callback
//! gets to return a `CallbackResult`), but the Event Router (build unit
//! 5, in hyperwm-daemon) owns keybind lookup: this module has no notion of
//! `hyperwm_config::Keybind` or actions, it just reports raw
//! keycode/shift-flag pairs.
//!
//! [`install`] attaches the tap to the *current* thread's `CFRunLoop` and
//! returns immediately with a guard, rather than blocking -- unlike the
//! rest of build unit 3, which had this module block forever on its own
//! run loop as the only source running. Build unit 5's daemon also needs
//! to pump `AXObserver` sources (unit 4) and an `NSWorkspace` notification
//! (this crate's `workspace` module) on that same run loop, so ownership
//! of "run the loop" moved to the daemon's `main`, which adds every
//! source first and then makes the one blocking call.

use std::cell::Cell;
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
/// diagnosis before calling [`install`].
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

/// A key event while hyper-active, reported instead of being forwarded to
/// the focused application. `shift` is the physical Shift key's live state
/// (`hyperwm_config::Keybind`'s `shift` modifier), not a separate watched
/// keycode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HyperKeyEvent {
    pub keycode: CGKeyCode,
    pub shift: bool,
}

/// Keeps the `CGEventTap` alive and enabled for as long as this is held;
/// dropping it disables and releases the tap (via `CGEventTap`'s own
/// `Drop`).
pub struct HyperkeyWatcher {
    _tap: CGEventTap<'static>,
}

/// Creates the event tap watching `watch_keycode` and attaches it to the
/// *current* thread's `CFRunLoop` in `kCFRunLoopCommonModes`, enabled
/// immediately. Does not block -- the caller is responsible for running
/// that run loop (e.g. `CFRunLoop::run_current()`), typically after
/// attaching other sources too.
///
/// `on_active_changed(true)` fires on key down of `watch_keycode`,
/// `on_active_changed(false)` on key up. `on_key` fires for every *other*
/// key-down event while hyper-active is true; that event (and every other
/// key event, up or down, while hyper-active) is swallowed rather than
/// forwarded, per architecture.md §2.2 -- `on_key`'s return value doesn't
/// control this, there is no "unbound key" pass-through case.
///
/// # Errors
///
/// Returns `Err` if the tap could not be created, which in practice means
/// Input Monitoring permission is missing. Check
/// [`crate::permissions::check`] first so that case has already been
/// explained to the user.
pub fn install(
    watch_keycode: CGKeyCode,
    on_active_changed: impl Fn(bool) + Send + 'static,
    on_key: impl Fn(HyperKeyEvent) + Send + 'static,
) -> Result<HyperkeyWatcher, TapCreationError> {
    let watch_keycode = i64::from(watch_keycode);
    let hyper_active = Cell::new(false);

    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::Default,
        vec![CGEventType::KeyDown, CGEventType::KeyUp],
        move |_proxy, event_type, event| {
            let keycode = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE);
            if keycode == watch_keycode {
                match event_type {
                    CGEventType::KeyDown => {
                        hyper_active.set(true);
                        on_active_changed(true);
                    }
                    CGEventType::KeyUp => {
                        hyper_active.set(false);
                        on_active_changed(false);
                    }
                    _ => {}
                }
                return CallbackResult::Drop;
            }

            if hyper_active.get() {
                if matches!(event_type, CGEventType::KeyDown) {
                    let shift = event
                        .get_flags()
                        .contains(core_graphics::event::CGEventFlags::CGEventFlagShift);
                    on_key(HyperKeyEvent {
                        keycode: keycode as CGKeyCode,
                        shift,
                    });
                }
                return CallbackResult::Drop;
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

    Ok(HyperkeyWatcher { _tap: tap })
}
