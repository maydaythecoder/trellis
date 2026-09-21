//! The agent loop: one model, a flat tool list, iterating.
//!
//! Not a committee. Blast radius and checks are tools the model calls, not
//! stages in a pipeline written around it. See
//! `docs/adr/0005-loop-shape.md`.

mod adapter;
mod agent;
mod budget;
mod governance;
mod orchestrator;
pub mod partition;
mod scripted;
mod tools;
mod wire;

pub use adapter::RuntimeModel;
pub use agent::{
    Model, NotConverged, Outcome, PriorContext, RepoContext, Seed, Step, ToolCall, Transcript,
    Turn, run, run_seeded,
};
pub use budget::{Budget, CostRegime, Exhaustion, Spend};
pub use governance::{Governance, Rule};
pub use orchestrator::{Handle, Orchestrator, Request};
pub use partition::{FailurePolicy, Partition, WorkUnit, plan};
pub use scripted::ScriptedModel;
pub use tools::{EditOutcome, ToolResult};
pub use wire::ToolCallWire;
