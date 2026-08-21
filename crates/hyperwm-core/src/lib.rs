//! Platform-independent tiling algorithm for hyperwm: the per-space BSP
//! tree, insertion, removal, directional-movement, and resize rules from
//! docs/architecture.md §3. No macOS dependencies — operates purely on
//! synthetic window rects, so it's testable without a real window server.

// Per-item doc comments on every private field/helper would fight
// CLAUDE.md's "no comments unless the WHY is non-obvious" rule; names carry
// the "what" here, and the tree module's existing comments cover the "why"
// where it isn't obvious.
#![allow(clippy::missing_docs_in_private_items)]

mod geometry;
mod id;
mod tree;

pub use geometry::{Direction, Gaps, Rect};
pub use id::WindowId;
pub use tree::{InsertHeuristic, InsertOutcome, SplitDirection, Tree};
