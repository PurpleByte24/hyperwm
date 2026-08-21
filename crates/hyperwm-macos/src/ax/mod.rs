//! Accessibility (AX) API window manipulation -- CLAUDE.md build unit 4.
//! Enumerate an app's windows, read/write a window's position and size,
//! find the frontmost app's focused window, and observe window
//! creation/destruction/move/resize via `AXObserver`.
//!
//! Wiring this into `hyperwm-core`'s BSP tree (so keybinds actually move
//! real windows, and external drags/closes update the tree instead of
//! drifting out of sync) is build unit 5, not this module -- everything
//! here operates on a single `AXUIElement` at a time with no notion of
//! tiling.

mod app;
mod element;
mod observer;

pub use app::{all_app_pids, frontmost_app_pid, pid_for_app_name};
pub use element::AXUIElement;
pub use observer::{AXNotification, ObserverCreationError, WindowObserver};
