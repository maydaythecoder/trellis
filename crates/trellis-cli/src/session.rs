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
use std::path::{Path, PathBuf};
use trellis_index::Index;

pub(crate) struct Session {
    root: PathBuf,
    index: Index,
    /// Parsed once, on the way in. Holding the string meant every consumer
    /// re-parsed it and disagreed about what was valid.
    model: Option<trellis_model::ModelSpec>,
    unconstrained: bool,
    /// Tasks run this session, for `/status`.
    tasks_run: u32,
}

/// What a passing run claims about its callers, given what was measured.
///
/// S3 turns on this sentence. Two shapes, because the honest thing to say
/// depends on whether there is evidence:
///
/// - **With coverage data**, real counts, never added together. `ModeCounts`
///   has no `total()` for that reason.
/// - **Without it**, the zero has to be labelled. "0 verified" invites exactly
///   one wrong reading — that nothing is verified — when what it means is that
///   nothing was measured.
///
/// Held here rather than inline so the tests read the shipped string. A test
/// carrying its own copy would keep passing after someone edited the real one,
/// which is the failure this sentence exists to prevent, one level up.
fn audit_mode_note(counts: trellis_trail::ModeCounts) -> String {
    if counts.verified == 0 && counts.reviewed == 0 {
        return NO_COVERAGE_NOTE.to_string();
    }
    format!(
        concat!(
            "
callers:  {} verified by an executed test, {} reviewed only.
",
            "          Reviewed means no test covered it — a weak signal, and ",
            "deliberately
          not added to the first number."
        ),
        counts.verified, counts.reviewed
    )
}

const NO_COVERAGE_NOTE: &str = concat!(
    "
verified: 0 — not measured, which is not the same as nothing being ",
    "verified.
",
    "          Your checks ran and passed; a run cannot converge without ",
    "that. What
",
    "          is missing is coverage data: add --cov-report=json to the test ",
    "command
          in .trellis/config.toml and this becomes a real count."
);

impl Session {
    pub(crate) fn open(root: &Path, model: Option<String>) -> Result<Self> {
        let started = std::time::Instant::now();
        let index = trellis_index::build_cached(root);
        let elapsed = started.elapsed();

        // A repository may name its own model, so a session in a project with
        // a local runtime starts ready rather than making the user retype a
        // host and port they already wrote down. An explicit argument still
        // wins: it is the more specific statement of intent.
        let cfg = crate::config::RepoConfig::load(root)?;
        let model = match model.or_else(|| cfg.model()) {
            Some(spec) => Some(spec.parse().with_context(|| {
                format!("`{spec}` was given as the model but is not a usable spec")
            })?),
            None => None,
        };

        let s = Self {
            root: root.to_path_buf(),
            index,
            model,
            unconstrained: cfg.unconstrained(),
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
            Some(m) => {
                println!("  model    {m}");
                self.warn_if_unreachable(m);
            }
            None => println!("  model    none — /model <spec> before running a task"),
        }
        println!();
        println!("Type a task to run the loop. /help for commands, /exit to leave.");
        // Named here rather than only in /help. Someone who wants the
        // full-screen view looks for it on the way in, not in a command
        // list they have to know to ask for.
        println!("`trellis tui` opens the same run full-screen: transcript, edits, checks.");
        println!();
    }

