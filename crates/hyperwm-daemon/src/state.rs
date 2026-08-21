//! Action Engine (architecture.md §1): the tiling/float/geometry operations
//! behind each built-in keybind, plus window lifecycle (§3.3, §3.4) and
//! drift correction (§3.10). This is the one place that mutates
//! `hyperwm-core`'s `Tree`, the floating list, and issues the AX calls that
//! make either one visible on screen.
//!
//! `DaemonState` deliberately does *not* cache a "currently focused window"
//! field -- [`DaemonState::resolve_focused`] queries the AX API live on
//! every action, matching architecture.md §3.10's "query live, don't
//! cache" rule for floating windows (extended here to focus in general,
//! since the daemon has no `AXFocusedWindowChanged` subscription and no
//! need for one: nothing here runs on a timer or needs focus between
//! actions).
//!
//! Creating a new per-app `AXObserver` needs a callback that can call back
//! into a `DaemonState` behind a shared `Rc<RefCell<_>>` -- see
//! `lifecycle.rs`, not here. Once that observer exists, everything in this
//! file only ever *uses* it (`WindowObserver::watch` on an existing
//! instance), so every method below is a plain `&mut self`.

use std::collections::HashMap;

use accessibility_sys::{
    kAXFocusedWindowAttribute, kAXStandardWindowSubrole, kAXSubroleAttribute,
    kAXUIElementDestroyedNotification, kAXWindowMovedNotification, kAXWindowResizedNotification,
    pid_t,
};
use core_graphics::geometry::{CGPoint, CGSize};

use hyperwm_config::{Action, Config, FloatPlacement};
use hyperwm_core::{Direction, InsertOutcome, Rect, Tree, WindowId};
use hyperwm_macos::ax::{self, AXUIElement, WindowObserver};
use hyperwm_macos::{identity, screen};

use crate::registry::WindowRegistry;

/// Fixed cascade offset (architecture.md §3.9's `new_float_placement =
/// "cascade"`) applied per already-floating window, wrapping after this
/// many steps so cascaded windows don't walk off screen. Not in
/// `examples/config.toml`'s schema -- architecture.md specifies the
/// *strategy* ("offset from the previous floating window's position") but
/// not a pixel amount, so this is an implementation-level default, not a
/// schema decision.
const CASCADE_STEP: f64 = 32.0;
const CASCADE_WRAP: usize = 8;

/// AX geometry read/writes agree on their own eventual-consistency at the
/// pixel level (see `docs/known-issues.md` on Chrome/Electron resize
/// quirks); comparing floats for exact equality when deciding whether a
/// window has drifted (architecture.md §3.10 step 3) would treat harmless
/// sub-pixel rounding as drift forever. Anything under this is "the same
/// rect" for drift-correction purposes.
const DRIFT_EPSILON: f64 = 0.5;

pub struct DaemonState {
    config: Config,
    tree: Tree,
    /// Membership + insertion order (used for cascade placement) of
    /// floating windows. Never their geometry -- that's queried live via
    /// AX wherever it's needed, per architecture.md §3.10.
    floating: Vec<WindowId>,
    windows: WindowRegistry,
    /// One `AXObserver` per pid, watching that app's `kAXWindowCreated`
    /// (always) plus each of its known windows' `kAXUIElementDestroyed`
    /// (always) and `kAXWindowMoved`/`kAXWindowResized` (tiled windows
    /// only, per §3.10). Created once per pid in `lifecycle::adopt_app`;
    /// every method here that needs a *new* notification watched on an
    /// already-known pid just calls `.watch()` on the existing entry.
    pub(crate) app_observers: HashMap<pid_t, WindowObserver>,
}

impl DaemonState {
    #[must_use]
    pub fn new(config: Config) -> Self {
        let tree = Tree::new(
            config.tiling.max_tiled_windows,
            config.tiling.insert_heuristic,
            config.tiling.split_ratio_default,
        );
        Self {
            config,
            tree,
            floating: Vec::new(),
            windows: WindowRegistry::default(),
            app_observers: HashMap::new(),
        }
    }

