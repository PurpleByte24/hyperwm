//! Maps hyperwm-core's opaque [`WindowId`] to the real `AXUIElement` (and
//! owning pid) it stands for. hyperwm-core's `Tree` and the daemon's
//! floating list only ever deal in `WindowId`s (architecture.md §3.1); this
//! is the one place that turns those back into something AX calls can act
//! on.
//!
//! A linear `Vec` scan is used for `AXUIElement` lookup rather than a
//! `HashMap` keyed on it: `AXUIElement` has no `Hash` impl (only
//! `PartialEq`/`Eq`, via `CFEqual` -- see `core-foundation`'s
//! `impl_TCFType!`), and the window counts involved here are small
//! (bounded by how many windows are open at once, not by anything the
//! daemon iterates per-keystroke), so a scan is simpler and plenty fast.

use accessibility_sys::pid_t;
use core_graphics::window::CGWindowID;
use hyperwm_core::WindowId;
use hyperwm_macos::ax::AXUIElement;

struct Entry {
    id: WindowId,
    element: AXUIElement,
    pid: pid_t,
    /// This window's `CGWindowID`, once correlated (architecture.md §3.11)
    /// -- `None` until `DaemonState::resolve_on_screen_window` has matched
    /// it to an on-screen `ax::OnScreenWindow` at least once. Cached rather
    /// than re-derived by pid+rect matching on every poll: `CGWindowList`
    /// bounds and AX `position()`/`size()` aren't always pixel-identical
    /// for the same window (confirmed via manual testing), so re-matching
    /// every tick was intermittently losing the correlation for windows
    /// hyperwm already knew perfectly well, which corrupted
    /// `handle_space_change`'s window-ID-set key. A `CGWindowID`, once
    /// known, is a stable, correlation-free identity for the rest of that
    /// window's life.
    cg_id: Option<CGWindowID>,
}

#[derive(Default)]
pub struct WindowRegistry {
    next_id: u64,
    entries: Vec<Entry>,
}

impl WindowRegistry {
    pub fn is_known(&self, element: &AXUIElement) -> bool {
        self.entries.iter().any(|e| &e.element == element)
    }

    pub fn id_for(&self, element: &AXUIElement) -> Option<WindowId> {
        self.entries
            .iter()
            .find(|e| &e.element == element)
            .map(|e| e.id)
    }

    pub fn element_for(&self, id: WindowId) -> Option<&AXUIElement> {
        self.entries.iter().find(|e| e.id == id).map(|e| &e.element)
    }

    pub fn pid_for(&self, id: WindowId) -> Option<pid_t> {
        self.entries.iter().find(|e| e.id == id).map(|e| e.pid)
    }

    /// Every currently registered window owned by `pid`, for the daemon's
    /// on-screen-window correlation (architecture.md §3.11) when a
    /// `CGWindowID` hasn't been correlated for it yet: there's no public
    /// API mapping a `CGWindowID` directly to an `AXUIElement`, so the
    /// caller matches by pid + live rect instead of by id in that case.
    pub fn ids_for_pid(&self, pid: pid_t) -> impl Iterator<Item = (WindowId, &AXUIElement)> {
        self.entries.iter().filter(move |e| e.pid == pid).map(|e| (e.id, &e.element))
    }

    /// The `WindowId` already correlated to this `CGWindowID`, if any (see
    /// `Entry::cg_id`'s doc comment). The fast, correlation-free path
    /// [`DaemonState::resolve_on_screen_window`] tries before falling back
    /// to pid+rect matching.
    pub fn id_for_cg_id(&self, cg_id: CGWindowID) -> Option<WindowId> {
        self.entries.iter().find(|e| e.cg_id == Some(cg_id)).map(|e| e.id)
    }

    /// Records that `id`'s window is `cg_id` on the `CGWindowList` side, so
    /// future polls can skip pid+rect matching for it entirely. No-ops if
    /// `id` isn't registered.
    pub fn set_cg_id(&mut self, id: WindowId, cg_id: CGWindowID) {
        if let Some(entry) = self.entries.iter_mut().find(|e| e.id == id) {
            entry.cg_id = Some(cg_id);
        }
    }

    /// Registers a not-yet-known `AXUIElement`, returning its new
    /// [`WindowId`]. Callers must check [`WindowRegistry::is_known`] first
    /// -- this always allocates a fresh id, even for an element already
    /// registered.
    pub fn register(&mut self, element: AXUIElement, pid: pid_t) -> WindowId {
        self.next_id += 1;
        let id = WindowId(self.next_id);
        self.entries.push(Entry { id, element, pid, cg_id: None });
        id
    }

    pub fn remove(&mut self, id: WindowId) {
        self.entries.retain(|e| e.id != id);
    }

    pub fn remove_by_element(&mut self, element: &AXUIElement) -> Option<WindowId> {
        let id = self.id_for(element)?;
        self.remove(id);
        Some(id)
    }
}
