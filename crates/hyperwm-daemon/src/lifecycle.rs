//! Bootstraps AX observation for one app (architecture.md §3.10's "window
//! lifecycle is always observed", for every app the daemon knows about).
//!
//! This is split out from `state.rs` because creating a *new* `AXObserver`
//! needs a callback that can call back into `DaemonState` later, which
//! means it needs a clone of the shared `Rc<RefCell<DaemonState>>` up
//! front -- unlike everything in `state.rs`, which only ever uses an
//! observer already created here.
//!
//! [`adopt_app`] is called for every pid owning an on-screen window at
//! daemon startup, and again from `hyperwm_macos::workspace`'s
//! newly-launched-app notification. Both cases ensure an `AXObserver`
//! exists for the pid (creating one the first time), then run every one
//! of that app's *current* windows through [`DaemonState`] if hyperwm
//! hasn't seen them yet -- catching up on existing windows on *every*
//! call, not just the startup one, is what closes the race where an
//! app's first window is already open by the time its launch
//! notification arrives and this function gets to attach a
//! `kAXWindowCreatedNotification` observer.
//!
//! The two call sites differ in [`AdoptionPolicy`]: a window discovered
//! because its app just launched is genuinely new activity and gets
//! tiled or floated per architecture.md §3.2/§3.3/§3.9's normal rule
//! (`ClassifyForTiling`); a window discovered at the one-time startup
//! scan already existed before the daemon did, so it's registered but
//! left floating (`FloatOnly`) -- otherwise whatever's on screen when the
//! daemon starts (e.g. the terminal it was launched from) would silently
//! occupy `max_tiled_windows` slots the user never asked to tile.

use std::cell::RefCell;
use std::rc::Rc;

use accessibility_sys::{
    kAXFocusedWindowChangedNotification, kAXUIElementDestroyedNotification,
    kAXWindowCreatedNotification, kAXWindowMovedNotification, kAXWindowResizedNotification,
    kAXWindowsAttribute, pid_t,
};
use hyperwm_macos::ax::{AXNotification, AXUIElement, WindowObserver};

use crate::state::DaemonState;

/// How to treat the windows [`adopt_app`] finds already open for `pid`
/// (see this module's doc comment for the rationale).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AdoptionPolicy {
    /// Run each window through the normal tile-or-float insertion rule
    /// (architecture.md §3.2/§3.3/§3.9) -- for an app that just launched
    /// during the daemon's own run.
    ClassifyForTiling,
    /// Register each window for lifecycle tracking but leave it floating,
    /// without spending a `max_tiled_windows` slot -- for the app
    /// already-running at daemon startup.
    FloatOnly,
}

pub fn adopt_app(state: &Rc<RefCell<DaemonState>>, pid: pid_t, policy: AdoptionPolicy) {
    let already_watched = state.borrow().app_observers.contains_key(&pid);
    if !already_watched {
        let callback_state = Rc::clone(state);
        match WindowObserver::new(pid, move |event| dispatch_notification(&callback_state, event))
        {
            Ok(observer) => {
                let app_element = AXUIElement::application(pid);
                if let Err(err) = observer.watch(&app_element, kAXWindowCreatedNotification) {
                    eprintln!(
                        "hyperwm-daemon: couldn't watch window creation for pid {pid}: {}",
                        accessibility_sys::error_string(err)
                    );
                }
                // architecture.md §3.3 step 1's insertion target depends
                // on which *existing* tiled window was last focused --
                // DaemonState only ever learns that during a hyper-key
                // action otherwise, so without this, opening several
                // windows in a row with no hyper-key press in between
                // left the tree's notion of "focused" stuck whenever it
                // was last set (or never set at all), and every insertion
                // fell back to the tree's own last-resort default
                // (lowest WindowId) instead of the actually-focused
                // window (confirmed in manual testing: it kept splitting
                // "the first window detected"'s leaf regardless of which
                // window was actually focused). Watching this keeps
                // DaemonState::handle_focus_changed feeding real focus
                // changes to the tree continuously, not just at the
                // moment a new window happens to be inserted.
                if let Err(err) = observer.watch(&app_element, kAXFocusedWindowChangedNotification)
                {
                    eprintln!(
                        "hyperwm-daemon: couldn't watch focus changes for pid {pid}: {}",
                        accessibility_sys::error_string(err)
                    );
                }
                observer.attach_to_current_runloop();
                state.borrow_mut().app_observers.insert(pid, observer);
            }
            Err(err) => {
                eprintln!("hyperwm-daemon: couldn't create AXObserver for pid {pid}: {err} \
                    (its windows won't be tiling-managed until it's re-checked)");
                return;
            }
        }
    }

    let app_element = AXUIElement::application(pid);
    let Ok(windows) = app_element.element_array_attribute(kAXWindowsAttribute) else {
        return;
    };
    for window in windows {
        // Both branches no-op on a window already registered, so no need
        // to pre-check membership here too.
        let mut state = state.borrow_mut();
        match policy {
            AdoptionPolicy::ClassifyForTiling => state.handle_new_window(pid, window),
            AdoptionPolicy::FloatOnly => state.adopt_preexisting_window(pid, window),
        }
    }
}

fn dispatch_notification(state: &Rc<RefCell<DaemonState>>, event: AXNotification) {
    let name = event.name.as_str();
    if name == kAXWindowCreatedNotification {
        if let Ok(pid) = event.element.pid() {
            state.borrow_mut().handle_new_window(pid, event.element);
        }
    } else if name == kAXUIElementDestroyedNotification {
        state.borrow_mut().handle_window_destroyed(&event.element);
    } else if name == kAXWindowMovedNotification || name == kAXWindowResizedNotification {
        state.borrow_mut().handle_drift(&event.element);
    } else if name == kAXFocusedWindowChangedNotification {
        state.borrow_mut().handle_focus_changed(&event.element);
    }
}