    /// Filters out AX elements that report as "windows" via
    /// `kAXWindowsAttribute`/`kAXWindowCreatedNotification` but aren't
    /// real, user-manageable app windows -- confirmed via manual testing's
    /// debug dump, which showed Notification Centre widget panels
    /// (180x180/360x360 "windows") and several 64x64 phantom entries all
    /// getting registered and tiled alongside real windows, each
    /// insertion re-splitting the tree (shrinking everything) on every
    /// one. `kAXSubroleAttribute == AXStandardWindow` is the standard
    /// public-API filter tiling window managers use for exactly this: a
    /// real document/app window has this subrole; panels, sheets, system
    /// dialogs, and other AX-reported pseudo-windows generally don't.
    fn is_standard_window(window: &AXUIElement) -> bool {
        window.string_attribute(kAXSubroleAttribute).as_deref() == Ok(kAXStandardWindowSubrole)
    }

    fn watch(&self, pid: pid_t, window: &AXUIElement, notification: &str) {
        let Some(observer) = self.app_observers.get(&pid) else {
            eprintln!(
                "hyperwm-daemon: no AXObserver for pid {pid}, can't watch {notification} \
                 (this means adopt_app wasn't called for it -- a bug, not a transient AX error)"
            );
            return;
        };
        if let Err(err) = observer.watch(window, notification) {
            // A tiled->float->tiled round trip re-watches moved/resized on
            // a window that was never unwatched (there's no
            // AXObserverRemoveNotification call in `ax::observer` -- see
            // its module doc); `handle_drift` already no-ops for anything
            // not in the tree, so this is harmless, just noisy if logged.
            if err != accessibility_sys::kAXErrorNotificationAlreadyRegistered {
                eprintln!(
                    "hyperwm-daemon: couldn't watch {notification} on pid {pid}: {}",
                    accessibility_sys::error_string(err)
                );
            }
        }
    }

    fn visible_frame_for_window(&self, window: &AXUIElement) -> Option<Rect> {
        let pos = window.position().ok()?;
        let size = window.size().ok()?;
        screen::visible_frame_containing(pos.x + size.width / 2.0, pos.y + size.height / 2.0)
    }

    /// The visible frame the tree is currently laid out against: the
    /// display holding any current tiled window, falling back to the
    /// primary display if the tree is empty. architecture.md §3.1 scopes
    /// one BSP tree per macOS Space; there's no public API to learn which
    /// Space (or, for multi-monitor, which display's Space) a window
    /// belongs to (see the "New-app detection"/"Visible frame" flags
    /// raised at the start of this unit), so this daemon runs one global
    /// tree and treats it as belonging to whichever display its windows
    /// are actually on -- correct for the common single-display case,
    /// best-effort otherwise.
    fn current_tree_visible_frame(&self) -> Option<Rect> {
        for id in self.tree.window_ids() {
            if let Some(element) = self.windows.element_for(id) {
                if let Some(frame) = self.visible_frame_for_window(element) {
                    return Some(frame);
                }
            }
        }
        screen::primary_visible_frame()
    }

    /// Recomputes the tree's layout from scratch and pushes every tiled
    /// window to its computed rect. Called after any tree mutation, not
    /// just the window(s) that moved -- this is also what makes a
    /// maximized tiled window "snap back" on the next tiling-affecting
    /// event (architecture.md §3.8): maximize never touches the tree, so
    /// the next full-layout pass simply reasserts every tiled window's
    /// tree-computed rect, including one that was maximized in between.
    fn apply_tree_layout(&mut self) {
        let Some(visible) = self.current_tree_visible_frame() else {
            return;
        };
        let rects = self.tree.layout(visible, self.config.gaps);
        for (id, rect) in rects {
            if let Some(element) = self.windows.element_for(id) {
                let _ = element.set_position(CGPoint::new(rect.x, rect.y));
                let _ = element.set_size(CGSize::new(rect.width, rect.height));
            }
        }
    }

