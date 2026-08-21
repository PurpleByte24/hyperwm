//! Per-space BSP tree: insertion, removal, directional movement, resize.
//! See docs/architecture.md §3 for the behavioral spec this implements.

use crate::geometry::{Direction, Gaps, Rect};
use crate::id::WindowId;

/// A BSP split's orientation (architecture.md §3.1). `Vertical` means the
/// split *line* is vertical, so children sit left/right; `Horizontal` means
/// the split line is horizontal, so children sit top/bottom.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

/// `tiling.insert_heuristic` (architecture.md §3.3, examples/config.toml).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertHeuristic {
    AspectRatio,
    AlwaysVertical,
    AlwaysHorizontal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InsertOutcome {
    Inserted,
    /// `max_tiled_windows` already reached; window was not inserted
    /// (architecture.md §3.2 — this is defined behavior, not an error).
    CapReached,
}

#[derive(Debug, Clone)]
enum Node {
    Leaf(WindowId),
    Split {
        direction: SplitDirection,
        /// Fraction of space allocated to `first`. 0.0–1.0.
        ratio: f64,
        first: Box<Node>,
        second: Box<Node>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Side {
    First,
    Second,
}

fn contains(node: &Node, id: WindowId) -> bool {
    match node {
        Node::Leaf(x) => *x == id,
        Node::Split { first, second, .. } => contains(first, id) || contains(second, id),
    }
}

fn count_leaves(node: &Node) -> usize {
    match node {
        Node::Leaf(_) => 1,
        Node::Split { first, second, .. } => count_leaves(first) + count_leaves(second),
    }
}

fn collect_ids(node: &Node, out: &mut Vec<WindowId>) {
    match node {
        Node::Leaf(id) => out.push(*id),
        Node::Split { first, second, .. } => {
            collect_ids(first, out);
            collect_ids(second, out);
        }
    }
}

/// DFS for `target`, recording the path of child choices from the root down
/// to its leaf. Every window ID is unique in the tree, so this path (if
/// found) is the only one.
fn find_path(node: &Node, target: WindowId, path: &mut Vec<Side>) -> bool {
    match node {
        Node::Leaf(id) => *id == target,
        Node::Split { first, second, .. } => {
            path.push(Side::First);
            if find_path(first, target, path) {
                return true;
            }
            path.pop();
            path.push(Side::Second);
            if find_path(second, target, path) {
                return true;
            }
            path.pop();
            false
        }
    }
}

fn get_mut_at_path<'a>(node: &'a mut Node, path: &[Side]) -> &'a mut Node {
    match path.split_first() {
        None => node,
        Some((head, rest)) => match node {
            Node::Split { first, second, .. } => {
                let child = match head {
                    Side::First => first.as_mut(),
                    Side::Second => second.as_mut(),
                };
                get_mut_at_path(child, rest)
            }
            Node::Leaf(_) => unreachable!("path is longer than the tree's depth"),
        },
    }
}

fn layout_node(node: &Node, rect: Rect, inner_gap: f64, out: &mut Vec<(WindowId, Rect)>) {
    match node {
        Node::Leaf(id) => out.push((*id, rect)),
        Node::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            let half_gap = inner_gap / 2.0;
            let (first_rect, second_rect) = match direction {
                SplitDirection::Vertical => {
                    let first_w = (rect.width * ratio - half_gap).max(0.0);
                    let second_w = (rect.width * (1.0 - ratio) - half_gap).max(0.0);
                    (
                        Rect {
                            x: rect.x,
                            y: rect.y,
                            width: first_w,
                            height: rect.height,
                        },
                        Rect {
                            x: rect.x + rect.width * ratio + half_gap,
                            y: rect.y,
                            width: second_w,
                            height: rect.height,
                        },
                    )
                }
                SplitDirection::Horizontal => {
                    let first_h = (rect.height * ratio - half_gap).max(0.0);
                    let second_h = (rect.height * (1.0 - ratio) - half_gap).max(0.0);
                    (
                        Rect {
                            x: rect.x,
                            y: rect.y,
                            width: rect.width,
                            height: first_h,
                        },
                        Rect {
                            x: rect.x,
                            y: rect.y + rect.height * ratio + half_gap,
                            width: rect.width,
                            height: second_h,
                        },
                    )
                }
            };
            layout_node(first, first_rect, inner_gap, out);
            layout_node(second, second_rect, inner_gap, out);
        }
    }
}

