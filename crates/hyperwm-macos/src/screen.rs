//! Display visible-frame lookup (architecture.md §4's "space's visible
//! frame" input) -- the area of a display excluding the menu bar and Dock.
//!
//! CoreGraphics alone (`CGDisplayBounds`) only gives a display's *full*
//! bounds, with no public call for Dock size/position/auto-hide state. The
//! standard public API for the Dock/menu-bar-aware area is AppKit's
//! `NSScreen.visibleFrame` -- so this module is the one place in hyperwm
//! that reaches beyond Accessibility API + CGEventTap (see CLAUDE.md's
//! hard constraints; confirmed with the user before adding this
//! dependency, since AppKit is fully public but outside that literal
//! two-API list).
//!
//! `NSScreen` reports frames in Cocoa's coordinate system: origin at the
//! *primary* (menu-bar) screen's bottom-left, y increasing upward. Every
//! other coordinate hyperwm-core and the AX API use (`kAXPositionAttribute`,
//! `CGDisplayBounds`, `hyperwm_core::Rect`) is Quartz's: origin at the
//! primary screen's top-left, y increasing downward. `screens()[0]` is
//! always the primary screen and always has Cocoa-frame origin (0, 0)
//! (Apple's documented guarantee), which is also Quartz's global origin --
//! so the two systems share one flip anchor: `quartz_y = primary_height -
//! cocoa_y - height`. This module converts at its boundary so nothing
//! upstream (hyperwm-core, the daemon) ever has to think about Cocoa
//! coordinates.

use objc2_app_kit::NSScreen;
use objc2_foundation::{MainThreadMarker, NSRect};

use hyperwm_core::Rect;

fn cocoa_to_quartz(rect: NSRect, primary_height: f64) -> Rect {
    Rect {
        x: rect.origin.x,
        y: primary_height - rect.origin.y - rect.size.height,
        width: rect.size.width,
        height: rect.size.height,
    }
}

fn rect_contains_point(rect: Rect, x: f64, y: f64) -> bool {
    x >= rect.x && x < rect.x + rect.width && y >= rect.y && y < rect.y + rect.height
}

/// The visible frame (Quartz/AX coordinates, menu bar and Dock excluded) of
/// whichever display contains `point` (also Quartz coordinates -- e.g. a
/// window's center, from `AXUIElement::position()`/`size()`). Falls back to
/// the primary display's visible frame if `point` isn't on any known
/// display (e.g. a window that hasn't been placed yet) or this isn't
/// called from the main thread (`NSScreen` requires it).
#[must_use]
pub fn visible_frame_containing(point_x: f64, point_y: f64) -> Option<Rect> {
    let mtm = MainThreadMarker::new()?;
    let screens = NSScreen::screens(mtm).to_vec();
    let primary = screens.first()?;
    let primary_height = primary.frame().size.height;

    let mut fallback = None;
    for screen in &screens {
        let frame = cocoa_to_quartz(screen.frame(), primary_height);
        let visible = cocoa_to_quartz(screen.visibleFrame(), primary_height);
        if fallback.is_none() {
            fallback = Some(visible);
        }
        if rect_contains_point(frame, point_x, point_y) {
            return Some(visible);
        }
    }
    fallback
}

/// The primary display's visible frame, for callers with no window/point
/// to anchor to yet (e.g. an empty tree with nothing focused).
#[must_use]
pub fn primary_visible_frame() -> Option<Rect> {
    visible_frame_containing(0.0, 0.0)
}
