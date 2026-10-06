//! The orchestrator: single owner of the index, serializer of writes.
//!
//! It owns the `Index` outright — no `Arc<RwLock<Index>>`, no shared mutable
//! graph. Everything else sends a message and awaits a `oneshot` reply, which
//! buys write/check serialization for free: the owner cannot process two
//! messages at once, so there is no lock anyone can forget to take.
//!
//! The cost is that a wide change serializes. That is a real ceiling and S5
//! says to measure it rather than assume it away.
//!
//! **Why an actor rather than `&mut Index` threaded through.** Agents are
//! concurrent tasks (v1.1) and checks are subprocesses, so the index has to be
//! reachable from several tasks at once. An actor makes "one writer" a
//! property of the topology; a borrow makes it a property of whoever wrote the
//! call chain.

use crate::agent::ToolCall;
use crate::gate::{GateDecision, GovernanceGate, ToolGate, tool_name};
use crate::hooks::{Hooks, TurnOutcome};
use crate::registry::ToolSet;
use crate::tools::{EditOutcome, ToolResult};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::sync::{mpsc, oneshot};
use trellis_checks::{CheckConfig, CheckKind};
use trellis_index::Session;

/// A request to the orchestrator. Every variant carries its own reply channel.
pub enum Request {
    BlastRadius {
        symbol: String,
        reply: oneshot::Sender<ToolResult>,
    },
    ReadFile {
        path: String,
        reply: oneshot::Sender<ToolResult>,
    },
    ApplyEdit {
        path: String,
        old: String,
        new: String,
        reply: oneshot::Sender<ToolResult>,
    },
    RunChecks {
        paths: Vec<String>,
        kinds: Vec<CheckKind>,
        reply: oneshot::Sender<ToolResult>,
    },
    /// A model-originated call, through the gates, with what a human decided
    /// about it if one was asked. The loop's own path.
    Call {
        call: ToolCall,
        /// What is left of the run's wall-clock budget as the loop sends
        /// the call, so a check it runs ends when the budget does
        /// (ADR 0004). `None`: the check's own `timeout_s` alone.
        budget_left: Option<Duration>,
        reply: oneshot::Sender<Dispatched>,
    },
    /// The governance audit the loop runs at convergence. **Not gated.**
    ///
    /// It is the harness's check on the model, not a call the model made, and
    /// a gate that could refuse it would decide convergence: the loop reads a
    /// refused audit as no audit, and a run whose audit never ran converged
    /// over whatever rule it broke. ADR 0018 says a gate cannot do that.
    Audit { reply: oneshot::Sender<ToolResult> },
    /// Files written during this run, for the rollback path and the trail.
    Touched { reply: oneshot::Sender<Vec<String>> },
    /// Whether the repository configured any governance rule at all.
    ///
    /// The convergence gate asks before auditing, so a repository with no
    /// rules gets no extra turn and no extra passing check. An audit of
    /// nothing is not evidence of anything and must not inflate the count a
    /// user reads at the end of a run.
    HasRules { reply: oneshot::Sender<bool> },
    /// Denials by named gates, for the trail (ADR 0023 §7).
    GateLog {
        reply: oneshot::Sender<Vec<GateDenial>>,
    },
}

/// A call a named gate refused: the pre_tool hook or the bash policy.
///
/// Kept here rather than on the loop's `Turn` because the gate decided it
/// here. `call` is the model call's position in the run, counted from 1 over
/// `Request::Call` only; the loop makes exactly one of those per turn and the
/// convergence audit goes by another door, so it is the turn number.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GateDenial {
    pub call: u32,
    /// The gate's name: `pre_tool` or `bash_policy`.
    pub gate: String,
    /// The tool and what it was aimed at, e.g. `bash rm -rf build`.
    pub target: String,
    pub reason: String,
}

/// A tool result, and the human decision that let it run or stopped it.
#[derive(Debug)]
pub struct Dispatched {
    pub result: ToolResult,
    /// For a radius, its call sites resolved to paths, lines and enclosing
    /// symbols, which only this side can do: the index and the files are
    /// here. Empty for every other result. Carried beside `result` rather
    /// than inside it, so what a `post_tool` hook receives is unchanged.
    pub radius_sites: Vec<crate::agent::RadiusSite>,
    /// `Some` only when a person was asked about this call.
    pub human: Option<HumanDecision>,
    /// Whether this call may have changed the working tree. See
    /// [`may_change_tree`] for the tool's part; a `pre_tool` or `post_tool`
    /// hook that changed the worktree (or whose effect could not be
    /// compared) also sets it, because a hook runs around every call — the
    /// passing check included — and would otherwise leave bytes behind that
    /// no check saw (ADR 0023, "after a pass").
    pub may_have_changed_tree: bool,
    /// Why trusted code no longer matches what was trusted, once the trust
    /// guard has found it so (ADR 0023 §5). Latched: every call after the
    /// first carries it too. The loop ends the run on it.
    pub trust_breach: Option<String>,
    /// The repository paths this call's carry-back wrote, in order: what a
    /// `bash` or custom-tool command changed that reached the repository
    /// (ADR 0032). Empty for every other tool, and for a command that
    /// changed nothing trellis carries back. The trail records it
    /// (ADR 0023 §7).
    pub changed: Vec<String>,
}

/// The tool's own part in [`Dispatched::may_have_changed_tree`], for a call
/// that ran. `wrote` is whether `edit` or `write_file` reached the point of
/// writing bytes, which is not the same as succeeding: a write that failed
/// half way, or an edit written and then refused by the index, still
/// changed the file.
///
/// Exhaustive on purpose, like `tool_result_variants_are_accounted_for`: a
/// new tool does not compile until someone decides here whether it writes,
/// and the answer for "not sure" is yes.
pub(crate) fn may_change_tree(call: &ToolCall, wrote: bool) -> bool {
    match call {
        ToolCall::Edit { .. } | ToolCall::WriteFile { .. } => wrote,
        // A process that ran may have written anything, whatever it
        // printed and however it exited.
        ToolCall::Bash { .. } | ToolCall::Custom { .. } => true,
        ToolCall::BlastRadius { .. }
        | ToolCall::ReadFile { .. }
        | ToolCall::RunChecks { .. }
        | ToolCall::ReadSkill { .. }
        | ToolCall::Grep { .. }
        | ToolCall::Find { .. }
        | ToolCall::Ls { .. }
        | ToolCall::SearchNotes { .. }
        | ToolCall::ReadNote { .. }
        | ToolCall::CallersOf { .. }
        | ToolCall::CalleesOf { .. }
        | ToolCall::CoverageReport { .. } => false,
        // Nothing is written: the question travels back to the loop in the
        // call itself, and the binary records it on the task.
        ToolCall::AskHuman { .. } => false,
    }
}

/// What a person said about one call, and how long they took to say it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HumanDecision {
    pub approved: bool,
    /// Time spent waiting on the answer. The loop leaves it out of the
    /// wall-clock budget: that ceiling bounds what a run costs in compute and
    /// in the user's patience with a model, and a person reading a diff is
    /// neither.
    pub waited: Duration,
}

/// Handle to a running orchestrator. Cloneable; every clone talks to the one
/// owner.
#[derive(Clone)]
pub struct Handle(mpsc::Sender<Request>);

impl Handle {
    async fn ask<T>(
        &self,
        make: impl FnOnce(oneshot::Sender<T>) -> Request,
        on_dead: impl FnOnce() -> T,
    ) -> T {
        let (tx, rx) = oneshot::channel();
        if self.0.send(make(tx)).await.is_err() {
            return on_dead();
        }
        rx.await.unwrap_or_else(|_| on_dead())
    }

    pub async fn blast_radius(&self, symbol: &str) -> ToolResult {
        let symbol = symbol.to_string();
        self.ask(
            |reply| Request::BlastRadius { symbol, reply },
            || ToolResult::Error("orchestrator stopped".into()),
        )
        .await
    }

    pub async fn read_file(&self, path: &str) -> ToolResult {
        let path = path.to_string();
        self.ask(
            |reply| Request::ReadFile { path, reply },
            || ToolResult::Error("orchestrator stopped".into()),
        )
        .await
    }

    pub async fn edit(&self, path: &str, old: &str, new: &str) -> ToolResult {
        let (path, old, new) = (path.to_string(), old.to_string(), new.to_string());
        self.ask(
            |reply| Request::ApplyEdit {
                path,
                old,
                new,
                reply,
            },
            || ToolResult::Error("orchestrator stopped".into()),
        )
        .await
    }

    pub async fn run_checks(&self, paths: Vec<String>, kinds: Vec<CheckKind>) -> ToolResult {
        self.ask(
            |reply| Request::RunChecks {
                paths,
                kinds,
                reply,
            },
            || ToolResult::Error("orchestrator stopped".into()),
        )
        .await
    }

    /// A model's call, through the gates.
    pub async fn call(&self, call: &ToolCall) -> Dispatched {
        self.dispatch(call, None).await
    }

    /// As [`call`](Self::call), with what is left of the run's wall-clock
    /// budget: a check the call runs is killed when the budget runs out, if
    /// that comes before its `timeout_s` (ADR 0004). Time a person spends
    /// deciding on the call is not charged, as the loop does not charge it.
    pub async fn call_within(&self, call: &ToolCall, budget_left: Duration) -> Dispatched {
        self.dispatch(call, Some(budget_left)).await
    }

    async fn dispatch(&self, call: &ToolCall, budget_left: Option<Duration>) -> Dispatched {
        let call = call.clone();
        self.ask(
            |reply| Request::Call {
                call,
                budget_left,
                reply,
            },
            || Dispatched {
                result: ToolResult::Error("orchestrator stopped".into()),
                radius_sites: Vec::new(),
                human: None,
                // A stopped actor owns the only write path and wrote nothing.
                may_have_changed_tree: false,
                trust_breach: None,
                changed: Vec::new(),
            },
        )
        .await
    }

    /// The convergence-time governance audit, which no gate may refuse.
    pub async fn audit_governance(&self) -> ToolResult {
        self.ask(
            |reply| Request::Audit { reply },
            || ToolResult::Error("orchestrator stopped".into()),
        )
        .await
    }

    pub async fn touched(&self) -> Vec<String> {
        self.ask(|reply| Request::Touched { reply }, Vec::new).await
    }

    /// Whether the orchestrator is still there to answer.
    ///
    /// `ask` already degrades safely when it is not — every call returns its
    /// `on_dead` value — but degrading safely and *noticing* are different
    /// things. Without this the loop reads a dead actor as a run of tool
    /// errors and spends the entire budget discovering that nothing works.
    pub fn is_alive(&self) -> bool {
        !self.0.is_closed()
    }