    /// The live-focused window's [`WindowId`], updating the tree's focus
    /// history (architecture.md §3.3 step 1) as a side effect. `None` if
    /// nothing is focused, the focused window isn't one hyperwm knows
    /// about yet (e.g. a genuine AX-notification race past
    /// `lifecycle::adopt_app`'s coverage -- see that module's doc comment),
    /// or the AX calls themselves failed.
    fn resolve_focused(&mut self) -> Option<WindowId> {
        let pid = ax::frontmost_app_pid()?;
        let app = AXUIElement::application(pid);
        let window = app.element_attribute(kAXFocusedWindowAttribute).ok()?;
        let id = self.windows.id_for(&window)?;
        self.tree.set_focus(Some(id));
        Some(id)
    }

    /// Where a brand-new floating window with no prior position (a
    /// freshly created `always_float_apps` window, or one that hit
    /// `max_tiled_windows`) should land, per `floating.new_float_placement`
    /// (architecture.md §3.9). Not used for toggle_float's tiled->floating
    /// transition, which keeps the window's current rect (architecture.md
    /// §3.8's "no jump").
    fn place_new_float(&self, window: &AXUIElement) {
        let Ok(size) = window.size() else { return };
        let Some(visible) = self.visible_frame_for_window(window) else {
            return;
        };
        let (x, y) = match self.config.floating.new_float_placement {
            FloatPlacement::Center => (
                visible.x + (visible.width - size.width) / 2.0,
                visible.y + (visible.height - size.height) / 2.0,
            ),
            FloatPlacement::Cascade => {
                let index = (self.floating.len().saturating_sub(1)) % CASCADE_WRAP;
                #[allow(clippy::cast_precision_loss)]
                let offset = CASCADE_STEP * index as f64;
                (visible.x + offset, visible.y + offset)
            }
        };
        let _ = window.set_position(CGPoint::new(x, y));
    }

    fn float_new_window(&mut self, id: WindowId, window: &AXUIElement) {
        self.floating.push(id);
        self.place_new_float(window);
    }

    /// Registers a window hyperwm is only now finding out about *without*
    /// running it through the tile-or-float insertion rule -- used
    /// exclusively for `lifecycle::adopt_app`'s one-time startup scan of
    /// windows that existed before the daemon itself did (see
    /// `lifecycle::AdoptionPolicy::FloatOnly`'s doc comment for why: a
    /// window the user had open before starting hyperwm isn't a "new
    /// window" in architecture.md §3.3's sense, and force-tiling it would
    /// silently spend a `max_tiled_windows` slot the user never asked
    /// for -- e.g. the terminal the daemon itself was launched from).
    /// Still registers for lifecycle tracking (destroy watched, same as
    /// any window) and keeps its current rect, same "no jump" rule as
    /// `toggle_float`'s tiled->floating transition -- this isn't a "new
    /// float with no prior position" either, so `place_new_float` doesn't
    /// apply.
    pub(crate) fn adopt_preexisting_window(&mut self, pid: pid_t, window: AXUIElement) {
        if self.windows.is_known(&window) || !Self::is_standard_window(&window) {
            return;
        }
        let id = self.windows.register(window.clone(), pid);
        self.watch(pid, &window, kAXUIElementDestroyedNotification);
        self.floating.push(id);
    }

