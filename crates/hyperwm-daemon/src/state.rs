//! Action Engine (architecture.md §1): the tiling/float/geometry operations
//! behind each built-in keybind, plus window lifecycle (§3.3, §3.4) and
//! drift correction (§3.10). This is the one place that mutates
//! `hyperwm-core`'s `Tree`, the floating list, and issues the AX calls that
//! make either one visible on screen.
//!
//! `DaemonState` does not cache a "currently focused window" field --
//! [`DaemonState::resolve_focused`] queries the AX API live wherever a
//! hyper-key action needs it (move/resize/toggle-float/maximize),
//! matching architecture.md §3.10's "query live, don't cache" rule for
//! floating windows. New-window insertion (architecture.md §3.3) does
//! *not* need focus at all -- it targets whichever tiled leaf currently
//! has the largest on-screen area, which `hyperwm-core::Tree::insert`
//! computes fresh from `apply_tree_layout`'s live rects every time.
//!
//! Creating a new per-app `AXObserver` needs a callback that can call back
//! into a `DaemonState` behind a shared `Rc<RefCell<_>>` -- see
//! `lifecycle.rs`, not here. Once that observer exists, everything in this
//! file only ever *uses* it (`WindowObserver::watch` on an existing
//! instance), so every method below is a plain `&mut self`.

use std::collections::{BTreeSet, HashMap};

