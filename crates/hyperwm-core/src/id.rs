/// Opaque handle to a window, unique within a single daemon run.
///
/// Used as the BSP tree's tie-break key (architecture.md §3.5 step 5): when
/// two movement candidates are exactly tied on angle and distance, the one
/// with the lower `WindowId` wins. The ordering has no meaning beyond that —
/// it doesn't correlate with creation order, z-order, or anything else.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct WindowId(pub u64);
