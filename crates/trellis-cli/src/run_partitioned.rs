//! A multi-agent run (`trellis run --agents N --target <symbol>...`, ADR
//! 0045): the plan its targets make, each partition by its own agent, and
//! the one trail they share. `run_cmd::run_body` branches here before it
//! would take a snapshot.

use crate::approval::{Human, answer_on_the_line};
use crate::driver::Driver;
use crate::observed_model::Observed;
use crate::run_cmd::{
    CompiledNote, RepoInputs, RunExit, TrailFacts, on_run_stack, read_only_action, record_trail,
    repo_context, report_trail, run_governance, summarise,
};
use crate::{config, context_files, hosted, output, tooling};
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::num::NonZeroUsize;
use std::path::Path;
use std::sync::{Arc, Mutex};
use trellis_core::PartitionId;

/// `trellis run --agents N --target <symbol>...` (ADR 0045, #466; N of 1
/// since #482): the plan the targets make, which targets each partition
/// holds, and how many partitions run at once.
pub(crate) struct Fanout {
    pub(crate) plan: trellis_loop::partition::Plan,
    /// Every target, as the user typed it.
    targets: Vec<String>,
    /// Each partition's targets, as typed, in command-line order.
    names: HashMap<PartitionId, Vec<String>>,
    /// `max(1, min(N, plan.parallel.len()))`: the plan decides concurrency,
    /// never the flag (ADR 0045 §4). N is already 1 on a backend not
    /// measured to batch (#483).
    pub(crate) width: NonZeroUsize,
    /// Why N is what it is, for the `agents:` line.
    why: String,
}

impl Fanout {
    /// The plan `targets` make, as `trellis partitions` shows it, run by up
    /// to `agents` at once. Each target resolves as `radius` resolves it; an
    /// unknown or ambiguous one is refused (ADR 0045 §1).
    pub(crate) fn plan(root: &Path, targets: &[String], agents: u32, why: String) -> Result<Self> {
        // A target named twice is one work unit, not two.
        let mut unique: Vec<String> = Vec::with_capacity(targets.len());
        for t in targets {
            if !unique.contains(t) {
                unique.push(t.clone());
            }
        }
        let targets = unique;
        let index = trellis_index::build_cached(root);
        let mut units = Vec::with_capacity(targets.len());
        let mut resolved = Vec::with_capacity(targets.len());
        for t in &targets {
            let id = crate::index_cmd::resolve_symbol(&index, t)?;
            units.push(trellis_loop::partition::unit_for(&index, id));
            resolved.push((t.clone(), id));
        }
        let plan = trellis_loop::partition::plan(&index, units);
        let names = plan
            .in_order()
            .map(|p| {
                let mine = resolved
                    .iter()
                    .filter(|(_, id)| p.units.iter().any(|u| u.target == *id))
                    .map(|(t, _)| t.clone())
                    .collect();
                (p.id, mine)
            })
            .collect();
        let width = NonZeroUsize::new(
            usize::try_from(agents)
                .unwrap_or(usize::MAX)
                .min(plan.parallel.len()),
        )
        .unwrap_or(NonZeroUsize::MIN);
        Ok(Self {
            plan,
            targets,
            names,
            width,
            why,
        })
    }

    /// What a script needs to choose one agent's steps: every target of the
    /// run, and the ones partition `id` holds.
    pub(crate) fn script_targets(&self, id: PartitionId) -> (&[String], &[String]) {
        (
            &self.targets,
            self.names.get(&id).map(Vec::as_slice).unwrap_or(&[]),
        )
    }
}

/// `trellis run --agents`, parsed (ADR 0045 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Agents {
    /// At least 1.
    Count(u32),
    /// From the backend's measured batching capacity (S18).
    Auto,
}

/// `0` would be a run with no agent, and a fraction or a word other than
/// `auto` has no meaning to guess at, so both are refused naming what is
/// accepted.
pub(crate) fn parse_agents(s: &str) -> Result<Agents> {
    if s == "auto" {
        return Ok(Agents::Auto);
    }
    match s.parse::<u32>() {
        Ok(n) if n >= 1 => Ok(Agents::Count(n)),
        _ => anyhow::bail!("--agents takes a number of at least 1, or `auto`; got `{s}`"),
    }
}

