//! The agent loop: one model, a flat tool list, iterating.
//!
//! Not a committee. Blast radius and checks are tools the model calls, not
//! stages in a pipeline written around it. See
//! `docs/adr/0005-loop-shape.md`.

mod adapter;
mod agent;
mod agent_clone;
mod budget;
mod exec;
#[cfg(test)]
mod extension_tests;
mod extra_context;
mod gate;
mod governance;
mod graph_tools;
mod hooks;
mod integrate;
mod isolate;
mod listing;
mod notes;
mod orchestrator;
pub mod partition;
mod registry;
mod scripted;
mod skills;
mod supervise;
mod tools;
mod trust;
mod wire;

pub use adapter::{CompactRequests, Compacted, HostedModel, RequestedCompaction, RuntimeModel};
pub use agent::{
    Model, NotConverged, Outcome, PriorContext, RadiusSite, RepoContext, Seed, Steer, Steering,
    SteeringLog, SteeringRx, Step, ToolCall, Transcript, Turn, VaultContext, run, run_seeded,
};
pub use agent_clone::{AgentClone, AgentCloneError, CarriedBack, NEW_FILE_WALK_CAP, NotCarried};
pub use budget::{Budget, BudgetTooSmall, CostRegime, Exhaustion, Spend};
pub use exec::{ExecError, default_shell};
pub use extra_context::{
    ContextFile, DEFAULT_ROLE, ExtraContext, FORCED_SKILL_HEAD, ForcedSkill, GROUNDING,
};
pub use gate::{
    APPROVABLE_TOOLS, ApprovalGate, ApprovalRequest, BashPolicyGate, DECLINED, EditPreview,
    GateDecision, GateFuture, GovernanceGate, ToolGate, preview_edit, tool_name,
};
pub use governance::{Governance, Rule};
pub use hooks::{
    BEFORE_RUN_CAP, DEFAULT_HOOK_TIMEOUT_S, DENY_EXIT, HookConfig, HookError, HookGate, Hooks,
    InputDecision, fence_before_run,
};
pub use integrate::{Integrated, Integration, SuspectedRecallGap};
pub use orchestrator::{
    Dispatched, GateDenial, Handle, HumanDecision, Orchestrator, Request, names_trellis_dir,
};
pub use partition::{FailurePolicy, Partition, WorkUnit, plan};
pub use registry::{
    BUILTIN_TOOLS, BashSettings, CustomTool, DEFAULT_CUSTOM_TIMEOUT_S, NotesSource, ToolCallError,
    ToolConfigError, ToolSelection, ToolSet, call_json,
};
pub use scripted::{ScriptError, ScriptedModel};
pub use skills::{
    DISCLOSURE_CAP, Disclosed, InvalidSkill, SKILLS_DIR, Skill, SkillAccess, SkillCatalog,
    SkillError, SkillPolicy, read_body, read_body_in,
};
pub use supervise::{
    PartitionFailure, PartitionReport, PartitionTask, SuperviseError, Supervised, Supervisor,
};
pub use tools::{EditOutcome, ToolResult};
pub use trust::{
    Fingerprint, HashCache, ItemChange, TrustDiagnosis, TrustGuard, TrustItem, TrustState,
    TrustStore,
};
pub use wire::ToolCallWire;
