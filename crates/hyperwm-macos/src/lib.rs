//! macOS integration. Build unit 3: `CGEventTap` hyperkey watcher +
//! Accessibility/Input Monitoring permission checks. Build unit 4 (this):
//! AX API window manipulation, in [`ax`].
//!
//! # Manual verification (unit 4)
//!
//! This crate's AX code depends on a real window server and a real
//! Accessibility permission grant, so it's not practical to exercise from
//! `cargo test`. Two example binaries drive it against real, running
//! apps instead:
//!
//! - `cargo run -p hyperwm-macos --example move_window -- "<app name>"` --
//!   moves and resizes that app's focused window to a hardcoded rect,
//!   prints the position/size read back, then restores the original
//!   geometry. Confirms `AXUIElementCopyAttributeValue`/
//!   `AXUIElementSetAttributeValue` round-trip position and size
//!   correctly.
//! - `cargo run -p hyperwm-macos --example watch_windows -- "<app name>"`
//!   -- watches that app for new windows and its focused window for
//!   move/resize/destroy, logging each `AXObserver` notification as it
//!   fires. Confirms observer callbacks actually arrive.
//!
//! Both need Accessibility permission granted to whatever process runs
//! `cargo run` (architecture.md §2.3); the first run triggers the system
//! prompt.

pub mod ax;
pub mod hyperkey;
pub mod keycode;
pub mod permissions;
