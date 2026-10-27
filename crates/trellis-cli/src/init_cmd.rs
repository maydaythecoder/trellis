//! `trellis init`: scaffolds `.trellis/` and reports the prerequisites a
//! run needs, with the same version probe `doctor` uses.

use crate::config;
use crate::doctor_cmd::version_of;
use anyhow::{Context, Result};
use std::path::Path;

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
    let files = [
        ("config.toml", CONFIG_TEMPLATE),
        ("context.md", CONTEXT_TEMPLATE),
        ("governance/README.md", GOVERNANCE_TEMPLATE),
    ];
    // A repository can commit `.trellis`, `.trellis/governance` or any of
    // these files as a link, and a dangling one reads as absent to
    // `exists()`, so the template was written at the link's target outside
    // the repository (#723). Every entry is checked before anything is
    // created, so a refusal leaves the disk as it was.
    for (rel, _) in &files {
        trellis_core::links::refuse_links_below(
            root,
            &Path::new(".trellis").join(rel),
            "`trellis init`'s scaffold",
        )?;
    }
    let mut wrote = Vec::new();
    let mut kept = Vec::new();
    for (rel, body) in files {
        let path = dir.join(rel);
        // `symlink_metadata`, not `exists()`, for the reason above.
        if std::fs::symlink_metadata(&path).is_ok() {
            kept.push(rel);
            continue;
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        // `create_new`: a link planted after the check above is not
        // followed, and an existing entry is never overwritten.
        trellis_core::links::create_new(&path, body.as_bytes())
            .with_context(|| format!("writing {}", path.display()))?;
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

#[cfg(test)]
mod init_tests {
    use super::*;
    use std::path::PathBuf;

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
}
