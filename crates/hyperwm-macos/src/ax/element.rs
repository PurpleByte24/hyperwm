//! Safe wrapper around `AXUIElementRef`, the Accessibility API's handle to
//! a UI object (an application, a window, ...). Build unit 4: enumerate an
//! app's windows, read/write a window's position and size
//! (`kAXPositionAttribute`/`kAXSizeAttribute`), and find the frontmost
//! app's focused window (`kAXFocusedWindowAttribute`). Wiring this into
//! `hyperwm-core`'s tree is build unit 5 -- this module only reads and
//! writes geometry, it doesn't know about tiling.

use std::ffi::c_void;
use std::ptr;

use accessibility_sys::{
    kAXErrorSuccess, pid_t, AXError, AXUIElementCopyAttributeValue, AXUIElementCreateApplication,
    AXUIElementCreateSystemWide, AXUIElementGetPid, AXUIElementGetTypeID, AXUIElementRef,
    AXUIElementSetAttributeValue, AXValueCreate, AXValueGetValue, AXValueRef, kAXValueTypeCGPoint,
    kAXValueTypeCGSize,
};
use core_foundation::array::CFArray;
use core_foundation::base::{CFRelease, CFTypeRef, TCFType};
use core_foundation::string::{CFString, CFStringRef};
use core_foundation::{declare_TCFType, impl_TCFType};
use core_graphics::geometry::{CGPoint, CGSize};

// Retain-count-managed wrapper around `AXUIElementRef`, matching the
// pattern `core-foundation`'s own types use (see `impl_TCFType!`'s macro
// doc). `AXUIElementRef` is a real CF object (`AXUIElementGetTypeID`
// exists), so it slots into that machinery directly.
declare_TCFType!(AXUIElement, AXUIElementRef);
impl_TCFType!(AXUIElement, AXUIElementRef, AXUIElementGetTypeID);

impl AXUIElement {
    /// The AX element representing the application with this pid.
    /// Resolving a pid from an app name/frontmost status is
    /// [`crate::ax::app`]'s job, not this type's.
    #[must_use]
    pub fn application(pid: pid_t) -> Self {
        unsafe { Self::wrap_under_create_rule(AXUIElementCreateApplication(pid)) }
    }

    /// The system-wide AX element, mostly useful as a root for
    /// element-at-position queries (not used by this build unit, but a
    /// natural companion to `application`).
    #[must_use]
    pub fn system_wide() -> Self {
        unsafe { Self::wrap_under_create_rule(AXUIElementCreateSystemWide()) }
    }

    /// The pid owning this element, via `AXUIElementGetPid`.
    pub fn pid(&self) -> Result<pid_t, AXError> {
        let mut pid: pid_t = 0;
        let err = unsafe { AXUIElementGetPid(self.as_concrete_TypeRef(), &mut pid) };
        ax_result(err, pid)
    }

    /// Copies a raw attribute value. Callers own the returned `CFTypeRef`
    /// (`AXUIElementCopyAttributeValue` follows the CF "copy" convention)
    /// and must release or re-wrap it.
    fn copy_attribute_raw(&self, attribute: &str) -> Result<CFTypeRef, AXError> {
        let attr = CFString::new(attribute);
        let mut value: CFTypeRef = ptr::null();
        let err = unsafe {
            AXUIElementCopyAttributeValue(
                self.as_concrete_TypeRef(),
                attr.as_concrete_TypeRef(),
                &mut value,
            )
        };
        ax_result(err, value)
    }

    fn set_attribute_raw(&self, attribute: &str, value: CFTypeRef) -> Result<(), AXError> {
        let attr = CFString::new(attribute);
        let err = unsafe {
            AXUIElementSetAttributeValue(
                self.as_concrete_TypeRef(),
                attr.as_concrete_TypeRef(),
                value,
            )
        };
        ax_result(err, ())
    }

    /// An attribute whose value is itself an AX element, e.g.
    /// `kAXFocusedWindowAttribute` on an application element.
    pub fn element_attribute(&self, attribute: &str) -> Result<AXUIElement, AXError> {
        let value = self.copy_attribute_raw(attribute)?;
        Ok(unsafe { AXUIElement::wrap_under_create_rule(value as AXUIElementRef) })
    }