/// One space's tiled-window tree (architecture.md §3.1). Floating windows
/// are not represented here — they live in a flat list owned by the caller.
pub struct Tree {
    root: Option<Node>,
    max_tiled_windows: usize,
    insert_heuristic: InsertHeuristic,
    split_ratio_default: f64,
    /// The window currently focused, tiled or not (may be `None`, or a
    /// floating window not present in this tree at all).
    current_focus: Option<WindowId>,
    /// Tiled windows this tree has seen focused, most-recent-first. Used by
    /// the insertion rule's fallback chain (architecture.md §3.3 step 1) and
    /// pruned whenever a window leaves the tree.
    focus_history: Vec<WindowId>,
}

impl Tree {
    #[must_use]
    pub fn new(
        max_tiled_windows: usize,
        insert_heuristic: InsertHeuristic,
        split_ratio_default: f64,
    ) -> Self {
        Self {
            root: None,
            max_tiled_windows,
            insert_heuristic,
            split_ratio_default,
            current_focus: None,
            focus_history: Vec::new(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.root.is_none()
    }

    #[must_use]
    pub fn tiled_count(&self) -> usize {
        self.root.as_ref().map_or(0, count_leaves)
    }

    #[must_use]
    pub fn contains(&self, id: WindowId) -> bool {
        self.root.as_ref().is_some_and(|r| contains(r, id))
    }

    #[must_use]
    pub fn window_ids(&self) -> Vec<WindowId> {
        let mut out = Vec::new();
        if let Some(root) = &self.root {
            collect_ids(root, &mut out);
        }
        out
    }

    /// Computes each tiled window's on-screen rect from the tree's current
    /// shape, given the space's visible frame (already excluding menu bar /
    /// Dock) and configured gaps (architecture.md §4).
    #[must_use]
    pub fn layout(&self, visible_frame: Rect, gaps: Gaps) -> Vec<(WindowId, Rect)> {
        let mut out = Vec::new();
        let Some(root) = &self.root else {
            return out;
        };
        let inset = Rect {
            x: visible_frame.x + gaps.outer,
            y: visible_frame.y + gaps.outer,
            width: (visible_frame.width - 2.0 * gaps.outer).max(0.0),
            height: (visible_frame.height - 2.0 * gaps.outer).max(0.0),
        };
        layout_node(root, inset, gaps.inner, &mut out);
        out
    }

    /// Records the currently focused window. `id` may be a tiled window in
    /// this tree, a floating window, or `None` — only tiled-window focus
    /// updates `focus_history` (architecture.md §3.3 step 1's fallback
    /// chain).
    pub fn set_focus(&mut self, id: Option<WindowId>) {
        self.current_focus = id;
        if let Some(id) = id {
            if self.contains(id) {
                self.focus_history.retain(|&x| x != id);
                self.focus_history.insert(0, id);
            }
        }
    }

    /// architecture.md §3.3 step 1's target-leaf selection, minus the
    /// empty-tree case (handled directly in `insert`).
    fn target_leaf_for_insert(&self) -> Option<WindowId> {
        if let Some(focused) = self.current_focus {
            if self.contains(focused) {
                return Some(focused);
            }
        }
        self.focus_history
            .iter()
            .copied()
            .find(|&id| self.contains(id))
    }

    /// Insertion rule (architecture.md §3.3). Applies the leaf-selection
    /// fallback chain, then splits that leaf per `insert_heuristic`.
    ///
    /// # Panics
    ///
    /// Never in practice: the internal `.expect()`s only guard invariants
    /// that hold once the tree is confirmed non-empty (a target leaf always
    /// exists and is always present in that leaf's own layout).
    pub fn insert(&mut self, window: WindowId, visible_frame: Rect, gaps: Gaps) -> InsertOutcome {
        if self.tiled_count() >= self.max_tiled_windows {
            return InsertOutcome::CapReached;
        }

        if self.root.is_none() {
            self.root = Some(Node::Leaf(window));
            return InsertOutcome::Inserted;
        }

        // Neither `current_focus` nor `focus_history` names a tiled window
        // still in this (non-empty) tree — e.g. nothing was ever focused via
        // `set_focus`. architecture.md §3.3 doesn't cover this case (in
        // practice the daemon always has AX focus info); fall back to the
        // lowest window ID for a deterministic pick, echoing the tie-break
        // convention in §3.5 step 5.
        let target = self
            .target_leaf_for_insert()
            .or_else(|| self.window_ids().into_iter().min())
            .expect("tree is non-empty, so it has at least one leaf");

        let rects = self.layout(visible_frame, gaps);
        let target_rect = rects
            .iter()
            .find(|(id, _)| *id == target)
            .map(|(_, r)| *r)
            .expect("target leaf is in this tree, so layout() must include it");

        let direction = match self.insert_heuristic {
            InsertHeuristic::AlwaysVertical => SplitDirection::Vertical,
            InsertHeuristic::AlwaysHorizontal => SplitDirection::Horizontal,
            InsertHeuristic::AspectRatio => {
                if target_rect.width >= target_rect.height {
                    SplitDirection::Vertical
                } else {
                    SplitDirection::Horizontal
                }
            }
        };

        let mut path = Vec::new();
        find_path(self.root.as_ref().unwrap(), target, &mut path);
        let leaf = get_mut_at_path(self.root.as_mut().unwrap(), &path);
        let Node::Leaf(existing) = *leaf else {
            unreachable!("find_path always locates a Leaf")
        };
        *leaf = Node::Split {
            direction,
            ratio: self.split_ratio_default,
            first: Box::new(Node::Leaf(existing)),
            second: Box::new(Node::Leaf(window)),
        };

        InsertOutcome::Inserted
    }

    /// Removal rule (architecture.md §3.4): the closed window's leaf is
    /// dropped and its sibling takes the place of their shared parent.
    /// Returns `false` if `window` isn't a tiled window in this tree.
    ///
    /// # Panics
    ///
    /// Never in practice: the internal `.unwrap()` only guards an invariant
    /// that holds once `find_path` has confirmed `window`'s leaf has a
    /// parent (i.e. it isn't the root).
    pub fn remove(&mut self, window: WindowId) -> bool {
        let Some(root) = self.root.as_ref() else {
            return false;
        };
        let mut path = Vec::new();
        if !find_path(root, window, &mut path) {
            return false;
        }

        if path.is_empty() {
            // The removed leaf was the tree's only window.
            self.root = None;
        } else {
            let removed_side = path[path.len() - 1];
            let parent_ref = get_mut_at_path(self.root.as_mut().unwrap(), &path[..path.len() - 1]);
            // Take ownership of the parent Split so its children can be
            // moved out by value; the placeholder is immediately discarded.
            let parent_owned = std::mem::replace(parent_ref, Node::Leaf(window));
            let Node::Split { first, second, .. } = parent_owned else {
                unreachable!("a leaf's parent is always a Split")
            };
            let sibling = match removed_side {
                Side::First => *second,
                Side::Second => *first,
            };
            *parent_ref = sibling;
        }

        self.focus_history.retain(|&id| id != window);
        if self.current_focus == Some(window) {
            self.current_focus = None;
        }
        true
    }

    fn swap_leaf_contents(&mut self, a: WindowId, b: WindowId) {
        fn swap_in(node: &mut Node, a: WindowId, b: WindowId) {
            match node {
                Node::Leaf(id) => {
                    if *id == a {
                        *id = b;
                    } else if *id == b {
                        *id = a;
                    }
                }
                Node::Split { first, second, .. } => {
                    swap_in(first, a, b);
                    swap_in(second, a, b);
                }
            }
        }
        if let Some(root) = &mut self.root {
            swap_in(root, a, b);
        }
    }

    /// Directional movement (architecture.md §3.5): a history-free,
    /// geometry-only leaf-content swap. Returns `true` if a swap happened,
    /// `false` on the no-op edge case (no candidate in `direction`).
    ///
    /// # Panics
    ///
    /// Never in practice: the internal `.expect()` only guards an invariant
    /// that holds once `focused` is confirmed to be in this tree (it must
    /// then appear in that tree's own layout).
    pub fn move_direction(
        &mut self,
        focused: WindowId,
        direction: Direction,
        visible_frame: Rect,
        gaps: Gaps,
    ) -> bool {
        if !self.contains(focused) {
            return false;
        }

        let rects = self.layout(visible_frame, gaps);
        let (wx, wy) = rects
            .iter()
            .find(|(id, _)| *id == focused)
            .expect("focused is in this tree, so layout() must include it")
            .1
            .center();

        // Screen y-down: "up" decreases y, "down" increases y.
        let dir_vec = match direction {
            Direction::Left => (-1.0, 0.0),
            Direction::Right => (1.0, 0.0),
            Direction::Up => (0.0, -1.0),
            Direction::Down => (0.0, 1.0),
        };
        let cos_45 = std::f64::consts::FRAC_1_SQRT_2;

        let mut best: Option<(WindowId, f64)> = None;
        for (id, rect) in &rects {
            if *id == focused {
                continue;
            }
            let (cx, cy) = rect.center();
            let (vx, vy) = (cx - wx, cy - wy);
            let dist = (vx * vx + vy * vy).sqrt();
            if dist <= f64::EPSILON {
                continue; // Coincident centers; not a meaningful direction.
            }
            let cos_theta = (vx * dir_vec.0 + vy * dir_vec.1) / dist;
            if cos_theta < cos_45 - f64::EPSILON {
                continue; // Outside the ±45° cone.
            }
            match best {
                None => best = Some((*id, dist)),
                Some((best_id, best_dist)) => {
                    if dist < best_dist - f64::EPSILON
                        || ((dist - best_dist).abs() <= f64::EPSILON && *id < best_id)
                    {
                        best = Some((*id, dist));
                    }
                }
            }
        }

        match best {
            Some((candidate, _)) => {
                self.swap_leaf_contents(focused, candidate);
                true
            }
            None => false,
        }
    }

    /// Resize (architecture.md §3.6): walks up from the focused leaf to the
    /// nearest ancestor split whose axis matches `direction`, then adjusts
    /// its ratio so the focused window grows toward `direction` and shrinks
    /// away from it, regardless of whether it's that split's first or
    /// second child. Returns `false` if `focused` isn't tiled or no
    /// matching-axis ancestor exists.
    ///
    /// # Panics
    ///
    /// Never in practice: the internal `.unwrap()` only guards an invariant
    /// that holds once `find_path` has confirmed `focused` is in this tree
    /// (every prefix of its path then addresses a `Split`).
    pub fn resize(
        &mut self,
        focused: WindowId,
        direction: Direction,
        step: f64,
        min_ratio: f64,
        max_ratio: f64,
    ) -> bool {
        let Some(root) = self.root.as_ref() else {
            return false;
        };
        let mut path = Vec::new();
        if !find_path(root, focused, &mut path) {
            return false;
        }

        let required_axis = match direction {
            Direction::Left | Direction::Right => SplitDirection::Vertical,
            Direction::Up | Direction::Down => SplitDirection::Horizontal,
        };
        let base_sign: f64 = match direction {
            Direction::Right | Direction::Down => 1.0,
            Direction::Left | Direction::Up => -1.0,
        };

        for depth in (0..path.len()).rev() {
            let node = get_mut_at_path(self.root.as_mut().unwrap(), &path[..depth]);
            let Node::Split {
                direction: split_axis,
                ratio,
                ..
            } = node
            else {
                unreachable!("a path prefix always addresses a Split")
            };
            if *split_axis != required_axis {
                continue;
            }
            let side_sign: f64 = match path[depth] {
                Side::First => 1.0,
                Side::Second => -1.0,
            };
            *ratio = (*ratio + base_sign * side_sign * step).clamp(min_ratio, max_ratio);
            return true;
        }
        false
    }
}

#[cfg(test)]
// Rects here come from clean, exact arithmetic (halves/tenths of round
// numbers), so exact equality is the intended assertion, not a bug.
#[allow(clippy::float_cmp)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect {
        x: 0.0,
        y: 0.0,
        width: 800.0,
        height: 600.0,
    };
    const NO_GAPS: Gaps = Gaps {
        outer: 0.0,
        inner: 0.0,
    };

