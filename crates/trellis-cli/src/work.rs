//! `trellis queue` and `trellis work` (ADR 0020).
//!
//! The queue and the heartbeat's decisions live in `trellis-queue`. This is
//! the part that needs the binary: a [`Runner`] whose `run` is `cmd_run`,
//! the same path `trellis run` takes, so a queued task and a typed one cannot
//! drift into two ways of running.

use crate::{Human, RunControl, RunExit, RunSeeding, cmd_run, config};
use anyhow::{Context, Result};
use clap::Subcommand;
use std::io::Write as _;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};
use trellis_queue::{Queue, RunEnd, Runner, State, WorkOptions, plain};

#[derive(Subcommand)]
pub(crate) enum QueueAction {
    /// Add a task. It runs when `trellis work` reaches it, oldest first.
    Add {
        /// Optional when `--prompt` names a template.
        #[arg(default_value = "")]
        task: String,
        /// The vault note to compile for this task's run, as `run --focus`.
        #[arg(long)]
        focus: Option<String>,
        /// A template in `.trellis/prompts/<name>.md`, as `run --prompt`.
        #[arg(long)]
        prompt: Option<String>,
    },
    /// Every task and its state: pending, claimed, done or failed.
    List,
    /// Put a task from claimed/ or failed/ back in pending/.
    ///
    /// For a claim a crashed process left behind. `work` never re-runs one
    /// on its own, because it cannot tell a dead claimant from a live one.
    Requeue { id: String },
}

#[derive(clap::Args)]
pub(crate) struct WorkArgs {
    /// Run at most one task, then exit.
    #[arg(long, conflicts_with_all = ["every", "max_tasks"])]
    once: bool,
    /// When the queue is empty, look again after this many seconds instead
    /// of exiting. Runs in the foreground; Ctrl-C stops it.
    #[arg(long, value_name = "SECONDS")]
    every: Option<u64>,
    /// Run at most this many tasks in this invocation.
    #[arg(long, value_name = "N")]
    max_tasks: Option<u32>,
    /// As `trellis run --model`; falls back to `[model] spec`.
    #[arg(long)]
    model: Option<String>,
}

pub(crate) fn queue(root: &Path, action: QueueAction) -> Result<()> {
    let q = Queue::for_repo(root)?;
    match action {
        QueueAction::Add {
            task,
            focus,
            prompt,
        } => {
            if task.trim().is_empty() && prompt.is_none() {
                anyhow::bail!("nothing to queue: give a task, or --prompt <name>, or both");
            }
            // Checked now rather than when the heartbeat reaches the task,
            // which may be hours later with nobody watching.
            if let Some(name) = &prompt {
                config::RepoConfig::prompt(root, name)?;
            }
            if let Some(stem) = &focus {
                check_focus(root, stem)?;
            }
            let id = q.add(&task, focus.as_deref(), prompt.as_deref())?;
            println!("queued:   {id}");
            println!("          {}", q.dir().join("pending").display());
            Ok(())
        }
        QueueAction::List => {
            let entries = q.list()?;
            if entries.is_empty() {
                println!("queue empty ({})", q.dir().display());
                return Ok(());
            }
            for e in entries {
                println!("{}", list_line(&e));
            }
            Ok(())
        }
        QueueAction::Requeue { id } => {
            let from = q.requeue(&id)?;
            println!(
                "requeued: {} ({}/ -> pending/)",
                plain(&id),
                from.dir_name()
            );
            Ok(())
        }
    }
}

/// The note must exist and compile, exactly as the run will ask.
fn check_focus(root: &Path, stem: &str) -> Result<()> {
    let cfg = config::RepoConfig::load(root)?;
    if let Some(v) = cfg.vault(root, Some(stem))? {
        trellis_vault::compile(&v.dir, stem, trellis_vault::DEFAULT_CAP).with_context(|| {
            format!(
                "the vault at {} would not compile for focus `{stem}`",
                v.source
            )
        })?;
    }
    Ok(())
}

fn list_line(e: &trellis_queue::Entry) -> String {
    let age = e.age.map(ago).unwrap_or_else(|| "?".to_string());
    let state = e.state.dir_name();
    let t = match &e.task {
        Ok(t) => t,
        Err(why) => return format!("{state:<8} {}  unreadable: {}", e.id, plain(why)),
    };
    let mut line = format!("{state:<8} {}  ", e.id);
    match e.state {
        State::Pending => line.push_str(&format!("added {age} ago")),
        State::Claimed => line.push_str(&format!(
            "claimed {age} ago by pid {}",
            t.claimed_by_pid
                .map(|p| p.to_string())
                .unwrap_or_else(|| "?".into())
        )),
        State::Done | State::Failed => {
            if let Some(o) = &t.outcome {
                line.push_str(&format!(
                    "{} ({})",
                    plain(&o.exit),
                    o.run_id.as_deref().map(plain).unwrap_or("no trail".into())
                ));
            }
        }
    }
    if t.attempts > 0 {
        line.push_str(&format!(", attempts {}", t.attempts));
    }
    line.push_str(&format!("\n         {}", plain(&t.task)));
    if let Some(f) = &t.focus {
        line.push_str(&format!(" [focus {}]", plain(f)));
    }
    if let Some(p) = &t.prompt {
        line.push_str(&format!(" [prompt {}]", plain(p)));
    }
    line
}

fn ago(d: Duration) -> String {
    let s = d.as_secs();
    match s {
        0..60 => format!("{s}s"),
        60..3600 => format!("{}m", s / 60),
        3600..86_400 => format!("{}h", s / 3600),
        _ => format!("{}d", s / 86_400),
    }
}