    pub async fn has_governance_rules(&self) -> bool {
        self.ask(|reply| Request::HasRules { reply }, || false)
            .await
    }

    /// Every call a named gate refused this run, in order.
    pub async fn gate_log(&self) -> Vec<GateDenial> {
        self.ask(|reply| Request::GateLog { reply }, Vec::new).await
    }
}

/// The owner. Holds the index and the only write path to the working tree.
pub struct Orchestrator {
    root: PathBuf,
    session: Session,
    checks: CheckConfig,
    touched: Vec<String>,
    /// Project rules, enforced here because this is the only write path to
    /// the working tree. A rule enforced anywhere else is a rule a tool call
    /// can route around.
    ///
    /// Held apart from `gates` so that it is first by construction rather
    /// than by the order someone called the builders in.
    governance: GovernanceGate,
    /// Every other gate, in the order they are consulted (ADR 0018).
    gates: Vec<Box<dyn ToolGate>>,
    /// The repository's rule documents, for the governance audit.
    rules: Vec<trellis_checks::governance::Rule>,
    /// The tools this run offers. A call outside it is refused before any
    /// gate is asked: a person should not be asked about a tool the run does
    /// not have (ADR 0023).
    tools: ToolSet,
    /// `TRELLIS_RUN_ID` and `TRELLIS_MODEL`, for `bash` and custom tools.
    env: Vec<(String, String)>,
    /// Trusted hooks, for `post_tool` and `turn_end`. `pre_tool` is a gate
    /// in `gates`.
    hooks: Option<Hooks>,
    /// Model calls served, for `GateDenial::call`.
    calls: u32,
    gate_log: Vec<GateDenial>,
    /// Where `read_skill` looks beyond the repository, and a profile's
    /// `[skills]` lists, enforced here because this is where the body is
    /// served (ADR 0026 §4).
    skills: crate::skills::SkillAccess,
    /// Whether the call being served reached a write, for
    /// [`may_change_tree`]. Reset at the start of every gated call.
    wrote: bool,
    /// Whether a gate refused the call being served, for `turn_end`'s
    /// `outcome`. Reset at the start of every gated call.
    denied: bool,
    /// What the call being served carried back into the repository, for
    /// [`Dispatched::changed`]. Reset at the start of every gated call.
    changed: Vec<String>,
    /// Re-checks trust before a custom tool starts and after any call that
    /// may have changed the tree (ADR 0023 §5). The same latch the hooks
    /// hold, so a breach found by either ends the run.
    guard: Option<crate::trust::TrustGuard>,
    /// The throwaway clone the hooks of the call being served run in
    /// (ADR 0032, "Hooks"): made at the first hook, refreshed before each
    /// later one, removed when the call ends.
    hook_clone: Option<crate::isolate::Sandbox>,
}

impl Orchestrator {
    pub fn new(root: &Path, checks: CheckConfig) -> Self {
        Self {
            root: root.to_path_buf(),
            session: Session::open(root),
            // Once per run, not per check spawn (review round 11).
            checks: checks.for_run(root),
            touched: Vec::new(),
            governance: GovernanceGate::new(crate::governance::Governance::default(), root),
            gates: Vec::new(),
            rules: Vec::new(),
            tools: ToolSet::default(),
            env: Vec::new(),
            hooks: None,
            calls: 0,
            gate_log: Vec::new(),
            skills: crate::skills::SkillAccess::default(),
            wrote: false,
            denied: false,
            changed: Vec::new(),
            guard: None,
            hook_clone: None,
        }
    }

    /// The trust guard for this run's trusted code: custom tools refuse to
    /// start once it fails, and a call that may have changed the tree is
    /// followed by a check, so a `bash` command that rewrote a hook ends the
    /// run whether or not a hook runs after it.
    #[must_use]
    pub fn with_trust_guard(mut self, guard: crate::trust::TrustGuard) -> Self {
        self.guard = Some(guard);
        self
    }

    /// User-level skill directories and an agent profile's skill lists, for
    /// `read_skill`. The default reads the repository's skills only, with
    /// no filter, which is what every run did before.
    #[must_use]
    pub fn with_skills(mut self, access: crate::skills::SkillAccess) -> Self {
        self.skills = access;
        self
    }

    /// The tool set this run offers; the same value the prompt was given.
    #[must_use]
    pub fn with_tools(mut self, tools: ToolSet) -> Self {
        self.tools = tools;
        self
    }

    /// Variables added to the environment of `bash` and custom tools.
    #[must_use]
    pub fn with_env(mut self, env: Vec<(String, String)>) -> Self {
        self.env = env;
        self
    }

    /// Trusted hooks, whose `post_tool` observes every dispatched call. The
    /// `pre_tool` gate is added with [`with_gate`](Self::with_gate), so its
    /// place in the order is the caller's to see.
    #[must_use]
    pub fn with_hooks(mut self, hooks: Hooks) -> Self {
        self.hooks = Some(hooks);
        self
    }

    /// Supplies the rule documents a governance audit evaluates.
    #[must_use]
    pub fn with_rules(mut self, rules: Vec<trellis_checks::governance::Rule>) -> Self {
        self.rules = rules;
        self
    }

    /// Applies the repository's rules to this run.
    #[must_use]
    pub fn with_governance(mut self, g: crate::governance::Governance) -> Self {
        self.governance = GovernanceGate::new(g, &self.root);
        self
    }

    /// Adds a gate after governance and after every gate added before it.
    #[must_use]
    pub fn with_gate(mut self, g: Box<dyn ToolGate>) -> Self {
        self.gates.push(g);
        self
    }

    /// Starts the actor on the current runtime and returns a handle.
    ///
    /// The channel is bounded: an agent that outruns the orchestrator should
    /// wait rather than queue unboundedly, because the queue would otherwise
    /// become an invisible second budget.
    pub fn spawn(self) -> (Handle, tokio::task::JoinHandle<Self>) {
        let (tx, rx) = mpsc::channel(32);
        let join = tokio::spawn(self.serve(rx));
        (Handle(tx), join)
    }

    #[tracing::instrument(skip_all)]
    async fn serve(mut self, mut rx: mpsc::Receiver<Request>) -> Self {
        while let Some(req) = rx.recv().await {
            // Every tool request goes through the gates, whichever door it
            // came in by. The single-purpose requests predate the gates and
            // are still how a caller asks for one tool; routing them around
            // the gates would make `deny_write` a property of which method
            // was called.
            match req {
                Request::BlastRadius { symbol, reply } => {
                    let call = ToolCall::BlastRadius { symbol };
                    let _ = reply.send(self.gated(call, None, false).await.result);
                }
                Request::ReadFile { path, reply } => {
                    let call = ToolCall::ReadFile { path };
                    let _ = reply.send(self.gated(call, None, false).await.result);
                }
                Request::ApplyEdit {
                    path,
                    old,
                    new,
                    reply,
                } => {
                    let call = ToolCall::Edit { path, old, new };
                    let _ = reply.send(self.gated(call, None, false).await.result);
                }
                Request::RunChecks {
                    paths,
                    kinds,
                    reply,
                } => {
                    let call = ToolCall::RunChecks { paths, kinds };
                    let _ = reply.send(self.gated(call, None, false).await.result);
                }
                Request::Call {
                    call,
                    budget_left,
                    reply,
                } => {
                    self.calls += 1;
                    let _ = reply.send(self.gated(call, budget_left, true).await);
                }
                Request::GateLog { reply } => {
                    let _ = reply.send(self.gate_log.clone());
                }
                Request::Audit { reply } => {
                    let _ = reply.send(self.run_checks(&[], &[CheckKind::Governance], None).await);
                }
                Request::Touched { reply } => {
                    let _ = reply.send(self.touched.clone());
                }
                Request::HasRules { reply } => {
                    let _ = reply.send(!self.rules.is_empty());
                }
            }
        }
        self
    }

    /// [`gated_inner`](Self::gated_inner), then `turn_end` when `turn`, then
    /// the trust guard, read on the way out whichever way the call ended: a
    /// `pre_tool` hook refused by the guard returns early as a denial, and
    /// that is a breach as much as a `bash` command that rewrote a hook.
    ///
    /// Only a model's call is a turn; the audit at convergence and the
    /// direct requests are not, so `turn_end` does not run for them.
    ///
    /// Stack headroom: this is polled on the run thread (`trellis-run`,
    /// 8 MiB since #180). Before that a debug build on Windows polled it on
    /// the 1 MiB main thread, and `tests/child_env.rs` overflowed it while
    /// the hook's spawn (about 19 KB of future) was held inline; the boxing
    /// in `Hooks::turn_end` is what fixed that then. `turn_end` is folded in
    /// here rather than wrapped around this so it adds no layer of its own.
    async fn gated(
        &mut self,
        call: ToolCall,
        budget_left: Option<Duration>,
        turn: bool,
    ) -> Dispatched {
        let tool = turn.then(|| tool_name(&call).to_string());
        let mut d = self.gated_inner(call, budget_left).await;
        if let Some(tool) = tool {
            d.may_have_changed_tree |= self.turn_end(&tool, &d.result).await;
        }
        // Whichever way the call ended; a panic drops it too.
        if let Some(sandbox) = self.hook_clone.take() {
            crate::isolate::dispose(sandbox).await;
        }
        if let Some(g) = &self.guard {
            if d.may_have_changed_tree {
                // Latches on failure; the reason is read below.
                let _ = g.check_async().await;
            }
            d.trust_breach = g.breach();
        }
        d
    }

    /// Runs `turn_end`, when one is declared, in the call's clone, carries
    /// back what it changed, and says whether the worktree changed across
    /// the two. Bracketed on its own, as `post_tool` is: a write here lands
    /// after the turn's check, so it marks the turn as one that may have
    /// changed the tree and the pass stops counting (ADR 0023, "after a
    /// pass"). The bracket stays for the hook that writes into the
    /// repository by absolute path, which no clone stops (ADR 0032).
    async fn turn_end(&mut self, tool: &str, result: &ToolResult) -> bool {
        if self
            .hooks
            .as_ref()
            .is_none_or(|h| h.config().turn_end.is_none())
        {
            return false;
        }
        let outcome = match result {
            _ if self.denied => TurnOutcome::Denied,
            ToolResult::Error(_) => TurnOutcome::Error,
            ToolResult::Checks(v) if v.passed() => TurnOutcome::CheckPassed,
            ToolResult::Checks(_) => TurnOutcome::CheckFailed,
            _ => TurnOutcome::Ok,
        };
        let before = Some(worktree_digest(&self.root).await);
        match self.hook_clone_path().await {
            Ok(dir) => {
                if let Some(h) = &self.hooks {
                    h.turn_end(&dir, self.calls, tool, outcome).await;
                }
                self.carry_back_hook("turn_end").await;
            }
            // It only observes: not run, and said so, rather than run in
            // the repository.
            Err(why) => tracing::warn!(hook = "turn_end", "hook not run: {why}"),
        }
        changed_since(&self.root, before).await
    }