    fn id(n: u64) -> WindowId {
        WindowId(n)
    }

    fn tree(max: usize) -> Tree {
        Tree::new(max, InsertHeuristic::AspectRatio, 0.5)
    }

    fn rect_of(rects: &[(WindowId, Rect)], w: WindowId) -> Rect {
        rects.iter().find(|(id, _)| *id == w).unwrap().1
    }

    // (a) Symmetric 2x2 grid.
    #[test]
    fn symmetric_2x2_grid() {
        let mut t = tree(4);
        let (w1, w2, w3, w4) = (id(1), id(2), id(3), id(4));

        assert_eq!(t.insert(w1, SCREEN, NO_GAPS), InsertOutcome::Inserted);
        t.set_focus(Some(w1));
        assert_eq!(t.insert(w2, SCREEN, NO_GAPS), InsertOutcome::Inserted); // vertical: w1|w2
        t.set_focus(Some(w1));
        assert_eq!(t.insert(w3, SCREEN, NO_GAPS), InsertOutcome::Inserted); // w1 (400x600) -> horizontal
        t.set_focus(Some(w2));
        assert_eq!(t.insert(w4, SCREEN, NO_GAPS), InsertOutcome::Inserted); // w2 (400x600) -> horizontal

        let rects = t.layout(SCREEN, NO_GAPS);
        assert_eq!(rects.len(), 4);
        assert_eq!(
            rect_of(&rects, w1),
            Rect {
                x: 0.0,
                y: 0.0,
                width: 400.0,
                height: 300.0
            }
        );
        assert_eq!(
            rect_of(&rects, w3),
            Rect {
                x: 0.0,
                y: 300.0,
                width: 400.0,
                height: 300.0
            }
        );
        assert_eq!(
            rect_of(&rects, w2),
            Rect {
                x: 400.0,
                y: 0.0,
                width: 400.0,
                height: 300.0
            }
        );
        assert_eq!(
            rect_of(&rects, w4),
            Rect {
                x: 400.0,
                y: 300.0,
                width: 400.0,
                height: 300.0
            }
        );
    }