/// The heartbeat. Returns the process exit code.
pub(crate) fn work(root: &Path, args: WorkArgs) -> Result<i32> {
    let cfg = config::RepoConfig::load(root)?;
    let model = args
        .model
        .or_else(|| cfg.model())
        .context("no model. Pass --model, or set [model] spec in .trellis/config.toml")?;
    // Parsed before anything is claimed, so a typo costs nothing rather than
    // failing the task at the head of the queue.
    let _: trellis_model::ModelSpec = model.parse()?;
    let q = Queue::for_repo(root)?;

    let stop = Arc::new(AtomicBool::new(false));
    listen_for_ctrl_c(stop.clone());

    // ADR 0025 §4: the human stream, so `work --json` keeps stdout JSONL.
    let _ = writeln!(
        crate::output::human(),
        "work:     queue at {}",
        q.dir().display()
    );
    let mut runner = CliRunner {
        root,
        model,
        unconstrained: cfg.unconstrained(),
        stop,
    };
    let opts = WorkOptions {
        max_tasks: if args.once { Some(1) } else { args.max_tasks },
        every: args.every.map(Duration::from_secs),
    };
    let (ran, why) = trellis_queue::work(&q, &mut runner, opts, &mut crate::output::human())?;
    let _ = writeln!(
        crate::output::human(),
        "work:     {ran} run(s) this invocation; stopped: {why}"
    );
    Ok(why.code())
}

/// Ctrl-C sets a flag the heartbeat reads between runs and while idle.
///
/// Needed for more than tidiness. `cmd_run` races its run against
/// `tokio::signal::ctrl_c()`, and once that has been called the process's
/// SIGINT handler is tokio's for good — so after the first task a plain
/// `sleep` in `--every` would swallow Ctrl-C and the loop could not be
/// stopped. During a run both listeners fire: `cmd_run` rolls back, and the
/// flag stops the heartbeat after it.
fn listen_for_ctrl_c(flag: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let Ok(rt) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        else {
            return;
        };
        rt.block_on(async {
            while tokio::signal::ctrl_c().await.is_ok() {
                flag.store(true, Ordering::SeqCst);
            }
        });
    });
}

struct CliRunner<'a> {
    root: &'a Path,
    model: String,
    unconstrained: bool,
    stop: Arc<AtomicBool>,
}

impl CliRunner<'_> {
    /// The task text as `trellis run --prompt` builds it.
    ///
    /// Repeated from the `run` dispatch rather than shared, to keep this
    /// change out of the lines other work is editing; both are four lines
    /// over `RepoConfig::prompt`, which is where the rules live.
    fn text(&self, t: &trellis_queue::Task) -> Result<String> {
        Ok(match &t.prompt {
            Some(name) => {
                let template = config::RepoConfig::prompt(self.root, name)?;
                if t.task.trim().is_empty() {
                    template
                } else {
                    format!("{template}\n\n{}", t.task)
                }
            }
            None => t.task.clone(),
        })
    }
}

/// The newest trail's run id: its file stem, which `record_trail` makes
/// `<timestamp>-<slug>`. Read by name only — the contents are untrusted, and
/// the name is all that is needed.
fn newest_run_id(root: &Path) -> Option<String> {
    let rd = std::fs::read_dir(root.join(".trellis").join("runs")).ok()?;
    rd.filter_map(Result::ok)
        .filter_map(|e| {
            let n = e.file_name().to_str()?.to_string();
            n.strip_suffix(".json").map(str::to_string)
        })
        .max()
}

impl Runner for CliRunner<'_> {
    fn blocker(&mut self) -> Option<String> {
        match trellis_vcs::pending(self.root) {
            Ok(p) if p.is_empty() => None,
            Ok(p) => Some(format!(
                "the worktree is not clean ({}), and a run needs a clean one (ADR 0009). \
                 If a converged change is waiting, accept or revert it, then run \
                 `trellis work` again",
                p.iter().map(|l| plain(l)).collect::<Vec<_>>().join("; ")
            )),
            Err(e) => Some(e.to_string()),
        }
    }

    fn run(&mut self, t: &trellis_queue::Task) -> RunEnd {
        let text = match self.text(t) {
            Ok(s) => s,
            Err(e) => {
                return RunEnd::Error {
                    run_id: None,
                    error: format!("{e:#}"),
                };
            }
        };
        let before = newest_run_id(self.root);
        let exit = cmd_run(
            self.root,
            &text,
            &self.model,
            self.unconstrained,
            RunSeeding {
                resume: false,
                focus: t.focus.as_deref(),
            },
            // Auto, whatever the config says: the heartbeat is the hands-off
            // end of the dial (ADR 0020 §5), and nobody is at the terminal to
            // answer. Approve mode here would refuse every task at the head
            // of the queue rather than run it.
            RunControl {
                flag: Some(config::ControlMode::Auto),
                human: Human::Absent,
            },
            &mut crate::output::human(),
        );
        let after = newest_run_id(self.root);
        let run_id = if after != before { after } else { None };
        match exit {
            Ok(RunExit::Converged) => RunEnd::Converged { run_id },
            Ok(RunExit::NotConverged { restored: true }) => RunEnd::RolledBack { run_id },
            Ok(RunExit::NotConverged { restored: false }) => RunEnd::NotRestored { run_id },
            Ok(RunExit::Interrupted { restored }) => RunEnd::Interrupted { run_id, restored },
            Err(e) => RunEnd::Error {
                run_id,
                error: format!("{e:#}"),
            },
        }
    }

    fn stop_requested(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn idle(&mut self, d: Duration) -> bool {
        let until = Instant::now() + d;
        while Instant::now() < until {
            if self.stop_requested() {
                return false;
            }
            std::thread::sleep(
                Duration::from_millis(100).min(until.saturating_duration_since(Instant::now())),
            );
        }
        !self.stop_requested()
    }
}
