use crate::index_cmd::graph_lines;
use crate::plain;
use crate::{config, doctor_extra, hosted};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

/// What a constraint probe means for the run about to start.
///
/// Returns whether to rebuild the driver unconstrained, and the line to print.
/// Separated from `cmd_run` so the three outcomes are testable without a
/// runtime — which matters, because the failure this exists to prevent is a
/// confident sentence rather than a crash.
pub(crate) fn schema_report(probed: &Result<bool, String>) -> (bool, String) {
    match probed {
        Ok(true) => (false, "schema:   honoured by this runtime (probed)".into()),
        Ok(false) => (
            true,
            "schema:   sent and ignored by this runtime, so it is not enforced. Running\n\
             \x20         unconstrained; malformed calls are retried rather than prevented."
                .into(),
        ),
        // A probe that could not run leaves the tier assumed. Saying so is the
        // difference between an unverified claim and a false one — and it must
        // not silently change the tier, because a transient network error is
        // not evidence about the runtime's capabilities.
        Err(e) => (
            false,
            format!("schema:   enforcement not verified ({e}); the tier above is assumed"),
        ),
    }
}

/// The codebase graph for `root`, as lines, for a caller with no stdout.
///
/// Builds the index rather than reading the cache: the interface offers this
/// while a repository is being edited, and a cached graph would describe the
/// tree as it stood before the run the user has just watched.
pub(crate) fn codebase_lines(root: &Path, limit: usize) -> Vec<String> {
    let sources = trellis_index::read_sources(root);
    let index = trellis_index::build_from_sources(&sources);
    graph_lines(&trellis_index::module_graph(&index), limit)
}

/// Cuts a module path to the column, keeping the end.
///
/// The tail is what distinguishes `poetry.console.commands.add` from
/// `poetry.console.commands.remove`; the head is shared by hundreds of them.
pub(crate) fn truncate_module(m: &str, width: usize) -> String {
    let n = m.chars().count();
    if n <= width {
        return m.to_string();
    }
    let keep = width.saturating_sub(1);
    std::iter::once('…')
        .chain(m.chars().skip(n - keep))
        .collect()
}

pub(crate) fn cmd_doctor(root: &Path, model_spec: Option<&str>, from_user: bool) -> Result<()> {
    use trellis_model::{AdapterTier, BatchingCapability, Probe};

    let repo_cfg = config::RepoConfig::load(root)?;
    let cfg = repo_cfg.checks();

    // Unknown until measured. Reporting an optimistic guess is the failure
    // S17 and S18 both describe: the user cannot tell tier 2 from tier 4, or
    // a batching runtime from one that will serialize their agents.
    let mut model_id = "not configured".to_string();
    let mut runtime = "not configured".to_string();
    let mut tier: Option<AdapterTier> = None;
    let mut batching = BatchingCapability::Unknown;
    let mut notes: Vec<String> = Vec::new();

    if let Some(spec) = model_spec {
        let spec: trellis_model::ModelSpec = spec.parse()?;
        // Tier 1 is the protocol, not a probe finding (ADR 0024 §2).
        if let Some(h) = hosted::resolve(&spec, &repo_cfg, from_user)? {
            let (m, r, n) = hosted::doctor(&h);
            model_id = m;
            runtime = r;
            notes.extend(n);
            tier = Some(AdapterTier::Native);
            batching = BatchingCapability::ContinuousUnmeasured;
        } else {
            let trellis_model::ModelSpec::Runtime {
                dialect,
                base,
                model,
            } = &spec
            else {
                anyhow::bail!(
                    concat!(
                        "`{}` is a scripted model, which has no runtime to probe. ",
                        "Pass a serving runtime, e.g. llamacpp:http://127.0.0.1:8080."
                    ),
                    spec
                );
            };
            let client = trellis_model::RuntimeClient::new(base, *dialect, model.clone())?
                .with_timeout(std::time::Duration::from_secs(30));
            runtime = format!("{dialect:?} at {base}");
            // True of llama.cpp, which serves what it was started with and is
            // never told. Not true once the spec names a weight — saying it
            // anyway would report ignorance the harness does not have, on the
            // one line a reader checks to confirm they are about to drive what
            // they think they are.
            model_id = match model {
                Some(m) => format!("{m} (named in the spec)"),
                None => "whatever the runtime has loaded".into(),
            };

            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            rt.block_on(async {
                match client.probe_constraint().await {
                    Ok(true) => {
                        tier = Some(AdapterTier::RuntimeConstrained);
                        notes.push(
                            concat!(
                                "The runtime returned the key the probe schema demanded, so it ",
                                "appears to honour guided decoding. This is evidence, not proof: ",
                                "a capable model can produce conforming JSON unprompted."
                            )
                            .into(),
                        );
                    }
                    Ok(false) => {
                        tier = Some(AdapterTier::ValidateAndRetry);
                        notes.push(
                        concat!(
                            "The runtime ignored the schema, so output is NOT constrained. Runs ",
                            "will retry on malformed tool calls and can fail on format alone. ",
                            "Pass --unconstrained to `trellis run` to make that explicit. "
                        )
                        .into(),
                    );
                    }
                    Err(e) => notes.push(format!("Could not probe the runtime: {e}")),
                }

                match client.probe_slots().await {
                    Ok(Some(n)) if n > 1 => {
                        batching = BatchingCapability::Continuous {
                            measured_concurrency: n,
                        };
                    }
                    Ok(Some(_)) => {
                        batching = BatchingCapability::SingleStream;
                        notes.push(
                        concat!(
                            "llama.cpp reports a single slot. Multi-agent mode would serialize; ",
                            "start it with --parallel N for real concurrency (S18). "
                        )
                        .into(),
                    );
                    }
                    Ok(None) => {
                        // Continuous batching is inherent here, but the concurrency
                        // its scheduler will actually grant is not a number to
                        // invent, so `--agents auto` stays conservative.
                        batching = BatchingCapability::ContinuousUnmeasured;
                        notes.push(
                        concat!(
                            "This runtime batches continuously, so multi-agent mode is supported ",
                            "in principle. The concurrency it will grant is its scheduler's ",
                            "decision and is not reported, so `--agents auto` stays at 1 rather ",
                            "than guessing. "
                        )
                        .into(),
                    );
                    }
                    Err(e) => notes.push(format!("Could not read slot count: {e}")),
                }
            });
        }
    }

    let probe = Probe {
        model_id,
        runtime,
        tier,
        batching,
        typecheck: version_of(&cfg.typecheck),
        tests: version_of(&cfg.tests),
        coverage_available: coverage_available(&cfg.tests),
    };

    for line in probe.report() {
        println!("{line}");
    }
    println!("{}", floor_line(model_spec.is_some()));
    for line in repo_lines(root, &repo_cfg) {
        println!("{line}");
    }
    println!("root:       {}", root.display());

    if model_spec.is_none() {
        println!();
        println!(concat!(
            "No --model given, so tier and batching are reported as unknown ",
            "rather than guessed.
",
            "Pass e.g. `--model llamacpp:http://127.0.0.1:8080` to probe."
        ));
    }
    for n in &notes {
        println!();
        println!("{n}");
    }
    Ok(())
}