    // (b) Asymmetric 2-left/1-right case (architecture.md §3.5): move
    // top-left window right, then back left, and land on the geometrically
    // correct leaf — recomputed fresh each time, never from memory.
    #[test]
    fn asymmetric_move_right_then_left_is_geometry_driven() {
        let mut t = tree(4);
        let (w1, w2, w3) = (id(1), id(2), id(3));

        t.insert(w1, SCREEN, NO_GAPS); // root
        t.set_focus(Some(w1));
        t.insert(w2, SCREEN, NO_GAPS); // vertical: w1 (left 400x600) | w2 (right 400x600)
        t.set_focus(Some(w1));
        t.insert(w3, SCREEN, NO_GAPS); // w1 (400x600, taller than wide) -> horizontal: w1 top-left | w3 bottom-left

        // Break the top/bottom symmetry so the return trip isn't a distance
        // tie: top-left grows to 60% of the left column's height.
        assert!(t.resize(w1, Direction::Down, 0.1, 0.1, 0.9));

        let before = t.layout(SCREEN, NO_GAPS);
        assert_eq!(
            rect_of(&before, w1),
            Rect {
                x: 0.0,
                y: 0.0,
                width: 400.0,
                height: 360.0
            }
        );
        assert_eq!(
            rect_of(&before, w3),
            Rect {
                x: 0.0,
                y: 360.0,
                width: 400.0,
                height: 240.0
            }
        );
        assert_eq!(
            rect_of(&before, w2),
            Rect {
                x: 400.0,
                y: 0.0,
                width: 400.0,
                height: 600.0
            }
        );

        // w1 (top-left) is the only candidate within the cone to the right:
        // w3 sits due south (90°, filtered out).
        assert!(t.move_direction(w1, Direction::Right, SCREEN, NO_GAPS));
        let mid = t.layout(SCREEN, NO_GAPS);
        assert_eq!(
            rect_of(&mid, w2),
            Rect {
                x: 0.0,
                y: 0.0,
                width: 400.0,
                height: 360.0
            }
        );
        assert_eq!(
            rect_of(&mid, w1),
            Rect {
                x: 400.0,
                y: 0.0,
                width: 400.0,
                height: 600.0
            }
        );
        assert_eq!(
            rect_of(&mid, w3),
            Rect {
                x: 0.0,
                y: 360.0,
                width: 400.0,
                height: 240.0
            }
        );

        // From the right column, both w2 (top-left) and w3 (bottom-left)
        // are within the leftward cone, but w2 is strictly nearer (taller
        // pane => its center sits closer to the right column's midline)
        // — not because w1 "remembers" starting there.
        assert!(t.move_direction(w1, Direction::Left, SCREEN, NO_GAPS));
        let after = t.layout(SCREEN, NO_GAPS);
        assert_eq!(
            rect_of(&after, w1),
            Rect {
                x: 0.0,
                y: 0.0,
                width: 400.0,
                height: 360.0
            }
        );
        assert_eq!(
            rect_of(&after, w2),
            Rect {
                x: 400.0,
                y: 0.0,
                width: 400.0,
                height: 600.0
            }
        );
        assert_eq!(
            rect_of(&after, w3),
            Rect {
                x: 0.0,
                y: 360.0,
                width: 400.0,
                height: 240.0
            }
        );
    }

