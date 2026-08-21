/// A window or layout rect. Coordinates follow the screen's y-down
/// convention (origin top-left, y increases downward) — see
/// architecture.md §3.5 step 3's note on sign conventions.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    #[must_use]
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

/// One of the four `hyper+hjkl` directions (architecture.md §3.5, §3.6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// `gaps.outer` / `gaps.inner` (architecture.md §4).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gaps {
    pub outer: f64,
    pub inner: f64,
}