/// Resolves every target and the number of agents `--agents` asks for, with
/// why it is that number.
///
/// Called before the snapshot, so an unknown target costs nothing. Targets
/// resolve before the number is judged, so a mistyped symbol is reported as
/// that rather than hidden behind a width.
pub(crate) fn agents_for(
    root: &Path,
    width: Agents,
    targets: &[String],
    spec: &str,
    from_user: bool,
) -> Result<(u32, String)> {
    let index = trellis_index::build_cached(root);
    for t in targets {
        crate::index_cmd::resolve_symbol(&index, t)?;
    }
    let (n, why) = match width {
        Agents::Count(1) => (1, "--agents is 1".to_string()),
        // An explicit N asks the backend as `auto` does (exit gate item 10,
        // #483): N agents on a runtime that serves one request at a time is
        // slower than one after another, and a user who typed N would not
        // know why. Only measured continuous batching keeps N.
        Agents::Count(n) => match batching(root, spec, from_user)? {
            (cap, why) if cap.supports_multi_agent() => (n, format!("--agents is {n}; {why}")),
            (_, why) => (
                1,
                format!(
                    "--agents {n} is 1 because {why}; {n} at once needs a backend measured to \
                     batch continuously, such as llama.cpp started with --parallel {n}"
                ),
            ),
        },
        Agents::Auto => {
            let (cap, why) = batching(root, spec, from_user)?;
            let n = cap.auto_agents();
            (n, format!("--agents auto is {n} because {why}"))
        }
    };
    Ok((n, why))
}

/// The backend's batching capability, by the probe `doctor` uses, with why it
/// is that. Reads the runtime and writes nothing; the probe's own timeout
/// bounds it. A slot count that could not be read is `Unknown`, said so.
fn batching(
    root: &Path,
    spec: &str,
    from_user: bool,
) -> Result<(trellis_model::BatchingCapability, String)> {
    use trellis_model::BatchingCapability as B;
    let parsed: trellis_model::ModelSpec = spec.parse()?;
    let cfg = config::RepoConfig::load(root)?;
    Ok(
        match crate::doctor_cmd::probe_batching(&parsed, &cfg, from_user) {
            Ok(
                cap @ B::Continuous {
                    measured_concurrency,
                },
            ) => (
                cap,
                format!("the runtime reports {measured_concurrency} slots"),
            ),
            Ok(cap @ B::SingleStream) => (cap, "the runtime reports a single slot".into()),
            Ok(cap @ B::ContinuousUnmeasured) => (
                cap,
                "the backend batches continuously but does not report how much".into(),
            ),
            Ok(cap @ B::Unknown) => (
                cap,
                "the backend's batching capability is unknown (a scripted model has none)".into(),
            ),
            Err(e) => (
                B::Unknown,
                format!(
                    "the runtime's slot count could not be read ({e}), so its capability is unknown"
                ),
            ),
        },
    )
}

/// What `run_body` hands a multi-agent run at its branch point: everything
/// decided before the snapshot.
pub(crate) struct Partitioned<'a> {
    pub(crate) root: &'a Path,
    pub(crate) task: &'a str,
    pub(crate) model_spec: &'a str,
    pub(crate) spec: &'a trellis_model::ModelSpec,
    /// Whether the run's driver is tier 4, after the schema probe.
    pub(crate) tier4: bool,
    pub(crate) model_from_user: bool,
    pub(crate) model_source: hosted::SpecSource,
    /// The endpoint `preflight` identified as colibri, when it did, whose
    /// drivers send its request profile (ADR 0047 decision 5).
    pub(crate) colibri: Option<&'a hosted::Resolved>,
    pub(crate) fanout: &'a Fanout,
    pub(crate) cfg: &'a config::RepoConfig,
    pub(crate) dial: &'a config::Control,
    pub(crate) tooling: &'a tooling::Tooling,
    pub(crate) vault_refusal: Option<&'a (String, String)>,
    pub(crate) vault_setting: Option<&'a config::VaultSetting>,
    pub(crate) vault: Option<CompiledNote>,
    pub(crate) prior: Option<trellis_loop::PriorContext>,
    pub(crate) added: &'a context_files::Loaded,
    pub(crate) skill_dirs: &'a [std::path::PathBuf],
    pub(crate) stamp: &'a (String, String),
    pub(crate) run_id: &'a str,
    pub(crate) label: Option<String>,
    pub(crate) abort: Option<std::sync::Arc<output::Abort>>,
    pub(crate) human: Human,
    /// The run's replay recording: one `targets` section per agent.
    pub(crate) rec: &'a mut trellis_loop::Recording,
}