    // Dedicated tie-break coverage (architecture.md §3.5 step 5): an exact
    // angle/distance tie resolves to the lower WindowId, deterministically.
    #[test]
    fn move_direction_tie_break_picks_lowest_window_id() {
        let mut t = tree(4);
        let (w1, w2, w3) = (id(1), id(2), id(3));

        t.insert(w1, SCREEN, NO_GAPS);
        t.set_focus(Some(w1));
        t.insert(w2, SCREEN, NO_GAPS); // vertical: w1 (left) | w2 (right)
        t.set_focus(Some(w1));
        t.insert(w3, SCREEN, NO_GAPS); // w1 -> horizontal, ratio 0.5: w1 top-left | w3 bottom-left

        // w2 (right column, full height) is exactly equidistant from w1
        // (top-left) and w3 (bottom-left) with a 0.5/0.5 split.
        assert!(t.move_direction(w2, Direction::Left, SCREEN, NO_GAPS));
        let rects = t.layout(SCREEN, NO_GAPS);
        // w1 has the lower ID, so w2 swaps into w1's (top-left) leaf.
        assert_eq!(
            rect_of(&rects, w2),
            Rect {
                x: 0.0,
                y: 0.0,
                width: 400.0,
                height: 300.0
            }
        );
        assert_eq!(
            rect_of(&rects, w1),
            Rect {
                x: 400.0,
                y: 0.0,
                width: 400.0,
                height: 600.0
            }
        );
        assert_eq!(
            rect_of(&rects, w3),
            Rect {
                x: 0.0,
                y: 300.0,
                width: 400.0,
                height: 300.0
            }
        );
    }