    /// The path of the call's clone for its hooks, made now if this is the
    /// call's first hook, and otherwise refreshed from the repository, so
    /// the hook sees what the tool and the hooks before it left there
    /// (ADR 0032, "Hooks"). A clone that cannot be refreshed is replaced.
    /// `Err` when no clone can be made; the hook is then not run.
    async fn hook_clone_path(&mut self) -> Result<PathBuf, String> {
        // Read before the copy, so the copy the hook runs is of what was
        // just checked. A failure latches, and the hook's own reading of
        // the same guard then refuses to start it.
        if let Some(g) = &self.guard {
            let _ = g.check_async().await;
        }
        if let Some(mut sandbox) = self.hook_clone.take() {
            let root = self.root.clone();
            let refreshed = tokio::task::spawn_blocking(move || {
                let r = sandbox.refresh(&root);
                (sandbox, r)
            })
            .await;
            match refreshed {
                Ok((sandbox, Ok(()))) => {
                    let dir = sandbox.path().to_path_buf();
                    self.hook_clone = Some(sandbox);
                    return Ok(dir);
                }
                Ok((sandbox, Err(e))) => {
                    tracing::warn!("a hook clone could not be refreshed, so another is made: {e}");
                    crate::isolate::dispose(sandbox).await;
                }
                // The clone was dropped, so removed, as the task unwound.
                Err(e) => tracing::warn!("refreshing a hook clone failed: {e}"),
            }
        }
        let sandbox = crate::isolate::prepare(&self.root).await?;
        let dir = sandbox.path().to_path_buf();
        self.hook_clone = Some(sandbox);
        Ok(dir)
    }

    /// Carries back into the repository what the hook `event` changed in
    /// the call's clone, through [`carry_back`](Self::carry_back): the
    /// refusals `bash` gets, `deny_write` and `allow_write`, recorded in
    /// `touched`. A hook has no observation to report into, so what was
    /// carried back and what was refused is logged.
    async fn carry_back_hook(&mut self, event: &'static str) {
        let Some(sandbox) = self.hook_clone.take() else {
            return;
        };
        let read = tokio::task::spawn_blocking(move || {
            let c = sandbox.changes();
            (sandbox, c)
        })
        .await;
        let Ok((sandbox, changes)) = read else {
            tracing::warn!(
                hook = event,
                "reading what the hook changed in its clone failed; nothing was carried back"
            );
            return;
        };
        let report = self.carry_back(&sandbox, changes);
        self.hook_clone = Some(sandbox);
        if !report.is_empty() {
            tracing::warn!(hook = event, report = %report.trim(), "a hook changed files in its clone");
        }
    }

    /// Consults the gates in order, then dispatches. The first `Deny` wins
    /// and nothing after it is asked — a human should not be asked about a
    /// call a rule has already refused.
    async fn gated_inner(&mut self, call: ToolCall, budget_left: Option<Duration>) -> Dispatched {
        let received = Instant::now();
        let mut human = None;
        let name = tool_name(&call);
        if !self.tools.enabled(name) {
            return Dispatched {
                result: ToolResult::Error(format!(
                    "`{name}` is not a tool in this run; the tools are {}",
                    self.tools.names().join(", ")
                )),
                radius_sites: Vec::new(),
                human,
                may_have_changed_tree: false,
                trust_breach: None,
                changed: Vec::new(),
            };
        }
        self.wrote = false;
        self.denied = false;
        self.changed.clear();
        // `pre_tool` and `post_tool` are repository code that runs around
        // every call, the passing check included, and nothing stops either
        // writing. The worktree is compared across each hook on its own,
        // so a hook that writes marks the turn as one that may have changed
        // the tree and the pass before it stops counting (ADR 0023, "after
        // a pass"). Across the hooks and not the whole dispatch: the tool
        // in between is classified by its kind (`may_change_tree`), and a
        // bracket that included it made a checker's own `.coverage` or
        // `__pycache__` void the pass it had just produced. Only when such
        // a hook is declared: git calls per turn are a cost only a
        // repository with hooks pays.
        let (pre, post) = self.hooks.as_ref().map_or((false, false), |h| {
            let c = h.config();
            (c.pre_tool.is_some(), c.post_tool.is_some())
        });
        let pre = pre || self.gates.iter().any(|g| g.runs_repository_code());
        // The gate loop holds `pre_tool`; the other gates (governance, the
        // bash policy, a person's approval) do not write.
        let before_gates = if pre {
            Some(worktree_digest(&self.root).await)
        } else {
            None
        };
        for i in 0..=self.gates.len() {
            // A gate that runs repository code (`pre_tool`) runs it in the
            // call's clone, made or refreshed before the borrow below, and
            // what it changed there is carried back after it.
            let isolated = i > 0 && self.gates[i - 1].runs_repository_code();
            let place = if isolated {
                Some(self.hook_clone_path().await)
            } else {
                None
            };
            let gate: &mut dyn ToolGate = match i {
                0 => &mut self.governance,
                _ => self.gates[i - 1].as_mut(),
            };

            let asked_at = Instant::now();
            if place.is_some() {
                gate.run_in(place);
            }
            let decision = gate.check(&call).await;
            if isolated {
                gate.run_in(None);
            }
            let human_decided = gate.human_decided();
            let gate_name = gate.name();
            if human_decided {
                human = Some(HumanDecision {
                    approved: decision == GateDecision::Allow,
                    waited: asked_at.elapsed(),
                });
            }
            if isolated {
                self.carry_back_hook(gate_name.unwrap_or("pre_tool")).await;
            }
            if let GateDecision::Deny { reason } = decision {
                self.denied = true;
                if let Some(gate) = gate_name {
                    self.gate_log.push(GateDenial {
                        call: self.calls,
                        gate: gate.to_string(),
                        target: crate::gate::label(&call),
                        reason: reason.clone(),
                    });
                }
                return Dispatched {
                    result: ToolResult::Error(reason),
                    radius_sites: Vec::new(),
                    human,
                    may_have_changed_tree: changed_since(&self.root, before_gates).await,
                    trust_breach: None,
                    changed: Vec::new(),
                };
            }
        }
        let gates_changed = changed_since(&self.root, before_gates).await;
        // When the run's budget ends, moved on by however long a person
        // took over this call: the loop does not charge that wait, and a
        // check cut short by it would fail for a diff read carefully.
        // `None` on overflow too: a budget past the clock's range bounds
        // nothing, and `timeout_s` still does.
        let deadline = budget_left
            .and_then(|left| received.checked_add(left))
            .and_then(|end| end.checked_add(human.map_or(Duration::ZERO, |h| h.waited)));
        let result = match &call {
            ToolCall::BlastRadius { symbol } => self.blast_radius(symbol),
            ToolCall::ReadFile { path } => self.read_file(path),
            ToolCall::Edit { path, old, new } => self.apply_edit(path, old, new),
            ToolCall::RunChecks { paths, kinds } => self.run_checks(paths, kinds, deadline).await,
            ToolCall::ReadSkill { name } => match self.skills.read(&self.root, name) {
                Ok(body) => ToolResult::FileContent(body),
                Err(e) => ToolResult::Error(e.to_string()),
            },
            ToolCall::WriteFile { path, content } => self.write_file(path, content).await,
            ToolCall::Grep { pattern, path } => match path.as_deref().filter(|p| !p.is_empty()) {
                Some(p) => match read_refusal(&self.root, p) {
                    Some(e) => ToolResult::Error(format!("grep refused: {e}")),
                    None => crate::listing::grep(&self.root, pattern, Some(p)).await,
                },
                None => crate::listing::grep(&self.root, pattern, None).await,
            },
            ToolCall::Find { glob } => {
                // The same predicate as every path: a `:` also rules out
                // git's pathspec magic (`:/`, `:(top)`).
                if unsafe_repo_path(glob).is_some() {
                    ToolResult::Error(format!(
                        "glob `{glob}` must be relative to the repository root, with no \
                         leading `/` or `\\`, no drive letter or `:`, and no `..`"
                    ))
                } else {
                    crate::listing::find(&self.root, glob).await
                }
            }
            ToolCall::Ls { path } => match read_refusal(&self.root, path) {
                Some(e) => ToolResult::Error(format!("ls refused: {e}")),
                None => crate::listing::ls(&self.root, path),
            },
            ToolCall::Bash { command } => self.bash(command).await,
            ToolCall::SearchNotes { query, limit } => match self.tools.notes() {
                Some(n) => crate::notes::search(n, query, *limit),
                None => ToolResult::Error("no note vault is configured for this run".into()),
            },
            ToolCall::ReadNote { note } => match self.tools.notes() {
                Some(n) => crate::notes::read(n, note),
                None => ToolResult::Error("no note vault is configured for this run".into()),
            },
            ToolCall::CallersOf { symbol } => self.callers_of(symbol),
            ToolCall::CalleesOf { symbol } => self.callees_of(symbol),
            ToolCall::CoverageReport { paths } => self.coverage_report(paths),
            ToolCall::Custom { name, args } => self.custom(name, args).await,
            // Nothing to run. `Output` is what tells the loop the gates let
            // the question through; it ends the run on it. The words are
            // for the trail, since no later turn reads them.
            ToolCall::AskHuman { .. } => ToolResult::Output(
                "question recorded; the run ends here without converging, and the task \
                 waits for a person's answer"
                    .into(),
            ),
        };
        // Read now, before `post_tool` runs: a hook that rewrites a file would
        // otherwise shift the bytes the line numbers are counted from.
        let radius_sites = match &result {
            ToolResult::Radius(report) => {
                crate::agent::radius_sites(self.session.index(), &self.root, report)
            }
            _ => Vec::new(),
        };
        // Observe only. The result is already decided and is what the model
        // receives, whatever the hook prints or returns.
        // In the call's clone, refreshed first, so the hook sees the tree
        // with what the tool just did; the result itself it gets on stdin.
        let mut post_changed = false;
        if post {
            let before_post = Some(worktree_digest(&self.root).await);
            match self.hook_clone_path().await {
                Ok(dir) => {
                    if let Some(h) = &self.hooks {
                        let as_json =
                            serde_json::to_value(&result).unwrap_or(serde_json::Value::Null);
                        h.post_tool(&dir, &call, as_json).await;
                    }
                    self.carry_back_hook("post_tool").await;
                }
                // It only observes: not run, and said so, rather than run
                // in the repository.
                Err(why) => tracing::warn!(hook = "post_tool", "hook not run: {why}"),
            }
            post_changed = changed_since(&self.root, before_post).await;
        }
        Dispatched {
            result,
            radius_sites,
            human,
            may_have_changed_tree: may_change_tree(&call, self.wrote)
                || gates_changed
                || post_changed,
            trust_breach: None,
            changed: std::mem::take(&mut self.changed),
        }
    }