/// What one partition's agent left behind, for the trail. Filled by the
/// partition's own task, whether or not it converged, because a partition
/// that did not converge reports only an error string to the supervisor.
struct PartitionRun {
    transcript: trellis_loop::Transcript,
    touched: Vec<String>,
    gates: Vec<trellis_loop::GateDenial>,
    tokens: Option<u64>,
    prompt_tokens: Option<u64>,
    compactions: Vec<trellis_loop::Compacted>,
    requested: Vec<trellis_loop::RequestedCompaction>,
    /// Every step its driver gave, for the replay recording (ADR 0056).
    steps: Vec<trellis_loop::Step>,
}

/// The task one partition's agent is given: the run's task, and which of
/// its targets are this agent's (#466).
fn partition_task(task: &str, mine: &[String], partitions: usize) -> String {
    format!(
        "{task}\n\nThis run has {partitions} agents, each in its own copy of the repository. \
         Yours works on: {}. Change what those need, and leave the other targets to their \
         agents.",
        mine.join(", ")
    )
}

/// Whether the root is back where the run found it: HEAD at `base` and
/// nothing uncommitted. Read, never restored: the rollback is
/// `run_integrated`'s, and this only reports on it. A tree that cannot be
/// read is not one known to be restored.
fn is_restored_to(root: &Path, base: &str) -> bool {
    head_of(root).is_ok_and(|h| h == base) && trellis_vcs::pending(root).is_ok_and(|p| p.is_empty())
}

