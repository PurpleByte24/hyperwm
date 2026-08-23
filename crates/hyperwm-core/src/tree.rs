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

    /// Insertion rule (architecture.md §3.3): targets the tiled leaf with
    /// the largest on-screen area (ties broken by lowest `WindowId`, same
    /// convention as §3.5 step 5), then splits that leaf per
    /// `insert_heuristic`. Deliberately not focus-based — see §3.3's
    /// rationale.
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

        let rects = self.layout(visible_frame, gaps);
        let mut best: Option<(WindowId, Rect, f64)> = None;
        for (id, rect) in &rects {
            let area = rect.width * rect.height;
            match best {
                None => best = Some((*id, *rect, area)),
                Some((best_id, _, best_area)) => {
                    if area > best_area + f64::EPSILON
                        || ((area - best_area).abs() <= f64::EPSILON && *id < best_id)
                    {
                        best = Some((*id, *rect, area));
                    }
                }
            }
        }
        let (target, target_rect, _) =
            best.expect("tree is non-empty, so layout() yields at least one leaf");

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

    /// Resize (architecture.md §3.6): walks up from the focused leaf to
    /// find the ancestor split whose adjustment actually moves *focused's
    /// own* edge in `direction`, and pushes that split's shared boundary
    /// in `direction`. Returns `false` if `focused` isn't tiled or no
    /// matching-axis ancestor exists at all.
    ///
    /// `ratio` is literally "how far across the split, left to right (or
    /// top to bottom), the shared boundary sits" -- `Direction::Right`/
    /// `Down` always pushes it further that way (`ratio` increases),
    /// `Left`/`Up` always pushes it the other way (`ratio` decreases).
    /// That's the *entire* rule for which way the number moves; there is
    /// deliberately no dependency on which side (first/second child)
    /// `focused` is on. Whether that reads as "focused grows" or "focused
    /// shrinks" falls out naturally from which side of the boundary it's
    /// on -- First's far edge is the boundary, so pushing it away
    /// (Right/Down) grows First and shrinks Second; Second's near edge is
    /// the boundary, so pushing it away (Left/Up) grows Second and
    /// shrinks First. An earlier version of this method multiplied in an
    /// extra sign based on `focused`'s side, meant to make "focused
    /// grows" hold regardless of side -- that's wrong: it made
    /// Left/Right's effect depend on which side of the split focused was
    /// on, so the *same keypress* grew a window on one side and shrank
    /// the identical-looking window on the other (confirmed against
    /// manual testing: hyper+shift+h on a right-side window grew its
    /// neighbor instead of it, and hyper+shift+l shrank it instead of
    /// growing it -- see `resize_second_child_grows_toward_its_own_edge`,
    /// which replaced a test of the same shape that had asserted the old,
    /// backwards behavior).
    ///
    /// Picking *which* split to push is a separate concern from the sign
    /// above, and still matters: a split's shared boundary is only
    /// focused's own edge in `direction` when focused sits on the side
    /// that boundary is *ahead of* for that direction -- First for
    /// Right/Down, Second for Left/Up. Two splits on the same axis can
    /// nest (`Root: Vertical(A: Vertical(w1, w3), B: w2)`), and the
    /// *nearest* axis-matching ancestor isn't always the right one: w3's
    /// true right edge is the *outer* Root|B boundary, not the *inner*
    /// A's w1|w3 boundary. So the walk up prefers the nearest
    /// axis-matching ancestor where focused is on the correct side for
    /// `direction`; if none exists anywhere on the path (focused has no
    /// real edge to move that way at any level -- e.g. it's flush against
    /// the screen edge, the `w1`/2-window case in
    /// `resize_first_child_falls_back_to_only_available_split`), it falls
    /// back to the nearest axis-matching ancestor regardless of side, so
    /// a press still does something rather than a silent no-op whenever
    /// any matching-axis split exists (`resize_clamps_to_min_max_ratio`
    /// covers this fallback case).
    ///
    /// # Panics
    ///
    /// Never in practice: the internal `.unwrap()`s only guard an
    /// invariant that holds once `find_path` has confirmed `focused` is
    /// in this tree (every prefix of its path then addresses a `Split`).
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
        let preferred_side = if base_sign > 0.0 { Side::First } else { Side::Second };

        let mut fallback_depth = None;
        let mut chosen_depth = None;
        for depth in (0..path.len()).rev() {
            let node = get_mut_at_path(self.root.as_mut().unwrap(), &path[..depth]);
            let Node::Split { direction: split_axis, .. } = node else {
                unreachable!("a path prefix always addresses a Split")
            };
            if *split_axis != required_axis {
                continue;
            }
            if fallback_depth.is_none() {
                fallback_depth = Some(depth);
            }
            if path[depth] == preferred_side {
                chosen_depth = Some(depth);
                break;
            }
        }

        let Some(depth) = chosen_depth.or(fallback_depth) else {
            return false;
        };
        let node = get_mut_at_path(self.root.as_mut().unwrap(), &path[..depth]);
        let Node::Split { ratio, .. } = node else {
            unreachable!("a path prefix always addresses a Split")
        };
        *ratio = (*ratio + base_sign * step).clamp(min_ratio, max_ratio);
        true
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

    // Ratio arithmetic in the resize tests below composes several
    // non-exact-in-binary steps (e.g. 0.5 - 0.01), so exact `assert_eq!`
    // isn't reliable there the way it is for the clean halves/tenths
    // elsewhere in this file -- this is the tolerance-based equivalent.
    fn assert_rect_approx(actual: Rect, expected: Rect) {
        let close = |a: f64, b: f64| (a - b).abs() < 1e-9;
        assert!(
            close(actual.x, expected.x)
                && close(actual.y, expected.y)
                && close(actual.width, expected.width)
                && close(actual.height, expected.height),
            "expected {expected:?}, got {actual:?}"
        );
    }

    // (a) Symmetric 2x2 grid.
    #[test]
    fn symmetric_2x2_grid() {
        let mut t = tree(4);
        let (w1, w2, w3, w4) = (id(1), id(2), id(3), id(4));

        assert_eq!(t.insert(w1, SCREEN, NO_GAPS), InsertOutcome::Inserted);
        assert_eq!(t.insert(w2, SCREEN, NO_GAPS), InsertOutcome::Inserted); // vertical: w1|w2
        assert_eq!(t.insert(w3, SCREEN, NO_GAPS), InsertOutcome::Inserted); // w1 (400x600) -> horizontal
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
        t.insert(w2, SCREEN, NO_GAPS); // vertical: w1 (left 400x600) | w2 (right 400x600)
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
        t.insert(w2, SCREEN, NO_GAPS); // vertical: w1 (left) | w2 (right)
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
        t.insert(w2, SCREEN, NO_GAPS);
        t.insert(w3, SCREEN, NO_GAPS);
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
        assert_eq!(t.insert(w2, SCREEN, NO_GAPS), InsertOutcome::Inserted);
        assert_eq!(t.insert(w3, SCREEN, NO_GAPS), InsertOutcome::CapReached);
        assert_eq!(t.tiled_count(), 2);
        assert!(!t.contains(w3));
    }

    // Regression coverage for `examples/config.toml`'s actual default
    // (max_tiled_windows = 4, not (e)'s max = 2): all 4 windows up to the
    // cap must be accepted -- the 4th insertion (bringing the count from 3
    // to 4) is still `Inserted`, only the 5th is `CapReached`.
    #[test]
    fn insert_accepts_windows_up_to_default_cap_of_four() {
        let mut t = tree(4);
        let (w1, w2, w3, w4, w5) = (id(1), id(2), id(3), id(4), id(5));
        assert_eq!(t.insert(w1, SCREEN, NO_GAPS), InsertOutcome::Inserted);
        assert_eq!(t.insert(w2, SCREEN, NO_GAPS), InsertOutcome::Inserted);
        assert_eq!(t.insert(w3, SCREEN, NO_GAPS), InsertOutcome::Inserted);
        assert_eq!(t.insert(w4, SCREEN, NO_GAPS), InsertOutcome::Inserted);
        assert_eq!(t.tiled_count(), 4);
        assert!(t.contains(w4));

        assert_eq!(t.insert(w5, SCREEN, NO_GAPS), InsertOutcome::CapReached);
        assert_eq!(t.tiled_count(), 4);
        assert!(!t.contains(w5));
    }

    // (f) Removal/collapse when a non-root leaf closes.
    #[test]
    fn remove_non_root_leaf_collapses_to_sibling() {
        let mut t = tree(4);
        let (w1, w2, w3) = (id(1), id(2), id(3));
        t.insert(w1, SCREEN, NO_GAPS);
        t.insert(w2, SCREEN, NO_GAPS); // vertical: w1 | w2
        t.insert(w3, SCREEN, NO_GAPS); // w1 -> horizontal: w1 top-left | w3 bottom-left

        assert!(t.remove(w3));
        assert_eq!(t.tiled_count(), 2);
        assert!(!t.contains(w3));

        // Collapsed back to exactly the 2-window layout.
        let mut expected = tree(4);
        expected.insert(w1, SCREEN, NO_GAPS);
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
        t.insert(w2, SCREEN, NO_GAPS); // 400x600, taller than wide, but forced vertical
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
        t.insert(w2, SCREEN, NO_GAPS); // vertical: w1 (first/left) | w2 (second/right)

        // w1 is the first child, so it's the *preferred* split for
        // Direction::Left too (there's nothing else to fall back to in a
        // 2-window tree): pressing Left always decreases the ratio, and
        // decreasing the first child's share shrinks it.
        for _ in 0..20 {
            t.resize(w1, Direction::Left, 0.1, 0.1, 0.9);
        }
        let rects = t.layout(SCREEN, NO_GAPS);
        assert!((rect_of(&rects, w1).width - 80.0).abs() < 1e-9); // 800 * 0.1
    }

    // w2 (second child) pressing Left is the *preferred* case for that
    // direction (Second's near edge -- its own left edge -- is this
    // split's shared boundary): the boundary moves left, growing w2. This
    // replaced a test that asserted the opposite (w2 shrinking to 320) --
    // that was the pre-fix "side_sign" bug: it made resize's grow/shrink
    // outcome depend on which side of the split focused was on, so
    // hyper+shift+h on a right-side window grew its *neighbor* instead of
    // it. See Tree::resize's doc comment for the full story.
    #[test]
    fn resize_second_child_grows_toward_its_own_edge() {
        let mut t = tree(4);
        let (w1, w2) = (id(1), id(2));
        t.insert(w1, SCREEN, NO_GAPS);
        t.insert(w2, SCREEN, NO_GAPS); // vertical: w1 (first) | w2 (second)

        assert!(t.resize(w2, Direction::Left, 0.1, 0.1, 0.9));
        let rects = t.layout(SCREEN, NO_GAPS);
        // Ratio (first child's share) decreases 0.5 -> 0.4, so w2 (second)
        // grows from 400 to 480 -- its own left edge moved left.
        assert!((rect_of(&rects, w1).width - 320.0).abs() < 1e-9); // 800 * 0.4
        assert!((rect_of(&rects, w2).width - 480.0).abs() < 1e-9); // 800 * 0.6

        // Direction::Right on w2 is the *fallback* case (w2 has no right-
        // adjacent boundary at all -- it's flush against the screen edge):
        // the ratio still just increases (base_sign alone, no side
        // dependence), shrinking w2 back toward its original size.
        assert!(t.resize(w2, Direction::Right, 0.1, 0.1, 0.9));
        let rects = t.layout(SCREEN, NO_GAPS);
        assert!((rect_of(&rects, w1).width - 400.0).abs() < 1e-9);
        assert!((rect_of(&rects, w2).width - 400.0).abs() < 1e-9);
    }

    // Reproduces the "grows in the opposite direction" bug reported from
    // manual verification: a *nested same-axis* split, where the nearest
    // enclosing split matching the requested axis is NOT the one whose
    // shared boundary is actually the focused window's edge in that
    // direction.
    //
    // Tree shape (forced with `always_vertical` so two Vertical splits
    // nest directly, which `aspect_ratio` can also produce on a wide
    // enough screen/column -- this isn't a heuristic-specific bug):
    //
    //   Root: Vertical(ratio .5)
    //   ├─ First: Vertical(ratio .5)     <- inner split
    //   │   ├─ First:  w1  (x: 0-200)
    //   │   └─ Second: w3  (x: 200-400)
    //   └─ Second: w2  (x: 400-800)
    //
    // w3's *true* right edge is at x=400 (the outer Root split's
    // boundary, shared with w2) -- w3 is the rightmost leaf of the whole
    // left half. Its left edge (x=200, shared with w1) is the inner
    // split's boundary. Pressing resize-right on w3 must move x=400
    // rightward (into w2) via the *outer* split, not move x=200 leftward
    // (into w1) via the inner one -- the latter is what the pre-fix code
    // did: wider, but on the wrong side, exactly "grows it in the
    // opposite direction."
    #[test]
    fn resize_nested_same_axis_split_picks_outer_boundary_not_inner() {
        let mut t = Tree::new(4, InsertHeuristic::AlwaysVertical, 0.5);
        let (w1, w2, w3) = (id(1), id(2), id(3));
        t.insert(w1, SCREEN, NO_GAPS); // root
        t.insert(w2, SCREEN, NO_GAPS); // Root: Vertical, w1 (First) | w2 (Second)
        t.insert(w3, SCREEN, NO_GAPS); // w1 -> Vertical again: w1 (First) | w3 (Second)

        let before = t.layout(SCREEN, NO_GAPS);
        assert_eq!(rect_of(&before, w1), Rect { x: 0.0, y: 0.0, width: 200.0, height: 600.0 });
        assert_eq!(rect_of(&before, w3), Rect { x: 200.0, y: 0.0, width: 200.0, height: 600.0 });
        assert_eq!(rect_of(&before, w2), Rect { x: 400.0, y: 0.0, width: 400.0, height: 600.0 });

        assert!(t.resize(w3, Direction::Right, 0.1, 0.1, 0.9));
        let after = t.layout(SCREEN, NO_GAPS);
        // The outer Root|w2 boundary moves right (400 -> 480 = 800*0.1
        // more for the whole left group), at w2's expense -- the key
        // property: w3's *right* edge (shared with w2) moved right, which
        // is what "resize right" must do. w1 grows too (200 -> 240): it's
        // proportionally rescaled along with w3 because both live inside
        // the same outer branch whose share of the screen just grew --
        // that's inherent to adjusting a ratio further up the tree, not a
        // bug (architecture.md §3.6 adjusts one split's ratio, not a
        // compensating cascade across multiple splits to hold siblings'
        // absolute sizes fixed).
        //
        // Contrast with the pre-fix behavior: it adjusted the *inner*
        // w1|w3 split instead (side_sign(Second) * base_sign(Right) =
        // -1), which shrank w1 to 160 and grew w3 to 240 by moving w3's
        // *left* edge from 200 to 160 -- while its right edge, shared
        // with w2, stayed frozen at 400. Wider, but on the wrong side:
        // exactly "grows it in the opposite direction."
        assert_eq!(rect_of(&after, w1), Rect { x: 0.0, y: 0.0, width: 240.0, height: 600.0 });
        assert_eq!(rect_of(&after, w3), Rect { x: 240.0, y: 0.0, width: 240.0, height: 600.0 });
        assert_eq!(rect_of(&after, w2), Rect { x: 480.0, y: 0.0, width: 320.0, height: 600.0 });
    }

    // Same class of bug as the test above, but arising from the *default*
    // `aspect_ratio` heuristic on a wide screen with a genuinely mixed
    // 4-window layout (two Vertical splits nested, one Horizontal split
    // mixed in) -- not a forced `always_vertical` config. Confirms the
    // fix isn't specific to the synthetic all-vertical shape.
    //
    // Area-based insertion (architecture.md §3.3) can't reach this shape
    // through *unaided* sequential inserts -- with every split exactly
    // halved, the largest leaf after N windows is always one of the
    // earliest, most-coarsely-split branches, never a leaf two levels
    // deep. So this test builds the shape with `resize` calls between
    // inserts (same technique `asymmetric_move_right_then_left_...` above
    // uses to break symmetry deliberately), shrinking w2's share and
    // growing w3's until w3 is both the largest leaf *and* narrower than
    // it is tall:
    //
    //   Root: Vertical(ratio .7)
    //   ├─ First: Vertical(ratio .49)         <- inner, same axis as Root
    //   │   ├─ First:  w1  (x:     0-823.2)
    //   │   └─ Second: Horizontal(ratio .5)
    //   │       ├─ First:  w3  (x: 823.2-1680, y:   0-450)
    //   │       └─ Second: w4  (x: 823.2-1680, y: 450-900)
    //   └─ Second: w2  (x: 1680-2400)
    //
    // w3's true right edge (x=1680) is the outer Root|w2 boundary, two
    // levels up -- the nearest Vertical-axis ancestor (the inner split,
    // one level up) is on the wrong side, same as above.
    #[test]
    fn resize_nested_same_axis_split_from_aspect_ratio_heuristic_on_wide_screen() {
        let wide = Rect { x: 0.0, y: 0.0, width: 2400.0, height: 900.0 };
        let mut t = tree(4);
        let (w1, w2, w3, w4) = (id(1), id(2), id(3), id(4));
        t.insert(w1, wide, NO_GAPS); // root
        t.insert(w2, wide, NO_GAPS); // w1 (2400x900, wider) -> vertical: w1 | w2, 1200 each
        t.insert(w3, wide, NO_GAPS); // tied areas -> lowest id (w1, 1200x900, still wider) -> vertical again: w1 | w3, 600 each

        // Shrink w2 (grows the outer Root split's First share, the w1/w3
        // group) until that group's leaves outweigh w2...
        assert!(t.resize(w2, Direction::Right, 0.2, 0.1, 0.9)); // root ratio .5 -> .7
        // ...then shrink w1 (grows the inner split's Second share, w3)
        // until w3 -- specifically -- is the single largest leaf, and
        // still narrower than it is tall (823.2 < 1680*0.51 < 900).
        assert!(t.resize(w3, Direction::Left, 0.01, 0.1, 0.9)); // inner ratio .5 -> .49

        t.insert(w4, wide, NO_GAPS); // w3 (856.8x900, taller) -> horizontal: w3 | w4, 450 each

        let before = t.layout(wide, NO_GAPS);
        assert_rect_approx(rect_of(&before, w1), Rect { x: 0.0, y: 0.0, width: 823.2, height: 900.0 });
        assert_rect_approx(rect_of(&before, w3), Rect { x: 823.2, y: 0.0, width: 856.8, height: 450.0 });
        assert_rect_approx(rect_of(&before, w4), Rect { x: 823.2, y: 450.0, width: 856.8, height: 450.0 });
        assert_rect_approx(rect_of(&before, w2), Rect { x: 1680.0, y: 0.0, width: 720.0, height: 900.0 });

        assert!(t.resize(w3, Direction::Right, 0.1, 0.1, 0.9));
        let after = t.layout(wide, NO_GAPS);
        // Root ratio 0.7 -> 0.8: the left group's total width grows from
        // 1680 to 1920, at w2's expense -- w1 and w4 grow alongside w3
        // (same "adjusting one ratio scales its whole branch" reasoning
        // as the test above), but the key property holds: w3's right
        // edge (823.2+856.8=1680 before) moves to 940.8+979.2=1920,
        // matching the group's new boundary -- not frozen in place the
        // way the pre-fix code would have left it while shrinking w1
        // instead.
        assert_rect_approx(rect_of(&after, w1), Rect { x: 0.0, y: 0.0, width: 940.8, height: 900.0 });
        assert_rect_approx(rect_of(&after, w3), Rect { x: 940.8, y: 0.0, width: 979.2, height: 450.0 });
        assert_rect_approx(rect_of(&after, w4), Rect { x: 940.8, y: 450.0, width: 979.2, height: 450.0 });
        assert_rect_approx(rect_of(&after, w2), Rect { x: 1920.0, y: 0.0, width: 480.0, height: 900.0 });
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

    // Dedicated tie-break coverage for insertion (architecture.md §3.3
    // step 1): with two leaves of exactly equal area, the lower WindowId
    // is the insertion target -- same convention as move_direction's tie-
    // break (§3.5 step 5), and what makes symmetric_2x2_grid's leaf
    // choices deterministic above rather than incidental.
    #[test]
    fn insert_tie_break_picks_lowest_window_id() {
        let mut t = tree(4);
        let (w1, w2, w3) = (id(1), id(2), id(3));
        t.insert(w1, SCREEN, NO_GAPS); // root
        t.insert(w2, SCREEN, NO_GAPS); // vertical: w1 (left) | w2 (right), tied areas

        // w1 and w2 are both 400x600 -- an exact area tie. w3 must land
        // in w1's leaf (lower id), not w2's.
        t.insert(w3, SCREEN, NO_GAPS);
        let rects = t.layout(SCREEN, NO_GAPS);
        assert_eq!(rect_of(&rects, w1).height, 300.0);
        assert_eq!(rect_of(&rects, w3).height, 300.0);
        assert_eq!(rect_of(&rects, w2).height, 600.0);
    }
}