    /// Creates a file that does not exist (ADR 0023 §2).
    ///
    /// Governance has already been consulted as a gate. What is checked here
    /// is what makes "rollback removes it" true: the path must be new, must
    /// not be inside `.git`, and must not be ignored by git, because
    /// rollback's `git clean -fd` leaves ignored files where they are.
    async fn write_file(&mut self, path: &str, content: &str) -> ToolResult {
        if let Some(why) = write_refusal(&self.root, path) {
            return ToolResult::Error(format!("write_file refused: {why}"));
        }
        let full = self.root.join(path);
        if full.exists() {
            return ToolResult::Error(format!(
                "{path} already exists. write_file creates new files only; use edit to \
                 change an existing one."
            ));
        }
        if let Some(why) = ignored_refusal(&self.root, path).await {
            return ToolResult::Error(format!("write_file refused: {why}"));
        }
        if let Some(dir) = full.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            return ToolResult::Error(format!("cannot create the directory for {path}: {e}"));
        }
        // `create_new`, not `write`: a file that appeared since the check
        // above is not overwritten. `wrote` is set first, because a write
        // that fails part way still leaves a file.
        self.wrote = true;
        let written = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&full)
            .and_then(|mut f| std::io::Write::write_all(&mut f, content.as_bytes()));
        if let Err(e) = written {
            return ToolResult::Error(format!("cannot write {path}: {e}"));
        }
        if !self.touched.iter().any(|p| p == path) {
            self.touched.push(path.to_string());
        }
        // A new Python file changes the module map, which `try_update`
        // cannot do, so the session is reopened: a full rebuild, paid so the
        // next blast radius describes the tree as it now is.
        if path.ends_with(".py") {
            self.session = Session::open(&self.root);
        }
        ToolResult::Output(format!(
            "created {path} ({} bytes, {} lines)",
            content.len(),
            content.lines().count()
        ))
    }

    async fn bash(&mut self, command: &str) -> ToolResult {
        let Some(b) = self.tools.bash() else {
            return ToolResult::Error("bash is not enabled in this run".into());
        };
        let (shell, timeout) = (b.shell.clone(), b.timeout);
        let mut argv = shell;
        argv.push(command.to_string());
        let (c, carried) = match self.isolated(&argv, None, timeout).await {
            Ok(r) => r,
            Err(e) => return ToolResult::Error(format!("bash was not run: {e}")),
        };
        let status = if c.timed_out {
            format!("timed out after {}s and was killed", timeout.as_secs())
        } else {
            match c.code {
                Some(code) => format!("exit {code}"),
                None => "ended without an exit code".to_string(),
            }
        };
        let mut s = format!("{status}\n");
        if !c.stdout.is_empty() {
            s.push_str(&c.stdout.render());
        }
        if !c.stderr.is_empty() {
            s.push_str("\nstderr:\n");
            s.push_str(&c.stderr.render());
        }
        s.push_str(&carried);
        // Not a verdict, and said where the model reads it: a command
        // printing "tests passed" has passed nothing. And where it ran, so a
        // file it created and then cannot find is not a mystery.
        s.push_str(
            "\n(bash ran in a throwaway clone of the repository: changes to files trellis \
             tracks are carried back, and nothing else is. Its output is an observation, \
             not a check)",
        );
        ToolResult::Output(s)
    }

    async fn custom(&mut self, name: &str, args: &serde_json::Value) -> ToolResult {
        let Some(tool) = self.tools.custom(name) else {
            return ToolResult::Error(format!("`{name}` is not a tool in this run"));
        };
        let command = tool.command.clone();
        let timeout = tool.timeout();
        // Its command is trusted code, and a script it names may have been
        // rewritten since (ADR 0023 §5). Checked before the clone is made,
        // so the copy it runs is of what was just checked.
        if let Some(g) = &self.guard
            && let Err(why) = g.check_async().await
        {
            return ToolResult::Error(format!("`{name}` was not started, because {why}"));
        }
        let stdin = Some(args.to_string().into_bytes());
        let (c, carried) = match self.isolated(&command, stdin, timeout).await {
            Ok(r) => r,
            Err(e) => return ToolResult::Error(format!("`{name}` was not run: {e}")),
        };
        if c.timed_out {
            return ToolResult::Error(format!(
                "`{name}` did not finish within {}s and was killed{carried}",
                timeout.as_secs()
            ));
        }
        if c.code == Some(0) {
            return ToolResult::Output(format!("{}{carried}", c.stdout.render()));
        }
        ToolResult::Error(format!(
            "`{name}` exited with {}: {}{}{carried}",
            c.code
                .map_or_else(|| "no exit code".to_string(), |c| c.to_string()),
            c.stdout.render(),
            if c.stderr.is_empty() {
                String::new()
            } else {
                format!("\nstderr:\n{}", c.stderr.render())
            }
        ))
    }

    /// Runs `argv` in a throwaway clone of the repository and carries back
    /// what it changed (ADR 0032, copy-in/diff-out): the process's output,
    /// and a report of what was carried back and what was not, empty when
    /// it changed nothing. `Err` when it was not run: a clone that could not
    /// be made is never a reason to run it in the repository instead.
    ///
    /// Whatever the process did, the clone is removed before this returns.
    /// A change is carried back even when the process failed or timed out:
    /// a person reading the run should find in the repository what the
    /// command did, as before it was isolated.
    async fn isolated(
        &mut self,
        argv: &[String],
        stdin: Option<Vec<u8>>,
        timeout: Duration,
    ) -> Result<(crate::exec::Captured, String), String> {
        let sandbox = crate::isolate::prepare(&self.root).await?;
        let r = crate::exec::run(crate::exec::Spec {
            argv,
            cwd: sandbox.path(),
            env: &self.env,
            env_remove: &crate::isolate::GIT_LOCATION_VARS,
            stdin,
            timeout,
            cap: crate::exec::OUTPUT_CAP,
            head: false,
            whole: false,
        })
        .await;
        // Off the runtime: every copied-in file is hashed again.
        let (sandbox, changes) = tokio::task::spawn_blocking(move || {
            let c = sandbox.changes();
            (sandbox, c)
        })
        .await
        .map_err(|e| format!("reading what it changed failed: {e}"))?;
        let captured = match r {
            Ok(c) => c,
            Err(e) => {
                crate::isolate::dispose(sandbox).await;
                return Err(e.to_string());
            }
        };
        let report = self.carry_back(&sandbox, changes);
        crate::isolate::dispose(sandbox).await;
        Ok((captured, report))
    }

    /// Writes into the repository what a command changed in its clone,
    /// through the rules `edit` obeys, and says what it did. The write path
    /// is this orchestrator's own: refused under `.git/` and `.trellis/`
    /// and through links out of the repository ([`write_refusal`]), refused
    /// where `deny_write` denies it or a profile's `allow_write` does not
    /// admit it, recorded in `touched` so the rollback
    /// and the trail both have it, and followed by a reindex.
    fn carry_back(
        &mut self,
        sandbox: &crate::isolate::Sandbox,
        ch: crate::isolate::Changes,
    ) -> String {
        let mut carried: Vec<String> = Vec::new();
        // Whole sentences, since `write_refusal`'s already name the path.
        let mut refused: Vec<String> = ch
            .refused
            .into_iter()
            .map(|(rel, why)| format!("`{rel}` {why}"))
            .collect();
        let mut reopen = false;
        for (rel, before) in ch.modified {
            if let Some(why) = write_refusal(&self.root, &rel) {
                refused.push(why);
                continue;
            }
            if let Some(clause) = self.governance.denies_write(&rel) {
                refused.push(format!("`{rel}` is {clause}"));
                continue;
            }
            if let Some(why) = self.governance.outside_allow_write(&rel) {
                refused.push(format!("`{rel}` is {why}"));
                continue;
            }
            let full = self.root.join(&rel);
            // The repository's copy is still what was copied in: nothing
            // else in this run writes while the orchestrator serves this
            // call, so a difference is a writer outside it (a hook, a
            // process the command started), and its bytes are not
            // overwritten.
            let unchanged = std::fs::symlink_metadata(&full).is_ok_and(|m| m.is_file())
                && crate::isolate::hash_file(&full).is_ok_and(|h| h == before);
            if !unchanged {
                refused.push(format!(
                    "`{rel}` changed in the repository while the command ran"
                ));
                continue;
            }
            let bytes = match sandbox.read_changed(&rel) {
                Ok(b) => b,
                Err(why) => {
                    refused.push(format!("`{rel}` {why}"));
                    continue;
                }
            };
            // Before the write: one that fails part way has still changed
            // the file.
            self.wrote = true;
            if let Err(e) = std::fs::write(&full, &bytes) {
                refused.push(format!("`{rel}` could not be written: {e}"));
                continue;
            }
            if !self.touched.iter().any(|p| p == &rel) {
                self.touched.push(rel.clone());
            }
            if rel.ends_with(".py") {
                let updated =
                    std::str::from_utf8(&bytes).is_ok_and(|src| self.session.try_update(&rel, src));
                reopen |= !updated;
            }
            carried.push(rel);
        }
        // A file the session did not know, or could not take as text: the
        // graph is rebuilt, so the next blast radius describes the tree.
        if reopen {
            self.session = Session::open(&self.root);
        }
        // The carry-back's own list, not a `git status` taken before and
        // after: these are exactly the files trellis wrote for this call,
        // where a status diff would also attribute to the command whatever
        // a hook or another writer changed meanwhile, and would cost two
        // git spawns per call to learn less.
        self.changed.extend(carried.iter().cloned());
        let mut s = String::new();
        if !carried.is_empty() {
            s.push_str(&format!(
                "\ncarried back from the clone: {}",
                trellis_vcs::ignored::shown(&carried)
            ));
        }
        if !refused.is_empty() {
            s.push_str(&format!(
                "\nnot carried back: {}",
                trellis_vcs::ignored::shown(&refused)
            ));
        }
        if !ch.deleted.is_empty() {
            s.push_str(&format!(
                "\ndeleted in the clone, and not carried back (trellis does not delete \
                 files): {}",
                trellis_vcs::ignored::shown(&ch.deleted)
            ));
        }
        s
    }

    pub fn index(&self) -> &trellis_index::Index {
        self.session.index()
    }

    fn blast_radius(&self, symbol: &str) -> ToolResult {
        let index = self.session.index();
        match crate::graph_tools::find_symbol(index, symbol) {
            Ok(id) => ToolResult::Radius(trellis_index::radius::blast_radius(index, id)),
            Err(e) => ToolResult::Error(e),
        }
    }

    /// `callers_of`, resolving `symbol` exactly as `blast_radius` does.
    fn callers_of(&self, symbol: &str) -> ToolResult {
        let index = self.session.index();
        match crate::graph_tools::find_symbol(index, symbol) {
            Ok(id) => crate::graph_tools::callers_of(index, &self.root, id),
            Err(e) => ToolResult::Error(e),
        }
    }

    /// `callees_of`, resolving `symbol` exactly as `blast_radius` does.
    fn callees_of(&self, symbol: &str) -> ToolResult {
        let index = self.session.index();
        match crate::graph_tools::find_symbol(index, symbol) {
            Ok(id) => crate::graph_tools::callees_of(index, &self.root, id),
            Err(e) => ToolResult::Error(e),
        }
    }

    /// `coverage_report`, after every path passes the check `read_file`
    /// makes: the report names files and lines, so a path outside the
    /// repository or into `.git` is refused rather than answered with a
    /// count of zero that would read as "nothing in doubt there".
    fn coverage_report(&self, paths: &[String]) -> ToolResult {
        if paths.is_empty() {
            return ToolResult::Error(
                "coverage_report needs at least one path; it reports on the files named, \
                 not on the whole repository"
                    .into(),
            );
        }
        for p in paths {
            if let Some(why) = read_refusal(&self.root, p) {
                return ToolResult::Error(format!("coverage_report refused: {why}"));
            }
        }
        crate::graph_tools::coverage_report(self.session.index(), &self.root, paths)
    }

    fn read_file(&self, path: &str) -> ToolResult {
        match self.resolve_in_repo(path) {
            Err(e) => ToolResult::Error(format!("read_file refused: {e}")),
            Ok(full) => match std::fs::read_to_string(full) {
                Ok(s) => ToolResult::FileContent(s),
                Err(e) => ToolResult::Error(format!("cannot read {path}: {e}")),
            },
        }
    }

    /// Applies a literal replacement, then reindexes the edited file.
    ///
    /// `old` must appear exactly once. A unique anchor is the difference
    /// between an edit the trail can describe and one that silently hit the
    /// wrong occurrence — and the model cannot check this for itself, because
    /// checking it is reading the file it is about to change.
    ///
    /// Project rules are not checked here. `GovernanceGate` refuses a denied
    /// path before this runs and before the file is read, so a denied path is
    /// refused identically whether or not it exists — otherwise the refusal
    /// would leak which denied files are present.
    fn apply_edit(&mut self, path: &str, old: &str, new: &str) -> ToolResult {
        // `.git/config` exists, so without this an edit could set
        // `core.hooksPath` or rewrite a hook, and rollback — git's own
        // reset — does not restore git's own directory. `.trellis/` holds
        // the hooks, rules and checks that judge this run.
        if let Some(why) = write_refusal(&self.root, path) {
            return ToolResult::Error(format!("edit refused: {why}"));
        }
        let full = self.root.join(path);
        let src = match std::fs::read_to_string(&full) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                // Creating files is not supported, and saying so beats a
                // confusing read error. `Session::update` only knows paths
                // that were present when the session opened, so a new file
                // would be written to disk and never enter the graph — the
                // exact tree/graph divergence the reindex-on-edit test exists
                // to prevent.
                return ToolResult::Error(format!(
                    concat!(
                        "{} does not exist. This tool edits existing files ",
                        "only; creating files would leave the index unaware of ",
                        "them."
                    ),
                    path
                ));
            }
            Err(e) => return ToolResult::Error(format!("cannot read {path}: {e}")),
        };

        // An edit that replaces text with itself is refused, not applied.
        //
        // It would otherwise "succeed": the anchor occurs once, the identical
        // bytes are written back, and the model is told `applied: true` about
        // a change that did not happen. Three consequences, none of them
        // visible at the time. The model believes it acted and reasons from
        // that. The file joins `touched`, so it is recorded in the trail as
        // proposed and is restored by a rollback that has nothing to restore.
        // And an iteration of the budget is spent on a no-op.
        //
        // Found in the S7 floor run, where a weak model emitted
        // `edit{old: "156", new: "156"}` repeatedly — the failure this refusal
        // makes legible rather than letting it read as progress.
        // A model writes `\n`. A repository checked out on Windows under
        // git's default `autocrlf` holds `\r\n`. So a multi-line anchor
        // copied faithfully out of a file this tool printed still matched
        // nothing, and the loop spent every iteration re-sending the one
        // correct edit it had already found.
        //
        // Matching happens on LF throughout and the file is written back the
        // way it was found, so a CRLF repository does not acquire a whole-file
        // diff as the price of a one-line change.
        let crlf = src.contains("\r\n");
        let src_lf = if crlf {
            src.replace("\r\n", "\n")
        } else {
            src.clone()
        };
        let old = &old.replace("\r\n", "\n");
        let new = &new.replace("\r\n", "\n");
        let src = &src_lf;

        if old == new {
            return ToolResult::Error(format!(
                concat!(
                    "edit refused: `old` and `new` are identical in {}, so this ",
                    "would change nothing. If the file is already correct, run ",
                    "the checks instead of editing."
                ),
                path
            ));
        }

        // Zero and many are different mistakes, and telling them apart is
        // what makes the next turn productive rather than another guess.
        //
        // Asked to fix `describe`, a model anchored on `def describe() -> int:`
        // — a plausible signature it had never read; the real one takes an
        // argument. The old message said only that the anchor occurred zero
        // times, so the model re-guessed, four turns running, until the budget
        // was gone. Nothing in the observation said what the file held, and
        // nothing suggested reading it.
        //
        // So a miss now shows the file. It is the cheapest thing that turns a
        // wasted iteration into an anchor copied from real text.
        let hits = src.matches(old).count();
        if hits != 1 {
            // Both failures end the same way: with the file. Only the
            // zero-hit branch used to, and the action log showed what that
            // cost — every converged run went AMBIGUOUS, then NOT-FOUND, then
            // applied. Told only that its anchor was ambiguous, with nothing
            // to extend it *from*, the model invented one; the invention
            // missed; the miss finally showed it the file; and the turn after
            // that succeeded.
            //
            // Measured over eight attempts at that exact turn against
            // qwen3:8b: 0 of 8 produced a usable anchor without the file, 6
            // of 8 with it. Without it the model wrote `def describe() -> int:`
            // every single time — a plausible signature for a function whose
            // real one takes an argument.
            //
            // The wording is unchanged, and that is also measured. A variant
            // asking for "one or more whole lines copied exactly" scored 3 of
            // 8: the longer instruction pushed more replies into spending the
            // whole reply budget reasoning and returning nothing.
            let why = if hits == 0 {
                format!(
                    "edit anchor not found in {path}. `old` must be text copied from \
                     the file, not reconstructed from the task."
                )
            } else {
                format!(
                    "edit anchor occurs {hits} times in {path}; it must occur exactly \
                     once so the edit is unambiguous. Extend `old` with the lines \
                     around it until only one place in the file matches."
                )
            };
            return ToolResult::Error(format!("{why} Here is {path}:\n{}", excerpt(src)));
        }
        let at = src.find(old.as_str()).expect("occurrence counted");
        let updated = format!("{}{}{}", &src[..at], new, &src[at + old.len()..]);
        // Written back in the file's own convention, not the model's.
        let updated = if crlf {
            updated.replace('\n', "\r\n")
        } else {
            updated
        };

        // Set before the write, not after it succeeds: a write that fails
        // part way has still changed the file.
        self.wrote = true;
        if let Err(e) = std::fs::write(&full, &updated) {
            return ToolResult::Error(format!("cannot write {path}: {e}"));
        }
        if !self.touched.iter().any(|p| p == path) {
            self.touched.push(path.to_string());
        }
        // Keep the graph in step with the tree. A stale index would let the
        // next blast radius describe code that no longer exists, so a refusal
        // here is reported rather than dropped — the write already happened.
        if !self.session.try_update(path, &updated) {
            return ToolResult::Error(format!(
                concat!(
                    "{} was written but is not part of the index session, so ",
                    "the graph is now stale. Re-open the repository before ",
                    "trusting any blast radius."
                ),
                path
            ));
        }

        // The span is an offset into the file, not into the normalized copy
        // this function matched against. On a CRLF checkout those differ by
        // one byte per preceding line, and the difference is now durable: the
        // trail's action log records this number, and a recorded figure that
        // does not point where it says is worse than no figure.
        let span = if crlf {
            let start = at + src[..at].matches('\n').count();
            let len = new.len() + new.matches('\n').count();
            (start, start + len)
        } else {
            (at, at + new.len())
        };

        ToolResult::Edit(EditOutcome {
            path: path.to_string(),
            applied: true,
            span,
        })
    }

    async fn run_checks(
        &self,
        paths: &[String],
        kinds: &[CheckKind],
        deadline: Option<Instant>,
    ) -> ToolResult {
        let checks = CheckConfig {
            deadline,
            ..self.checks.clone()
        };
        // `requested` is kept distinct from the defaulted list below. Shadowing
        // the two together made the governance arm's "no paths given" branch
        // unreachable — `paths` was already `["."]` by the time it ran — so the
        // audit scoped to a directory it could not read and passed having
        // checked nothing. Every measured model calls this with no paths.
        let requested = paths;
        let paths: Vec<PathBuf> = if requested.is_empty() {
            vec![PathBuf::from(".")]
        } else {
            requested.iter().map(PathBuf::from).collect()
        };
        // One verdict per call. Asking for several kinds runs the first that
        // fails, because a type error makes the test result uninformative
        // rather than additionally informative.
        // Last by position: the wire layer does not dedupe `kinds`, and a
        // kind listed twice must not end the call at its first pass.
        for (i, kind) in kinds.iter().enumerate() {
            let v = match kind {
                CheckKind::Typecheck => {
                    trellis_checks::typecheck(&self.root, &checks, &paths).await
                }
                CheckKind::Tests => trellis_checks::run_tests(&self.root, &checks, &paths).await,
                CheckKind::Import => {
                    let module = paths.first().and_then(|p| p.to_str()).unwrap_or(".");
                    trellis_checks::import_check(&self.root, &checks, module).await
                }
                CheckKind::Governance => {
                    // Audited over the files the run touched, not the whole
                    // tree: a rule broken somewhere this run never went is a
                    // pre-existing condition, and failing a run for it would
                    // make every run in a non-compliant repository fail.
                    let scope: Vec<String> = if requested.is_empty() {
                        self.touched.clone()
                    } else {
                        requested.to_vec()
                    };
                    trellis_checks::governance_audit(&self.root, &self.rules, &scope)
                }
            };
            if !v.passed() {
                return ToolResult::Checks(v);
            }
            if i + 1 == kinds.len() {
                return ToolResult::Checks(v);
            }
        }
        ToolResult::Error("no check kinds requested".into())
    }

    /// `rel` under the root, for reading, or why it may not be read
    /// ([`read_refusal`]).
    fn resolve_in_repo(&self, rel: &str) -> Result<PathBuf, String> {
        match read_refusal(&self.root, rel) {
            Some(why) => Err(why),
            None => Ok(self.root.join(rel)),
        }
    }
}

