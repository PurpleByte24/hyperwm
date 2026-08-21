//! macOS integration. Build unit 3 (this): `CGEventTap` hyperkey watcher +
//! Accessibility/Input Monitoring permission checks. AX window
//! manipulation lands in build unit 4 (see CLAUDE.md build order).

pub mod hyperkey;
pub mod keycode;
pub mod permissions;