use accessibility_sys::{
    kAXFocusedWindowAttribute, kAXMinimizedAttribute, kAXStandardWindowSubrole,
    kAXSubroleAttribute, kAXUIElementDestroyedNotification, kAXWindowMovedNotification,
    kAXWindowResizedNotification, kAXWindowsAttribute, pid_t,
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

/// Tolerance for two kinds of rect comparison architecture.md §3.11
/// introduces: (a) disambiguating which of a pid's *multiple* windows a
/// `CGWindowList` entry's bounds refers to, when
/// [`DaemonState::resolve_on_screen_window`]'s unambiguous
/// single-candidate fast path doesn't apply; (b) deciding whether a
/// Space's on-screen rects still match what [`DaemonState::space_cache`]
/// last saw for that exact window-ID set. Noticeably looser than
/// [`DRIFT_EPSILON`]: this compares `CGWindowList` bounds against AX
/// `position()`/`size()`, two different enumeration systems that aren't
/// always pixel-identical for the same window (confirmed via manual
/// testing), rather than the same AX read taken twice.
const SPACE_RECT_EPSILON: f64 = 8.0;

fn rects_approx_eq(a: Rect, b: Rect, epsilon: f64) -> bool {
    (a.x - b.x).abs() < epsilon
        && (a.y - b.y).abs() < epsilon
        && (a.width - b.width).abs() < epsilon
        && (a.height - b.height).abs() < epsilon
}

/// A snapshot of one Space's tiled/floating state (architecture.md §3.11):
/// what `self.tree`/`self.floating` looked like the last time this exact
/// window-ID set was active, plus the rect each window had then (for
/// deciding whether a return visit is a cache hit). Keyed in
/// [`DaemonState::space_cache`] by that window-ID set, not by any Space
/// identifier -- there is no persistent per-Space tree; this cache is what
/// makes returning to an unchanged Space look like one anyway.
#[derive(Clone)]
struct SpaceLayout {
    tree: Tree,
    floating: Vec<WindowId>,
    rects: HashMap<WindowId, Rect>,
}

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
    /// Left-mouse-button state, driven by `hyperwm_macos::mouse::install`'s
    /// tap via [`DaemonState::set_mouse_down`] (architecture.md §3.10).
    /// `handle_drift` no-ops entirely while this is `true`, rather than
    /// fighting an in-progress hand drag/resize.
    mouse_down: bool,
    /// Architecture.md §3.11's "in-memory cache keyed by the current
    /// on-screen window-ID set": every window-ID set this daemon has ever
    /// had active, mapped to the tiled/floating layout it had at the time
    /// -- what lets a return visit to an unchanged Space restore its
    /// tiling instead of starting over. See [`SpaceLayout`].
    space_cache: HashMap<BTreeSet<WindowId>, SpaceLayout>,
    /// Debounce for `handle_space_change`'s poll: the on-screen resolution
    /// from the *previous* tick, if it differed from what's currently
    /// active. A transition only commits once the same resolution (same
    /// ids *and* matching rects) is observed on two consecutive polls --
    /// confirmed via manual testing that a single poll can land mid-Space-
    /// switch-swipe-animation (a transient on-screen rect reads roughly
    /// one screen-width off from where the window actually settles), and
    /// treating that one glitchy sample as ground truth was corrupting the
    /// cache with garbage rects and causing spurious churn.
    pending_space_change: Option<Vec<(WindowId, Rect)>>,
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
            mouse_down: false,
            app_observers: HashMap::new(),
            space_cache: HashMap::new(),
            pending_space_change: None,
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

    /// The live-focused window's [`WindowId`], for hyper-key actions that
    /// operate on "the focused window" (move/resize/toggle-float/
    /// maximize). `None` if nothing is focused, the focused window isn't
    /// one hyperwm knows about yet, or the AX calls themselves failed.
    fn resolve_focused(&self) -> Option<WindowId> {
        let pid = ax::frontmost_app_pid()?;
        let app = AXUIElement::application(pid);
        let window = app.element_attribute(kAXFocusedWindowAttribute).ok()?;
        self.windows.id_for(&window)
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
        let id = self.register_only(pid, window);
        self.floating.push(id);
    }

    /// Registers a not-yet-known window for lifecycle tracking (destroy
    /// watched) without touching `self.tree`/`self.floating` either way --
    /// the shared first half of [`DaemonState::adopt_preexisting_window`]
    /// and [`DaemonState::resolve_on_screen_window`]. The two differ in
    /// what happens *after* registering: `adopt_preexisting_window` always
    /// floats the window; `resolve_on_screen_window` deliberately doesn't,
    /// since at the point it runs, `self.floating` may still be a
    /// *different* Space's list (architecture.md §3.11's
    /// [`DaemonState::handle_space_change`] hasn't decided yet whether it's
    /// restoring a cached Space or building a fresh one) -- pushing there
    /// prematurely would contaminate whichever Space's list happens to be
    /// active at that moment.
    fn register_only(&mut self, pid: pid_t, window: AXUIElement) -> WindowId {
        if let Some(id) = self.windows.id_for(&window) {
            return id;
        }
        let id = self.windows.register(window.clone(), pid);
        self.watch(pid, &window, kAXUIElementDestroyedNotification);
        id
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

        // architecture.md §3.11: `kAXWindowCreatedNotification` is
        // per-app, not per-Space, so this can fire well before
        // `handle_space_change`'s next poll tick notices a Space switch --
        // reconcile against what's actually on screen right now first, or
        // this window gets classified against the *previous* Space's
        // tree/floating list. See `reconcile_before_insert`'s doc comment.
        self.reconcile_before_insert(id);

        let identity = identity::identity_for_pid(pid).unwrap_or_default();
        let always_float = self
            .config
            .floating
            .always_float_apps
            .iter()
            .any(|pattern| identity.matches(pattern));
        if always_float {
            self.float_new_window(id, &window);
        } else if let Some(visible_frame) = self.visible_frame_for_window(&window) {
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
        } else {
            // No display info to lay it out against; float rather than
            // silently dropping the window from hyperwm's tracking.
            self.float_new_window(id, &window);
        }

        // architecture.md §3.11 step 4: "cache the layout whenever it's
        // established or changes" -- covers a layout reached by ordinary
        // window creation, not just a hyperkey action (`dispatch` already
        // does its own save for those). Without this, a Space's tiled
        // layout built purely by opening windows was never cached, so
        // `handle_space_change` would find no cache entry on a later
        // revisit and flatten it to floating (confirmed via manual
        // testing).
        self.save_current_space();
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
    /// (or none) rather than N repeated ones. No-ops entirely while
    /// [`DaemonState::mouse_down`] is `true` -- correction resumes as one
    /// sweep when [`DaemonState::set_mouse_down`] sees the button
    /// released, rather than fighting an in-progress hand drag/resize.
    pub(crate) fn handle_drift(&mut self, element: &AXUIElement) {
        if self.mouse_down {
            return;
        }
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
        Self::correct_if_drifted(element, target);
    }

    /// Left-mouse-button state (architecture.md §3.10), driven by
    /// `hyperwm_macos::mouse::install`'s tap. On release (`down ==
    /// false`), sweeps every currently tiled window once so whatever
    /// drifted during the drag/resize -- which `handle_drift` ignored
    /// entirely while the button was held -- gets corrected in one shot,
    /// regardless of which window(s) the drag actually touched.
    pub(crate) fn set_mouse_down(&mut self, down: bool) {
        self.mouse_down = down;
        if !down {
            self.correct_all_tiled_drift();
        }
    }

    fn correct_all_tiled_drift(&self) {
        let Some(visible) = self.current_tree_visible_frame() else {
            return;
        };
        for (id, target) in self.tree.layout(visible, self.config.gaps) {
            if let Some(element) = self.windows.element_for(id) {
                Self::correct_if_drifted(element, target);
            }
        }
    }

    /// Architecture.md §3.10 steps 1-3: compares `element`'s live rect to
    /// `target` and writes back only if they differ by more than
    /// `DRIFT_EPSILON`.
    fn correct_if_drifted(element: &AXUIElement, target: Rect) {
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

    /// Resolves one `ax::on_screen_windows()` entry to a [`WindowId`],
    /// registering it first if hyperwm doesn't already know it.
    /// Deliberately does *not* touch `self.tree`/`self.floating` either
    /// way -- see [`DaemonState::register_only`]'s doc comment for why:
    /// callers (`handle_space_change`, `auto_tile`) decide the
    /// classification themselves, once, after they know which Space's
    /// state they're actually building.
    ///
    /// Tries the stored `CGWindowID` correlation first (`WindowRegistry::
    /// id_for_cg_id`) -- a pure lookup, no AX calls, and immune to the rect-
    /// matching flakiness [`DaemonState::resolve_uncorrelated`] documents.
    /// Only a window whose `CGWindowID` has never been seen before falls
    /// through to that slower path, which also records the correlation
    /// (`WindowRegistry::set_cg_id`) so this fast path covers it from then
    /// on -- every later poll for the *same* window, tiled or not, no
    /// longer needs to rect-match it at all.
    ///
    /// Filters out a minimized window regardless of which path resolved
    /// it: confirmed via manual testing that a minimized window's Dock
    /// genie-effect thumbnail still passes `kCGWindowListOptionOnScreenOnly`
    /// (with the thumbnail's tiny bounds, not the real window's), which
    /// otherwise contaminates every single Space's on-screen set with
    /// whatever's minimized in the Dock at the time -- not specific to any
    /// one Space, so it should never count as "on" one.
    fn resolve_on_screen_window(&mut self, w: &ax::OnScreenWindow) -> Option<WindowId> {
        let id = match self.windows.id_for_cg_id(w.id) {
            Some(id) => id,
            None => {
                let id = self.resolve_uncorrelated(w.pid, w.rect)?;
                self.windows.set_cg_id(id, w.id);
                id
            }
        };
        let element = self.windows.element_for(id)?;
        if element.bool_attribute(kAXMinimizedAttribute).unwrap_or(false) {
            return None;
        }
        Some(id)
    }

    /// The pid+rect correlation [`DaemonState::resolve_on_screen_window`]
    /// falls back to for a `CGWindowID` it hasn't seen before -- there's no
    /// public API mapping a `CGWindowID` directly to an `AXUIElement`.
    /// Rect-matching is used only to *disambiguate*: when `pid` has
    /// exactly one candidate window (registered or not), that's returned
    /// unconditionally, without checking its rect against `cg_rect` at
    /// all. This matters in practice, not just in theory: `CGWindowList`'s
    /// bounds and AX's `position()`/`size()` aren't always pixel-identical
    /// for the same window (confirmed via manual testing: architecture.md
    /// §3.11's poll can catch a window mid-Space-switch-animation, and
    /// some apps' window chrome makes the two APIs disagree by more than a
    /// few pixels even at rest), so requiring a rect match for the common
    /// single-window-per-app case was intermittently failing to resolve a
    /// window hyperwm already knew perfectly well. Only a pid with
    /// *multiple* windows (e.g. several Finder windows) has any real
    /// ambiguity to resolve with a rect at all -- and even then, only
    /// once per window, ever, since the caller remembers the result.
    fn resolve_uncorrelated(&mut self, pid: pid_t, cg_rect: Rect) -> Option<WindowId> {
        let known: Vec<(WindowId, &AXUIElement)> = self.windows.ids_for_pid(pid).collect();
        if let [(id, _)] = known[..] {
            return Some(id);
        }
        if known.len() > 1 {
            if let Some(id) = known.iter().find_map(|&(id, element)| {
                let pos = element.position().ok()?;
                let size = element.size().ok()?;
                let live = Rect { x: pos.x, y: pos.y, width: size.width, height: size.height };
                rects_approx_eq(live, cg_rect, SPACE_RECT_EPSILON).then_some(id)
            }) {
                return Some(id);
            }
        }

        let app = AXUIElement::application(pid);
        let standard: Vec<AXUIElement> = app
            .element_array_attribute(kAXWindowsAttribute)
            .ok()?
            .into_iter()
            .filter(Self::is_standard_window)
            .collect();
        if let [window] = &standard[..] {
            return Some(self.register_only(pid, window.clone()));
        }
        let window = standard.into_iter().find(|w| {
            w.position().ok().zip(w.size().ok()).is_some_and(|(pos, size)| {
                let live = Rect { x: pos.x, y: pos.y, width: size.width, height: size.height };
                rects_approx_eq(live, cg_rect, SPACE_RECT_EPSILON)
            })
        })?;
        Some(self.register_only(pid, window))
    }

    /// Live `position()`/`size()` for every id in `ids` that still resolves
    /// (an id whose window has since closed is silently dropped -- it'll be
    /// gone from `self.tree`/`self.floating` too by the time anything reads
    /// this back, via `handle_window_destroyed`). Used to snapshot a
    /// Space's layout into [`DaemonState::space_cache`], and to build the
    /// window-ID set that snapshot is keyed on -- deliberately AX-derived
    /// rather than `ax::on_screen_windows()`'s `CGWindowList` bounds, since
    /// the *outgoing* Space's windows are no longer on screen by the time
    /// [`DaemonState::handle_space_change`] runs (the notification fires
    /// after the switch completes) -- AX position/size queries still work
    /// for a window on an inactive Space, unlike on-screen enumeration.
    fn rects_for(&self, ids: impl Iterator<Item = WindowId>) -> HashMap<WindowId, Rect> {
        ids.filter_map(|id| {
            let element = self.windows.element_for(id)?;
            let pos = element.position().ok()?;
            let size = element.size().ok()?;
            Some((id, Rect { x: pos.x, y: pos.y, width: size.width, height: size.height }))
        })
        .collect()
    }

    /// Startup-only: `main.rs`'s pre-existing-window adoption pass
    /// (`lifecycle::AdoptionPolicy::FloatOnly`) floats every window
    /// belonging to every already-running app, across *every* Space --
    /// `kAXWindowsAttribute` has no per-Space filter, only
    /// `ax::on_screen_windows()` does, and that's an on-screen query with
    /// nowhere to plug into `lifecycle::adopt_app`'s per-pid loop. Left
    /// alone, that seeds `self.floating` (and, on the first
    /// `handle_space_change`, `self.space_cache`'s baseline entry) with
    /// windows that were never actually on the daemon's starting Space --
    /// exactly the cross-Space contamination architecture.md §3.11 exists
    /// to prevent. This trims `self.floating` back down to just what's
    /// really on screen right now, once, after that startup pass finishes.
    /// Windows this drops stay registered (their `AXObserver` watch is
    /// unaffected) -- they're picked back up, still floating, the first
    /// time their actual Space is visited (`handle_space_change`'s passive
    /// adoption already handles that; nothing extra is needed here for it).
    pub(crate) fn establish_startup_space(&mut self) {
        let on_screen: BTreeSet<WindowId> = ax::on_screen_windows()
            .into_iter()
            .filter_map(|w| self.resolve_on_screen_window(&w))
            .collect();
        self.floating.retain(|id| on_screen.contains(id));
        self.save_current_space();
    }

    /// Snapshots whatever `self.tree`/`self.floating` currently represent
    /// into `self.space_cache`, keyed by their own window-ID set (not
    /// necessarily the Space that's active *now* -- see call sites).
    /// Architecture.md §3.11 step 4: "cache the layout whenever it's
    /// established or changes."
    fn save_current_space(&mut self) {
        let ids = self.tree.window_ids().into_iter().chain(self.floating.iter().copied());
        let rects = self.rects_for(ids);
        if rects.is_empty() {
            return; // Nothing tracked yet (e.g. daemon just started).
        }
        let key: BTreeSet<WindowId> = rects.keys().copied().collect();
        self.space_cache.insert(
            key,
            SpaceLayout { tree: self.tree.clone(), floating: self.floating.clone(), rects },
        );
    }

    /// Called on every `workspace::watch_space_changes` tick (poll-driven,
    /// not event-driven -- see that function's doc comment for why;
    /// architecture.md §3.11 originally specified
    /// `NSWorkspace.activeSpaceDidChangeNotification`, which never fired
    /// for this daemon in practice). Fires far more often than the
    /// on-screen set actually changes, so the `new_key == old_key` check
    /// below is the common case, not just an edge guard. Never issues an
    /// AX write -- a cache hit restores the remembered
    /// `self.tree`/`self.floating` verbatim (step 2), and a miss only
    /// registers not-yet-known on-screen windows, entirely floating (step
    /// 3's passive adoption) -- neither path resizes or repositions
    /// anything. `auto_tile` is the only action that forces a computed
    /// layout onto a Space.
    ///
    /// `self.tree`/`self.floating` are always "whichever Space's state was
    /// last established or restored" -- there's no persistent tree per
    /// Space (§3.11's explicit non-goal); returning to an unchanged Space
    /// is recognized purely by matching its window-ID set and rects against
    /// [`DaemonState::space_cache`], not by any Space identifier.
    ///
    /// A genuine transition only actually takes effect once confirmed
    /// stable across two consecutive polls -- see
    /// [`DaemonState::pending_space_change`]'s doc comment.
    pub(crate) fn handle_space_change(&mut self) {
        let on_screen = ax::on_screen_windows();
        let resolved: Vec<(WindowId, Rect)> = on_screen
            .iter()
            .filter_map(|w| self.resolve_on_screen_window(w).map(|id| (id, w.rect)))
            .collect();
        if on_screen.is_empty() {
            // `ax::on_screen_windows()` -- the raw `CGWindowList` read,
            // before any resolving/filtering -- can transiently come back
            // completely empty: confirmed via manual testing, three
            // consecutive empty polls right after daemon startup despite
            // real windows being on screen the whole time (the
            // CGWindowList connection hadn't warmed up yet). Gating on
            // this raw read rather than on `resolved.is_empty()` matters:
            // `resolved` can legitimately be empty while `on_screen` isn't
            // (every on-screen window minimized, say) and that's real
            // information worth committing below, not a misread. Since
            // this function never moves or resizes a window either way,
            // leaving the previously-tracked state in place until a real
            // (non-empty) raw reading comes back is harmless regardless of
            // whether this is a genuine empty-desktop Space or a transient
            // misread. Doesn't touch `pending_space_change`: a genuine
            // in-progress transition's debounce shouldn't be reset by one
            // stray empty tick landing in the middle of it.
            return;
        }
        let new_key: BTreeSet<WindowId> = resolved.iter().map(|(id, _)| *id).collect();

        let old_key: BTreeSet<WindowId> = self
            .tree
            .window_ids()
            .into_iter()
            .chain(self.floating.iter().copied())
            .collect();
        if new_key == old_key {
            self.pending_space_change = None;
            return; // Same window set as what's already active -- no Space actually changed.
        }
        // Debounce: a single poll can land mid-Space-switch-swipe-animation
        // (confirmed via manual testing: on a single-display Mac, a window
        // read a live x-coordinate roughly one screen-width off from its
        // settled position, then read correctly again half a second later)
        // -- only commit once the exact same resolution (same ids, matching
        // rects) repeats on the very next poll.
        let stable = self.pending_space_change.as_ref().is_some_and(|prev| {
            prev.len() == resolved.len()
                && prev.iter().all(|(id, rect)| {
                    resolved.iter().any(|(rid, rrect)| {
                        rid == id && rects_approx_eq(*rect, *rrect, SPACE_RECT_EPSILON)
                    })
                })
        });
        if !stable {
            self.pending_space_change = Some(resolved);
            return;
        }
        self.pending_space_change = None;
        self.commit_space_change(resolved, new_key);
    }

    /// The actual "switch to a different, now-confirmed Space" transition
    /// (architecture.md §3.11 steps 2-4): save the outgoing Space's
    /// tiled/floating state, then either restore a cache hit verbatim
    /// (step 2) or passively adopt `resolved` as the new state, entirely
    /// floating (step 3). No geometry writes either way. Shared by
    /// [`DaemonState::handle_space_change`] (only after its poll debounce
    /// confirms two consecutive matching reads) and
    /// [`DaemonState::reconcile_before_insert`] (immediately, no debounce
    /// needed -- see its doc comment for why a debounce doesn't apply
    /// there).
    fn commit_space_change(&mut self, resolved: Vec<(WindowId, Rect)>, new_key: BTreeSet<WindowId>) {
        // Save the outgoing Space's tiled/floating state before swapping
        // away from it (step 4).
        self.save_current_space();

        if let Some(cached) = self.space_cache.get(&new_key) {
            let unchanged = resolved.iter().all(|(id, rect)| {
                cached
                    .rects
                    .get(id)
                    .is_some_and(|cached_rect| rects_approx_eq(*cached_rect, *rect, SPACE_RECT_EPSILON))
            });
            if unchanged {
                // Step 2: cache hit, rects match -- restore the remembered
                // layout verbatim, no AX writes.
                self.tree = cached.tree.clone();
                self.floating = cached.floating.clone();
                return;
            }
        }

        // Step 3: no cache hit (or rects moved) -- adopt whatever's on
        // screen as the new known state, entirely floating. No geometry
        // writes.
        self.tree = Tree::new(
            self.config.tiling.max_tiled_windows,
            self.config.tiling.insert_heuristic,
            self.config.tiling.split_ratio_default,
        );
        self.floating = resolved.iter().map(|(id, _)| *id).collect();
        self.save_current_space();
    }

    /// Reconciles Space state immediately, with no debounce, right before
    /// [`DaemonState::handle_new_window`] decides whether to tile or float
    /// a genuinely new window. `kAXWindowCreatedNotification` is per-app,
    /// not per-Space -- it fires the instant a new window appears on
    /// *any* Space, which can be well before `watch_space_changes`'s next
    /// poll tick (confirmed via manual testing: up to ~1.5s of lag). Left
    /// unreconciled, a window created right after switching Spaces gets
    /// inserted against `self.tree`/`self.floating` from the *previous*
    /// Space -- e.g. joining its tree as a phantom extra member and
    /// shrinking everything to fit.
    ///
    /// No debounce, unlike `handle_space_change`'s poll: that debounce
    /// exists because a blind periodic sample can land mid-Space-switch-
    /// swipe-animation and see transient garbage. A window-created event
    /// is different -- it only fires because the user deliberately opened
    /// a window, which happens after they've already landed on a Space,
    /// not mid-swipe. Treating this single read as authoritative is safe.
    ///
    /// `exclude` is the new window's own id (already registered by the
    /// time this runs, so it resolves like any other on-screen window,
    /// but hasn't been classified as tiled or floating yet) -- excluded
    /// from both the comparison and the adopted set so it's left for
    /// `handle_new_window`'s own insertion decision instead of being
    /// swept into a passive floating adopt here.
    fn reconcile_before_insert(&mut self, exclude: WindowId) {
        let on_screen = ax::on_screen_windows();
        let resolved: Vec<(WindowId, Rect)> = on_screen
            .iter()
            .filter_map(|w| self.resolve_on_screen_window(w).map(|id| (id, w.rect)))
            .filter(|(id, _)| *id != exclude)
            .collect();
        if on_screen.is_empty() {
            // Same reasoning as `handle_space_change`'s identical guard --
            // gate on the raw `on_screen_windows()` read, not on
            // `resolved` (which excludes `exclude` on top of any
            // resolve/minimize filtering). Confirmed via manual testing
            // that gating on `resolved.is_empty()` instead was wrong here:
            // a window that's genuinely alone on a freshly-visited Space
            // makes `resolved` empty too (the only on-screen entry is the
            // excluded window itself) -- that's real information (this
            // Space's tracked state really should become empty before the
            // caller inserts the new window fresh), not a misread, and
            // skipping it left the *previous* Space's tree in place,
            // causing the new window to be inserted into it instead
            // (confirmed live: a Finder window opened alone on a new Space
            // ended up sharing a 2-way split with the previous Space's
            // tiled window). A genuinely empty raw `on_screen` read is the
            // actual inconclusive case this guards against.
            return;
        }
        let new_key: BTreeSet<WindowId> = resolved.iter().map(|(id, _)| *id).collect();

        let old_key: BTreeSet<WindowId> = self
            .tree
            .window_ids()
            .into_iter()
            .chain(self.floating.iter().copied())
            .collect();
        if new_key == old_key {
            return; // Already reconciled -- same Space as last tracked.
        }

        self.commit_space_change(resolved, new_key);
        // A synchronous reconcile just happened; don't let the ongoing
        // poll's debounce act on whatever it had pending from before this.
        self.pending_space_change = None;
    }

    /// `auto_tile` (architecture.md §3.11's explicit escape hatch): forces
    /// the current on-screen window set (up to `max_tiled_windows`, in
    /// on-screen enumeration order) into a freshly computed tree layout,
    /// via the same insertion rule as §3.3. This is the *only* place §3.11
    /// writes window geometry automatically -- everything else
    /// (`handle_space_change`) is deliberately passive.
    ///
    /// Whatever the existing tree held that isn't part of this on-screen
    /// set (e.g. a different Space's tiled windows) falls out of tiled
    /// tracking here; it isn't on screen right now, so there's nothing to
    /// move -- it's just reclassified floating, no jump, the same "no jump"
    /// rule `toggle_float`'s tiled->floating transition uses (architecture.md
    /// §3.8). Windows beyond `max_tiled_windows` get the same treatment
    /// (architecture.md §3.2).
    pub fn auto_tile(&mut self) {
        let on_screen = ax::on_screen_windows();
        let mut ids: Vec<WindowId> = Vec::with_capacity(on_screen.len());
        for w in &on_screen {
            if let Some(id) = self.resolve_on_screen_window(w) {
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        if ids.is_empty() {
            return;
        }
        // Same "try each until one resolves a display, else fall back to
        // the primary" robustness as `current_tree_visible_frame` -- not
        // just the first candidate, in case its AX position/size query
        // happens to fail transiently.
        let visible = ids
            .iter()
            .find_map(|&id| {
                self.windows.element_for(id).and_then(|e| self.visible_frame_for_window(e))
            })
            .or_else(screen::primary_visible_frame);
        let Some(visible) = visible else { return };

        let displaced: Vec<WindowId> =
            self.tree.window_ids().into_iter().filter(|id| !ids.contains(id)).collect();

        let mut new_tree = Tree::new(
            self.config.tiling.max_tiled_windows,
            self.config.tiling.insert_heuristic,
            self.config.tiling.split_ratio_default,
        );
        let mut overflow = Vec::new();
        for &id in &ids {
            self.floating.retain(|&f| f != id); // May already be floating; joining the tree now.
            match new_tree.insert(id, visible, self.config.gaps) {
                InsertOutcome::Inserted => {
                    if let (Some(pid), Some(element)) =
                        (self.windows.pid_for(id), self.windows.element_for(id).cloned())
                    {
                        self.watch(pid, &element, kAXWindowMovedNotification);
                        self.watch(pid, &element, kAXWindowResizedNotification);
                    }
                }
                InsertOutcome::CapReached => overflow.push(id),
            }
        }

        self.tree = new_tree;
        for id in displaced.into_iter().chain(overflow) {
            if !self.floating.contains(&id) {
                self.floating.push(id);
            }
        }
        self.apply_tree_layout();
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
            Action::AutoTile => self.auto_tile(),
        }
        // Architecture.md §3.11 step 4: "via any hyperkey action" -- keeps
        // the cache accurate for the currently active Space so a later
        // revisit restores this, not a stale snapshot from whenever it was
        // last adopted.
        self.save_current_space();
    }
}
