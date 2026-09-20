//! The interactive session: `trellis` with no subcommand.
//!
//! One-shot commands were the wrong default shape for this tool. Every
//! invocation re-derived the whole index, answered one question and threw it
//! away, and the user re-typed `--root` and `--model` each time. The
//! architecture already wanted the other shape: the orchestrator owns the
//! `Index` as a single-owner actor precisely so it can be held across many
//! operations.
//!
//! So the session holds state the one-shot path cannot:
//!
//! - **The index, built once.** `/radius` after the first is instant.
//! - **The model.** Set once with `/model`, used by every task.
//! - **What the last run did.** So `/resume` and a follow-up task mean
//!   something.
//!
//! **It does not hold a conversation.** Each task is a fresh run with a fresh
//! budget and a fresh snapshot, because a run's guarantee is that it rolls
//! back to the state it started from — and a run that inherited another's
//! uncommitted edits could not make that promise. Carrying context forward is
//! `--resume`, which reads a committed trail, and is deliberate rather than
//! automatic (S11).

use anyhow::{Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use trellis_index::Index;

pub struct Session {
    root: PathBuf,
    index: Index,
    model: Option<String>,
    unconstrained: bool,
    /// Tasks run this session, for `/status`.
    tasks_run: u32,
}

impl Session {
    pub fn open(root: &Path, model: Option<String>) -> Result<Self> {
        let started = std::time::Instant::now();
        let index = trellis_index::build_cached(root);
        let elapsed = started.elapsed();

        let s = Self {
            root: root.to_path_buf(),
            index,
            model,
            unconstrained: false,
            tasks_run: 0,
        };
        s.banner(elapsed);
        Ok(s)
    }

    fn banner(&self, indexed_in: std::time::Duration) {
        let c = &self.index.coverage;
        println!();
        println!("TRELLIS  ·  {}", self.root.display());
        println!(
            "  index    {} files, {} definitions, {} first-party call sites ({:.0} ms)",
            self.index.files.len(),
            self.index.symbols.len(),
            c.total(),
            indexed_in.as_millis()
        );
        // The coverage figure is the product's honesty mechanism and it does
        // not get to be absent just because the surface changed (S2).
        match c.resolved_fraction() {
            Some(f) => println!(
                "           {:.1}% bound — a lower bound on recall, not recall",
                f * 100.0
            ),
            None => println!("           no call sites examined"),
        }
        match &self.model {
            Some(m) => println!("  model    {m}"),
            None => println!("  model    none — /model <spec> before running a task"),
        }
        println!();
        println!("Type a task to run the loop. /help for commands, /exit to leave.");
        println!();
    }

    /// Reads until end of input or `/exit`.
    pub fn run(mut self) -> Result<()> {
        let stdin = std::io::stdin();
        loop {
            let pending = trellis_vcs::pending(&self.root)
                .map(|p| p.len())
                .unwrap_or(0);
            if pending > 0 {
                print!("[{pending} pending] › ");
            } else {
                print!("› ");
            }
            std::io::stdout().flush().ok();

            let mut line = String::new();
            // Zero bytes is end of input — a piped script that ran out, or
            // Ctrl-D. Leaving is the only sensible reading of it.
            match stdin.read_line(&mut line) {
                Ok(0) => {
                    println!();
                    return Ok(());
                }
                Ok(_) => {}
                Err(e) => return Err(e).context("reading input"),
            }

            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // A command that fails must not end the session. The whole point
            // of staying resident is that a typo costs a line, not a restart.
            if let Err(e) = self.dispatch(line) {
                if e.to_string() == EXIT {
                    return Ok(());
                }
                println!("error: {e:#}");
            }
            println!();
        }
    }

    fn dispatch(&mut self, line: &str) -> Result<()> {
        let (cmd, rest) = match line.split_once(char::is_whitespace) {
            Some((c, r)) => (c, r.trim()),
            None => (line, ""),
        };

        match cmd {
            "/exit" | "/quit" | "/q" => anyhow::bail!(EXIT),
            "/help" | "/?" => {
                help();
                Ok(())
            }
            "/status" => {
                self.status();
                Ok(())
            }
            "/index" => self.reindex(),
            "/radius" if rest.is_empty() => {
                anyhow::bail!("/radius needs a symbol, e.g. `/radius describe`")
            }
            "/radius" => crate::radius_of(&self.index, rest),
            "/check" => {
                let paths = rest.split_whitespace().map(PathBuf::from).collect();
                crate::cmd_check(&self.root, paths)
            }
            "/doctor" => crate::cmd_doctor(&self.root, self.model.as_deref()),
            "/resume" => crate::cmd_resume(&self.root),
            "/diff" => self.show_pending(),
            "/accept" => self.accept(rest),
            "/revert" => self.revert(),
            "/model" if rest.is_empty() => {
                match &self.model {
                    Some(m) => println!("model    {m}"),
                    None => println!("model    none"),
                }
                Ok(())
            }
            "/model" => {
                // Parsed now rather than at first use, so a typo is reported
                // while the user is still thinking about it.
                crate::Driver::parse(rest, self.unconstrained)
                    .with_context(|| format!("`{rest}` is not a usable model spec"))?;
                println!("model    {rest}");
                self.model = Some(rest.to_string());
                Ok(())
            }
            "/unconstrained" => {
                self.unconstrained = !self.unconstrained;
                println!(
                    "tier     {}",
                    if self.unconstrained {
                        "4 (validate and retry) — a run can fail on format alone"
                    } else {
                        "2 (runtime-constrained) where the runtime supports it"
                    }
                );
                Ok(())
            }
            other if other.starts_with('/') => {
                anyhow::bail!("unknown command `{other}`. /help lists them.")
            }
            _ => self.task(line),
        }
    }

    fn reindex(&mut self) -> Result<()> {
        let started = std::time::Instant::now();
        // `build` rather than `build_cached`: `/index` is the command whose
        // job is to refresh, so reading the cache would make it a no-op.
        self.index = trellis_index::build(&self.root);
        let c = &self.index.coverage;
        println!(
            "indexed {} files in {:.0} ms",
            self.index.files.len(),
            started.elapsed().as_millis()
        );
        println!("  {}", c.summary());
        Ok(())
    }

    fn task(&mut self, task: &str) -> Result<()> {
        let Some(model) = self.model.clone() else {
            anyhow::bail!(concat!(
                "no model set, so there is nothing to drive the loop.\n",
                "  /model llamacpp:http://127.0.0.1:8080   a serving runtime\n",
                "  /model scripted:fix.json                a fixed list of tool calls\n",
                "Everything else — /index, /radius, /check, /doctor — works without one."
            ));
        };

        self.tasks_run += 1;
        let outcome = crate::cmd_run(&self.root, task, &model, self.unconstrained, "1", false);

        // A run writes to the tree, so the session's index is now behind it.
        // Rebuilding here rather than lazily means the next `/radius` answers
        // about the code as it now stands, which is the only version anyone
        // cares about.
        self.index = trellis_index::build(&self.root);
        outcome
    }

    /// What a converged run left behind.
    ///
    /// A run leaves its edits uncommitted on purpose — acceptance is the
    /// user's — but in a session that means the *next* task cannot start,
    /// because a run needs a clean worktree to guarantee its rollback. So the
    /// session has to close that loop rather than sending the user to another
    /// terminal to do it.
    fn show_pending(&self) -> Result<()> {
        let pending = trellis_vcs::pending(&self.root)?;
        if pending.is_empty() {
            println!("nothing pending; the worktree is clean");
            return Ok(());
        }
        for line in &pending {
            println!("  {line}");
        }
        println!();
        println!("/accept [message]  commit these    /revert  discard them");
        Ok(())
    }

    fn accept(&self, message: &str) -> Result<()> {
        let pending = trellis_vcs::pending(&self.root)?;
        if pending.is_empty() {
            anyhow::bail!("nothing pending to accept");
        }
        // Paths, not `commit -a`: a run records exactly what it proposed, and
        // sweeping up unrelated edits the user happened to have open would
        // make the commit a worse record than the trail beside it.
        let paths: Vec<String> = pending
            .iter()
            .filter_map(|l| l.split_whitespace().nth(1).map(str::to_string))
            .collect();
        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let msg = if message.is_empty() {
            "trellis: accepted proposed changes"
        } else {
            message
        };
        trellis_vcs::commit_paths(&self.root, &refs, msg)?;
        println!("committed {} path(s)", refs.len());
        for p in &refs {
            println!("  {p}");
        }
        Ok(())
    }

    fn revert(&self) -> Result<()> {
        let pending = trellis_vcs::pending(&self.root)?;
        if pending.is_empty() {
            anyhow::bail!("nothing pending to revert");
        }
        trellis_vcs::discard_pending(&self.root)?;
        println!("discarded {} pending change(s)", pending.len());
        Ok(())
    }

    fn status(&self) {
        let c = &self.index.coverage;
        println!("root     {}", self.root.display());
        println!(
            "index    {} files, {} first-party call sites, {} resolved",
            self.index.files.len(),
            c.total(),
            c.resolved
        );
        match &self.model {
            Some(m) => println!("model    {m}"),
            None => println!("model    none"),
        }
        println!(
            "tier     {}",
            if self.unconstrained {
                "4 (validate and retry)"
            } else {
                "2 where supported"
            }
        );
        println!("tasks    {} this session", self.tasks_run);
    }
}

/// Sentinel for "the user asked to leave", carried as an error so one `?` in
/// `dispatch` unwinds to the loop. Not a failure, and `run` checks for it
/// before reporting anything.
const EXIT: &str = "__trellis_exit__";

fn help() {
    println!(
        "{}",
        concat!(
            "  <task>            run the agent loop against a task\n",
            "\n",
            "  /index            rebuild the index\n",
            "  /radius <symbol>  who calls it, with the doubt beside it\n",
            "  /check [paths]    run the repository's checks\n",
            "  /doctor           probe runtime, tier, checkers\n",
            "  /resume           what the last recorded run established\n",
            "\n",
            "  /diff             what the last run left uncommitted\n",
            "  /accept [msg]     commit it\n",
            "  /revert           discard it\n",
            "\n",
            "  /model [spec]     set or show the model\n",
            "  /unconstrained    toggle tier 4 (no schema; can fail on format)\n",
            "  /status           root, index, model, tier\n",
            "  /exit             leave\n",
            "\n",
            "Each task is a fresh run: its own snapshot, its own budget, and a\n",
            "rollback to the state it started from if it does not converge. The\n",
            "session keeps the index and the model, not a conversation."
        )
    );
}
