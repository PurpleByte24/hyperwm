//! Startup permission checks (architecture.md §2.3). [`check`] uses the
//! OS-prompting variant of each API, so a fresh install (permission never
//! granted or denied before) gets a native macOS "Allow" dialog the first
//! time it runs. Once a permission has been denied in a past session,
//! macOS will not prompt again -- there's no programmatic way to
//! re-request after a denial, only the user manually re-enabling it in
//! System Settings > Privacy & Security. [`PermissionStatus::instructions`]
//! is the fallback for that case (and for the "user clicked Deny just
//! now" case, which looks the same to us). Either way, the daemon must
//! never crash or crash-loop when a permission is missing; it reports and
//! lets the caller decide whether to exit or wait.

use core_foundation::base::TCFType;
use core_foundation::boolean::CFBoolean;
use core_foundation::dictionary::CFDictionary;
use core_foundation::string::CFString;

#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    /// Requests Input Monitoring access for this process, triggering the
    /// native OS prompt the first time it's called if the permission has
    /// never been granted or denied before. Returns the resulting (or
    /// already-current) access state. Public API since macOS 10.15.
    fn CGRequestListenEventAccess() -> bool;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PermissionStatus {
    pub accessibility: bool,
    pub input_monitoring: bool,
}

impl PermissionStatus {
    #[must_use]
    pub fn all_granted(&self) -> bool {
        self.accessibility && self.input_monitoring
    }

    /// Human-readable, actionable instructions for whichever permissions
    /// are missing. Empty string if everything is granted.
    #[must_use]
    pub fn instructions(&self) -> String {
        let mut lines = Vec::new();
        if !self.accessibility {
            lines.push(
                "  - Accessibility: System Settings > Privacy & Security > Accessibility, \
                 then enable it for this app (add it with \"+\" if it isn't listed)."
                    .to_string(),
            );
        }
        if !self.input_monitoring {
            lines.push(
                "  - Input Monitoring: System Settings > Privacy & Security > Input \
                 Monitoring, then enable it for this app (add it with \"+\" if it isn't \
                 listed)."
                    .to_string(),
            );
        }
        lines.join("\n")
    }
}

/// Checks both permissions required by architecture.md §2.3, using each
/// API's OS-prompting variant. On a fresh install this surfaces the native
/// system dialog; if a permission was already denied (or the user denies
/// it in the dialog just shown), this simply returns `false` for it like a
/// silent check would -- there is no blocking or looping here, the caller
/// decides what to do with the result.
#[must_use]
pub fn check() -> PermissionStatus {
    let accessibility = ax_is_trusted_prompting();
    let input_monitoring = unsafe { CGRequestListenEventAccess() };
    PermissionStatus {
        accessibility,
        input_monitoring,
    }
}

/// `AXIsProcessTrustedWithOptions` with `kAXTrustedCheckOptionPrompt` set,
/// which triggers the native "App wants to control this computer using
/// Accessibility features" dialog the first time it's called if this
/// process hasn't been granted or denied yet.
fn ax_is_trusted_prompting() -> bool {
    unsafe {
        let key = CFString::wrap_under_get_rule(accessibility_sys::kAXTrustedCheckOptionPrompt);
        let options = CFDictionary::from_CFType_pairs(&[(key, CFBoolean::true_value())]);
        accessibility_sys::AXIsProcessTrustedWithOptions(options.as_concrete_TypeRef())
    }
}