/// Why `write_file` may not create `rel` because git ignores it, or
/// `None`. Rollback's `git clean -fd` leaves ignored files, so a file
/// created there would outlive a run that did not converge.
///
/// `check-ignore` exits 0 for ignored, 1 for not ignored, and 128 on a
/// fatal error. Only a 1 lets the write go ahead: a check that could not be
/// made is not a "no", and reading it as one wrote files a rollback's `git
/// clean` might never remove. Fail closed. The approval gate's prediction
/// of `write_file` asks this same function, so a write the orchestrator
/// refuses is not first put to a person.
pub(crate) async fn ignored_refusal(root: &Path, rel: &str) -> Option<String> {
    // Overrides and key scrub come with the constructor.
    let mut git = tokio::process::Command::from(trellis_vcs::command());
    git.args(["check-ignore", "-q", "--no-index", "--"])
        .arg(rel)
        .current_dir(root)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let status = git.status().await;
    match status.as_ref().map(|s| s.code()) {
        Ok(Some(1)) => None,
        Ok(Some(0)) => Some(format!(
            "`{rel}` is ignored by git, so a rollback could not remove it. Choose a path \
             git tracks."
        )),
        other => {
            let why = match other {
                Err(e) => format!("git could not be started: {e}"),
                Ok(Some(c)) => format!("`git check-ignore` exited {c}"),
                Ok(None) => "`git check-ignore` was ended by a signal".to_string(),
            };
            Some(format!(
                "could not ask git whether `{rel}` is ignored ({why}), and a file git \
                 ignores would survive a rollback. Nothing was written."
            ))
        }
    }
}

