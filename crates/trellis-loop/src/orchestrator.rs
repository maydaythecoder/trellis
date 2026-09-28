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
use crate::hooks::Hooks;
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
    /// `Some` only when a person was asked about this call.
    pub human: Option<HumanDecision>,
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
        let call = call.clone();
        self.ask(
            |reply| Request::Call { call, reply },
            || Dispatched {
                result: ToolResult::Error("orchestrator stopped".into()),
                human: None,
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
    /// Trusted hooks, for `post_tool`. `pre_tool` is a gate in `gates`.
    hooks: Option<Hooks>,
    /// Model calls served, for `GateDenial::call`.
    calls: u32,
    gate_log: Vec<GateDenial>,
}

impl Orchestrator {
    pub fn new(root: &Path, checks: CheckConfig) -> Self {
        Self {
            root: root.to_path_buf(),
            session: Session::open(root),
            checks,
            touched: Vec::new(),
            governance: GovernanceGate(crate::governance::Governance::default()),
            gates: Vec::new(),
            rules: Vec::new(),
            tools: ToolSet::default(),
            env: Vec::new(),
            hooks: None,
            calls: 0,
            gate_log: Vec::new(),
        }
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
        self.governance = GovernanceGate(g);
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
                    let _ = reply.send(self.gated(call).await.result);
                }
                Request::ReadFile { path, reply } => {
                    let call = ToolCall::ReadFile { path };
                    let _ = reply.send(self.gated(call).await.result);
                }
                Request::ApplyEdit {
                    path,
                    old,
                    new,
                    reply,
                } => {
                    let call = ToolCall::Edit { path, old, new };
                    let _ = reply.send(self.gated(call).await.result);
                }
                Request::RunChecks {
                    paths,
                    kinds,
                    reply,
                } => {
                    let call = ToolCall::RunChecks { paths, kinds };
                    let _ = reply.send(self.gated(call).await.result);
                }
                Request::Call { call, reply } => {
                    self.calls += 1;
                    let _ = reply.send(self.gated(call).await);
                }
                Request::GateLog { reply } => {
                    let _ = reply.send(self.gate_log.clone());
                }
                Request::Audit { reply } => {
                    let _ = reply.send(self.run_checks(&[], &[CheckKind::Governance]).await);
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

    /// Consults the gates in order, then dispatches. The first `Deny` wins
    /// and nothing after it is asked — a human should not be asked about a
    /// call a rule has already refused.
    async fn gated(&mut self, call: ToolCall) -> Dispatched {
        let mut human = None;
        let name = tool_name(&call);
        if !self.tools.enabled(name) {
            return Dispatched {
                result: ToolResult::Error(format!(
                    "`{name}` is not a tool in this run; the tools are {}",
                    self.tools.names().join(", ")
                )),
                human,
            };
        }
        for i in 0..=self.gates.len() {
            let gate: &mut dyn ToolGate = match i {
                0 => &mut self.governance,
                _ => self.gates[i - 1].as_mut(),
            };

            let asked_at = Instant::now();
            let decision = gate.check(&call).await;
            if gate.human_decided() {
                human = Some(HumanDecision {
                    approved: decision == GateDecision::Allow,
                    waited: asked_at.elapsed(),
                });
            }
            if let GateDecision::Deny { reason } = decision {
                if let Some(gate) = gate.name() {
                    self.gate_log.push(GateDenial {
                        call: self.calls,
                        gate: gate.to_string(),
                        target: crate::gate::label(&call),
                        reason: reason.clone(),
                    });
                }
                return Dispatched {
                    result: ToolResult::Error(reason),
                    human,
                };
            }
        }
        let result = match &call {
            ToolCall::BlastRadius { symbol } => self.blast_radius(symbol),
            ToolCall::ReadFile { path } => self.read_file(path),
            ToolCall::Edit { path, old, new } => self.apply_edit(path, old, new),
            ToolCall::RunChecks { paths, kinds } => self.run_checks(paths, kinds).await,
            ToolCall::ReadSkill { name } => match crate::skills::read_body(&self.root, name) {
                Ok(body) => ToolResult::FileContent(body),
                Err(e) => ToolResult::Error(e),
            },
            ToolCall::WriteFile { path, content } => self.write_file(path, content).await,
            ToolCall::Grep { pattern, path } => match path.as_deref().filter(|p| !p.is_empty()) {
                Some(p) => match self.resolve_in_repo(p) {
                    Err(e) => ToolResult::Error(e),
                    Ok(_) => crate::listing::grep(&self.root, pattern, Some(p)).await,
                },
                None => crate::listing::grep(&self.root, pattern, None).await,
            },
            ToolCall::Find { glob } => {
                if glob.starts_with('/') || glob.split(['/', '\\']).any(|s| s == "..") {
                    ToolResult::Error(format!(
                        "glob `{glob}` must be relative to the repository root and must not \
                         contain `..`"
                    ))
                } else {
                    crate::listing::find(&self.root, glob).await
                }
            }
            ToolCall::Ls { path } => match self.resolve_in_repo(path) {
                Err(e) => ToolResult::Error(e),
                Ok(full) => crate::listing::ls(&full, path),
            },
            ToolCall::Bash { command } => self.bash(command).await,
            ToolCall::Custom { name, args } => self.custom(name, args).await,
        };
        // Observe only. The result is already decided and is what the model
        // receives, whatever the hook prints or returns.
        if let Some(h) = &self.hooks {
            let as_json = serde_json::to_value(&result).unwrap_or(serde_json::Value::Null);
            h.post_tool(&call, as_json).await;
        }
        Dispatched { result, human }
    }

    /// Creates a file that does not exist (ADR 0023 §2).
    ///
    /// Governance has already been consulted as a gate. What is checked here
    /// is what makes "rollback removes it" true: the path must be new, must
    /// not be inside `.git`, and must not be ignored by git, because
    /// rollback's `git clean -fd` leaves ignored files where they are.
    async fn write_file(&mut self, path: &str, content: &str) -> ToolResult {
        let full = match self.resolve_in_repo(path) {
            Ok(f) => f,
            Err(e) => return ToolResult::Error(e),
        };
        if Path::new(path)
            .components()
            .any(|c| c.as_os_str() == ".git")
        {
            return ToolResult::Error(format!("write_file refused: `{path}` is inside .git"));
        }
        if full.exists() {
            return ToolResult::Error(format!(
                "{path} already exists. write_file creates new files only; use edit to \
                 change an existing one."
            ));
        }
        let ignored = tokio::process::Command::new("git")
            .args(["check-ignore", "-q", "--no-index", "--"])
            .arg(path)
            .current_dir(&self.root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .await
            .is_ok_and(|s| s.success());
        if ignored {
            return ToolResult::Error(format!(
                "write_file refused: `{path}` is ignored by git, so a rollback could not \
                 remove it. Choose a path git tracks."
            ));
        }
        if let Some(dir) = full.parent()
            && let Err(e) = std::fs::create_dir_all(dir)
        {
            return ToolResult::Error(format!("cannot create the directory for {path}: {e}"));
        }
        // `create_new`, not `write`: a file that appeared since the check
        // above is not overwritten.
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

    async fn bash(&self, command: &str) -> ToolResult {
        let Some(b) = self.tools.bash() else {
            return ToolResult::Error("bash is not enabled in this run".into());
        };
        let mut argv = b.shell.clone();
        argv.push(command.to_string());
        let r = crate::exec::run(crate::exec::Spec {
            argv: &argv,
            cwd: &self.root,
            env: &self.env,
            stdin: None,
            timeout: b.timeout,
            cap: crate::exec::OUTPUT_CAP,
        })
        .await;
        match r {
            Err(e) => ToolResult::Error(e),
            Ok(c) => {
                let status = if c.timed_out {
                    format!("timed out after {}s and was killed", b.timeout.as_secs())
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
                // Not a verdict, and said where the model reads it: a
                // command printing "tests passed" has passed nothing.
                s.push_str("\n(bash output is an observation, not a check)");
                ToolResult::Output(s)
            }
        }
    }

    async fn custom(&self, name: &str, args: &serde_json::Value) -> ToolResult {
        let Some(tool) = self.tools.custom(name) else {
            return ToolResult::Error(format!("`{name}` is not a tool in this run"));
        };
        let r = crate::exec::run(crate::exec::Spec {
            argv: &tool.command,
            cwd: &self.root,
            env: &self.env,
            stdin: Some(args.to_string().into_bytes()),
            timeout: CUSTOM_TIMEOUT,
            cap: crate::exec::OUTPUT_CAP,
        })
        .await;
        match r {
            Err(e) => ToolResult::Error(e),
            Ok(c) if c.timed_out => ToolResult::Error(format!(
                "`{name}` did not finish within {}s and was killed",
                CUSTOM_TIMEOUT.as_secs()
            )),
            Ok(c) if c.code == Some(0) => ToolResult::Output(c.stdout.render()),
            Ok(c) => ToolResult::Error(format!(
                "`{name}` exited with {}: {}{}",
                c.code
                    .map_or_else(|| "no exit code".to_string(), |c| c.to_string()),
                c.stdout.render(),
                if c.stderr.is_empty() {
                    String::new()
                } else {
                    format!("\nstderr:\n{}", c.stderr.render())
                }
            )),
        }
    }

    pub fn index(&self) -> &trellis_index::Index {
        self.session.index()
    }

    fn blast_radius(&self, symbol: &str) -> ToolResult {
        let index = self.session.index();
        let qualified = |s: &trellis_index::SymbolEntry| {
            format!(
                "{}.{}",
                index.files[s.file.index()].module,
                index.interner.resolve(s.name)
            )
        };
        let matches: Vec<_> = index
            .symbols
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                let q = qualified(s);
                q == symbol
                    || q.ends_with(&format!(".{symbol}"))
                    || index.interner.resolve(s.name) == symbol
            })
            .map(|(i, _)| trellis_core::SymbolId::from_index(i))
            .collect();

        match matches.len() {
            0 => ToolResult::Error(format!(
                "no first-party definition matches `{symbol}`; only definitions \
                 inside the repository root are indexed"
            )),
            1 => ToolResult::Radius(trellis_index::radius::blast_radius(index, matches[0])),
            n => ToolResult::Error(format!(
                "`{symbol}` matches {n} definitions; qualify it further"
            )),
        }
    }

    fn read_file(&self, path: &str) -> ToolResult {
        match self.resolve_in_repo(path) {
            Err(e) => ToolResult::Error(e),
            Ok(full) => match std::fs::read_to_string(&full) {
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
        let full = match self.resolve_in_repo(path) {
            Ok(f) => f,
            Err(e) => return ToolResult::Error(e),
        };
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

    async fn run_checks(&self, paths: &[String], kinds: &[CheckKind]) -> ToolResult {
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
        for kind in kinds {
            let v = match kind {
                CheckKind::Typecheck => {
                    trellis_checks::typecheck(&self.root, &self.checks, &paths).await
                }
                CheckKind::Tests => {
                    trellis_checks::run_tests(&self.root, &self.checks, &paths).await
                }
                CheckKind::Import => {
                    let module = paths.first().and_then(|p| p.to_str()).unwrap_or(".");
                    trellis_checks::import_check(&self.root, &self.checks, module).await
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
            if kind == kinds.last().expect("non-empty") {
                return ToolResult::Checks(v);
            }
        }
        ToolResult::Error("no check kinds requested".into())
    }

    /// Rejects anything that escapes the repository root.
    ///
    /// The model proposes these paths and the grammar guarantees only that
    /// they are well-formed strings (S7 metric 3). `..` is the cheap way a
    /// well-formed path becomes a write outside the tree the rollback covers.
    fn resolve_in_repo(&self, rel: &str) -> Result<PathBuf, String> {
        let p = Path::new(rel);
        if p.is_absolute() || p.components().any(|c| c.as_os_str() == "..") {
            return Err(format!(
                "path `{rel}` must be relative to the repository root and must not \
                 contain `..`"
            ));
        }
        Ok(self.root.join(p))
    }
}

/// How long a custom tool may run. The same default as `bash`: a custom
/// tool is a command the repository wrote, and it is bounded like one.
const CUSTOM_TIMEOUT: Duration = Duration::from_secs(120);

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
            "a refused edit must not enter the rollback set or the trail's              proposed list"
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
}
