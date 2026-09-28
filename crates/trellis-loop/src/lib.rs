//! The agent loop: one model, a flat tool list, iterating.
//!
//! Not a committee. Blast radius and checks are tools the model calls, not
//! stages in a pipeline written around it. See
//! `docs/adr/0005-loop-shape.md`.

mod adapter;
mod agent;
mod budget;
mod gate;
mod governance;
mod orchestrator;
pub mod partition;
mod scripted;
mod tools;
mod wire;

pub use adapter::RuntimeModel;
pub use agent::{
    Model, NotConverged, Outcome, PriorContext, RepoContext, Seed, Step, ToolCall, Transcript,
    Turn, VaultContext, run, run_seeded,
};
pub use budget::{Budget, CostRegime, Exhaustion, Spend};
pub use gate::{
    APPROVABLE_TOOLS, ApprovalGate, ApprovalRequest, DECLINED, EditPreview, GateDecision,
    GateFuture, GovernanceGate, ToolGate, preview_edit, tool_name,
};
pub use governance::{Governance, Rule};
pub use orchestrator::{Dispatched, Handle, HumanDecision, Orchestrator, Request};
pub use partition::{FailurePolicy, Partition, WorkUnit, plan};
pub use scripted::ScriptedModel;
pub use tools::{EditOutcome, ToolResult};
pub use wire::ToolCallWire;