/// The commit `HEAD` names.
fn head_of(root: &Path) -> Result<String> {
    let out = trellis_vcs::command()
        .args(["rev-parse", "HEAD"])
        .current_dir(root)
        .output()
        .context("running git rev-parse HEAD")?;
    anyhow::ensure!(
        out.status.success(),
        "rev-parse HEAD failed: {}",
        String::from_utf8_lossy(&out.stderr).trim()
    );
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// A multi-agent run (#466): each partition of the plan by its own agent in
/// its own worktree, then the integrated check over the merged union. At
/// width 1 the partitions run one after another on the same path (#482).
///
/// `Supervisor::run_integrated` owns the snapshot and the rollback; nothing
/// here takes or restores one. Each partition's orchestrator has the run's
/// governance (the vault's notes root included, ADR 0049), rules, tools and
/// checks, rooted at its worktree; its own driver; and its share of the run's
/// one budget (`Budget::shares`, rule 9). The policy is all-or-nothing, so a
/// partition that did not converge merges nothing, and a partition's own pass
/// never converges the run: only the integrated verdicts do (rules 5 and 6).
/// The trail is one, with every partition's actions in plan order.
///
/// Refused before this is reached: approve mode at a width above 1, trusted
/// repository code (its guards watch the root), and `--image`.
pub(crate) fn run_partitioned(
    p: Partitioned<'_>,
    out: &mut (dyn std::io::Write + Send),
) -> Result<RunExit> {
    let Partitioned {
        root,
        task,
        model_spec,
        spec,
        tier4,
        model_from_user,
        model_source,
        colibri,
        fanout,
        cfg,
        dial,
        tooling,
        vault_refusal,
        vault_setting,
        vault,
        prior,
        added,
        skill_dirs,
        stamp,
        run_id,
        label,
        abort,
        human,
        rec,
    } = p;
    let plan = &fanout.plan;
    let count = plan.in_order().count();

    // Every partition's driver before anything is written, so a script
    // refused for one of them, or a budget too small to divide, costs
    // nothing.
    let mut drivers = HashMap::new();
    for part in plan.in_order() {
        let driver = match colibri {
            Some(r) => Driver::Hosted(Box::new(hosted::build(r, cfg, true)?)),
            None => Driver::build(
                spec,
                tier4,
                &tooling.tools,
                cfg,
                model_from_user,
                model_source,
                Some(fanout.script_targets(part.id)),
            )?,
        };
        drivers.insert(part.id, driver);
    }
    let budget = cfg.budget();
    budget.shares(u32::try_from(count).unwrap_or(u32::MAX))?;
    let base = head_of(root)?;

    let (rules, governance) = run_governance(root, cfg, vault_refusal, None)?;
    let (repo, skill_policy) = repo_context(
        RepoInputs {
            root,
            profile: None,
            governance: &governance,
            rules: &rules,
            vault_refusal,
            vault_setting,
            vault,
            tooling,
            added,
            skill_dirs,
        },
        &mut *out,
    )?;
    // At width 1 the partitions run one after another (ADR 0045 §2), and the
    // line says so rather than "up to 1 at once" (#482).
    let pace = match fanout.width.get() {
        1 => "one at a time".to_string(),
        n => format!("up to {n} at once"),
    };
    let _ = writeln!(
        out,
        "agents:   {count} partition(s), {pace} ({})",
        fanout.why
    );
    for part in plan.in_order() {
        let _ = writeln!(
            out,
            "          partition {}: {}{}",
            part.id.0,
            fanout
                .names
                .get(&part.id)
                .map(|n| n.join(", "))
                .unwrap_or_default(),
            if part.conservative {
                " (runs first, alone: the graph cannot vouch for it)"
            } else {
                ""
            }
        );
    }
    let _ = writeln!(out, "base:     {base}");

    let checks = cfg.checks();
    let results: Arc<Mutex<HashMap<PartitionId, PartitionRun>>> = Arc::default();
    let drivers = Arc::new(Mutex::new(drivers));
    // At width 1 approve mode is allowed: one agent asks at a time, about
    // its own worktree. Above 1 it was refused in `run_body`.
    let (gate_tx, mut answering) = match (dial.mode, human) {
        (config::ControlMode::Auto, _) | (_, Human::Absent) => (None, None),
        (config::ControlMode::Approve, Human::Channel(tx)) => (Some(tx), None),
        (config::ControlMode::Approve, Human::Line(read)) => {
            let (tx, rx) = tokio::sync::mpsc::channel(1);
            (Some(tx), Some((rx, read)))
        }
    };
    let skills = trellis_loop::SkillAccess {
        user_dirs: skill_dirs.to_vec(),
        policy: skill_policy,
    };
    let supervisor = trellis_loop::Supervisor {
        root,
        run: run_id,
        base: &base,
        budget: &budget,
        width: fanout.width,
        policy: trellis_loop::FailurePolicy::AllOrNothing,
    };
    let work = |part: trellis_loop::PartitionTask| {
        let driver = drivers.lock().ok().and_then(|mut d| d.remove(&part.id));
        let mut orchestrator = trellis_loop::Orchestrator::new(&part.root, checks.clone())
            .with_governance(governance.clone())
            .with_rules(rules.clone())
            .with_tools(tooling.tools.clone())
            .with_env(tooling.env.clone())
            .with_skills(skills.clone());
        if let Some(tx) = &gate_tx {
            orchestrator = orchestrator.with_gate(Box::new(trellis_loop::ApprovalGate::new(
                &part.root,
                dial.approve.clone(),
                tx.clone(),
            )));
        }
        let mine = fanout.names.get(&part.id).cloned().unwrap_or_default();
        let text = partition_task(task, &mine, count);
        let seed = trellis_loop::Seed {
            prior: prior.clone(),
            repo: repo.clone(),
            steering: None,
            // None for `run_end`: each partition's loop keeps its own clock
            // against its own share (ADR 0008), alongside others at a width
            // above 1, so no one of them is the run's spend.
            elapsed: None,
            // rpc starts no partitioned run, and `pause` reaches only rpc's.
            hold: None,
            // A raise is to the run's whole ceiling and a partition runs on
            // its share of it; handing each partition the whole would grant
            // one raise N times (rule 9). Re-dealt through `Budget::shares`
            // by whatever first lets a partitioned run be raised.
            budget: None,
            // A partition is given no steering channel to withdraw from.
            fates: None,
        };
        let results = Arc::clone(&results);
        async move {
            let Some(mut driver) = driver else {
                return Err("no driver was built for it".to_string());
            };
            let (handle, join) = orchestrator.spawn();
            let budget = &part.budget;
            // Behind `Observed` for its steps, with no steering port; the
            // summary is not used here.
            let (mut done, mut steps) = (None, Vec::new());
            let (outcome, transcript) = match &mut driver {
                Driver::Scripted(m) => {
                    let mut m = Observed::new(m, None, &mut done, &mut steps);
                    trellis_loop::run_seeded(&text, &mut m, &handle, budget, seed).await
                }
                Driver::Runtime(m) => {
                    let mut m = Observed::new(m, None, &mut done, &mut steps);
                    trellis_loop::run_seeded(&text, &mut m, &handle, budget, seed).await
                }
                Driver::Hosted(m) => {
                    let mut m = Observed::new(&mut **m, None, &mut done, &mut steps);
                    trellis_loop::run_seeded(&text, &mut m, &handle, budget, seed).await
                }
            };
            let touched = handle.touched().await;
            let gates = handle.gate_log().await;
            // The orchestrator ends once its handle is gone; waited for, so
            // nothing of it is still running when the worktree is merged and
            // removed.
            drop(handle);
            let _ = join.await;
            let failed = match &outcome {
                trellis_loop::Outcome::Converged { .. } => None,
                trellis_loop::Outcome::NotConverged { reason, .. } => Some(reason.explain()),
            };
            if let Ok(mut r) = results.lock() {
                r.insert(
                    part.id,
                    PartitionRun {
                        transcript,
                        touched,
                        gates,
                        tokens: driver.tokens(),
                        prompt_tokens: driver.prompt_tokens(),
                        compactions: driver.compactions(),
                        requested: driver.requested_compactions(),
                        steps,
                    },
                );
            }
            // `Err` for a partition that did not converge, or the supervisor
            // would merge edits no check passed.
            match failed {
                None => Ok(()),
                Some(why) => Err(why),
            }
        }
    };

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let raced = on_run_stack(|| {
        rt.block_on(async {
            let answer = async {
                if let Some((rx, read)) = answering.take() {
                    answer_on_the_line(rx, read, &mut *out).await;
                }
                std::future::pending::<()>().await
            };
            // Ctrl-C drops `run_integrated`'s future, whose own guard rolls
            // the root back to its snapshot; the partitions' tasks are
            // aborted with it and their worktrees removed as they drop.
            tokio::select! {
                r = supervisor.run_integrated(plan, &checks, work) => Some(r),
                _ = answer => None,
                _ = tokio::signal::ctrl_c() => None,
                _ = output::aborted(abort) => None,
            }
        })
    })?;
    // Not a plain drop, for the reason `run_body` gives: a line reader
    // blocked on stdin would hold it. The partitions' tasks are dropped
    // either way, and with them their worktrees.
    rt.shutdown_background();
    drop(gate_tx);

    let mut results = match Arc::try_unwrap(results) {
        Ok(m) => m.into_inner().unwrap_or_default(),
        Err(shared) => shared
            .lock()
            .map(|mut m| std::mem::take(&mut *m))
            .unwrap_or_default(),
    };
    let mut trail = PartitionedTrail::default();
    let reports: HashMap<PartitionId, String> = match &raced {
        Some(Ok(i)) => i
            .reports
            .iter()
            .map(|r| {
                (
                    r.id,
                    match &r.outcome {
                        Ok(()) => "converged in its worktree".to_string(),
                        Err(e) => e.to_string(),
                    },
                )
            })
            .collect(),
        _ => HashMap::new(),
    };
    let _ = writeln!(out);
    // Nothing ran when the snapshot refused the tree, and there is nothing
    // to record (ADR 0056).
    let refused_at_snapshot = matches!(
        &raced,
        Some(Err(trellis_loop::SuperviseError::Vcs(
            trellis_vcs::VcsError::DirtyWorktree(_) | trellis_vcs::VcsError::NotARepo(_)
        )))
    );
    if !refused_at_snapshot {
        rec.base_commit = base.clone();
    }
    for part in plan.in_order() {
        let status = reports
            .get(&part.id)
            .cloned()
            .unwrap_or_else(|| "did not finish".to_string());
        let _ = writeln!(out, "partition {}: {status}", part.id.0);
        let names = fanout.names.get(&part.id).map(Vec::as_slice).unwrap_or(&[]);
        let mut run = results.remove(&part.id);
        // Keyed by the agent's first target: `ScriptedModel::from_json_for`
        // gives an agent the one section among its targets, and refuses two.
        // An agent that never finished left no steps to record.
        if let (Some(r), Some(first)) = (&mut run, names.first()) {
            rec.targets
                .insert(first.clone(), std::mem::take(&mut r.steps));
        }
        trail.partition(part.id, names, &status, run);
    }

    let facts = |gaps, integrated, plan_actions| TrailFacts {
        approved: trellis_trail::Approval::None,
        base_commit: &base,
        model: Some(model_spec),
        control: Some(dial.describe()),
        gates: &trail.gates,
        stamp: Some(stamp),
        tokens: trail.tokens,
        prompt_tokens: trail.prompt_tokens,
        agent: None,
        label: label.clone(),
        inputs: added.inputs.clone(),
        compactions: &trail.compactions,
        requested: &trail.requested,
        services: &[],
        plan: plan_actions,
        integrated,
        suspected_recall_gaps: gaps,
    };
    let transcript = trellis_loop::Transcript {
        task: task.to_string(),
        turns: std::mem::take(&mut trail.turns),
        final_notice: false,
        steering: trellis_loop::SteeringLog::default(),
        prior: None,
        repo: trellis_loop::RepoContext::default(),
    };

    let Some(result) = raced else {
        let _ = writeln!(
            out,
            "INTERRUPTED. The partitions were stopped and the root rolled back to its \
             snapshot; nothing is accepted on an abort."
        );
        let restored = is_restored_to(root, &base);
        let _ = writeln!(out, "Worktree restored: {restored}.");
        let plan_actions = std::mem::take(&mut trail.plan);
        let written = record_trail(
            root,
            task,
            &transcript,
            &trail.touched,
            facts(Vec::new(), Vec::new(), plan_actions),
            &mut *out,
        );
        report_trail(&mut *out, written, Some("interrupted"));
        return Ok(RunExit::Interrupted { restored });
    };
    let integrated = match result {
        Ok(i) => i,
        // Refused at the snapshot: nothing ran, and there is nothing to
        // record or roll back.
        Err(trellis_loop::SuperviseError::Vcs(
            e @ (trellis_vcs::VcsError::DirtyWorktree(_) | trellis_vcs::VcsError::NotARepo(_)),
        )) => {
            return Err(anyhow::Error::new(e))
                .context("a clean git worktree is required before a run");
        }
        Err(e) => {
            // A merge that failed part-way, or a rollback that did: the
            // message says which, and how to finish a failed one by hand.
            let _ = writeln!(out, "\nNOT CONVERGED: {e}");
            let restored = is_restored_to(root, &base);
            let _ = writeln!(out, "Worktree restored: {restored}.");
            let plan_actions = std::mem::take(&mut trail.plan);
            let written = record_trail(
                root,
                task,
                &transcript,
                &trail.touched,
                facts(Vec::new(), Vec::new(), plan_actions),
                &mut *out,
            );
            report_trail(&mut *out, written, Some("survives the rollback"));
            return Ok(RunExit::NotConverged { restored });
        }
    };

    // The integrated verdicts: the only ones that can converge this run.
    let mut integrated_checks = Vec::new();
    let mut plan_actions = std::mem::take(&mut trail.plan);
    let after = u32::try_from(transcript.turns.len())
        .unwrap_or(u32::MAX)
        .saturating_add(1);
    for (v, ms) in integrated.checks.iter().zip(&integrated.check_ms) {
        let kind = format!("{:?}", v.kind());
        let _ = writeln!(
            out,
            "integrated {kind}: {}",
            if v.passed() { "passed" } else { "FAILED" }
        );
        if !v.passed() {
            const SHOWN: usize = 24;
            let d = v.detail();
            for line in d.stdout.lines().chain(d.stderr.lines()).take(SHOWN) {
                let _ = writeln!(out, "  {line}");
            }
        }
        plan_actions.push(read_only_action(
            after,
            "integrated_check".to_string(),
            format!("{kind} on whole repository"),
            if v.passed() { "passed" } else { "failed" }.to_string(),
        ));
        integrated_checks.push(trellis_trail::CheckOutcome {
            kind,
            passed: v.passed(),
            // The supervisor's timing around the call (#622).
            duration_ms: Some(*ms),
        });
    }
    let gaps: Vec<trellis_trail::SuspectedRecallGap> = integrated
        .suspected_recall_gaps
        .iter()
        .map(|g| trellis_trail::SuspectedRecallGap {
            partitions: g.partitions.iter().map(|p| p.0).collect(),
            files: g.files.clone(),
            check: format!("{:?}", g.check),
        })
        .collect();
    for g in &gaps {
        let _ = writeln!(
            out,
            "recall:   the integrated {} failure named {} from partitions {}: a collision \
             the plan did not predict, so an edge the graph may have missed (suspected)",
            g.check,
            g.files.join(", "),
            g.partitions
                .iter()
                .map(u32::to_string)
                .collect::<Vec<_>>()
                .join(" and ")
        );
    }

    match &integrated.integration {
        trellis_loop::Integration::Converged => {
            let _ = writeln!(out);
            let _ = writeln!(
                out,
                "CONVERGED: {count} partition(s) merged, and the integrated checks passed on \
                 the union."
            );
            let _ = writeln!(
                out,
                "files written: {}",
                if integrated.merged.is_empty() {
                    "none".to_string()
                } else {
                    integrated.merged.join(", ")
                }
            );
            let path = record_trail(
                root,
                task,
                &transcript,
                &integrated.merged,
                facts(gaps, integrated_checks, plan_actions),
                &mut *out,
            )?;
            let _ = writeln!(out, "trail:    {}", path.display());
            let _ = writeln!(
                out,
                "{}",
                concat!(
                    "Changes are left in the working tree for you to review. ",
                    "Nothing is committed:\n",
                    "acceptance is yours, and the trail records the run as unapproved ",
                    "until you give it."
                )
            );
            Ok(RunExit::Converged)
        }
        trellis_loop::Integration::RolledBack(why) => {
            let _ = writeln!(out);
            let _ = writeln!(out, "NOT CONVERGED: {why}.");
            let restored = is_restored_to(root, &base);
            let _ = writeln!(out, "Rolled back to {base}. Worktree restored: {restored}.");
            // After the rollback, as in `run_body`: the rollback cleans
            // untracked files, and the trail is one until it is committed.
            let written = record_trail(
                root,
                task,
                &transcript,
                &trail.touched,
                facts(gaps, integrated_checks, plan_actions),
                &mut *out,
            );
            report_trail(&mut *out, written, Some("survives the rollback"));
            Ok(RunExit::NotConverged { restored })
        }
    }
}

/// Every partition's record, joined in plan order into what one trail
/// holds: turns numbered on from the partition before, and an action
/// opening each partition that names its targets and how it ended.
#[derive(Default)]
struct PartitionedTrail {
    turns: Vec<trellis_loop::Turn>,
    plan: Vec<trellis_trail::Action>,
    gates: Vec<trellis_loop::GateDenial>,
    compactions: Vec<trellis_loop::Compacted>,
    requested: Vec<trellis_loop::RequestedCompaction>,
    /// Every path a partition wrote, in plan order, once each.
    touched: Vec<String>,
    tokens: Option<u64>,
    prompt_tokens: Option<u64>,
    /// Whether any partition has been added, so a sum starts at the first
    /// rather than at a zero nobody measured.
    started: bool,
}

impl PartitionedTrail {
    fn partition(
        &mut self,
        id: PartitionId,
        names: &[String],
        status: &str,
        run: Option<PartitionRun>,
    ) {
        let offset = u32::try_from(self.turns.len()).unwrap_or(u32::MAX);
        self.plan.push(read_only_action(
            offset.saturating_add(1),
            "partition".to_string(),
            format!("{} ({})", id.0, names.join(", ")),
            summarise(status),
        ));
        // A partition that never ran counts nothing, and a sum over the
        // ones that did would be part of the run presented as all of it.
        let (tokens, prompt_tokens) = run
            .as_ref()
            .map_or((None, None), |r| (r.tokens, r.prompt_tokens));
        let sum = |acc: Option<u64>, this: Option<u64>, first: bool| {
            if first {
                this
            } else {
                acc.zip(this).map(|(a, b)| a.saturating_add(b))
            }
        };
        self.tokens = sum(self.tokens, tokens, !self.started);
        self.prompt_tokens = sum(self.prompt_tokens, prompt_tokens, !self.started);
        self.started = true;
        let Some(run) = run else {
            return;
        };
        self.turns.extend(run.transcript.turns);
        self.gates.extend(run.gates.into_iter().map(|mut g| {
            g.call = g.call.saturating_add(offset);
            g
        }));
        self.compactions
            .extend(run.compactions.into_iter().map(|mut c| {
                c.turn = c.turn.saturating_add(offset);
                c
            }));
        self.requested
            .extend(run.requested.into_iter().map(|mut c| {
                c.turn = c.turn.saturating_add(offset);
                c
            }));
        for t in run.touched {
            if !self.touched.contains(&t) {
                self.touched.push(t);
            }
        }
    }
}
