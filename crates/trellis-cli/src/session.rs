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

            // Control characters are never a task.
            //
            // Without raw mode an arrow key arrives as an escape sequence —
            // ESC `[` `A` for up — which does not begin with `/`, so it fell
            // through to the model. Pressing up by reflex ran a full agent
            // task: the sequence slugified to "a", a trail was written under
            // that name, and files were edited.
            //
            // Reflex input must never start work. The line is refused and the
            // session says what it does not support, which beats silently
            // discarding it — a user whose up-arrow does nothing at all will
            // press it again.
            if line.chars().any(char::is_control) {
                println!("{}", NO_LINE_EDITING);
                println!();
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

/// Shown when input carries an escape sequence, which is what an arrow key is
/// without raw mode. Names the limit rather than pretending the key did
/// something.
const NO_LINE_EDITING: &str = concat!(
    "that looked like a key this session does not handle (arrow keys, ",
    "history).
There is no line editing yet, so nothing was run. Type the ",
    "line, or /help."
);

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

#[cfg(test)]
mod tests {
    use super::*;

    /// A session over a throwaway repository.
    ///
    /// `Session::open` builds a real index and prints a banner; both are
    /// wanted. Testing a session against a stub would test the stub, and the
    /// defects worth catching here are the ones where a command is routed
    /// somewhere plausible and wrong.
    fn session(dir: &Path) -> Session {
        std::fs::create_dir_all(dir.join("app")).unwrap();
        std::fs::write(
            dir.join("app/m.py"),
            "def helper():\n    return 1\n\n\ndef go():\n    return helper()\n",
        )
        .unwrap();
        Session::open(dir, None).expect("a session over a real directory")
    }

    fn git_repo(dir: &Path) {
        let run = |args: &[&str]| {
            std::process::Command::new("git")
                .args(args)
                .current_dir(dir)
                .output()
                .expect("git");
        };
        run(&["init", "-q"]);
        run(&["config", "user.email", "t@example.invalid"]);
        run(&["config", "user.name", "t"]);
        // Otherwise a checkout rewrites LF to CRLF under a global
        // `autocrlf=true`, and the byte-for-byte assertion below measures
        // git's line-ending policy instead of whether revert restored the
        // file.
        run(&["config", "core.autocrlf", "false"]);
        run(&["add", "-A"]);
        run(&["commit", "-qm", "init"]);
    }

    #[test]
    fn exit_leaves_without_reporting_an_error() {
        // `/exit` unwinds through the same `Result` as a failure, so the
        // sentinel has to be recognised before anything is printed. Getting
        // this wrong means the session exits *and* accuses the user of a
        // mistake on the way out.
        let d = tempfile::tempdir().unwrap();
        let mut s = session(d.path());
        for word in ["/exit", "/quit", "/q"] {
            let e = s.dispatch(word).expect_err("exit unwinds as an error");
            assert_eq!(e.to_string(), EXIT, "{word} must use the sentinel");
        }
    }

    #[test]
    fn an_escape_sequence_never_becomes_a_task() {
        // Found by pressing up-arrow. Without raw mode the key arrives as
        // ESC `[` `A`, which does not begin with `/`, so it fell through to
        // the model: the sequence slugified to "a", a trail was written under
        // that name, and files were edited.
        //
        // Reflex input must never start work. The guard lives in the read
        // loop, so `dispatch` is never reached — this asserts the predicate
        // the guard uses, which is the part that can silently stop matching.
        let up = "\u{1b}[A";
        let down = "\u{1b}[B";
        let del = "\u{7f}";
        let ctrl_c = "\u{3}";
        for seq in [up, down, del, ctrl_c] {
            assert!(
                seq.chars().any(char::is_control),
                "{seq:?} must be recognised as control input"
            );
        }
        // And ordinary input must not be caught by it, or the session refuses
        // everything a user actually types.
        for ok in [
            "fix the type error",
            "/radius describe",
            "rename Foo to Bar",
        ] {
            assert!(
                !ok.chars().any(char::is_control),
                "{ok:?} is ordinary input"
            );
        }
    }

    #[test]
    fn an_unknown_slash_command_is_not_run_as_a_task() {
        // The dangerous misroute. Everything that is not a command is a task
        // for the model, so a mistyped `/radiuss` falling through would spend
        // a budget and edit files because of a typo.
        let d = tempfile::tempdir().unwrap();
        let mut s = session(d.path());
        let e = s.dispatch("/radiuss describe").expect_err("refused");
        assert!(e.to_string().contains("unknown command"), "{e}");
        assert!(
            e.to_string().contains("/help"),
            "and says where to look: {e}"
        );
    }

    #[test]
    fn a_task_without_a_model_says_what_to_do_about_it() {
        let d = tempfile::tempdir().unwrap();
        let mut s = session(d.path());
        let e = s.dispatch("rename helper to assist").expect_err("no model");
        let msg = e.to_string();
        assert!(msg.contains("no model set"), "{msg}");
        // The two spellings, because "set a model" is not actionable to
        // someone who has not read the --model docs.
        assert!(msg.contains("/model llamacpp:"), "{msg}");
        assert!(msg.contains("/model scripted:"), "{msg}");
        // And what still works without one, so the answer is not "come back
        // later".
        assert!(msg.contains("/radius"), "{msg}");
    }

    #[test]
    fn a_bad_model_spec_is_refused_and_does_not_become_the_model() {
        // Validated when set rather than at first use: a typo discovered on
        // the next task is discovered after the user has moved on.
        let d = tempfile::tempdir().unwrap();
        let mut s = session(d.path());
        let e = s.dispatch("/model gpt-4").expect_err("not a spec");
        assert!(e.to_string().contains("not a usable model spec"), "{e}");
        assert!(s.model.is_none(), "a refused spec must not be stored");
    }

    #[test]
    fn a_scripted_model_is_accepted_and_held_across_commands() {
        let d = tempfile::tempdir().unwrap();
        let script = d.path().join("s.json");
        std::fs::write(&script, r#"{"steps":[{"tool":"done","summary":"x"}]}"#).unwrap();

        let mut s = session(d.path());
        s.dispatch(&format!("/model scripted:{}", script.display()))
            .expect("a readable script is a valid spec");
        assert!(s.model.is_some());
        // Held — the point of a session is not re-typing it.
        s.dispatch("/status").expect("status");
        assert!(s.model.is_some(), "the model survives another command");
    }

    #[test]
    fn radius_without_a_symbol_asks_for_one_rather_than_guessing() {
        let d = tempfile::tempdir().unwrap();
        let mut s = session(d.path());
        let e = s.dispatch("/radius").expect_err("needs a symbol");
        assert!(e.to_string().contains("needs a symbol"), "{e}");
    }

    #[test]
    fn radius_answers_from_the_index_the_session_already_holds() {
        let d = tempfile::tempdir().unwrap();
        let mut s = session(d.path());
        s.dispatch("/radius helper").expect("helper is defined");
        s.dispatch("/radius nosuchthing")
            .expect_err("an unknown symbol is an error, not an empty radius");
    }

    #[test]
    fn accept_and_revert_refuse_when_there_is_nothing_pending() {
        // Both are destructive in opposite directions, and both are typed
        // from muscle memory. Refusing a no-op is how a user learns the tree
        // is already clean rather than assuming something was committed.
        let d = tempfile::tempdir().unwrap();
        let mut s = session(d.path());
        git_repo(d.path());

        assert!(
            s.dispatch("/accept")
                .unwrap_err()
                .to_string()
                .contains("nothing pending"),
        );
        assert!(
            s.dispatch("/revert")
                .unwrap_err()
                .to_string()
                .contains("nothing pending"),
        );
        s.dispatch("/diff")
            .expect("a clean tree is reportable, not an error");
    }

    #[test]
    fn revert_discards_a_pending_change_and_accept_commits_one() {
        let d = tempfile::tempdir().unwrap();
        let mut s = session(d.path());
        git_repo(d.path());
        let f = d.path().join("app/m.py");
        let original = std::fs::read_to_string(&f).unwrap();

        std::fs::write(&f, "def helper():\n    return 2\n").unwrap();
        s.dispatch("/revert").expect("discards");
        assert_eq!(
            std::fs::read_to_string(&f).unwrap(),
            original,
            "revert restores the file exactly"
        );

        std::fs::write(&f, "def helper():\n    return 3\n").unwrap();
        s.dispatch("/accept keeping this one").expect("commits");
        assert!(
            trellis_vcs::pending(d.path()).unwrap().is_empty(),
            "after accept the tree is clean, which is what unblocks the next task"
        );
        assert!(std::fs::read_to_string(&f).unwrap().contains("return 3"));
    }

    #[test]
    fn unconstrained_toggles_rather_than_latching() {
        let d = tempfile::tempdir().unwrap();
        let mut s = session(d.path());
        assert!(!s.unconstrained);
        s.dispatch("/unconstrained").unwrap();
        assert!(s.unconstrained, "tier 4 is opt-in and must be reachable");
        s.dispatch("/unconstrained").unwrap();
        assert!(!s.unconstrained, "and reversible without restarting");
    }

    #[test]
    fn a_command_and_its_argument_are_split_on_the_first_space_only() {
        // `/accept fix the thing` is one command and a four-word message, not
        // five tokens. Splitting on every space would silently truncate every
        // commit message to its first word.
        let d = tempfile::tempdir().unwrap();
        let mut s = session(d.path());
        git_repo(d.path());
        std::fs::write(d.path().join("app/m.py"), "def helper():\n    return 9\n").unwrap();

        s.dispatch("/accept a message with several words")
            .expect("commits");
        let log = std::process::Command::new("git")
            .args(["log", "-1", "--pretty=%s"])
            .current_dir(d.path())
            .output()
            .unwrap();
        assert_eq!(
            String::from_utf8_lossy(&log.stdout).trim(),
            "a message with several words"
        );
    }
}
