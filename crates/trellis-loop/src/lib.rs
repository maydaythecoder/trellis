//! The agent loop: one model, a flat tool list, iterating.
//!
//! Not a committee. Blast radius and checks are tools the model calls, not
//! stages in a pipeline written around it. See
//! `docs/adr/0005-loop-shape.md`.

mod adapter;
mod agent;
mod budget;
mod exec;
#[cfg(test)]
mod extension_tests;
mod extra_context;
mod gate;
mod governance;
mod hooks;
mod listing;
mod notes;
mod orchestrator;
pub mod partition;
mod registry;
mod scripted;
mod skills;
mod tools;
mod trust;
mod wire;

pub use adapter::{Compacted, HostedModel, RuntimeModel};
pub use agent::{
    Model, NotConverged, Outcome, PriorContext, RepoContext, Seed, Step, ToolCall, Transcript,
    Turn, VaultContext, run, run_seeded,
};
pub use budget::{Budget, CostRegime, Exhaustion, Spend};
pub use exec::default_shell;
pub use extra_context::{
    ContextFile, DEFAULT_ROLE, ExtraContext, FORCED_SKILL_HEAD, ForcedSkill, GROUNDING,
};
pub use gate::{
    APPROVABLE_TOOLS, ApprovalGate, ApprovalRequest, BashPolicyGate, DECLINED, EditPreview,
    GateDecision, GateFuture, GovernanceGate, ToolGate, preview_edit, tool_name,
};
pub use governance::{Governance, Rule};
pub use hooks::{
    BEFORE_RUN_CAP, DEFAULT_HOOK_TIMEOUT_S, DENY_EXIT, HookConfig, HookGate, Hooks,
    fence_before_run,
};
pub use orchestrator::{
    Dispatched, GateDenial, Handle, HumanDecision, Orchestrator, Request, names_trellis_dir,
};
pub use partition::{FailurePolicy, Partition, WorkUnit, plan};
pub use registry::{
    BUILTIN_TOOLS, BashSettings, CustomTool, NotesSource, ToolSelection, ToolSet, call_json,
};
pub use scripted::ScriptedModel;
pub use skills::{
    DISCLOSURE_CAP, Disclosed, InvalidSkill, SKILLS_DIR, Skill, SkillAccess, SkillCatalog,
    SkillPolicy, read_body, read_body_in,
};
pub use tools::{EditOutcome, ToolResult};
pub use trust::{Fingerprint, HashCache, TrustGuard, TrustItem, TrustState, TrustStore};
pub use wire::ToolCallWire;
