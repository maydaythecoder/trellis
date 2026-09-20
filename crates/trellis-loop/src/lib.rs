//! The agent loop: one model, a flat tool list, iterating.
//!
//! Not a committee. Blast radius and checks are tools the model calls, not
//! stages in a pipeline written around it. See
//! `docs/adr/0005-loop-shape.md`.

pub mod adapter;
pub mod agent;
pub mod budget;
pub mod orchestrator;
pub mod partition;
pub mod scripted;
pub mod tools;
pub mod wire;

pub use adapter::RuntimeModel;
pub use agent::{
    Model, NotConverged, Outcome, PriorContext, Step, ToolCall, Transcript, Turn, run,
    run_with_prior,
};
pub use budget::{Budget, CostRegime, Exhaustion, Spend};
pub use orchestrator::{Handle, Orchestrator, Request};
pub use partition::{FailurePolicy, Partition, WorkUnit, plan};
pub use scripted::ScriptedModel;
pub use tools::{EditOutcome, ToolResult};
pub use wire::ToolCallWire;