    /// Reads until end of input or `/exit`.
    ///
    /// `rustyline` rather than `read_line`, because a prompt with no line
    /// editing is not merely inconvenient — an arrow key arrived as an escape
    /// sequence, fell past the command match, and ran an agent task. The
    /// control-character guard below stays as well: it costs nothing, and it
    /// is the thing that actually caught that defect.
    pub(crate) fn run(mut self) -> Result<()> {
        let mut rl = rustyline::DefaultEditor::new().context("starting the line editor")?;

        let history = history_path(&self.root);
        if let Some(h) = &history {
            if let Some(parent) = h.parent() {
                std::fs::create_dir_all(parent).ok();
            }
            rl.load_history(h).ok();
        }

        loop {
            let pending = trellis_vcs::pending(&self.root)
                .map(|p| p.len())
                .unwrap_or(0);
            let prompt = if pending > 0 {
                format!("[{pending} pending] > ")
            } else {
                "> ".to_string()
            };

            let line = match rl.readline(&prompt) {
                Ok(l) => l,
                // Ctrl-C cancels the line; it does not leave. Quitting on it
                // would turn a half-typed task into an exit, which is the
                // opposite of what every other prompt does.
                Err(rustyline::error::ReadlineError::Interrupted) => continue,
                // Ctrl-D, or the end of a piped script.
                Err(rustyline::error::ReadlineError::Eof) => break,
                Err(e) => return Err(e).context("reading input"),
            };

            let line = line.trim();
            if line.is_empty() {
                continue;
            }

            // Control characters are never a task.
            //
            // Kept after moving to a line editor: defence in depth now rather
            // than the only defence, but it is what caught the defect. Without
            // raw mode an arrow key arrived as ESC `[` `A`, which does not
            // begin with `/`, so it fell through to the model — the sequence
            // slugified to "a", a trail was written under that name, and files
            // were edited. Reflex input must never start work.
            if line.chars().any(char::is_control) {
                println!("{NO_LINE_EDITING}");
                println!();
                continue;
            }

            rl.add_history_entry(line).ok();

            // A command that fails must not end the session. The whole point
            // of staying resident is that a typo costs a line, not a restart.
            if let Err(e) = self.dispatch(line) {
                if e.to_string() == EXIT {
                    break;
                }
                println!("error: {e:#}");
            }
            println!();
        }

        // Best effort: losing history is a nuisance, not a failure, and a
        // read-only or absent `.trellis` must not turn leaving into an error.
        if let Some(h) = &history {
            rl.save_history(h).ok();
        }
        Ok(())
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
            "/doctor" => {
                let spec = self.model.as_ref().map(ToString::to_string);
                crate::cmd_doctor(&self.root, spec.as_deref())
            }
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
                // while the user is still thinking about it. Parsing is all
                // this does: it used to build an entire `Driver` — reading the
                // script file, constructing an HTTP client — purely to find
                // out whether a string was well formed.
                let spec: trellis_model::ModelSpec = rest.parse()?;
                // Parsing settles the syntax; this settles the environment.
                // Kept here rather than deferred to the first task, because a
                // typo found on the next task is found after the user has
                // moved on — and kept to one `stat` rather than building a
                // whole driver to discover it.
                if let trellis_model::ModelSpec::Scripted(path) = &spec
                    && !path.is_file()
                {
                    anyhow::bail!("{} is not a readable file", path.display());
                }
                println!("model    {spec}");
                self.probe_model(&spec);
                self.model = Some(spec);
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

    /// Says whether a runtime is actually there, and at which tier.
    ///
    /// Parsing a spec proves it is well formed, not that anything is
    /// listening. Without this the first task is where a user finds out, and
    /// by then they have typed a task and waited for it to fail — which is
    /// exactly what happens when a serving process is stopped underneath a
    /// session that is still holding its address.
    ///
    /// A warning, never a refusal. The runtime may not be started yet, and a
    /// tool that would not let you name a server before launching it is
    /// wrong about the order people work in.
    /// Says so, in the banner, when the configured runtime is not listening.
    ///
    /// The spec is kept either way — a runtime started a minute later will
    /// be used, and rewriting the user's configuration because a port was
    /// briefly closed would be worse than a warning.
    ///
    /// A connect rather than `probe_constraint`, which sends a real
    /// completion: this runs on every session start and must not make
    /// opening a prompt wait on a model's first token. Tier is still probed
    /// by `/model`, where the user asked for it.
    fn warn_if_unreachable(&self, spec: &trellis_model::ModelSpec) {
        let trellis_model::ModelSpec::Runtime {
            dialect,
            base,
            model,
        } = spec
        else {
            return;
        };
        let Ok(client) = trellis_model::RuntimeClient::new(base, *dialect, model.clone()) else {
            return;
        };
        let Ok(rt) = tokio::runtime::Runtime::new() else {
            return;
        };
        if let Err(e) = rt.block_on(client.reachable()) {
            println!("           NOT REACHABLE — {e}");
            println!(concat!(
                "           A task would be refused before anything is ",
                "snapshotted.
",
                "           Start the runtime, or /model <spec> to point ",
                "somewhere else."
            ));
        }
    }

    fn probe_model(&self, spec: &trellis_model::ModelSpec) {
        // A scripted model has no runtime to probe, and the type says so —
        // this used to re-split the string and return silently on three
        // different kinds of unparseable input, which meant a typo produced
        // no message at all.
        let trellis_model::ModelSpec::Runtime {
            dialect,
            base,
            model,
        } = spec
        else {
            return;
        };
        let Ok(client) = trellis_model::RuntimeClient::new(base, *dialect, model.clone())
            .map(|c| c.with_timeout(std::time::Duration::from_secs(5)))
        else {
            return;
        };

        let Ok(rt) = tokio::runtime::Runtime::new() else {
            return;
        };
        rt.block_on(async {
            match client.probe_constraint().await {
                Ok(true) => println!("tier     2 — the runtime honours guided decoding"),
                Ok(false) => println!(concat!(
                    "tier     4 — the runtime ignored the schema, so output is ",
                    "NOT constrained
",
                    "         runs can fail on format alone; /unconstrained ",
                    "makes that explicit"
                )),
                Err(e) => println!(
                    concat!(
                        "         not reachable yet: {}
",
                        "         the spec is kept — start the runtime and it will ",
                        "be used"
                    ),
                    e
                ),
            }
        });
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
        let Some(model) = self.model.as_ref().map(ToString::to_string) else {
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
        // The session stays open whichever way the run went: a run that did
        // not converge is a normal outcome here, not the end of the program.
        outcome.map(|_| ())
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

        // The actual change, not just its filename. "Acceptance is yours" is
        // not a real offer if taking it up means leaving for another terminal.
        match trellis_vcs::diff(&self.root) {
            Ok(d) if !d.trim().is_empty() => {
                println!();
                let lines: Vec<&str> = d.lines().collect();
                for line in lines.iter().take(DIFF_LINES) {
                    println!("{line}");
                }
                if lines.len() > DIFF_LINES {
                    println!(
                        "... {} more lines — `git diff` for the rest",
                        lines.len() - DIFF_LINES
                    );
                }
            }
            // An untracked file has no diff: git cannot show what it has
            // never seen. The listing above already named it.
            Ok(_) => {}
            Err(e) => println!(
                "
(could not read the diff: {e})"
            ),
        }

        println!();
        println!("/accept [message]  commit these    /revert  discard them");
        Ok(())
    }

    fn accept(&mut self, message: &str) -> Result<()> {
        self.accept_with(message, &mut Self::ask_stdin)
    }

    /// `/accept`, with the approval question supplied.
    ///
    /// Split from [`Self::accept`] so the flow can be tested without a
    /// terminal. The three paths S6 asks for are control flow, not prose, and
    /// a prompt that only exists behind a TTY is one no test ever sees.
    fn accept_with(&mut self, message: &str, ask: &mut dyn FnMut() -> Option<char>) -> Result<()> {
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

        // What the run claims about its callers is printed at every approval,
        // whether or not anything is unresolved. A clean coverage line is
        // exactly when someone is most likely to read "converged" as
        // "tested", so that is the moment the distinction matters most.
        //
        // It was nested inside the doubt block until a live run showed it
        // silent on a repository with nothing unresolved — while the comment
        // above it said "printed unconditionally".
        self.print_audit_mode(&paths);

        let doubt = trellis_index::doubt_in_files(&self.index, &paths);
        if doubt.unbound > 0 || doubt.unexamined() > 0 {
            self.print_coverage(&doubt);
            loop {
                match ask() {
                    Some('a') => break,
                    Some('i') => {
                        self.inspect_doubt(&doubt);
                        Self::print_choices();
                    }
                    Some('t') => {
                        self.what_to_test(&doubt);
                        Self::print_choices();
                    }
                    // Cancelling is the safe default for an unrecognised key,
                    // because the destructive direction here is committing
                    // something the user had not decided on.
                    _ => {
                        println!("cancelled; nothing committed, nothing discarded");
                        return Ok(());
                    }
                }
            }
        }

        let refs: Vec<&str> = paths.iter().map(String::as_str).collect();
        let base = if message.is_empty() {
            "trellis: accepted proposed changes"
        } else {
            message
        };
        // The gap goes into the commit, which is what "proceed with gaps
        // recorded" has to mean for it to survive: the trail records the run,
        // and this records what was unknown at the moment a human said yes.
        // `git log` is where someone asks that question later.
        let msg = if doubt.unbound == 0 && doubt.unexamined() == 0 {
            base.to_string()
        } else {
            format!(
                "{base}\n\nCoverage at approval: {}.\nUnresolved calls are edges the graph could \
                 not follow, not defects; see docs on Constraint 1.",
                doubt.one_line()
            )
        };
        trellis_vcs::commit_paths(&self.root, &refs, &msg)?;
        println!("committed {} path(s)", refs.len());
        for p in &refs {
            println!("  {p}");
        }
        if doubt.unbound > 0 {
            println!("gap recorded in the commit message.");
        }
        Ok(())
    }

    /// The coverage note. One line by default; detail only when asked.
    ///
    /// S6, and `04-v1.0-harness.md`'s rule for it: *"Design for the tenth
    /// partial-coverage run in an hour, not the first."* Partial coverage is
    /// routine — Constraint 1 guarantees it — so this never blocks: the gap
    /// attaches to the approval, not to the run. A tool that stops ten times
    /// an hour gets switched off, and then its honesty mechanism is off too.
    ///
    /// Scoped to the files being accepted, deliberately. Repository-wide doubt
    /// on the benchmark corpus is 11,233 sites, a figure whose only effect is
    /// to teach a reader to skip the line.
    fn print_coverage(&self, d: &trellis_index::FileDoubt) {
        println!();
        println!("coverage: {}", d.one_line());
        if d.unexamined() > 0 {
            // Not a rounding error. A changed file the index never parsed
            // contributes zero doubt, which would read as confidence about a
            // file nothing is known about.
            println!(
                "          {} of them {} not in the index, so nothing is known about {}.",
                d.unexamined(),
                if d.unexamined() == 1 { "is" } else { "are" },
                if d.unexamined() == 1 { "it" } else { "them" }
            );
        }
        if d.unbound > 0 {
            println!(
                "          {}",
                concat!(
                    "Any of them may call what you changed. The checks ran on ",
                    "what the graph could see."
                )
            );
        }
        Self::print_choices();
    }

    /// What a passing run does and does not claim.
    ///
    /// S3, and `04-v1.0-harness.md` asks for it here specifically: *"The first
    /// time something breaks after a clean run, this wording decides whether
    /// the user feels informed or lied to."* Approval is that moment — it is
    /// where someone decides to trust the work — so the limit is stated at it
    /// rather than only in a document they have not opened.
    ///
    /// Printed unconditionally, including when nothing is unresolved. A clean
    /// coverage line is exactly when someone is most likely to read
    /// "converged" as "tested", which is the reading this prevents.
    fn print_audit_mode(&self, paths: &[String]) {
        println!("{}", audit_mode_note(crate::audit_modes(&self.root, paths)));
    }

    fn print_choices() {
        println!();
        println!("  a  accept, with the gap recorded in the commit");
        println!("  i  inspect the unresolved sites");
        println!("  t  what to test to close them");
        println!("  anything else cancels");
    }

    fn ask_stdin() -> Option<char> {
        use std::io::Write;
        print!("accept> ");
        let _ = std::io::stdout().flush();
        let mut line = String::new();
        if std::io::stdin().read_line(&mut line).ok()? == 0 {
            // EOF: a piped or closed stdin must not be read as approval.
            return None;
        }
        Some(
            line.trim()
                .chars()
                .next()
                .unwrap_or('x')
                .to_ascii_lowercase(),
        )
    }

    /// The unresolved sites, with line numbers.
    ///
    /// Line numbers are computed here rather than stored, because the index
    /// does no I/O — and a byte offset is not something a user can act on.
    fn inspect_doubt(&self, d: &trellis_index::FileDoubt) {
        const SHOWN: usize = 20;
        println!();
        for s in d.sites.iter().take(SHOWN) {
            let line = std::fs::read_to_string(self.root.join(&s.path))
                .map(|src| src[..s.offset.min(src.len())].matches('\n').count() + 1)
                .unwrap_or(0);
            let where_ = match &s.enclosing {
                Some(e) => format!("in {e}"),
                None => "at module level".to_string(),
            };
            println!("  {}:{}  {}  {}", s.path, line, s.reason, where_);
        }
        if d.sites.len() > SHOWN {
            println!("  ... and {} more", d.sites.len() - SHOWN);
        }
        println!();
        println!(
            "{}",
            concat!(
                "These are calls the graph could not follow, not calls that are ",
                "wrong. Complete\nrecall is not available in Python, so this list ",
                "is what honesty costs rather\nthan a defect list."
            )
        );
    }

    /// What to test, named concretely.
    ///
    /// "Add a test" is not advice. The functions containing unresolved calls
    /// are, because a test that executes one turns an edge the graph cannot
    /// follow into behaviour a check can observe — the only kind of evidence
    /// this tool accepts.
    fn what_to_test(&self, d: &trellis_index::FileDoubt) {
        let mut by_fn: std::collections::BTreeMap<(&str, &str), u32> =
            std::collections::BTreeMap::new();
        for s in &d.sites {
            if let Some(e) = &s.enclosing {
                *by_fn.entry((s.path.as_str(), e.as_str())).or_insert(0) += 1;
            }
        }
        println!();
        if by_fn.is_empty() {
            println!(
                "{}",
                concat!(
                    "Every unresolved call here is at module level, so there is ",
                    "no function to\nexercise. Importing the module is the test."
                )
            );
            return;
        }
        let mut ranked: Vec<_> = by_fn.into_iter().collect();
        ranked.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

        println!("A test that executes these turns an unfollowable edge into an observed one:");
        for ((path, func), n) in ranked.iter().take(8) {
            println!("  {func:<32} {n:>3} unresolved call(s)   {path}");
        }
        println!();
        println!(
            "{}",
            concat!(
                "TRELLIS does not write it: a check it generated and then ran ",
                "would be the\nmodel grading its own work, which is the thing ",
                "this tool refuses."
            )
        );
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
/// Where this repository's prompt history lives — in user state, never in the
/// repository.
///
/// It went to `.trellis/history` first, and that was wrong in a way the
/// product is unusually sensitive to: the file is untracked, so it dirtied the
/// worktree, and a dirty worktree **blocks the next run**. A convenience
/// feature would have made the tool refuse to work. `.trellis/` cannot simply
/// be ignored either, because the trail inside it is deliberately committed.
///
/// So it goes where a shell keeps its history: user state, keyed by repository
/// so recalling a task in a different checkout is not noise. No dependency —
/// one env var per platform and a hash of the root path.
fn history_path(root: &Path) -> Option<PathBuf> {
    use std::hash::{Hash, Hasher};

    let base = if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| {
                std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local").join("share"))
            })
    }?;

    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    root.hash(&mut hasher);
    let key = hasher.finish();

    // The directory name is included for a human reading the folder; the hash
    // is what makes it unique, since two checkouts can share a basename.
    let name = root
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("repo")
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect::<String>();

    Some(
        base.join("trellis")
            .join(format!("{name}-{key:016x}.history")),
    )
}

/// How much of a diff `/diff` prints before deferring to `git diff`.
///
/// A wide change can run to thousands of lines, and a terminal that scrolls
/// the decision off the top has not helped anyone review it. The count of
/// what was withheld is printed, so the limit is visible rather than silent.
const DIFF_LINES: usize = 60;

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
            "
",
            "  trellis tui       the full-screen view: transcript, run state
",
            "                    and task entry at once. A separate command
",
            "                    rather than a slash, because it takes over
",
            "                    the terminal — leaving it returns here.
",
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
        crate::testutil::git_init(dir);
        crate::testutil::git_commit_all(dir, "init");
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
    fn history_never_lands_inside_the_repository() {
        // This is the property, not an implementation detail. History went to
        // `.trellis/history` first: untracked, so it dirtied the worktree, and
        // a dirty worktree blocks the next run. A convenience feature made the
        // tool refuse to work.
        //
        // `.trellis/` cannot just be ignored either — the trail inside it is
        // deliberately committed — so the file has to live somewhere else
        // entirely.
        let d = tempfile::tempdir().unwrap();
        let root = d.path().join("repo");
        std::fs::create_dir_all(&root).unwrap();

        let Some(h) = history_path(&root) else {
            // No HOME and no LOCALAPPDATA. Returning None is correct: no
            // history beats history in the repository.
            return;
        };
        assert!(
            !h.starts_with(&root),
            "history must not be written inside the repository: {}",
            h.display()
        );
    }

    #[test]
    fn history_is_stable_per_repository_and_distinct_between_them() {
        let d = tempfile::tempdir().unwrap();
        let a = d.path().join("alpha");
        let b = d.path().join("beta");
        // Same basename, different paths — two checkouts of one project is
        // the ordinary case, and they must not share a history file.
        let a2 = d.path().join("nested").join("alpha");
        for p in [&a, &b, &a2] {
            std::fs::create_dir_all(p).unwrap();
        }

        let (Some(ha), Some(hb), Some(ha2)) =
            (history_path(&a), history_path(&b), history_path(&a2))
        else {
            return;
        };

        assert_eq!(ha, history_path(&a).unwrap(), "stable across calls");
        assert_ne!(ha, hb, "different repositories, different history");
        assert_ne!(
            ha, ha2,
            "same basename at a different path must not share history"
        );
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
    fn an_unreachable_runtime_is_a_warning_and_the_spec_is_kept() {
        // A refusal here would be wrong about the order people work in: you
        // name the server, then start it. The session says it could not reach
        // it and holds the address anyway.
        let d = tempfile::tempdir().unwrap();
        let mut s = session(d.path());
        // Port 1 is reserved and refuses immediately, so this does not sit on
        // the probe timeout.
        s.dispatch("/model llamacpp:http://127.0.0.1:1")
            .expect("an unreachable runtime is not an error");
        assert_eq!(
            s.model.as_ref().map(ToString::to_string).as_deref(),
            Some("llamacpp:http://127.0.0.1:1"),
            "the spec is kept so the runtime can be started afterwards"
        );
    }

    #[test]
    fn a_scripted_model_is_not_probed_for_a_tier() {
        // There is no runtime to ask, and printing a tier for a script would
        // be inventing one. A scripted run has no adapter tier at all.
        let d = tempfile::tempdir().unwrap();
        let script = d.path().join("s.json");
        std::fs::write(&script, r#"{"steps":[{"tool":"done","summary":"x"}]}"#).unwrap();

        let mut s = session(d.path());
        s.dispatch(&format!("/model scripted:{}", script.display()))
            .expect("a readable script is a valid spec");
        assert!(s.model.is_some());
    }

    #[test]
    fn a_bad_model_spec_is_refused_and_does_not_become_the_model() {
        // Validated when set rather than at first use: a typo discovered on
        // the next task is discovered after the user has moved on.
        let d = tempfile::tempdir().unwrap();
        let mut s = session(d.path());
        let e = s.dispatch("/model gpt-4").expect_err("not a spec");
        assert!(
            e.to_string().contains("llamacpp"),
            "the refusal names the shapes that would work: {e}"
        );
        assert!(s.model.is_none(), "a refused spec must not be stored");

        // A script that does not exist is refused here too, not on the next
        // task: the same rationale, and the reason `/model` does one `stat`
        // rather than constructing a driver to find out.
        let e = s
            .dispatch("/model scripted:no-such-file.json")
            .expect_err("a missing script is not usable");
        assert!(e.to_string().contains("readable file"), "{e}");
        assert!(s.model.is_none());
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
    fn diff_shows_the_change_not_just_the_filename() {
        // "Acceptance is yours" is not a real offer if taking it up means
        // leaving for another terminal. A decision made on a list of
        // filenames is a guess with extra steps.
        let d = tempfile::tempdir().unwrap();
        let mut s = session(d.path());
        git_repo(d.path());
        std::fs::write(d.path().join("app/m.py"), "def helper():\n    return 42\n").unwrap();

        let raw = trellis_vcs::diff(d.path()).expect("a diff");
        assert!(raw.contains("-    return 1"), "the old line: {raw}");
        assert!(raw.contains("+    return 42"), "the new line: {raw}");
        // And the command itself runs rather than erroring on a real change.
        s.dispatch("/diff").expect("diff reports");
    }

    #[test]
    fn an_untracked_file_is_listed_even_though_it_has_no_diff() {
        // git cannot diff what it has never seen, so the listing is the only
        // thing that names a new file. Dropping it would make `/accept` commit
        // something the user was never shown.
        let d = tempfile::tempdir().unwrap();
        let mut s = session(d.path());
        git_repo(d.path());
        std::fs::write(d.path().join("app/brand_new.py"), "x = 1\n").unwrap();

        let pending = trellis_vcs::pending(d.path()).unwrap();
        assert!(
            pending.iter().any(|l| l.contains("brand_new.py")),
            "an untracked file must still be listed: {pending:?}"
        );
        assert!(
            trellis_vcs::diff(d.path()).unwrap().trim().is_empty(),
            "and it genuinely has no diff, which is why the listing matters"
        );
        s.dispatch("/diff").expect("reports without erroring");
    }

    #[test]
    fn a_staged_change_is_still_in_the_diff() {
        // The diff is against HEAD, not the index. A run's edits may already
        // be staged, and a diff that omitted them would understate what
        // accepting now would commit.
        let d = tempfile::tempdir().unwrap();
        let _s = session(d.path());
        git_repo(d.path());
        std::fs::write(d.path().join("app/m.py"), "def helper():\n    return 7\n").unwrap();
        std::process::Command::new("git")
            .args(["add", "-A"])
            .current_dir(d.path())
            .output()
            .expect("git add");

        let raw = trellis_vcs::diff(d.path()).unwrap();
        assert!(
            raw.contains("+    return 7"),
            "a staged change must still show: {raw}"
        );
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

    // ---- S6: degradation UX at the approval prompt ---------------------

    /// A repository with a call the resolver cannot bind, so the approval
    /// prompt has something real to report.
    fn repo_with_doubt() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("app")).unwrap();
        std::fs::write(
            d.path().join("app/util.py"),
            // `thing` has no derivable type, so `thing.run()` is
            // MethodDispatchUntyped — the dominant unresolved reason on the
            // benchmark corpus, and the one a user will actually meet.
            "def helper(thing):\n    return thing.run()\n\n\ndef other():\n    return 1\n",
        )
        .unwrap();
        init_git(d.path());
        d
    }

    fn init_git(root: &std::path::Path) {
        crate::testutil::git_init(root);
        crate::testutil::git_commit_all(root, "init");
    }

    #[test]
    fn the_approval_prompt_reports_doubt_in_the_files_being_accepted() {
        let d = repo_with_doubt();
        let mut s = Session::open(d.path(), None).unwrap();
        std::fs::write(
            d.path().join("app/util.py"),
            "def helper(thing):\n    return thing.run()\n\n\ndef other():\n    return 2\n",
        )
        .unwrap();

        // 'a' — proceed with the gap recorded.
        let mut answers = vec!['a'].into_iter();
        s.accept_with("test commit", &mut || answers.next())
            .unwrap();

        let log = std::process::Command::new("git")
            .args(["log", "-1", "--format=%B"])
            .current_dir(d.path())
            .output()
            .unwrap();
        let msg = String::from_utf8_lossy(&log.stdout);
        assert!(msg.contains("test commit"), "{msg}");
        assert!(
            msg.contains("Coverage at approval"),
            "proceeding with a gap has to record the gap, or 'recorded' is a \
             word rather than a behaviour: {msg}"
        );
        assert!(
            msg.contains("bound to nothing"),
            "and it must carry the figure, not just a mention: {msg}"
        );
    }

    #[test]
    fn the_approval_prompt_says_what_a_passing_run_does_not_claim() {
        // S3's wording test. `04-v1.0-harness.md` asks for this at the
        // approval prompt specifically, because that is the moment someone
        // decides to trust the work — and a zero that reads as "nothing is
        // verified" is worse than no number at all.
        let d = repo_with_doubt();
        let mut s = Session::open(d.path(), None).unwrap();
        std::fs::write(
            d.path().join("app/util.py"),
            "def helper(thing):
    return thing.run()


def other():
    return 9
",
        )
        .unwrap();

        let mut answers = vec!['a'].into_iter();
        s.accept_with("m", &mut || answers.next()).unwrap();
        // The text is printed, not returned, so this asserts the function
        // exists and runs in the accept path; the wording itself is asserted
        // below where it can be read directly.
        assert!(trellis_vcs::pending(d.path()).unwrap().is_empty());
    }

    #[test]
    fn the_audit_mode_note_distinguishes_unmeasured_from_unverified() {
        // "0 verified" invites exactly one wrong reading. Asserted against
        // the shipped constant, not a copy: a test holding its own version
        // would keep passing after someone edited the real one.
        let text = NO_COVERAGE_NOTE;
        assert!(text.contains("not measured"), "{text}");
        assert!(
            text.contains("not the same as nothing being verified"),
            "the zero must be labelled, or it reads as a result: {text}"
        );
        assert!(
            text.contains("checks ran and passed"),
            "and it must not undersell what the run DID establish: {text}"
        );
        for line in text.lines() {
            let body = line.trim_start();
            assert!(!body.contains("  "), "folded indentation: {body:?}");
        }
    }

    #[test]
    fn real_counts_replace_the_unmeasured_note_and_are_never_summed() {
        // The whole point of populating it. With evidence the note reports
        // two numbers; without, it labels the zero. `ModeCounts` has no
        // `total()`, and the rendered line must not invent one either.
        let with = audit_mode_note(trellis_trail::ModeCounts {
            verified: 2,
            reviewed: 1,
        });
        assert!(with.contains("2 verified"), "{with}");
        assert!(with.contains("1 reviewed"), "{with}");
        assert!(
            with.contains("weak signal"),
            "reviewed must stay labelled as weak, or the two read as equivalent: {with}"
        );
        assert!(
            !with.contains(" 3 "),
            "the two counts are never added: {with}"
        );
        assert!(!with.contains("not measured"), "{with}");

        let without = audit_mode_note(trellis_trail::ModeCounts::default());
        assert!(without.contains("not measured"), "{without}");
        assert!(
            without.contains("--cov-report=json"),
            "and it must say how to get a real number: {without}"
        );

        // Past the label column, as `Probe::report`'s guard does: `callers:`
        // is padded to align with `coverage:` on purpose, and the defect is a
        // run of spaces inside a sentence.
        for text in [with, without] {
            for line in text.lines() {
                let body = line.split_once(':').map_or(line, |(_, r)| r).trim_start();
                assert!(!body.contains("  "), "folded indentation: {body:?}");
            }
        }
    }

    #[test]
    fn an_unrecognised_key_cancels_rather_than_accepting() {
        // The destructive direction at this prompt is committing something
        // the user had not decided on, so anything but `a` is a cancel.
        let d = repo_with_doubt();
        let mut s = Session::open(d.path(), None).unwrap();
        std::fs::write(
            d.path().join("app/util.py"),
            "def helper(thing):\n    return thing.run()\n\n\ndef other():\n    return 3\n",
        )
        .unwrap();

        let mut answers = vec!['q'].into_iter();
        s.accept_with("", &mut || answers.next()).unwrap();

        assert!(
            !trellis_vcs::pending(d.path()).unwrap().is_empty(),
            "a cancel must leave the change pending — not committed, and not \
             discarded either"
        );
    }

    #[test]
    fn eof_on_the_prompt_is_not_approval() {
        // A piped or closed stdin returns None. Reading that as 'yes' would
        // make `trellis < /dev/null` commit unreviewed work.
        let d = repo_with_doubt();
        let mut s = Session::open(d.path(), None).unwrap();
        std::fs::write(
            d.path().join("app/util.py"),
            "def helper(thing):\n    return thing.run()\n\n\ndef other():\n    return 4\n",
        )
        .unwrap();

        s.accept_with("", &mut || None).unwrap();
        assert!(!trellis_vcs::pending(d.path()).unwrap().is_empty());
    }

    #[test]
    fn inspect_and_test_return_to_the_prompt_rather_than_deciding() {
        // `i` and `t` are detail-on-request, not answers. A user who asks to
        // see the sites has not yet said whether to accept them.
        let d = repo_with_doubt();
        let mut s = Session::open(d.path(), None).unwrap();
        std::fs::write(
            d.path().join("app/util.py"),
            "def helper(thing):\n    return thing.run()\n\n\ndef other():\n    return 5\n",
        )
        .unwrap();

        let mut answers = vec!['i', 't', 'a'].into_iter();
        let mut asked = 0;
        s.accept_with("", &mut || {
            asked += 1;
            answers.next()
        })
        .unwrap();

        assert_eq!(asked, 3, "i and t must come back and ask again");
        assert!(
            trellis_vcs::pending(d.path()).unwrap().is_empty(),
            "and the final 'a' still accepts"
        );
    }

    #[test]
    fn a_file_with_no_doubt_costs_the_user_no_keystroke() {
        // The tenth run of the hour. If a clean file still prompted, the
        // prompt would be noise and would be dismissed without reading —
        // taking the honest cases down with it.
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("app")).unwrap();
        std::fs::write(
            d.path().join("app/util.py"),
            "def helper():\n    return 1\n",
        )
        .unwrap();
        init_git(d.path());

        let mut s = Session::open(d.path(), None).unwrap();
        std::fs::write(
            d.path().join("app/util.py"),
            "def helper():\n    return 2\n",
        )
        .unwrap();

        let mut asked = false;
        s.accept_with("", &mut || {
            asked = true;
            Some('a')
        })
        .unwrap();

        assert!(!asked, "nothing to report means nothing to ask");
        assert!(trellis_vcs::pending(d.path()).unwrap().is_empty());
    }

    #[test]
    fn the_coverage_line_is_scoped_to_the_changed_files_not_the_repository() {
        // S6's severity rule: doubt on a symbol nobody is changing is noise.
        // Repository-wide it is 11,233 sites on the benchmark corpus, a number
        // whose only effect is to teach a reader to skip the line.
        let d = repo_with_doubt();
        std::fs::write(
            d.path().join("app/elsewhere.py"),
            "def far(x):\n    return x.also_unresolved()\n",
        )
        .unwrap();
        init_git(d.path());
        let s = Session::open(d.path(), None).unwrap();

        let whole = trellis_index::doubt_in_files(
            &s.index,
            &["app/util.py".to_string(), "app/elsewhere.py".to_string()],
        );
        let one = trellis_index::doubt_in_files(&s.index, &["app/util.py".to_string()]);
        assert!(whole.unbound > one.unbound, "the fixture must have both");
        assert_eq!(one.files_examined, 1);
        assert!(
            one.sites.iter().all(|x| x.path.ends_with("util.py")),
            "a site in a file the user is not accepting must not be reported \
             as if it were"
        );
    }

    #[test]
    fn a_changed_file_the_index_never_parsed_is_named_rather_than_counted_as_clean() {
        // Zero doubt in an unexamined file reads as confidence about a file
        // nothing is known about, which is the S2 failure inverted.
        let d = repo_with_doubt();
        let s = Session::open(d.path(), None).unwrap();
        let doubt = trellis_index::doubt_in_files(
            &s.index,
            &[
                "app/util.py".to_string(),
                "app/never_indexed.py".to_string(),
            ],
        );
        assert_eq!(doubt.files_named, 2);
        assert_eq!(doubt.files_examined, 1);
        assert_eq!(doubt.unexamined(), 1);
    }

    #[test]
    fn the_one_line_summary_names_reasons_and_never_becomes_a_wall() {
        let d = repo_with_doubt();
        let s = Session::open(d.path(), None).unwrap();
        let doubt = trellis_index::doubt_in_files(&s.index, &["app/util.py".to_string()]);
        let line = doubt.one_line();

        assert!(line.contains("MethodDispatchUntyped"), "{line}");
        assert!(line.contains("bound to nothing"), "{line}");
        assert!(!line.contains('\n'), "one line means one line: {line:?}");
        assert!(!line.contains("  "), "{line:?}");
        assert!(
            !line.contains('%'),
            "no ratio: the two populations differ and dividing them printed \
             3.7% for a resolver audited at 72%: {line}"
        );
    }
    #[test]
    fn the_history_filename_keeps_what_is_readable_and_drops_what_is_not() {
        // The filter is four conditions and every one of them survived the
        // mutation run. Each is a different way to get a filename that is
        // either unreadable or not a legal path component: drop the letters
        // and the name is punctuation, keep the spaces and quoting breaks,
        // keep the `!` and a shell surprises someone.
        let Some(p) = history_path(Path::new("/x/my repo!@#-name_1")) else {
            // No HOME and no LOCALAPPDATA, as the neighbouring tests allow.
            return;
        };
        let file = p.file_name().unwrap().to_str().unwrap();
        let stem = file.split('-').next().unwrap();

        assert!(
            file.starts_with("myrepo-name_1-"),
            "expected the readable characters kept in order and the rest \
             dropped, then the hash; got {file}"
        );
        for bad in [' ', '!', '@', '#'] {
            assert!(
                !stem.contains(bad),
                "`{bad}` survived into the filename: {file}"
            );
        }
        assert!(
            file.contains('-') && file.contains('_'),
            "`-` and `_` are legal and readable and must survive: {file}"
        );
    }

    #[test]
    fn audit_mode_note_picks_the_unmeasured_shape_only_when_nothing_was_measured() {
        // The neighbouring test asserts what `NO_COVERAGE_NOTE` *says*.
        // Nothing asserted *when it is chosen*, which is a different
        // question and the one the surviving mutant answers wrongly.
        //
        // The `&&` selecting between the two shapes survives as `||`, and as
        // `||` any run with one zero reports "nothing was measured" over
        // figures it actually has. Verified-without-reviewed is the *good*
        // outcome — every caller covered by an executed test — so the mutant
        // loses precisely the result worth reporting. S3 is the whole
        // distinction this product turns on.
        assert_eq!(
            audit_mode_note(trellis_trail::ModeCounts {
                verified: 0,
                reviewed: 0,
            }),
            NO_COVERAGE_NOTE,
            "an unmeasured run must say so rather than report zeroes"
        );

        for (v, r) in [(3, 0), (0, 4), (2, 5)] {
            let note = audit_mode_note(trellis_trail::ModeCounts {
                verified: v,
                reviewed: r,
            });
            assert_ne!(
                note, NO_COVERAGE_NOTE,
                "counts of {v}/{r} were reported as nothing measured"
            );
            assert!(
                note.contains(&format!("{v} verified")) && note.contains(&format!("{r} reviewed")),
                "both figures must appear, and never added: {note}"
            );
        }
    }
    /// The message of the commit `accept` just wrote.
    fn head_message(root: &Path) -> String {
        let out = std::process::Command::new("git")
            .args(["log", "-1", "--format=%B"])
            .current_dir(root)
            .output()
            .expect("git log");
        String::from_utf8_lossy(&out.stdout).to_string()
    }

    #[test]
    fn a_changed_file_the_index_never_parsed_still_asks() {
        // The other half of the doubt condition, and the one no test reached.
        // A file the index never saw contributes *zero* unresolved calls,
        // because nothing was resolved in it either — so a condition looking
        // only at `unbound` reads a total unknown as a clean bill of health.
        //
        // The session builds its index at open, so any file the run creates
        // afterwards is exactly this case, which makes it the ordinary one
        // rather than a corner.
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("app")).unwrap();
        std::fs::write(
            d.path().join("app/util.py"),
            "def helper():\n    return 1\n",
        )
        .unwrap();
        init_git(d.path());

        let mut s = Session::open(d.path(), None).unwrap();
        // Created after the index was built, so it is unexamined rather than
        // merely unresolved.
        std::fs::write(
            d.path().join("app/brand_new.py"),
            "def fresh():\n    return 2\n",
        )
        .unwrap();

        let doubt = trellis_index::doubt_in_files(&s.index, &["app/brand_new.py".to_string()]);
        assert_eq!(
            doubt.unbound, 0,
            "this test is only meaningful when the *unexamined* half is what triggers the prompt"
        );
        assert!(doubt.unexamined() > 0, "the file must be unexamined");

        let mut asked = false;
        s.accept_with("", &mut || {
            asked = true;
            Some('a')
        })
        .unwrap();
        assert!(
            asked,
            "a file nothing is known about was accepted without asking"
        );
    }

    #[test]
    fn a_commit_with_no_doubt_carries_no_coverage_paragraph() {
        // The gap goes into the commit so it survives — `git log` is where
        // someone asks later what was unknown when a human said yes. The
        // condition choosing between the two messages is two comparisons,
        // and inverting either puts a coverage paragraph on a commit that
        // had nothing to report. A caveat attached to everything is a
        // caveat nobody reads.
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("app")).unwrap();
        std::fs::write(
            d.path().join("app/util.py"),
            "def helper():\n    return 1\n",
        )
        .unwrap();
        init_git(d.path());

        let mut s = Session::open(d.path(), None).unwrap();
        std::fs::write(
            d.path().join("app/util.py"),
            "def helper():\n    return 2\n",
        )
        .unwrap();
        s.accept_with("a clean change", &mut || Some('a')).unwrap();

        let msg = head_message(d.path());
        assert!(msg.contains("a clean change"), "got:\n{msg}");
        assert!(
            !msg.contains("Coverage at approval"),
            "nothing was unknown, so nothing should be recorded as unknown:\n{msg}"
        );
    }

    #[test]
    fn a_commit_made_over_doubt_records_what_was_unknown() {
        // And the direction that matters. "Proceed with gaps recorded" only
        // means something if the gap is still there to read afterwards; a
        // trail beside the repository records the run, and this records what
        // a human accepted in spite of.
        let d = repo_with_doubt();
        let mut s = Session::open(d.path(), None).unwrap();
        std::fs::write(
            d.path().join("app/util.py"),
            "def helper(thing):\n    return thing.run()\n\n\ndef other():\n    return 12\n",
        )
        .unwrap();
        s.accept_with("a change over doubt", &mut || Some('a'))
            .unwrap();

        let msg = head_message(d.path());
        assert!(msg.contains("a change over doubt"), "got:\n{msg}");
        assert!(
            msg.contains("Coverage at approval"),
            "the gap has to reach `git log`, not only the terminal:\n{msg}"
        );
        assert!(
            msg.contains("not defects"),
            "and it must say what an unresolved call is, or the note reads \
             as a list of bugs someone shipped:\n{msg}"
        );
    }
}
