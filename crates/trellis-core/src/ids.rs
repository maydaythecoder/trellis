//! Dense `u32` newtype identifiers.
//!
//! Every id is an index into a `Vec` owned by the `Index` (or, for
//! `PartitionId`, by the orchestrator). They are dense and stable only
//! within a single index generation: a rebuild may renumber them, which is
//! why nothing outside the process persists a raw id. `.trellis/index.bin`
//! stores the whole arena together so ids stay internally consistent.

macro_rules! dense_id {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Debug, serde::Serialize, serde::Deserialize)]
        pub struct $name(pub u32);

        impl $name {
            #[inline]
            pub fn index(self) -> usize {
                self.0 as usize
            }

            #[inline]
            pub fn from_index(i: usize) -> Self {
                debug_assert!(i <= u32::MAX as usize, "id space exhausted");
                Self(i as u32)
            }
        }
    };
}

dense_id!(
    /// A `.py` file inside the repository root.
    FileId
);
dense_id!(
    /// A first-party function, method, or class definition.
    SymbolId
);
dense_id!(
    /// A single syntactic call expression.
    CallSiteId
);
dense_id!(
    /// A group of work units whose blast radii and files are disjoint from
    /// every other partition's. See `docs/adr/0005-loop-shape.md`.
    PartitionId
);

/// One invocation of `trellis run`. Names the trail file and the worktree
/// directory, so it is a string rather than a dense index.
#[derive(Clone, PartialEq, Eq, Hash, Debug, serde::Serialize, serde::Deserialize)]
pub struct RunId(pub String);

impl std::fmt::Display for RunId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
