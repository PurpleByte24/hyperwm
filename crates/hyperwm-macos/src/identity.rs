//! App identity lookup (bundle ID + display name) for a pid, so the daemon
//! can match a window's owning app against `floating.always_float_apps`
//! (architecture.md §3.9), which accepts either form.

use accessibility_sys::pid_t;
use objc2_app_kit::NSRunningApplication;

/// A running app's bundle ID and/or display name, whichever `pid`'s
/// `NSRunningApplication` reports (either can be absent, e.g. for a
/// process with no bundle).
#[derive(Debug, Clone, Default)]
pub struct AppIdentity {
    pub bundle_id: Option<String>,
    pub name: Option<String>,
}

impl AppIdentity {
    /// Whether `pattern` (from `floating.always_float_apps`) matches this
    /// app's bundle ID or name, case-insensitively.
    #[must_use]
    pub fn matches(&self, pattern: &str) -> bool {
        self.bundle_id
            .as_deref()
            .is_some_and(|id| id.eq_ignore_ascii_case(pattern))
            || self
                .name
                .as_deref()
                .is_some_and(|name| name.eq_ignore_ascii_case(pattern))
    }
}

/// Looks up `pid`'s identity via `NSRunningApplication`. `None` if no
/// running app has that pid.
#[must_use]
pub fn identity_for_pid(pid: pid_t) -> Option<AppIdentity> {
    let app = NSRunningApplication::runningApplicationWithProcessIdentifier(pid)?;
    Some(AppIdentity {
        bundle_id: app.bundleIdentifier().map(|s| s.to_string()),
        name: app.localizedName().map(|s| s.to_string()),
    })
}