    /// New-window insertion (architecture.md §3.3, §3.2, §3.9). Called
    /// both for a live `kAXWindowCreatedNotification` and for
    /// `lifecycle::adopt_app`'s catch-up enumeration of a *newly launched*
    /// app's windows (`AdoptionPolicy::ClassifyForTiling`) -- both are
    /// genuinely new activity during the daemon's own lifetime, which is
    /// what architecture.md §3.3's insertion rule describes. Pre-existing
    /// windows at daemon startup go through
    /// [`DaemonState::adopt_preexisting_window`] instead, not this.
    pub(crate) fn handle_new_window(&mut self, pid: pid_t, window: AXUIElement) {
        if self.windows.is_known(&window) || !Self::is_standard_window(&window) {
            return;
        }
        let id = self.windows.register(window.clone(), pid);
        self.watch(pid, &window, kAXUIElementDestroyedNotification);

        let identity = identity::identity_for_pid(pid).unwrap_or_default();
        let always_float = self
            .config
            .floating
            .always_float_apps
            .iter()
            .any(|pattern| identity.matches(pattern));
        if always_float {
            self.float_new_window(id, &window);
            return;
        }

        let Some(visible_frame) = self.visible_frame_for_window(&window) else {
            // No display info to lay it out against; float rather than
            // silently dropping the window from hyperwm's tracking.
            self.float_new_window(id, &window);
            return;
        };
        match self.tree.insert(id, visible_frame, self.config.gaps) {
            InsertOutcome::Inserted => {
                self.watch(pid, &window, kAXWindowMovedNotification);
                self.watch(pid, &window, kAXWindowResizedNotification);
                self.apply_tree_layout();
            }
            InsertOutcome::CapReached => {
                self.float_new_window(id, &window);
            }
        }
    }

    /// Window removal (architecture.md §3.4). `element` is whatever
    /// `kAXUIElementDestroyedNotification` handed back -- by the time this
    /// runs the window is already gone, so no further AX calls on it are
    /// possible or needed.
    pub(crate) fn handle_window_destroyed(&mut self, element: &AXUIElement) {
        let Some(id) = self.windows.remove_by_element(element) else {
            return;
        };
        if self.tree.remove(id) {
            self.apply_tree_layout();
        } else {
            self.floating.retain(|&w| w != id);
        }
    }

    /// Drift correction for a tiled window (architecture.md §3.10):
    /// compares the window's live rect to the tree's computed rect and
    /// only writes back if they differ, so a burst of redundant
    /// notifications from one drag settles into a single corrective write
    /// (or none) rather than N repeated ones.
    pub(crate) fn handle_drift(&mut self, element: &AXUIElement) {
        let Some(id) = self.windows.id_for(element) else {
            return;
        };
        if !self.tree.contains(id) {
            // Only tiled windows are watched for move/resize at all
            // (floating windows are queried live, never subscribed to --
            // see the module doc), so this shouldn't happen; no-op if it
            // somehow does rather than acting on stale membership.
            return;
        }
        let Some(visible) = self.current_tree_visible_frame() else {
            return;
        };
        let rects = self.tree.layout(visible, self.config.gaps);
        let Some((_, target)) = rects.into_iter().find(|(rid, _)| *rid == id) else {
            return;
        };
        let (Ok(pos), Ok(size)) = (element.position(), element.size()) else {
            return;
        };
        let matches = (pos.x - target.x).abs() < DRIFT_EPSILON
            && (pos.y - target.y).abs() < DRIFT_EPSILON
            && (size.width - target.width).abs() < DRIFT_EPSILON
            && (size.height - target.height).abs() < DRIFT_EPSILON;
        if !matches {
            let _ = element.set_position(CGPoint::new(target.x, target.y));
            let _ = element.set_size(CGSize::new(target.width, target.height));
        }
    }

    /// `hyper+f` (architecture.md §3.8).
    pub fn toggle_float(&mut self) {
        let Some(id) = self.resolve_focused() else {
            return;
        };
        if self.tree.contains(id) {
            self.tree.remove(id);
            self.floating.push(id); // Current rect kept as-is: no jump.
            self.apply_tree_layout();
        } else if self.floating.contains(&id) {
            let Some(element) = self.windows.element_for(id).cloned() else {
                return;
            };
            let Some(visible_frame) = self.visible_frame_for_window(&element) else {
                return;
            };
            if self.tree.insert(id, visible_frame, self.config.gaps) == InsertOutcome::Inserted {
                self.floating.retain(|&w| w != id);
                if let Some(pid) = self.windows.pid_for(id) {
                    self.watch(pid, &element, kAXWindowMovedNotification);
                    self.watch(pid, &element, kAXWindowResizedNotification);
                }
                self.apply_tree_layout();
            }
            // CapReached: stays floating, no-op (architecture.md §3.8).
        }
    }

