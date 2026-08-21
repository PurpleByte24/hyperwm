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
use hyperwm_core::WindowId;
use hyperwm_macos::ax::AXUIElement;

struct Entry {
    id: WindowId,
    element: AXUIElement,
    pid: pid_t,
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

    /// Registers a not-yet-known `AXUIElement`, returning its new
    /// [`WindowId`]. Callers must check [`WindowRegistry::is_known`] first
    /// -- this always allocates a fresh id, even for an element already
    /// registered.
    pub fn register(&mut self, element: AXUIElement, pid: pid_t) -> WindowId {
        self.next_id += 1;
        let id = WindowId(self.next_id);
        self.entries.push(Entry { id, element, pid });
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