    // (c) Directional movement at a tree edge: no-op.
    #[test]
    fn move_direction_at_edge_is_noop() {
        let mut t = tree(4);
        let (w1, w2, w3, w4) = (id(1), id(2), id(3), id(4));
        t.insert(w1, SCREEN, NO_GAPS);
        t.set_focus(Some(w1));
        t.insert(w2, SCREEN, NO_GAPS);
        t.set_focus(Some(w1));
        t.insert(w3, SCREEN, NO_GAPS);
        t.set_focus(Some(w2));
        t.insert(w4, SCREEN, NO_GAPS); // symmetric 2x2, see symmetric_2x2_grid

        let before = t.layout(SCREEN, NO_GAPS);
        // w2 is top-right: nothing further right, nothing further up.
        assert!(!t.move_direction(w2, Direction::Right, SCREEN, NO_GAPS));
        assert!(!t.move_direction(w2, Direction::Up, SCREEN, NO_GAPS));
        let after = t.layout(SCREEN, NO_GAPS);
        assert_eq!(before, after);
    }

    // (d) Insertion when the tree is empty.
    #[test]
    fn insert_into_empty_tree_becomes_root() {
        let mut t = tree(4);
        let w1 = id(1);
        assert!(t.is_empty());
        assert_eq!(t.insert(w1, SCREEN, NO_GAPS), InsertOutcome::Inserted);
        assert!(!t.is_empty());
        assert_eq!(t.tiled_count(), 1);
        assert_eq!(t.window_ids(), vec![w1]);
    }