/// Defined in `trellis-core`, where the binary's prompt-input reader uses
/// the same predicate.
pub(crate) use trellis_core::paths::names_git_dir;

/// Whether `rel` names trellis's own directory, `.trellis/` at the root,
/// by any spelling that resolves to it: its first component (after any
/// `.`) compared case-insensitively, after the trailing dots and spaces
/// Windows strips, and including the 8.3 short name `TRELLI~n` Windows
/// gives a dot-leading name. Only the first component: a nested
/// `app/.trellis/` is not the one trellis reads.
///
/// The one predicate for "under `.trellis/`" (ADR 0023 §5): the model's
/// `edit` and `write_file`, the approval gate's prediction of them, and a
/// process agent's breach test all use it.
pub fn names_trellis_dir(rel: &str) -> bool {
    let Some(first) = rel
        .split(['/', '\\'])
        .find(|seg| !seg.is_empty() && *seg != ".")
    else {
        return false;
    };
    let s = first.trim_end_matches(['.', ' ']).to_ascii_lowercase();
    s == ".trellis"
        || s.strip_prefix("trelli~")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
}

/// `rel` as it really lands, through any link on the way: the canonical
/// form of its nearest existing ancestor, relative to the canonical root,
/// with the part that does not exist yet appended as written. `Err` when
/// that ancestor is outside the repository: a committed link to `/etc` or
/// to `~` is a path the string rules cannot see.
pub(crate) fn real_rel(root: &Path, rel: &str) -> Result<String, String> {
    let Ok(real_root) = std::fs::canonicalize(root) else {
        return Ok(rel.to_string());
    };
    let mut at = root.join(rel);
    let mut tail: Vec<std::ffi::OsString> = Vec::new();
    loop {
        if let Ok(real) = std::fs::canonicalize(&at) {
            let Ok(inside) = real.strip_prefix(&real_root) else {
                return Err(format!(
                    "`{rel}` leads outside the repository, through a link"
                ));
            };
            let mut p = inside.to_path_buf();
            for t in tail.iter().rev() {
                p.push(t);
            }
            return Ok(p.to_string_lossy().replace('\\', "/"));
        }
        match (at.file_name().map(|n| n.to_os_string()), at.parent()) {
            (Some(name), Some(parent)) => {
                tail.push(name);
                at = parent.to_path_buf();
            }
            _ => return Ok(rel.to_string()),
        }
    }
}

/// Why the model may not read or list `rel`, or `None`: a path outside the
/// repository, or git's own directory by any spelling or through any link.
///
/// The model proposes these paths and the grammar guarantees only that
/// they are well-formed strings (S7 metric 3). `..` is the cheap way a
/// well-formed path becomes a write outside the tree the rollback covers,
/// and a committed link the less cheap one.
/// `.git/config` can hold a remote URL with a credential in it, and an
/// observation goes into a hosted request body and the committed trail.
pub(crate) fn read_refusal(root: &Path, rel: &str) -> Option<String> {
    if let Some(why) = unsafe_repo_path(rel) {
        return Some(why);
    }
    match real_rel(root, rel) {
        Err(why) => Some(why),
        Ok(real) if names_git_dir(rel) || names_git_dir(&real) => {
            Some(format!("`{rel}` is inside .git"))
        }
        Ok(_) => None,
    }
}

/// Why the model may not write at `rel`, or `None`. The one predicate
/// `edit`, `write_file` and the approval gate's prediction of them share,
/// so a call the gate lets through unasked is one the orchestrator
/// refuses: outside the repository, inside `.git` (rollback is git's own
/// reset and does not restore git's directory), or under `.trellis/`,
/// which holds the hooks, rules and checks that judge the run — by any
/// spelling, and through any link.
pub(crate) fn write_refusal(root: &Path, rel: &str) -> Option<String> {
    if let Some(why) = read_refusal(root, rel) {
        return Some(why);
    }
    let real = real_rel(root, rel).unwrap_or_else(|_| rel.to_string());
    if names_trellis_dir(rel) || names_trellis_dir(&real) {
        return Some(format!(
            "`{rel}` is under .trellis/, which holds the hooks, rules and checks that judge \
             this run. Nothing a run does may change them."
        ));
    }
    None
}

/// Why `rel` cannot be used as a path inside the repository, or `None`.
///
/// Shared with the approval gate, which lets through (unasked) any call it
/// predicts the orchestrator will refuse: a gate and an orchestrator that
/// disagreed would skip the person on a call that then ran. Defined in
/// `trellis-core`, where the binary's prompt-template names obey it too;
/// the rules it applies are documented there.
pub(crate) use trellis_core::paths::unsafe_repo_path;

/// How many lines of a file a failed anchor is allowed to quote back.
///
/// The window is the scarce resource on local weights (S5), and the
/// observation is truncated from the front further downstream, so an
/// unbounded excerpt would push the rest of the transcript out rather than
/// help. A file longer than this says so and names `read_file`.
const EXCERPT_LINES: usize = 120;

/// How many bytes of a file a failed anchor may quote back.
///
/// A line bound alone is not a bound. Generated Python embeds data blobs on a
/// single line, so 120 lines can be megabytes — and this text goes into an
/// observation, which goes into the prompt, where the window is the scarce
/// resource (S5). Roughly 120 lines of ordinary code, so the line limit is
/// what bites for a file a person wrote and this is what bites for one a
/// program wrote.
const EXCERPT_BYTES: usize = 8_192;

/// A file, as text to put in front of the model, bounded and honest about it.
fn excerpt(src: &str) -> String {
    let total = src.lines().count();
    let mut out: String = if total <= EXCERPT_LINES {
        src.to_string()
    } else {
        let head: Vec<&str> = src.lines().take(EXCERPT_LINES).collect();
        format!(
            "{}\n...{} more line(s) not shown. Use read_file for the rest.",
            head.join("\n"),
            total - EXCERPT_LINES
        )
    };

    if out.len() > EXCERPT_BYTES {
        // On a character boundary: a Python file is UTF-8 and slicing by
        // bytes would panic on the identifier of anyone who does not write
        // in ASCII.
        let cut = (0..=EXCERPT_BYTES)
            .rev()
            .find(|i| out.is_char_boundary(*i))
            .unwrap_or(0);
        out.truncate(cut);
        out.push_str("\n...cut here; the rest of this file is too large to show.");
    }
    out
}

/// The worktree digest around a hook (ADR 0023, "after a pass"), off the
/// async runtime: it is two blocking git processes, and on the runtime's
/// own thread it stalled every other task for their duration. `None` when
/// it could not be taken, a panicked blocking task included.
async fn worktree_digest(root: &Path) -> Option<String> {
    let root = root.to_path_buf();
    tokio::task::spawn_blocking(move || trellis_vcs::worktree_digest(&root).ok())
        .await
        .ok()
        .flatten()
}

