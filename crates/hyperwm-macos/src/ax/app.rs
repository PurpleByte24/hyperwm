//! Resolving a running application to a pid, so
//! [`crate::ax::AXUIElement::application`] has something to call
//! `AXUIElementCreateApplication` with. Neither architecture.md nor
//! CLAUDE.md's build order specifies how to do this; this module answers
//! it with `CGWindowListCopyWindowInfo`, which is public `CoreGraphics`
//! API (not the AX API itself, but not a private framework either) and
//! avoids needing an `NSWorkspace`/`AppKit` dependency just to find a pid.

use accessibility_sys::pid_t;
use core_foundation::array::CFArray;
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_graphics::window::{
    copy_window_info, kCGNullWindowID, kCGWindowBounds, kCGWindowLayer,
    kCGWindowListExcludeDesktopElements, kCGWindowListOptionOnScreenOnly, kCGWindowNumber,
    kCGWindowOwnerName, kCGWindowOwnerPID, CGWindowID,
};
use hyperwm_core::Rect;

struct NormalWindow {
    owner_pid: pid_t,
    owner_name: String,
}

/// Normal (layer 0 -- excludes menu bar items, the Dock, etc.) on-screen
/// windows, front-to-back: `CGWindowListCopyWindowInfo` with
/// `kCGWindowListOptionOnScreenOnly` documents this ordering, which is
/// what lets `frontmost_app_pid` just take the first entry.
fn normal_on_screen_windows() -> Vec<NormalWindow> {
    let Some(untyped) = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID,
    ) else {
        return Vec::new();
    };
    let dicts: CFArray<CFDictionary<CFString, CFType>> =
        unsafe { CFArray::wrap_under_get_rule(untyped.as_concrete_TypeRef() as *const _) };

    dicts
        .iter()
        .filter_map(|dict| {
            let layer = dict
                .find(unsafe { kCGWindowLayer })?
                .downcast::<CFNumber>()?
                .to_i32()?;
            if layer != 0 {
                return None;
            }
            let owner_pid = dict
                .find(unsafe { kCGWindowOwnerPID })?
                .downcast::<CFNumber>()?
                .to_i32()?;
            let owner_name = dict
                .find(unsafe { kCGWindowOwnerName })
                .and_then(|value| value.downcast::<CFString>())
                .map(|s| s.to_string())
                .unwrap_or_default();
            Some(NormalWindow {
                owner_pid,
                owner_name,
            })
        })
        .collect()
}

/// pid of the app owning the frontmost normal on-screen window.
#[must_use]
pub fn frontmost_app_pid() -> Option<pid_t> {
    normal_on_screen_windows().into_iter().next().map(|w| w.owner_pid)
}

/// pid of the frontmost-most running app whose `kCGWindowOwnerName`
/// matches `name` case-insensitively. `None` if no on-screen window is
/// owned by an app with that name.
#[must_use]
pub fn pid_for_app_name(name: &str) -> Option<pid_t> {
    normal_on_screen_windows()
        .into_iter()
        .find(|w| w.owner_name.eq_ignore_ascii_case(name))
        .map(|w| w.owner_pid)
}

/// Distinct pids owning at least one normal on-screen window, in
/// front-to-back order with duplicates removed (first occurrence kept).
/// Used at daemon startup to attach a `kAXWindowCreatedNotification`
/// observer to every already-running app (architecture.md §3.10 requires
/// lifecycle observation for windows from apps that were already running,
/// not just ones launched afterward -- see `hyperwm_macos::workspace` for
/// that half).
#[must_use]
pub fn all_app_pids() -> Vec<pid_t> {
    let mut seen = Vec::new();
    for w in normal_on_screen_windows() {
        if !seen.contains(&w.owner_pid) {
            seen.push(w.owner_pid);
        }
    }
    seen
}

/// One normal (layer 0) on-screen window's `CGWindowID`, owning pid, and
/// current bounds -- architecture.md §3.11 step 1's enumeration. `CGWindowID`
/// is a different identity system than [`crate::ax::AXUIElement`] (there's
/// no public API mapping one to the other -- see the daemon's window
/// correlation logic, which matches on `pid` + `rect` instead), so this is
/// deliberately a separate, minimal struct rather than folding into
/// `AXUIElement`-based enumeration.
pub struct OnScreenWindow {
    pub id: CGWindowID,
    pub pid: pid_t,
    pub rect: Rect,
    pub owner_name: String,
}

fn cg_rect(dict: &CFDictionary<CFString, CFType>) -> Option<Rect> {
    let bounds_value = dict.find(unsafe { kCGWindowBounds })?;
    // `CFType::downcast` only accepts `ConcreteCFType`s (e.g.
    // `CFDictionary<*const c_void, *const c_void>`, not this typed alias),
    // so re-wrap the raw ref directly instead -- same technique
    // `on_screen_windows` uses for the outer array.
    let bounds: CFDictionary<CFString, CFType> =
        unsafe { CFDictionary::wrap_under_get_rule(bounds_value.as_CFTypeRef() as *const _) };
    let field = |key: &str| -> Option<f64> {
        bounds.find(CFString::new(key))?.downcast::<CFNumber>()?.to_f64()
    };
    Some(Rect {
        x: field("X")?,
        y: field("Y")?,
        width: field("Width")?,
        height: field("Height")?,
    })
}

/// Normal (layer 0) on-screen windows, front-to-back (architecture.md
/// §3.11's "on-screen enumeration order") -- the basis for Space-switch
/// handling and the `auto_tile` action. Unlike [`normal_on_screen_windows`],
/// this keeps each window's `CGWindowID` and live bounds, which §3.11's
/// window-ID-set cache and `auto_tile`'s forced layout both need.
#[must_use]
pub fn on_screen_windows() -> Vec<OnScreenWindow> {
    let Some(untyped) = copy_window_info(
        kCGWindowListOptionOnScreenOnly | kCGWindowListExcludeDesktopElements,
        kCGNullWindowID,
    ) else {
        return Vec::new();
    };
    let dicts: CFArray<CFDictionary<CFString, CFType>> =
        unsafe { CFArray::wrap_under_get_rule(untyped.as_concrete_TypeRef() as *const _) };

    dicts
        .iter()
        .filter_map(|dict| {
            let layer = dict
                .find(unsafe { kCGWindowLayer })?
                .downcast::<CFNumber>()?
                .to_i32()?;
            if layer != 0 {
                return None;
            }
            let id = dict
                .find(unsafe { kCGWindowNumber })?
                .downcast::<CFNumber>()?
                .to_i32()?;
            let pid = dict
                .find(unsafe { kCGWindowOwnerPID })?
                .downcast::<CFNumber>()?
                .to_i32()?;
            let rect = cg_rect(&dict)?;
            let owner_name = dict
                .find(unsafe { kCGWindowOwnerName })
                .and_then(|value| value.downcast::<CFString>())
                .map(|s| s.to_string())
                .unwrap_or_default();
            Some(OnScreenWindow { id: id as CGWindowID, pid, rect, owner_name })
        })
        .collect()
}