    // (e) Insertion when max_tiled_windows is reached.
    #[test]
    fn insert_beyond_cap_does_not_enter_tree() {
        let mut t = tree(2);
        let (w1, w2, w3) = (id(1), id(2), id(3));
        assert_eq!(t.insert(w1, SCREEN, NO_GAPS), InsertOutcome::Inserted);
        t.set_focus(Some(w1));
        assert_eq!(t.insert(w2, SCREEN, NO_GAPS), InsertOutcome::Inserted);
        assert_eq!(t.insert(w3, SCREEN, NO_GAPS), InsertOutcome::CapReached);
        assert_eq!(t.tiled_count(), 2);
        assert!(!t.contains(w3));
    }

    // (f) Removal/collapse when a non-root leaf closes.
    #[test]
    fn remove_non_root_leaf_collapses_to_sibling() {
        let mut t = tree(4);
        let (w1, w2, w3) = (id(1), id(2), id(3));
        t.insert(w1, SCREEN, NO_GAPS);
        t.set_focus(Some(w1));
        t.insert(w2, SCREEN, NO_GAPS); // vertical: w1 | w2
        t.set_focus(Some(w1));
        t.insert(w3, SCREEN, NO_GAPS); // w1 -> horizontal: w1 top-left | w3 bottom-left

        assert!(t.remove(w3));
        assert_eq!(t.tiled_count(), 2);
        assert!(!t.contains(w3));

        // Collapsed back to exactly the 2-window layout.
        let mut expected = tree(4);
        expected.insert(w1, SCREEN, NO_GAPS);
        expected.set_focus(Some(w1));
        expected.insert(w2, SCREEN, NO_GAPS);
        assert_eq!(t.layout(SCREEN, NO_GAPS), expected.layout(SCREEN, NO_GAPS));
    }

    // (g) Removal when the last window closes.
    #[test]
    fn remove_last_window_empties_tree() {
        let mut t = tree(4);
        let w1 = id(1);
        t.insert(w1, SCREEN, NO_GAPS);
        assert!(t.remove(w1));
        assert!(t.is_empty());
        assert_eq!(t.tiled_count(), 0);
        assert_eq!(t.window_ids(), Vec::new());
    }

    #[test]
    fn remove_unknown_window_is_noop() {
        let mut t = tree(4);
        let (w1, w2) = (id(1), id(2));
        t.insert(w1, SCREEN, NO_GAPS);
        assert!(!t.remove(w2));
        assert_eq!(t.tiled_count(), 1);
    }

    #[test]
    fn insert_heuristic_always_vertical_ignores_aspect_ratio() {
        let mut t = Tree::new(4, InsertHeuristic::AlwaysVertical, 0.5);
        let (w1, w2, w3) = (id(1), id(2), id(3));
        t.insert(w1, SCREEN, NO_GAPS);
        t.set_focus(Some(w1));
        t.insert(w2, SCREEN, NO_GAPS); // 400x600, taller than wide, but forced vertical
        t.set_focus(Some(w1));
        t.insert(w3, SCREEN, NO_GAPS);

        let rects = t.layout(SCREEN, NO_GAPS);
        // Three vertical strips, each full height.
        for (_, r) in &rects {
            assert_eq!(r.height, 600.0);
        }
    }