    /// `hyper+m` (architecture.md §3.8). Pure geometry: doesn't touch the
    /// tree or the floating list either way.
    pub fn maximize(&mut self) {
        let Some(id) = self.resolve_focused() else {
            return;
        };
        let Some(element) = self.windows.element_for(id).cloned() else {
            return;
        };
        let Some(visible) = self.visible_frame_for_window(&element) else {
            return;
        };
        let outer = self.config.gaps.outer;
        let rect = Rect {
            x: visible.x + outer,
            y: visible.y + outer,
            width: (visible.width - 2.0 * outer).max(0.0),
            height: (visible.height - 2.0 * outer).max(0.0),
        };
        let _ = element.set_position(CGPoint::new(rect.x, rect.y));
        let _ = element.set_size(CGSize::new(rect.width, rect.height));
    }

    /// A focused floating window's fixed-step nudge (architecture.md
    /// §3.7) -- independent of, and much simpler than, tiled movement's
    /// geometry search.
    fn move_floating(&self, id: WindowId, direction: Direction) {
        let Some(element) = self.windows.element_for(id) else {
            return;
        };
        let Ok(pos) = element.position() else { return };
        let step = self.config.floating.float_move_step;
        let (dx, dy) = match direction {
            Direction::Left => (-step, 0.0),
            Direction::Right => (step, 0.0),
            Direction::Up => (0.0, -step),
            Direction::Down => (0.0, step),
        };
        let _ = element.set_position(CGPoint::new(pos.x + dx, pos.y + dy));
    }

    /// `hyper+hjkl`: tiled geometric neighbor-swap (architecture.md §3.5)
    /// if the focused window is tiled, floating nudge (§3.7) if it's
    /// floating.
    pub fn move_direction(&mut self, direction: Direction) {
        let Some(id) = self.resolve_focused() else {
            return;
        };
        if self.tree.contains(id) {
            let Some(visible) = self.current_tree_visible_frame() else {
                return;
            };
            if self
                .tree
                .move_direction(id, direction, visible, self.config.gaps)
            {
                self.apply_tree_layout();
            }
            // No candidate in that direction: no-op (architecture.md §3.5
            // step 7 -- no wraparound).
        } else if self.floating.contains(&id) {
            self.move_floating(id, direction);
        }
    }

    /// `hyper+shift+hjkl` (architecture.md §3.6). Only defined for tiled
    /// windows -- no-op if the focused window is floating.
    pub fn resize(&mut self, direction: Direction) {
        let Some(id) = self.resolve_focused() else {
            return;
        };
        if !self.tree.contains(id) {
            return;
        }
        let resize = self.config.tiling.resize;
        if self
            .tree
            .resize(id, direction, resize.step, resize.min_ratio, resize.max_ratio)
        {
            self.apply_tree_layout();
        }
    }

    /// Runs the Action Engine operation the Event Router resolved a
    /// keypress to.
    pub fn dispatch(&mut self, action: Action) {
        match action {
            Action::ToggleFloat => self.toggle_float(),
            Action::Maximize => self.maximize(),
            Action::MoveLeft => self.move_direction(Direction::Left),
            Action::MoveDown => self.move_direction(Direction::Down),
            Action::MoveUp => self.move_direction(Direction::Up),
            Action::MoveRight => self.move_direction(Direction::Right),
            Action::ResizeLeft => self.resize(Direction::Left),
            Action::ResizeDown => self.resize(Direction::Down),
            Action::ResizeUp => self.resize(Direction::Up),
            Action::ResizeRight => self.resize(Direction::Right),
        }
    }
}