    /// An attribute whose value is an array of AX elements, e.g.
    /// `kAXWindowsAttribute` on an application element.
    pub fn element_array_attribute(&self, attribute: &str) -> Result<Vec<AXUIElement>, AXError> {
        let value = self.copy_attribute_raw(attribute)?;
        let array: CFArray<AXUIElement> =
            unsafe { CFArray::wrap_under_create_rule(value as *const _) };
        Ok(array.iter().map(|item| item.clone()).collect())
    }

    /// A `CFString`-valued attribute, e.g. `kAXTitleAttribute`.
    pub fn string_attribute(&self, attribute: &str) -> Result<String, AXError> {
        let value = self.copy_attribute_raw(attribute)?;
        let string = unsafe { CFString::wrap_under_create_rule(value as CFStringRef) };
        Ok(string.to_string())
    }

    /// `kAXPositionAttribute`, unpacked from its `AXValue` (`CGPoint`)
    /// wrapper.
    pub fn position(&self) -> Result<CGPoint, AXError> {
        let value = self.copy_attribute_raw(accessibility_sys::kAXPositionAttribute)?;
        let point = unsafe { ax_value_get::<CGPoint>(value, kAXValueTypeCGPoint) };
        unsafe { CFRelease(value) };
        point.ok_or(accessibility_sys::kAXErrorFailure)
    }

    /// `kAXSizeAttribute`, unpacked from its `AXValue` (`CGSize`) wrapper.
    pub fn size(&self) -> Result<CGSize, AXError> {
        let value = self.copy_attribute_raw(accessibility_sys::kAXSizeAttribute)?;
        let size = unsafe { ax_value_get::<CGSize>(value, kAXValueTypeCGSize) };
        unsafe { CFRelease(value) };
        size.ok_or(accessibility_sys::kAXErrorFailure)
    }

    /// Sets `kAXPositionAttribute` via a boxed `AXValue` (`CGPoint`).
    /// Instant reposition only, per architecture.md §7 (no animation).
    pub fn set_position(&self, point: CGPoint) -> Result<(), AXError> {
        let value = unsafe { ax_value_create(kAXValueTypeCGPoint, &point) }
            .ok_or(accessibility_sys::kAXErrorFailure)?;
        let result = self.set_attribute_raw(accessibility_sys::kAXPositionAttribute, value);
        unsafe { CFRelease(value) };
        result
    }

    /// Sets `kAXSizeAttribute` via a boxed `AXValue` (`CGSize`).
    pub fn set_size(&self, size: CGSize) -> Result<(), AXError> {
        let value = unsafe { ax_value_create(kAXValueTypeCGSize, &size) }
            .ok_or(accessibility_sys::kAXErrorFailure)?;
        let result = self.set_attribute_raw(accessibility_sys::kAXSizeAttribute, value);
        unsafe { CFRelease(value) };
        result
    }
}

/// # Safety
/// `T` must have the exact memory layout `AXValueGetValue` expects for
/// `value_type` (`CGPoint` for `kAXValueTypeCGPoint`, `CGSize` for
/// `kAXValueTypeCGSize`) -- both are `#[repr(C)]` pairs of `f64` in
/// `core-graphics-types`, matching Apple's layout exactly.
unsafe fn ax_value_get<T: Default>(value: CFTypeRef, value_type: u32) -> Option<T> {
    let mut out = T::default();
    let ok = AXValueGetValue(
        value as AXValueRef,
        value_type,
        &mut out as *mut T as *mut c_void,
    );
    ok.then_some(out)
}

/// # Safety
/// Same layout requirement as [`ax_value_get`].
unsafe fn ax_value_create<T>(value_type: u32, value: &T) -> Option<CFTypeRef> {
    let created = AXValueCreate(value_type, value as *const T as *const c_void);
    if created.is_null() {
        None
    } else {
        Some(created as CFTypeRef)
    }
}

fn ax_result<T>(err: AXError, value: T) -> Result<T, AXError> {
    if err == kAXErrorSuccess {
        Ok(value)
    } else {
        Err(err)
    }
}