    #[test]
    fn insert_heuristic_always_horizontal_ignores_aspect_ratio() {
        let mut t = Tree::new(4, InsertHeuristic::AlwaysHorizontal, 0.5);
        let (w1, w2) = (id(1), id(2));
        t.insert(w1, SCREEN, NO_GAPS);
        t.set_focus(Some(w1));
        t.insert(w2, SCREEN, NO_GAPS);

        let rects = t.layout(SCREEN, NO_GAPS);
        for (_, r) in &rects {
            assert_eq!(r.width, 800.0);
        }
    }

    #[test]
    fn resize_clamps_to_min_max_ratio() {
        let mut t = tree(4);
        let (w1, w2) = (id(1), id(2));
        t.insert(w1, SCREEN, NO_GAPS);
        t.set_focus(Some(w1));
        t.insert(w2, SCREEN, NO_GAPS); // vertical: w1 (first/left) | w2 (second/right)

        // w1 is the first child: resize_left shrinks it (base_sign(-1) *
        // side_sign(first=+1) = -1 per the ratio each press).
        for _ in 0..20 {
            t.resize(w1, Direction::Left, 0.1, 0.1, 0.9);
        }
        let rects = t.layout(SCREEN, NO_GAPS);
        assert!((rect_of(&rects, w1).width - 80.0).abs() < 1e-9); // 800 * 0.1
    }

    #[test]
    fn resize_second_child_sign_is_mirrored() {
        let mut t = tree(4);
        let (w1, w2) = (id(1), id(2));
        t.insert(w1, SCREEN, NO_GAPS);
        t.set_focus(Some(w1));
        t.insert(w2, SCREEN, NO_GAPS); // vertical: w1 (first) | w2 (second)

        // w2 is the second child: the sign flips relative to a first-child
        // resize. base_sign(Left) = -1, side_sign(Second) = -1, so the
        // ratio (first child's share) *increases* by step here, same
        // direction key as first_child_resize_shrinks_it but opposite
        // effect on the ratio itself.
        assert!(t.resize(w2, Direction::Left, 0.1, 0.1, 0.9));
        let rects = t.layout(SCREEN, NO_GAPS);
        assert!((rect_of(&rects, w2).width - 320.0).abs() < 1e-9); // 800 * (1 - 0.6)
    }

    #[test]
    fn resize_wrong_axis_walks_up_or_noops() {
        let mut t = tree(4);
        let w1 = id(1);
        t.insert(w1, SCREEN, NO_GAPS);
        // No splits exist at all: no matching ancestor in any direction.
        assert!(!t.resize(w1, Direction::Left, 0.1, 0.1, 0.9));
        assert!(!t.resize(w1, Direction::Up, 0.1, 0.1, 0.9));
    }

    #[test]
    fn set_focus_on_float_falls_back_to_tiled_history() {
        let mut t = tree(4);
        let (w1, w2, floater) = (id(1), id(2), id(3));
        t.insert(w1, SCREEN, NO_GAPS);
        t.set_focus(Some(w1));
        t.insert(w2, SCREEN, NO_GAPS); // vertical: w1 (left) | w2 (right)

        t.set_focus(Some(w1));
        // Focus moves to a floating window not in this tree at all.
        t.set_focus(Some(floater));

        // Insertion should target w1 (most-recently-focused tiled leaf),
        // not fail or pick arbitrarily.
        let w3 = id(4);
        t.insert(w3, SCREEN, NO_GAPS);
        let rects = t.layout(SCREEN, NO_GAPS);
        // w1 was 400x600 (taller than wide) -> split horizontal.
        assert_eq!(rect_of(&rects, w1).height, 300.0);
        assert_eq!(rect_of(&rects, w3).height, 300.0);
    }
}
