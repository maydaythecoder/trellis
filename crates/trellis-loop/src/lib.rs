//! The agent loop: one model, a flat tool list, iterating.
//!
//! Not a committee. Blast radius and checks are tools the model calls, not
//! stages in a pipeline written around it. See
//! `docs/ADRs/0005-loop-shape.md`.

pub mod budget;
pub mod partition;
pub mod tools;

pub use budget::{Budget, CostRegime, Exhaustion, Spend};
pub use partition::{FailurePolicy, Partition, WorkUnit, plan};
pub use tools::{EditOutcome, ToolResult};
