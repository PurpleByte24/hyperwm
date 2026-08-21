//! macOS integration. Build unit 3: `CGEventTap` hyperkey watcher +
//! Accessibility/Input Monitoring permission checks. Build unit 4 (this):
//! AX API window manipulation, in [`ax`].

pub mod ax;
pub mod hyperkey;
pub mod keycode;
pub mod permissions;
