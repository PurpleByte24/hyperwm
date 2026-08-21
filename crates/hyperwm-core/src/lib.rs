//! Platform-independent tiling algorithm for hyperwm: the per-space BSP
//! tree, insertion, removal, directional-movement, and resize rules from
//! docs/architecture.md §3. No macOS dependencies — operates purely on
//! synthetic window rects, so it's testable without a real window server.

mod geometry;
mod id;
mod tree;

pub use geometry::{Direction, Gaps, Rect};
pub use id::WindowId;
pub use tree::{InsertHeuristic, InsertOutcome, SplitDirection, Tree};