/// Whether the worktree changed since `before`, which is `None` when no
/// hook was bracketed. A digest that could not be taken on either side
/// counts as a change: the guard fails closed, because "could not tell" is
/// not "unchanged".
async fn changed_since(root: &Path, before: Option<Option<String>>) -> bool {
    let Some(before) = before else {
        return false;
    };
    match (before, worktree_digest(root).await) {
        (Some(b), Some(a)) => a != b,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(d.path().join("app")).unwrap();
        std::fs::write(
            d.path().join("app/util.py"),
            "def helper():\n    return 1\n",
        )
        .unwrap();
        std::fs::write(
            d.path().join("app/main.py"),
            "from app.util import helper\n\ndef go():\n    return helper()\n",
        )
        .unwrap();
        d
    }

    #[tokio::test]
    async fn a_radius_comes_back_with_its_coverage() {
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();
        match h.blast_radius("helper").await {
            ToolResult::Radius(r) => {
                assert_eq!(r.certain.len(), 1, "go calls helper");
                // S2: there is no API that hands back reach without limits.
                // The reach and the doubt are asserted separately because
                // there is no total to assert — see ADR 0014.
                assert_eq!(r.coverage.reached(), 1);
                assert!(
                    r.coverage.touched_files > 0,
                    "the doubt figure must arrive with the scale it ranges over"
                );
            }
            other => panic!("expected a radius, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_edit_updates_the_graph_the_next_radius_sees() {
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();

        let before = match h.blast_radius("helper").await {
            ToolResult::Radius(r) => r.certain.len(),
            other => panic!("{other:?}"),
        };
        assert_eq!(before, 1);

        let out = h
            .edit(
                "app/main.py",
                "    return helper()",
                "    helper()\n    return helper()",
            )
            .await;
        assert!(matches!(out, ToolResult::Edit(e) if e.applied));

        // A stale index would still say 1 here, and every later claim about
        // blast radius would be describing code that no longer exists.
        match h.blast_radius("helper").await {
            ToolResult::Radius(r) => assert_eq!(r.certain.len(), 2),
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn an_ambiguous_edit_anchor_is_refused() {
        let d = repo();
        std::fs::write(
            d.path().join("app/main.py"),
            "def a():\n    return 1\n\ndef b():\n    return 1\n",
        )
        .unwrap();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();

        let out = h.edit("app/main.py", "    return 1", "    return 2").await;
        match out {
            ToolResult::Error(e) => assert!(e.contains("occurs 2 times"), "{e}"),
            other => panic!("an ambiguous anchor must not be applied: {other:?}"),
        }
        // And nothing was written.
        let after = std::fs::read_to_string(d.path().join("app/main.py")).unwrap();
        assert!(after.contains("def a():\n    return 1"));
        // Ambiguity is fixed by a longer anchor, and the message has to say
        // so — "occurs 2 times" alone leaves the model to work that out.
        match h.edit("app/main.py", "    return 1", "    return 2").await {
            ToolResult::Error(e) => {
                assert!(e.contains("Extend `old`"), "{e}");
                // And it has to say what to extend it *from*. Measured at the
                // turn after this failure against qwen3:8b: 0 of 8 attempts
                // produced a usable anchor without the file, 6 of 8 with it.
                assert!(
                    e.contains("def a():") && e.contains("def b():"),
                    "an anchor cannot be extended from a file the model has \
                     not been shown: {e}"
                );
            }
            other => panic!("{other:?}"),
        }
    }

    #[tokio::test]
    async fn an_anchor_that_is_not_there_gets_shown_the_file() {
        // The failure that ate a whole run: asked to fix `describe`, the model
        // anchored on a signature it had invented, was told only that it
        // occurred zero times, and invented another. Four turns, no edit.
        let d = repo();
        std::fs::write(
            d.path().join("app/main.py"),
            "def describe(n: int) -> int:\n    return f\"count={n}\"\n",
        )
        .unwrap();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();

        let out = h
            .edit(
                "app/main.py",
                "def describe() -> int:",
                "def describe() -> str:",
            )
            .await;
        match out {
            ToolResult::Error(e) => {
                assert!(e.contains("not found"), "{e}");
                assert!(
                    e.contains("def describe(n: int) -> int:"),
                    "the real text is what makes the next anchor copyable: {e}"
                );
            }
            other => panic!("a missing anchor must not be applied: {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_multi_line_anchor_matches_a_file_checked_out_with_crlf() {
        // The bug that stopped the first real run converging. The model had
        // already found the right edit; it was refused five times because it
        // wrote `\n` and the file, checked out on Windows under git's default
        // `autocrlf`, held `\r\n`. Nothing in the message said so.
        let d = repo();
        let p = d.path().join("app/main.py");
        std::fs::write(
            &p,
            "def describe(n: int) -> int:\r\n    return f\"c={n}\"\r\n",
        )
        .unwrap();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();

        let out = h
            .edit(
                "app/main.py",
                "def describe(n: int) -> int:\n    return f\"c={n}\"",
                "def describe(n: int) -> str:\n    return f\"c={n}\"",
            )
            .await;
        assert!(
            matches!(&out, ToolResult::Edit(e) if e.applied),
            "an anchor written with LF must match a CRLF file: {out:?}"
        );

        let after = std::fs::read_to_string(&p).unwrap();
        assert!(after.contains("-> str:"), "{after:?}");
        // And the file keeps its own convention: rewriting every line ending
        // would show up as a whole-file diff for a one-line change.
        assert!(
            after.contains("\r\n"),
            "CRLF must survive the edit: {after:?}"
        );
        assert!(
            !after.contains("\n\n") && !after.replace("\r\n", "").contains('\n'),
            "no line ending may be left bare: {after:?}"
        );
    }

    #[tokio::test]
    async fn the_recorded_span_points_into_the_file_not_into_the_copy() {
        // The trail's action log records this number. Matching happens on an
        // LF-normalized copy, so on a CRLF checkout the offset drifts by one
        // byte per preceding line — and a recorded figure that does not point
        // where it says is worse than no figure at all.
        let d = repo();
        let p = d.path().join("app/main.py");
        let text = "a = 1\r\nb = 2\r\nc = 3\r\ntarget = 4\r\n";
        std::fs::write(&p, text).unwrap();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();

        let out = h.edit("app/main.py", "target = 4", "target = 5").await;
        let ToolResult::Edit(e) = out else {
            panic!("{out:?}")
        };
        let (start, end) = e.span;
        let after = std::fs::read_to_string(&p).unwrap();
        assert_eq!(
            &after[start..end],
            "target = 5",
            "the span has to select the text it claims, in the file as written"
        );
        assert_eq!(start, text.find("target").unwrap(), "{:?}", e.span);
    }

    #[tokio::test]
    async fn an_lf_file_is_not_given_carriage_returns_it_never_had() {
        let d = repo();
        let p = d.path().join("app/main.py");
        std::fs::write(&p, "def a() -> int:\n    return 1\n").unwrap();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();

        let out = h.edit("app/main.py", "-> int", "-> str").await;
        assert!(matches!(&out, ToolResult::Edit(e) if e.applied), "{out:?}");
        let after = std::fs::read_to_string(&p).unwrap();
        assert!(!after.contains('\r'), "an LF file stays LF: {after:?}");
    }

    #[test]
    fn a_long_file_is_cut_and_says_how_much_it_cut() {
        // Constraint 1's habit applied to an observation: a silent cut reads
        // as the whole file, and an anchor chosen from it would miss again.
        let src: String = (0..200).map(|i| format!("line {i}\n")).collect();
        let e = excerpt(&src);
        assert!(e.contains("line 0") && e.contains("line 119"), "{e}");
        assert!(!e.contains("line 120"), "{e}");
        assert!(e.contains("80 more line(s) not shown"), "{e}");
    }

    #[test]
    fn a_short_file_is_quoted_whole_and_unannotated() {
        let src = "a\nb\nc\n";
        assert_eq!(excerpt(src), src, "nothing to say about an uncut file");
    }

    #[test]
    fn a_few_enormous_lines_are_bounded_too() {
        // A line bound alone is not a bound. Generated Python embeds data
        // blobs on one line, so a file well under the line limit can still be
        // megabytes — and this goes into the prompt, where the window is the
        // scarce resource (S5).
        let src = format!("data = \"{}\"\n", "x".repeat(200_000));
        let e = excerpt(&src);
        assert!(e.len() <= EXCERPT_BYTES + 64, "{} bytes", e.len());
        assert!(e.contains("too large to show"), "and it says so");
    }

    #[test]
    fn a_cut_never_lands_inside_a_character() {
        // Python source is UTF-8, and an identifier does not have to be
        // ASCII. Slicing by bytes would panic on the first contributor who
        // does not write in English.
        let src = format!("# {}\n", "é".repeat(200_000));
        let e = excerpt(&src);
        assert!(e.len() <= EXCERPT_BYTES + 64, "{} bytes", e.len());
        // Round-trips, which it could not do if the cut split a code point.
        assert_eq!(String::from_utf8(e.clone().into_bytes()).unwrap(), e);
    }

    #[tokio::test]
    async fn paths_escaping_the_repository_are_refused() {
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();

        for bad in ["../outside.py", "app/../../outside.py"] {
            match h.read_file(bad).await {
                ToolResult::Error(e) => assert!(e.contains(".."), "{e}"),
                other => panic!("`{bad}` must be refused, got {other:?}"),
            }
            match h.edit(bad, "a", "b").await {
                ToolResult::Error(_) => {}
                other => panic!("`{bad}` must be refused for writes too, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn creating_a_file_is_refused_in_terms_that_say_why() {
        // The failure this prevents is silent: a new file would be written to
        // disk and never enter the index, so every later blast radius would
        // be computed against a tree it does not describe.
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();

        match h
            .edit(
                "app/brand_new.py",
                "",
                "def x():
    pass
",
            )
            .await
        {
            ToolResult::Error(e) => {
                assert!(e.contains("does not exist"), "{e}");
                assert!(
                    e.contains("index"),
                    "the message must say why, not just that it failed: {e}"
                );
            }
            other => panic!("creating a file must be refused, got {other:?}"),
        }
        assert!(!d.path().join("app/brand_new.py").exists());
    }

    #[tokio::test]
    async fn a_denied_path_is_refused_by_the_orchestrator_not_asked_about() {
        // The rule has to bind here, at the only write path to the tree. A
        // rule that lives in the prompt is a suggestion, and "do not edit
        // generated files" is worthless as a suggestion.
        let d = repo();
        std::fs::create_dir_all(d.path().join("vendor")).unwrap();
        std::fs::write(
            d.path().join("vendor/lib.py"),
            "def x():
    return 1
",
        )
        .unwrap();

        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default())
            .with_governance(crate::governance::Governance::new(&["vendor/".into()]))
            .spawn();

        match h.edit("vendor/lib.py", "return 1", "return 2").await {
            ToolResult::Error(e) => {
                assert!(e.contains("vendor/"), "the refusal names the rule: {e}");
                assert!(!e.contains("  "), "{e:?}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
        assert!(
            std::fs::read_to_string(d.path().join("vendor/lib.py"))
                .unwrap()
                .contains("return 1"),
            "the file must be untouched"
        );
        assert!(h.touched().await.is_empty());
        // And an allowed path still works, or the rule has eaten the tool.
        assert!(matches!(
            h.edit("app/util.py", "return 1", "return 2").await,
            ToolResult::Edit(_)
        ));
    }

    fn forbid_rule(root: &std::path::Path, pattern: &str, scope: &str) {
        let g = root.join(".trellis").join("governance");
        std::fs::create_dir_all(&g).unwrap();
        std::fs::write(
            g.join("r.toml"),
            format!(
                "name = \"No direct DB access\"
rank = \"non-negotiable\"
                 forbid = {{ pattern = \"{pattern}\" }}
scope = [\"{scope}\"]
"
            ),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn a_broken_non_negotiable_fails_the_verdict_through_the_check_dispatch() {
        // The audit function has its own unit test. This one covers the wiring,
        // which is where S20 actually happened: a patch described in three
        // documents, tested at the layer below, and never reached by a run.
        // `last_check_passed` is what convergence reads, so a governance
        // violation that does not arrive as a failing Verdict is a violation a
        // model converges straight past.
        let d = repo();
        forbid_rule(d.path(), "psycopg2.connect", "app/");
        std::fs::write(
            d.path().join("app/util.py"),
            "def helper():
    return psycopg2.connect('x')
",
        )
        .unwrap();

        let rules = trellis_checks::governance::load(d.path()).expect("rules load");
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default())
            .with_rules(rules)
            .spawn();

        match h
            .run_checks(vec!["app/util.py".into()], vec![CheckKind::Governance])
            .await
        {
            ToolResult::Checks(v) => {
                assert!(
                    !v.passed(),
                    "a non-negotiable violation must fail the audit"
                );
                assert_eq!(v.detail().kind, CheckKind::Governance);
                assert!(
                    v.detail().stdout.contains("psycopg2.connect"),
                    "the failure names what was broken: {:?}",
                    v.detail().stdout
                );
            }
            other => panic!("expected a verdict, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn the_audit_scopes_to_what_the_run_touched_not_the_whole_tree() {
        // A rule broken somewhere this run never went is a pre-existing
        // condition. Failing the run for it would make every run in a
        // non-compliant repository fail, which trains a user to ignore
        // governance entirely — the opposite of the point.
        let d = repo();
        forbid_rule(d.path(), "psycopg2.connect", "app/");
        std::fs::write(
            d.path().join("app/main.py"),
            "def go():
    return psycopg2.connect('x')
",
        )
        .unwrap();

        let rules = trellis_checks::governance::load(d.path()).expect("rules load");
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default())
            .with_rules(rules)
            .spawn();

        // The run touches util.py, which is clean. main.py is dirty and
        // untouched.
        assert!(matches!(
            h.edit("app/util.py", "return 1", "return 2").await,
            ToolResult::Edit(_)
        ));
        match h.run_checks(vec![], vec![CheckKind::Governance]).await {
            ToolResult::Checks(v) => {
                assert!(
                    v.passed(),
                    "a violation in a file the run never touched is not this run's: {:?}",
                    v.detail().stdout
                );
                // Asserted directly rather than through pass/fail. The
                // regression this guards — the caller's paths shadowed by the
                // `["."]` default — produces a scope that is merely *out of*
                // every rule's scope, which also passes. Only the count says
                // which file was actually examined.
                assert!(
                    v.detail().command.contains("1 file(s) read of 1 in scope"),
                    "the audit must have read the one file the run touched: {:?}",
                    v.detail().command
                );
            }
            other => panic!("expected a verdict, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_denied_path_is_refused_identically_whether_or_not_it_exists() {
        // The check runs before the file is read, so the message cannot be
        // used to learn which denied files are present. A small thing, but the
        // alternative is a refusal that differs by whether the file is there,
        // which is an oracle nobody intended to build.
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default())
            .with_governance(crate::governance::Governance::new(&["secrets/".into()]))
            .spawn();

        let missing = h.edit("secrets/nope.py", "a", "b").await;
        match missing {
            ToolResult::Error(e) => {
                assert!(e.contains("secrets/"), "{e}");
                assert!(
                    !e.contains("does not exist"),
                    "a denied path must not report whether it exists: {e}"
                );
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_edit_that_changes_nothing_is_refused_not_applied() {
        // S7 floor run: a weak model emitted edit{old: "156", new: "156"} over
        // and over. Without this the anchor occurs once, the identical bytes
        // are written back, and the model is told the edit applied — so it
        // reasons from a change that never happened, the file is recorded in
        // the trail as proposed, and a rollback restores something untouched.
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();

        let before = std::fs::read_to_string(d.path().join("app/util.py")).unwrap();
        match h.edit("app/util.py", "return 1", "return 1").await {
            ToolResult::Error(e) => {
                assert!(e.contains("change nothing"), "{e}");
                assert!(
                    !e.contains("  "),
                    "a message the model reads must not carry formatter debris: {e:?}"
                );
            }
            other => panic!("a no-op edit must be refused, got {other:?}"),
        }
        assert_eq!(
            std::fs::read_to_string(d.path().join("app/util.py")).unwrap(),
            before
        );
        assert!(
            h.touched().await.is_empty(),
            "a refused edit must not enter the rollback set or the trail's \
             proposed list"
        );
    }

    #[tokio::test]
    async fn a_missing_file_is_refused_in_one_readable_sentence() {
        // The message is assembled from several source lines, which is how it
        // acquired a run of twenty spaces the last time it was written. It
        // goes to the model and to the user, so it is asserted, not assumed.
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();
        match h.edit("app/nope.py", "a", "b").await {
            ToolResult::Error(e) => {
                assert!(e.contains("does not exist"), "{e}");
                assert!(!e.contains("  "), "{e:?}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn coverage_report_with_no_paths_is_refused_in_one_readable_sentence() {
        // Written across two source lines, so formatter debris can reach the
        // model here as it did in the refusals above (#404).
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default())
            .with_tools(crate::registry::ToolSet::every_builtin())
            .spawn();
        let out = h.call(&ToolCall::CoverageReport { paths: vec![] }).await;
        match out.result {
            ToolResult::Error(e) => {
                assert!(e.contains("at least one path"), "{e}");
                assert!(!e.contains("  "), "{e:?}");
            }
            other => panic!("expected a refusal, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_kind_listed_twice_does_not_skip_the_kinds_between() {
        // The wire layer does not dedupe `kinds` (#404). Finding the last
        // kind by value returned the first `governance` pass and never ran
        // the typecheck between, reporting a pass for a check never run.
        let d = repo();
        let checks = CheckConfig {
            typecheck: vec!["trellis-404-no-such-checker".into()],
            ..CheckConfig::default()
        };
        let (h, _j) = Orchestrator::new(d.path(), checks).spawn();
        let kinds = vec![
            CheckKind::Governance,
            CheckKind::Typecheck,
            CheckKind::Governance,
        ];
        match h.run_checks(vec![], kinds).await {
            ToolResult::Checks(v) => {
                assert!(!v.passed(), "the typecheck cannot run, so it fails");
                assert!(
                    v.detail().command.contains("trellis-404-no-such-checker"),
                    "the typecheck between the two governance kinds must run: {:?}",
                    v.detail().command
                );
            }
            other => panic!("expected a verdict, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn edited_files_are_recorded_for_rollback() {
        let d = repo();
        let (h, _j) = Orchestrator::new(d.path(), CheckConfig::default()).spawn();
        assert!(h.touched().await.is_empty());

        h.edit("app/util.py", "return 1", "return 2").await;
        h.edit("app/util.py", "return 2", "return 3").await;
        assert_eq!(
            h.touched().await,
            vec!["app/util.py"],
            "a file edited twice is one file to restore, not two"
        );
    }

    /// The sandbox boundary, and nothing tested it.
    ///
    /// `resolve_in_repo` is what keeps a model's proposed path inside the
    /// tree rollback covers. Mutation reported `replace || with &&` as
    /// surviving, and that mutant only rejects a path that is **both**
    /// absolute **and** contains `..` — so `../../etc/passwd`, which is
    /// neither exotic nor hard for a model to emit, would be accepted and
    /// written outside the repository.
    ///
    /// The grammar guarantees these strings are well-formed and nothing
    /// more (S7 metric 3). Well-formed is not safe.
    #[test]
    fn a_path_that_escapes_the_repository_is_refused() {
        let d = repo();
        let o = Orchestrator::new(d.path(), CheckConfig::default());

        for bad in [
            "../outside.py",
            "../../etc/passwd",
            "app/../../outside.py",
            "app/./../../outside.py",
        ] {
            let e = o
                .resolve_in_repo(bad)
                .expect_err(&format!("`{bad}` escapes the root and must be refused"));
            assert!(e.contains(".."), "the error must say why: {e}");
        }

        // Absolute paths, in both spellings this project runs on.
        for bad in ["/etc/passwd", r"C:\Windows\System32\drivers\etc\hosts"] {
            if Path::new(bad).is_absolute() {
                assert!(
                    o.resolve_in_repo(bad).is_err(),
                    "`{bad}` is absolute and must be refused"
                );
            }
        }
    }

    #[test]
    fn an_ordinary_relative_path_resolves_under_the_root() {
        // The other half: the guard must not be so strict that normal work
        // is impossible. A refusal that blocks `app/util.py` would be found
        // in a minute; one that admits `../` would not.
        let d = repo();
        let o = Orchestrator::new(d.path(), CheckConfig::default());

        let p = o.resolve_in_repo("app/util.py").expect("an ordinary path");
        assert!(p.starts_with(d.path()), "{p:?} is outside {:?}", d.path());
        assert!(p.ends_with("util.py"));

        // A `.` component is not an escape.
        assert!(o.resolve_in_repo("./app/util.py").is_ok());
    }

    #[test]
    fn rooted_and_drive_relative_paths_are_refused_on_every_platform() {
        // The risk: on Windows `/x`, `\x` and `C:x` are not `is_absolute()`,
        // and `root.join` of any of them leaves the repository — `/x` and
        // `\x` to the root of the drive, `C:x` to drive C's current
        // directory. Asserted on every platform, because the rule is on the
        // string, not on how this host happens to read it.
        let d = repo();
        let o = Orchestrator::new(d.path(), CheckConfig::default());
        for bad in [
            "/x",
            r"\x",
            "C:x",
            r"C:\x",
            "c:/x",
            r"\\server\share\x",
            r"\\?\C:\x",
            "app\\..\\..\\x",
            "app/.. /x",
            "app/../x",
            "a.py:stream",
        ] {
            assert!(
                o.resolve_in_repo(bad).is_err(),
                "`{bad}` must be refused as a repository path"
            );
            assert!(
                unsafe_repo_path(bad).is_some(),
                "`{bad}`: the predicate the approval gate shares must agree"
            );
        }
        for ok in [
            "app/util.py",
            "./app/util.py",
            "a/b.c/d.py",
            "..hidden/x.py",
        ] {
            assert!(unsafe_repo_path(ok).is_none(), "`{ok}` is ordinary");
        }
    }
}