/// Scaffolds `.trellis/` and reports what is missing before it bites.
///
/// `04-v1.0-harness.md`, task item 7. Two jobs in one command, and the second
/// is the one that matters: writing three files is easy, and a user who
/// discovers on their first run that no type checker is installed has already
/// formed an opinion about the tool.
///
/// **Nothing here is overwritten.** These files live in the user's
/// repository, and a scaffolder that clobbers a config someone tuned is worse
/// than one that refuses to run at all.
pub(crate) fn cmd_init(root: &Path) -> Result<()> {
    if !root.join(".git").exists() {
        anyhow::bail!(concat!(
            "this is not a git repository, and TRELLIS rolls back with git.\n",
            "  git init && git add -A && git commit -m 'initial'\n",
            "then run `trellis init` again."
        ));
    }

    let dir = root.join(".trellis");
    let mut wrote = Vec::new();
    let mut kept = Vec::new();
    for (rel, body) in [
        ("config.toml", CONFIG_TEMPLATE),
        ("context.md", CONTEXT_TEMPLATE),
        ("governance/README.md", GOVERNANCE_TEMPLATE),
    ] {
        let path = dir.join(rel);
        if path.exists() {
            kept.push(rel);
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
        wrote.push(rel);
    }

    for f in &wrote {
        println!("created  .trellis/{f}");
    }
    for f in &kept {
        println!("kept     .trellis/{f} (already present, not overwritten)");
    }
    if wrote.is_empty() {
        println!("nothing to create; `.trellis/` is already set up.");
    }

    println!();
    println!("Prerequisites");
    let cfg = config::RepoConfig::load(root)?.checks();
    let mut blocking = 0;

    match version_of(&cfg.typecheck) {
        Some(v) => println!("  typecheck  {v}"),
        None => {
            blocking += 1;
            println!("  typecheck  NOT FOUND: {}", cfg.typecheck.join(" "));
            println!("             No typecheck verdict can be produced, so no run can");
            println!("             converge. Install it, or set [checks] typecheck in");
            println!("             .trellis/config.toml to the command you already use.");
        }
    }
    match version_of(&cfg.tests) {
        Some(v) => println!("  tests      {v}"),
        None => {
            blocking += 1;
            println!("  tests      NOT FOUND: {}", cfg.tests.join(" "));
            println!("             The grounding contract has nothing to ground on. A");
            println!("             repository without a test suite is a stated v1.0");
            println!("             non-goal, not an oversight.");
        }
    }

    match trellis_vcs::pending(root) {
        Ok(p) if p.is_empty() => println!("  worktree   clean"),
        Ok(p) => {
            // Not counted as blocking: `init` itself just wrote files, so a
            // dirty tree here is usually this command's own doing and saying
            // "blocked" would be alarming and wrong.
            println!("  worktree   {} uncommitted change(s)", p.len());
            println!("             A run needs a clean tree so rollback is exact. Commit");
            println!("             what you want to keep — including .trellis/ — first.");
        }
        Err(e) => println!("  worktree   cannot be read: {e}"),
    }

    println!();
    if blocking > 0 {
        println!("{blocking} prerequisite(s) missing. `trellis doctor` re-checks at any time.");
    } else {
        println!("Ready. `trellis doctor` re-checks at any time, and `trellis` opens a session.");
    }
    Ok(())
}

/// The scaffolded config.
///
/// Every value is commented out and shows the default it would replace, so
/// the file documents what TRELLIS will do rather than changing it. A
/// scaffold that writes active settings makes the tool's behaviour depend on
/// a file the user never read.
const CONFIG_TEMPLATE: &str = r#"# TRELLIS configuration. Every setting below is commented out and shows the
# default it would override, so this file starts by changing nothing.
#
# This file belongs in version control. `.trellis/index.bin` does not, and
# TRELLIS writes a .gitignore next to it saying so.

# [checks]
# What TRELLIS runs to decide whether a change is correct. These are the only
# source of a passing verdict: the model never issues one.
#
# `--no-incremental` on mypy is a correctness flag, not a preference. mypy
# keys its cache on file size and mtime, and a rollback restores a previous
# version of the same byte length — so without it mypy can report a cached
# result for code it never read.
# typecheck = ["mypy", "--no-incremental"]
# tests     = ["pytest", "-q"]
# python    = "python"

# [budget]
# A run stops when any of these is reached, and rolls back.
# max_iterations   = 40
# max_wall_clock_s = 1200
# max_tokens       = 2000000
# regime           = "local"   # or "metered", when tokens cost money

# [model]
# Saves retyping a runtime address every session. Deliberately not a place for
# a key: this is a host and port, or a path to a script. Your API key never
# goes in a file TRELLIS reads — configure the runtime with it directly.
# spec          = "llamacpp:http://127.0.0.1:8080"
# unconstrained = false

# [governance]
# Paths no run may edit, enforced at the orchestrator's write path rather than
# asked of the model. A rule that lives only in a prompt is a suggestion.
# deny_write = ["vendor/", "*.lock", "migrations/"]

# [context]
# A note vault in Apex format (ADR 0017). One focus note, its parent chain,
# its children's titles and the notes it links, compiled in memory when a run
# starts and shown to the model. It informs; it does not bind. Switch note
# per run with `trellis run --focus <stem>` rather than by editing this file,
# which would dirty the worktree a run needs clean.
# vault = "notes"
# focus = "some-note"
"#;

/// The starter context file.
///
/// Written as questions rather than as a template to fill in. An empty
/// headed outline gets committed empty; a question either gets answered or
/// gets deleted, and both are better than a heading with nothing under it.
const CONTEXT_TEMPLATE: &str = r#"# Project context

Notes for whatever is driving the loop. This file is read verbatim and put in
front of the model at the start of every run, so it is worth keeping short:
everything here costs context window on every single turn.

It **informs**; it does not bind. Nothing written here can relax the grounding
contract — a note saying "the tests are slow, just declare success" changes
nothing, because convergence reads an executed check. Rules that must bind go
in `[governance] deny_write` or `.trellis/governance/`, which the orchestrator
enforces at the write path.

Worth answering, if the answer is not obvious from the code:

- What is this project for, in two sentences?
- Which directories are the important ones, and which are generated?
- Any convention a newcomer gets wrong on their first change?
- Anything that looks like a bug and is deliberate?

Delete the questions as you answer them, or delete the file: an empty context
file is treated as no context rather than as an instruction to say nothing.
"#;

/// What governance rules are, next to where they go.
///
/// A `README.md` rather than an example `.toml`, deliberately: `governance/`
/// loads every `*.toml` in it and a malformed rule is a hard error, so an
/// example file shipped here would either load as a real rule or break the
/// repository on first run.
const GOVERNANCE_TEMPLATE: &str = r#"# Governance rules

One `.toml` file per rule in this directory. Every `*.toml` here is loaded,
and a malformed one is an error rather than a skip — a rule that quietly
failed to load would leave you believing it was in force.

That is also why this file is Markdown: an example rule saved as `.toml` would
be loaded as a real one.

A rule looks like this:

    name = "No direct DB access from handlers"
    rank = "non-negotiable"          # or "nice-to-have"
    description = "Handlers go through the repository layer."
    forbid = { pattern = "psycopg2.connect" }
    scope = ["app/handlers/"]        # omit for the whole repository

Two mechanisms, with different consequences:

- `forbid` fails an audit after the fact. A `non-negotiable` violation stops
  the run from converging and the tree is rolled back; a `nice-to-have` is
  reported and does not.
- `deny-write = { paths = [...] }` refuses an edit before it reaches disk.
  Enforced at the orchestrator's only write path, so it binds whatever the
  model intended.

Matching is substring only, and that is deliberate. A governance check that
exercised judgment would be a model deciding whether it had followed the
rules, which is the thing this tool exists not to do.
"#;

/// Runs `<program> --version` and returns the first line, trimmed.
///
/// A version rather than a yes/no. "found" cannot tell the checker the user
/// configured from a different one earlier on `PATH`, and a verdict is only
/// as good as the process that produced it.
fn version_of(cmd: &[String]) -> Option<String> {
    let (program, pre) = cmd.split_first()?;
    // The configured command may be `python -m mypy`, where `--version` has
    // to follow the module, not the interpreter.
    let mut c = std::process::Command::new(program);
    c.args(pre).arg("--version");
    trellis_core::secrets::scrub(&mut c);
    let out = c.output().ok()?;
    if !out.status.success() {
        return None;
    }
    let text = if out.stdout.is_empty() {
        String::from_utf8_lossy(&out.stderr).into_owned()
    } else {
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let line = text.lines().next()?.trim();
    (!line.is_empty()).then(|| line.to_string())
}

/// Whether the test runner has a coverage plugin available.
///
/// Reported because `Verified` versus `Reviewed` (S3) is the distinction the
/// product turns on, and with no coverage data nothing can populate
/// `Verified` — a user reading `0 verified` should know whether that is a
/// result or a missing tool.
fn coverage_available(tests: &[String]) -> bool {
    let Some((program, pre)) = tests.split_first() else {
        return false;
    };
    let mut c = std::process::Command::new(program);
    c.args(pre).arg("--help");
    trellis_core::secrets::scrub(&mut c);
    c.output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| {
            let text = String::from_utf8_lossy(&o.stdout).to_lowercase();
            text.contains("--cov")
        })
        .unwrap_or(false)
}

/// The repository-side half of `doctor`: index, worktree, budget, floor.
///
/// `04-v1.0-harness.md` asks for these four alongside the model probe, and
/// every line that can be wrong says what to do about it.
fn repo_lines(root: &Path, cfg: &config::RepoConfig) -> Vec<String> {
    let mut out = Vec::new();

    // Index. Deliberately NOT the milestone document's "91.3% resolved".
    //
    // CLAUDE.md: what the resolver bound is a lower bound on recall, not
    // recall, and it must never be compared to the thresholds. Printing it
    // bare next to a tick would make `doctor` the place that claim finally
    // escapes — the same failure ADR 0014 fixed in `radius`, which had
    // printed "3.7% resolved" for a resolver audited at 72%.
    let started = std::time::Instant::now();
    let index = trellis_index::build_cached(root);
    let built = started.elapsed();
    let c = &index.coverage;
    if index.files.is_empty() {
        out.push(
            concat!(
                "index:      EMPTY — no Python sources found under this root.\n",
                "            Check --root, or whether the tree is one `discover` skips ",
                "(.venv,\n            site-packages, build, dist, __pycache__)."
            )
            .to_string(),
        );
    } else {
        out.push(format!(
            "index:      {} definitions across {} files · {} of {} first-party call sites \
             bound\n            ({:.1}% — a lower bound on recall, NOT recall; only the \
             hand audit is\n            comparable to the gate. See CLAUDE.md) · {} ms",
            index.symbols.len(),
            index.files.len(),
            c.resolved,
            c.total(),
            c.resolved_fraction().unwrap_or(0.0) * 100.0,
            built.as_millis()
        ));
    }

    // Worktree. The first wall users hit, so it says how to get past it.
    match trellis_vcs::pending(root) {
        Ok(p) if p.is_empty() => out.push("worktree:   clean".to_string()),
        Ok(p) => out.push(format!(
            "worktree:   {} uncommitted change(s) — `trellis run` will refuse to start.\n\
             {:12}Commit them, stash them, or `/revert` in a session. A run needs a clean\n\
             {:12}tree so rollback is exact (ADR 0009).",
            p.len(),
            "",
            ""
        )),
        Err(e) => out.push(format!(
            "worktree:   cannot read git status: {e}\n\
             {:12}TRELLIS needs a git repository; `git init` if this is not one.",
            ""
        )),
    }

    // Skills (ADR 0019): what was found, what a run would list and leave out,
    // and every directory skipped with the reason. A skill that is silently
    // not loaded is one its author believes the model can read.
    let skills = trellis_loop::SkillCatalog::discover(root);
    let found = skills.valid.len() + skills.invalid.len();
    if found == 0 {
        out.push(format!(
            "skills:     none under {}",
            trellis_loop::SKILLS_DIR
        ));
    } else {
        let d = skills.disclosed();
        out.push(format!(
            "skills:     {found} found · {} listed · {} omitted (cap {} B) · {} invalid",
            d.listed.len(),
            d.omitted,
            trellis_loop::DISCLOSURE_CAP,
            skills.invalid.len()
        ));
        for i in &skills.invalid {
            out.push(format!(
                "            skipped {}: {}",
                plain(&i.dir),
                plain(&i.reason)
            ));
        }
    }

    // The control dial (ADR 0018). Said here because approve mode refuses to
    // start without a terminal, and that is better learned from `doctor`
    // than from a CI job.
    out.push(match cfg.control(None) {
        Ok(c) if c.mode == config::ControlMode::Approve => format!(
            "control:    {} — a run asks before each; needs a terminal, or `--auto`",
            c.describe()
        ),
        Ok(c) => format!("control:    {}", c.describe()),
        Err(e) => format!("control:    INVALID — {e}"),
    });

    // Budget, in the units it is actually enforced in.
    let b = cfg.budget();
    out.push(format!(
        "budget:     {:?} — {} iterations / {} s / {} tokens",
        b.regime,
        b.max_iterations,
        b.max_wall_clock.as_secs(),
        b.max_tokens
    ));

    // Integrated (ADR 0021 wave I): the user config, context files, user
    // skills, trust, profiles and routines, and the vault.
    out.extend(doctor_extra::lines(root, cfg));

    out
}

/// The model floor line.
///
/// The milestone document's mockup shows `Floor  above measured minimum ✓`.
/// **TRELLIS cannot know that**, and saying it would be the exact failure S7
/// exists to prevent. A floor is an interval located by a configuration that
/// *passes*; two have been measured and both are below it, so there is no
/// minimum to be above. The honest line says so and points at the evidence.
fn floor_line(model_configured: bool) -> String {
    if model_configured {
        concat!(
            "floor:      unknown for this model. The S7 floor is not located: two ",
            "configurations\n            have been measured (0.5B, Coder-3B) and both ",
            "failed every task, so\n            there is no measured minimum to be above. ",
            "See BENCHMARK_AND_EVAL.md §5."
        )
        .to_string()
    } else {
        concat!(
            "floor:      not applicable without --model. The S7 floor is not located ",
            "either:\n            two configurations measured, both below it. See ",
            "BENCHMARK_AND_EVAL.md §5."
        )
        .to_string()
    }
}

pub(crate) fn cmd_check(root: &Path, paths: Vec<PathBuf>) -> Result<()> {
    // `for_run`: two checks, one walk of `.trellis/` between them.
    let cfg = config::RepoConfig::load(root)?.checks().for_run(root);
    // An empty path list would invoke the checker with no arguments, which
    // most tools treat as an error rather than as "everything". Make the
    // documented default real.
    let paths = if paths.is_empty() {
        vec![PathBuf::from(".")]
    } else {
        paths
    };
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;

    let verdicts = rt.block_on(async {
        let tc = trellis_checks::typecheck(root, &cfg, &paths).await;
        let tests = trellis_checks::run_tests(root, &cfg, &paths).await;
        [tc, tests]
    });

    let mut all_passed = true;
    for v in &verdicts {
        let d = v.detail();
        println!(
            "{:?}: {} ({} ms)",
            d.kind,
            if v.passed() { "passed" } else { "FAILED" },
            d.duration_ms
        );
        println!("  command: {}", d.command);
        if !v.passed() {
            all_passed = false;
            let tail: String = d
                .stdout
                .lines()
                .rev()
                .take(20)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("\n");
            if !tail.trim().is_empty() {
                println!("{tail}");
            }
            if !d.stderr.trim().is_empty() {
                println!("{}", d.stderr.trim());
            }
        }
    }

    if !all_passed {
        std::process::exit(1);
    }
    Ok(())
}

#[cfg(test)]
mod doctor_tests {
    use super::*;

    fn repo(name: &str) -> PathBuf {
        let dir = crate::testutil::scratch(&format!("doctor-{name}"));
        std::fs::create_dir_all(dir.join("app")).unwrap();
        crate::testutil::git_init(&dir);
        std::fs::write(
            dir.join("app/util.py"),
            "def helper():
    return 1
",
        )
        .unwrap();
        crate::testutil::git_commit_all(&dir, "init");
        dir
    }

    /// The milestone mockup prints `Index  48,102 symbols · 91.3% resolved ✓`.
    /// CLAUDE.md forbids exactly that: what the resolver bound is a lower
    /// bound on recall, and comparing it to the gate thresholds is the error
    /// ADR 0014 already fixed once in `radius`, which printed "3.7% resolved"
    /// for a resolver audited at 72%.
    #[test]
    fn the_index_line_refuses_to_present_the_bound_as_recall() {
        let dir = repo("index-line");
        let cfg = config::RepoConfig::load(&dir).unwrap();
        let lines = repo_lines(&dir, &cfg).join(
            "
",
        );

        assert!(lines.contains("index:"), "{lines}");
        assert!(
            lines.contains("NOT recall"),
            "the caveat is the point of the line: {lines}"
        );
        assert!(
            lines.contains("hand audit"),
            "and it must name what IS comparable: {lines}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// "Every line that can be wrong should say what to do about it."
    #[test]
    fn a_dirty_worktree_says_how_to_get_past_it() {
        let dir = repo("dirty");
        std::fs::write(
            dir.join("app/util.py"),
            "def helper():
    return 2
",
        )
        .unwrap();
        let cfg = config::RepoConfig::load(&dir).unwrap();
        let lines = repo_lines(&dir, &cfg).join(
            "
",
        );

        assert!(lines.contains("uncommitted change"), "{lines}");
        assert!(
            lines.contains("Commit them") || lines.contains("stash"),
            "the first wall users hit must name the way through: {lines}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_clean_worktree_says_so_plainly() {
        let dir = repo("clean");
        let cfg = config::RepoConfig::load(&dir).unwrap();
        let lines = repo_lines(&dir, &cfg).join(
            "
",
        );
        assert!(lines.contains("worktree:   clean"), "{lines}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The mockup's `Floor  above measured minimum ✓` is a claim TRELLIS
    /// cannot make. A floor is located by a configuration that *passes*; two
    /// have been measured and both failed every task, so there is no minimum
    /// to be above. Printing the tick would be the exact failure S7 exists to
    /// prevent.
    #[test]
    fn the_floor_line_never_claims_a_minimum_that_was_never_located() {
        for configured in [true, false] {
            let line = floor_line(configured);
            assert!(
                line.contains("not located"),
                "the floor is an interval and neither endpoint passes: {line}"
            );
            assert!(
                !line.contains("above measured minimum"),
                "TRELLIS cannot know this and must not print it: {line}"
            );
            assert!(
                line.contains("BENCHMARK_AND_EVAL"),
                "and it must point at the evidence: {line}"
            );
        }
    }

    #[test]
    fn an_empty_index_says_what_to_check_rather_than_reporting_zero() {
        // Zero definitions reads as a repository with no code. The likelier
        // cause is a wrong --root or a tree `discover` skips, and the line
        // says so instead of leaving a user staring at 0.
        let dir = std::env::temp_dir().join(format!("trellis-doctor-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        crate::testutil::git_ok(&dir, &["init", "-q"]);
        let cfg = config::RepoConfig::load(&dir).unwrap();
        let lines = repo_lines(&dir, &cfg).join(
            "
",
        );

        assert!(lines.contains("index:      EMPTY"), "{lines}");
        assert!(lines.contains("--root"), "{lines}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod init_tests {
    use super::*;
    use crate::index_cmd::MAX_EDGES_DRAWN;
    use crate::{action_from, summarise};

    fn bare(name: &str) -> PathBuf {
        crate::testutil::scratch(&format!("init-{name}"))
    }

    fn repo(name: &str) -> PathBuf {
        let d = bare(name);
        crate::testutil::git_init(&d);
        d
    }

    #[test]
    fn init_refuses_outside_a_repository_and_says_how_to_make_one() {
        // The command for getting started must work before you have started,
        // and must name the missing step rather than the missing root.
        let d = bare("no-git");
        let e = cmd_init(&d).unwrap_err().to_string();
        assert!(e.contains("not a git repository"), "{e}");
        assert!(e.contains("git init"), "it must name the fix: {e}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn init_scaffolds_and_never_overwrites() {
        let d = repo("scaffold");
        cmd_init(&d).unwrap();
        assert!(d.join(".trellis/config.toml").exists());
        assert!(d.join(".trellis/context.md").exists());
        assert!(d.join(".trellis/governance/README.md").exists());

        // These files live in the user's repository. A scaffolder that
        // clobbers a config someone tuned is worse than one that refuses.
        std::fs::write(
            d.join(".trellis/config.toml"),
            "# mine
",
        )
        .unwrap();
        cmd_init(&d).unwrap();
        assert_eq!(
            std::fs::read_to_string(d.join(".trellis/config.toml")).unwrap(),
            "# mine
"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_scaffolded_config_parses_and_changes_nothing() {
        // Every value is commented out on purpose: a scaffold that writes
        // active settings makes the tool's behaviour depend on a file the
        // user has not read.
        let d = repo("config-inert");
        cmd_init(&d).unwrap();
        let cfg = config::RepoConfig::load(&d).expect("the scaffold must parse");
        assert_eq!(
            cfg.checks().typecheck,
            trellis_checks::CheckConfig::default().typecheck,
            "the scaffold must not change what runs"
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_governance_example_is_markdown_so_it_is_never_loaded_as_a_rule() {
        // `governance/` loads every *.toml and a malformed one is a hard
        // error, so an example rule shipped as TOML would either take effect
        // or break the repository on first run.
        let d = repo("governance-example");
        cmd_init(&d).unwrap();
        let rules = trellis_checks::governance::load(&d).expect("the scaffold must not break it");
        assert!(rules.is_empty(), "a scaffold must not install live rules");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A turn, for building action-log cases without a model.
    fn turn(call: trellis_loop::ToolCall, observation: &str) -> trellis_loop::Turn {
        trellis_loop::Turn {
            call,
            observation: observation.to_string(),
            verdict_passed: None,
            check_kind: None,
            prompt_bytes: Some(1_000),
            radius_counts: None,
            radius_summarised: None,
            human: None,
            may_have_changed_tree: false,
            changed: Vec::new(),
        }
    }

    #[test]
    fn an_escape_sequence_from_a_trail_cannot_repaint_the_terminal() {
        // A trail is untrusted on read-back (S10) and `trellis log` prints it
        // to a terminal, which interprets what it is given. A run that did not
        // converge could otherwise be made to look like one that did, in the
        // command a user reads to find out which.
        let forged = "fix the parser\x1b[2K\x1b[Aconverged, 2 checks passed";
        let out = plain(forged);
        assert!(!out.contains('\x1b'), "{out:?}");
        assert!(
            out.contains("fix the parser"),
            "the real text survives: {out:?}"
        );
        assert!(
            out.contains("\\u{1b}") || out.contains("\\x1b") || out.contains("\\e"),
            "and the escape is shown rather than dropped: {out:?}"
        );
    }

    #[test]
    fn ordinary_text_passes_through_a_sanitiser_unchanged() {
        // Including the em dashes and arrows this project's own messages use.
        for s in [
            "the describe function is annotated to return int",
            "app/calc.py",
            "2 rule(s) checked over 1 file(s); none broken — nothing to report",
        ] {
            assert_eq!(plain(s), s);
        }
    }

    #[test]
    fn a_hub_module_does_not_draw_an_unbounded_number_of_edges() {
        use trellis_index::{ModuleGraph, ModuleNode};
        // `--limit` caps modules and used to cap nothing else, so one module
        // every other one imports could emit more rows than the whole rest of
        // the listing — including into the interface's transcript.
        let g = ModuleGraph {
            nodes: vec![ModuleNode {
                module: "app.cli".into(),
                path: "app/cli.py".into(),
                defines: 1,
                calls: (0..40).map(|i| (format!("app.m{i}"), 40 - i)).collect(),
                ambiguous: 0,
                unresolved: 0,
            }],
        };
        let lines = graph_lines(&g, 40);
        let edges = lines.iter().filter(|l| l.contains("─▶")).count();
        assert_eq!(edges, MAX_EDGES_DRAWN, "{lines:#?}");
        assert!(
            lines.iter().any(|l| l.contains("30 more module(s) called")),
            "and the remainder is stated, not dropped: {lines:#?}"
        );
        // The heaviest edges are the ones kept — they are sorted by count.
        assert!(lines.iter().any(|l| l.contains("app.m0")), "{lines:#?}");
        assert!(!lines.iter().any(|l| l.contains("app.m39")), "{lines:#?}");
    }

    #[test]
    fn an_ignored_schema_downgrades_the_tier_rather_than_the_sentence() {
        // The defect: every run against ollama printed "tier 2, schema
        // enforced by the runtime" for a runtime that accepts `guided_json`
        // and ignores it. Rewording would have swapped a false claim for a
        // vaguer one; the tier itself has to change.
        let (unconstrain, note) = schema_report(&Ok(false));
        assert!(
            unconstrain,
            "the run must not claim a tier it does not have"
        );
        assert!(note.contains("ignored"), "{note}");
        assert!(
            note.contains("retried rather than prevented"),
            "and say what that costs: {note}"
        );
    }

    #[test]
    fn an_honoured_schema_keeps_the_tier_and_says_it_was_probed() {
        let (unconstrain, note) = schema_report(&Ok(true));
        assert!(!unconstrain);
        assert!(
            note.contains("probed"),
            "the claim names its evidence: {note}"
        );
    }

    #[test]
    fn a_probe_that_could_not_run_says_the_tier_is_assumed() {
        // A transient network error is not evidence about the runtime's
        // capabilities, so it must not silently change the tier — but it must
        // also not leave the banner asserting something nothing checked.
        let (unconstrain, note) = schema_report(&Err("connection reset".into()));
        assert!(
            !unconstrain,
            "a failed probe is not a measurement of an ignored schema"
        );
        assert!(note.contains("not verified"), "{note}");
        assert!(note.contains("connection reset"), "with the reason: {note}");
        assert!(note.contains("assumed"), "{note}");
    }

    #[test]
    fn an_applied_edit_is_the_only_action_that_records_a_write() {
        use trellis_loop::ToolCall;
        // `proposed` names the files a run ended up touching, not which action
        // did the touching. A run whose every edit was refused has an empty
        // `proposed` and a full list of actions.
        let refused = action_from(
            0,
            &turn(
                ToolCall::Edit {
                    path: "app/calc.py".into(),
                    old: "a".into(),
                    new: "b".into(),
                },
                "error: edit anchor not found in app/calc.py",
            ),
        );
        assert_eq!(refused.turn, 1, "turns are 1-based, as a run prints them");
        assert!(!refused.wrote, "a refused edit wrote nothing");

        let applied = action_from(
            1,
            &turn(
                ToolCall::Edit {
                    path: "app/calc.py".into(),
                    old: "a".into(),
                    new: "b".into(),
                },
                "edited app/calc.py (applied: true, bytes 59..87)",
            ),
        );
        assert!(applied.wrote, "an applied edit did");
        assert_eq!(applied.target, "app/calc.py");
    }

    #[test]
    fn reading_a_file_never_counts_as_a_write() {
        use trellis_loop::ToolCall;
        // The `applied: true` test is a substring match, so a read whose
        // content happened to contain that phrase must still not count.
        let a = action_from(
            0,
            &turn(
                ToolCall::ReadFile {
                    path: "app/calc.py".into(),
                },
                "applied: true",
            ),
        );
        assert!(!a.wrote, "only an edit can write");
        assert_eq!(a.tool, "read_file");
    }

    #[test]
    fn reading_a_skill_is_recorded_by_name_and_writes_nothing() {
        // ADR 0019 adds one closed-enum variant; the trail has to name it
        // rather than fold it into `read_file`, or a reader cannot tell which
        // procedure the model consulted.
        let a = action_from(
            2,
            &turn(
                trellis_loop::ToolCall::ReadSkill {
                    name: "rust-unsafe".into(),
                },
                "skill `rust-unsafe`, from .trellis/skills. applied: true",
            ),
        );
        assert_eq!(
            (a.turn, a.tool.as_str(), a.target.as_str(), a.wrote),
            (3, "read_skill", "rust-unsafe", false)
        );
    }

    #[test]
    fn checks_against_no_paths_say_so_rather_than_rendering_blank() {
        use trellis_checks::CheckKind;
        use trellis_loop::ToolCall;
        let a = action_from(
            0,
            &turn(
                ToolCall::RunChecks {
                    paths: Vec::new(),
                    kinds: vec![CheckKind::Tests],
                },
                "Tests: passed",
            ),
        );
        assert_eq!(a.target, "Tests on whole repository", "{a:?}");
    }

    #[test]
    fn changed_paths_reach_the_trail_action_cut_to_fifty_with_the_true_count() {
        // The risk: the paths a bash call changed reaching the turn and
        // stopping there, or reaching the trail whole. And `wrote` keeping
        // the meaning older trails gave it: set by `edit` and `write_file`.
        let mut t = turn(
            trellis_loop::ToolCall::Bash {
                command: "python gen.py".into(),
            },
            "exit 0
carried back from the clone: app/util.py",
        );
        t.changed = vec!["app/util.py".into()];
        let a = action_from(0, &t);
        assert_eq!(a.changed, ["app/util.py"]);
        assert_eq!(a.changed_count, 1);
        assert!(!a.wrote);

        t.changed = (0..60).map(|i| format!("pkg/f{i:02}.py")).collect();
        let a = action_from(0, &t);
        assert_eq!(a.changed.len(), 50);
        assert_eq!(a.changed[..], t.changed[..50]);
        assert_eq!(a.changed_count, 60);

        let read = action_from(
            1,
            &turn(
                trellis_loop::ToolCall::ReadFile {
                    path: "app/util.py".into(),
                },
                "def helper(): ...",
            ),
        );
        assert!(read.changed.is_empty() && read.changed_count == 0);
    }

    #[test]
    fn an_outcome_is_one_line_and_says_when_it_was_cut() {
        // A check's output runs to thousands of lines. A trail that stored it
        // would be mostly type-checker output, and a silently truncated line
        // reads as the whole outcome.
        assert_eq!(summarise("first\nsecond\nthird"), "first");
        assert_eq!(summarise("\n\n  indented  \nnext"), "indented");
        assert_eq!(summarise(""), "");
        let long = "x".repeat(400);
        let cut = summarise(&long);
        assert_eq!(cut.chars().count(), 120);
        assert!(cut.ends_with('…'), "a cut line says it was cut: {cut}");
    }

    #[test]
    fn a_multibyte_outcome_is_cut_on_a_character_boundary() {
        // `…` and an em dash both appear in this repository's own messages,
        // and slicing by bytes would panic on one.
        let long = "é".repeat(400);
        assert_eq!(summarise(&long).chars().count(), 120);
    }
}
