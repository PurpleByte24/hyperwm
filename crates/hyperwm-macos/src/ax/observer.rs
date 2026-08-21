//! `AXObserver` wrapper: watches one application's AX notifications (e.g.
//! `kAXWindowCreatedNotification` on the app element,
//! `kAXWindowMovedNotification`/`kAXWindowResizedNotification`/
//! `kAXUIElementDestroyedNotification` on a specific window element) and
//! invokes a caller-supplied callback when they fire.
//!
//! Build unit 4's job is only to prove the callbacks fire and log
//! something useful -- turning these into `hyperwm-core` tree updates
//! (so externally-dragged/closed windows don't drift out of sync with the
//! tree) is build unit 5.

use std::ffi::c_void;
use std::fmt;
use std::ptr;

use accessibility_sys::{
    kAXErrorSuccess, pid_t, AXError, AXObserverAddNotification, AXObserverCreate,
    AXObserverGetRunLoopSource, AXObserverRef, AXUIElementRef,
};
use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop, CFRunLoopSource};
use core_foundation::string::{CFString, CFStringRef};

use super::AXUIElement;

/// A notification fired for `element`, e.g. `kAXWindowMovedNotification`.
/// Passed to the callback given to [`WindowObserver::new`].
pub struct AXNotification {
    pub name: String,
    pub element: AXUIElement,
}

type Callback = Box<dyn FnMut(AXNotification) + 'static>;

/// The `AXObserver` could not be created -- in practice this means
/// Accessibility permission is missing (see
/// [`crate::permissions::check`]) or `pid` doesn't belong to a running
/// process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObserverCreationError(pub AXError);

impl fmt::Display for ObserverCreationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "failed to create the AXObserver ({})",
            accessibility_sys::error_string(self.0)
        )
    }
}

impl std::error::Error for ObserverCreationError {}

/// Watches AX notifications for one application (by pid), invoking a
/// callback on each one. Register interest in specific notifications via
/// [`WindowObserver::watch`], then call [`WindowObserver::run`] to start
/// receiving them (blocks forever, like [`crate::hyperkey::watch`]).
pub struct WindowObserver {
    observer: AXObserverRef,
    // Leaked and reclaimed in `Drop`; `AXObserverAddNotification`'s
    // `refcon` is a raw pointer, so the callback can't live as a normal
    // owned field the trampoline could borrow from.
    callback: *mut Callback,
}

impl WindowObserver {
    /// Creates an observer for `pid`. Does not watch anything yet --
    /// call [`WindowObserver::watch`] for each element/notification pair
    /// of interest.
    pub fn new(
        pid: pid_t,
        callback: impl FnMut(AXNotification) + 'static,
    ) -> Result<Self, ObserverCreationError> {
        let callback: *mut Callback = Box::into_raw(Box::new(Box::new(callback)));

        let mut observer: AXObserverRef = ptr::null_mut();
        let err = unsafe { AXObserverCreate(pid, trampoline, &mut observer) };
        if err != kAXErrorSuccess {
            // Safety: `callback` was just allocated above and nothing else
            // has taken ownership of it yet.
            drop(unsafe { Box::from_raw(callback) });
            return Err(ObserverCreationError(err));
        }

        Ok(Self { observer, callback })
    }

    /// Registers this observer for `notification` on `element`. Per AX
    /// semantics, `element` should be the *application* element for
    /// `kAXWindowCreatedNotification`, and the specific *window* element
    /// for `kAXWindowMovedNotification`/`kAXWindowResizedNotification`/
    /// `kAXUIElementDestroyedNotification`.
    pub fn watch(&self, element: &AXUIElement, notification: &str) -> Result<(), AXError> {
        let notification = CFString::new(notification);
        let err = unsafe {
            AXObserverAddNotification(
                self.observer,
                element.as_concrete_TypeRef(),
                notification.as_concrete_TypeRef(),
                self.callback.cast::<c_void>(),
            )
        };
        if err == kAXErrorSuccess {
            Ok(())
        } else {
            Err(err)
        }
    }

    /// Adds this observer's run loop source to the *current* thread's
    /// `CFRunLoop` (in `kCFRunLoopCommonModes`, so it keeps firing
    /// alongside other sources sharing that loop -- e.g. hyperwm-daemon's
    /// `hyperkey` tap and other apps' `WindowObserver`s). Does not block;
    /// the caller runs the loop itself (e.g. `CFRunLoop::run_current()`)
    /// once every source it needs is attached.
    pub fn attach_to_current_runloop(&self) {
        let source_ref = unsafe { AXObserverGetRunLoopSource(self.observer) };
        let source = unsafe { CFRunLoopSource::wrap_under_get_rule(source_ref) };
        let run_loop = CFRunLoop::get_current();
        run_loop.add_source(&source, unsafe { kCFRunLoopCommonModes });
    }

    /// Convenience for standalone callers (e.g. this crate's manual
    /// verification examples) that only ever watch one observer: attaches
    /// to the current run loop, then blocks on it forever. Does not
    /// return under normal operation.
    pub fn run(&self) -> ! {
        self.attach_to_current_runloop();
        CFRunLoop::run_current();
        unreachable!("CFRunLoopRun does not return")
    }
}

impl Drop for WindowObserver {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.observer.cast::<c_void>() as CFTypeRef);
            drop(Box::from_raw(self.callback));
        }
    }
}

unsafe extern "C" fn trampoline(
    _observer: AXObserverRef,
    element: AXUIElementRef,
    notification: CFStringRef,
    refcon: *mut c_void,
) {
    // Safety: `refcon` is always the `callback` pointer we handed to
    // `AXObserverAddNotification` in `watch`, which stays valid for the
    // observer's lifetime (freed only in `Drop`, after which no more
    // callbacks can fire).
    let callback = unsafe { &mut *(refcon.cast::<Callback>()) };
    let name = unsafe { CFString::wrap_under_get_rule(notification) }.to_string();
    // Safety: `element` is a "get rule" reference per AXObserver's
    // callback contract -- we don't own it going in, so retain our own
    // reference rather than assuming ownership.
    let element = unsafe { AXUIElement::wrap_under_get_rule(element) };
    callback(AXNotification { name, element });
}
