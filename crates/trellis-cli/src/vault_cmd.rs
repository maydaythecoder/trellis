//! `trellis vault` (ADR 0022): check, search, inspect and edit an
//! Apex-format vault.
//!
//! The work is in `trellis_vault`. This module picks the vault directory,
//! supplies the one thing the library will not compute — local calendar
//! days, for `dates.json` — and prints.
//!
//! These are commands a human runs. None is a tool the model has, so none
//! passes the tool gate (ADR 0018), and none checks for a clean worktree:
//! the edit is the human's, and `git diff` shows it.

use crate::config;
use anyhow::{Context, Result};
use clap::Subcommand;
use std::path::{Path, PathBuf};
use trellis_vault::dates::{self, FileDays, State};
use trellis_vault::edit::{self, Outcome};
use trellis_vault::search::Index;
use trellis_vault::{Graph, Issue, Vault};

#[derive(clap::Args)]
pub(crate) struct VaultArgs {
    /// The vault directory. Defaults to `[context] vault` in
    /// `.trellis/config.toml`. A `notes/` subdirectory, when present, is
    /// the notes root, as in Apex.
    #[arg(long, value_name = "DIR")]
    vault: Option<PathBuf>,

    #[command(subcommand)]
    action: VaultAction,
}

#[derive(Subcommand)]
pub(crate) enum VaultAction {
    /// Apex's report line, the graph issues, size warnings, and whether
    /// `trellis run` would accept the vault. Exits 1 when it would not.
    Check {
        /// Rewrite notes whose frontmatter the run gate rejects (extra,
        /// duplicate or missing keys; a non-canonical parent), a wrong
        /// manifest, and stale layout keys. Nothing already correct is
        /// touched.
        #[arg(long)]
        fix: bool,
    },
    /// BM25 over title, path and body, with a trigram fallback.
    Search {
        #[arg(required = true)]
        query: Vec<String>,
        #[arg(long, default_value = "12")]
        limit: usize,
    },
    /// A note's lineage, children, weighted links in and out, and depth.
    Graph { note: String },
    /// Create a note. The filename is the title's slug, de-duplicated; a
    /// title already taken gets " 2".
    New {
        title: String,
        #[arg(long)]
        parent: Option<String>,
    },
    /// Set a note's parent, or make it a root with `--no-parent`.
    ///
    /// Not `--root`: that is the global repository-root option, and it
    /// takes a path.
    Reparent {
        note: String,
        #[arg(required_unless_present = "no_parent")]
        parent: Option<String>,
        #[arg(long, conflicts_with = "parent")]
        no_parent: bool,
    },
    /// Append `[[to]]` to `from`'s body.
    Link { from: String, to: String },
    /// Change a note's title and rewrite every link that resolved to it.
    Retitle { note: String, title: String },
    /// Delete a note. Links to it become plain text; its children become
    /// roots. Git is the undo.
    Rm { note: String },
    /// Create a directory with an empty manifest.json and layout.json.
    Init { dir: PathBuf },
    /// Validate dates.json and show how much of it still matches the notes.
    Dates {
        /// Seed notes with no entry from their file times, reconcile notes
        /// whose text changed, and drop entries for deleted notes — what
        /// Apex does on opening a folder.
        #[arg(long)]
        seed: bool,
    },
}

pub(crate) fn run(root: &Path, args: VaultArgs) -> Result<()> {
    if let VaultAction::Init { dir } = &args.action {
        let out = edit::init(dir)?;
        return print_outcome(dir, &out);
    }
    let dir = vault_dir(root, args.vault)?;
    match args.action {
        VaultAction::Init { .. } => unreachable!("handled above"),
        VaultAction::Check { fix } => check(&dir, fix),
        VaultAction::Search { query, limit } => search(&dir, &query.join(" "), limit),
        VaultAction::Graph { note } => graph(&dir, &note),
        VaultAction::New { title, parent } => {
            let (_, out) = edit::create(&dir, &title, parent.as_deref())?;
            print_outcome(&dir, &out)
        }
        VaultAction::Reparent {
            note,
            parent,
            no_parent: _,
        } => {
            let out = edit::reparent(&dir, &note, parent.as_deref())?;
            print_outcome(&dir, &out)
        }
        VaultAction::Link { from, to } => {
            let out = edit::link(&dir, &from, &to)?;
            print_outcome(&dir, &out)
        }
        VaultAction::Retitle { note, title } => {
            let out = edit::retitle(&dir, &note, &title)?;
            print_outcome(&dir, &out)
        }
        VaultAction::Rm { note } => {
            let out = edit::remove(&dir, &note)?;
            print_outcome(&dir, &out)
        }
        VaultAction::Dates { seed } => dates_cmd(&dir, seed),
    }
}

/// `--vault`, as given, or `[context] vault` under the repository root.
fn vault_dir(root: &Path, flag: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(d) = flag {
        return Ok(d);
    }
    let cfg = config::RepoConfig::load(root)?;
    match cfg.vault(root, None)? {
        Some(v) => Ok(v.dir),
        None => anyhow::bail!(
            "no vault: pass --vault <dir>, or set [context] vault = \"<dir>\" in \
             .trellis/config.toml"
        ),
    }
}

/// Today, in the system time zone, as JavaScript's `localDay()` gives it.
fn today() -> Option<String> {
    Some(jiff::Zoned::now().date().to_string())
}

/// `dayFromTimestamp`: a file time as a local day, `None` outside years
/// 1..=9999 or when the platform has no such time.
fn local_day(t: std::io::Result<std::time::SystemTime>) -> Option<String> {
    let ts = jiff::Timestamp::try_from(t.ok()?).ok()?;
    let date = ts.to_zoned(jiff::tz::TimeZone::system()).date();
    (1..=9999).contains(&date.year()).then(|| date.to_string())
}

fn file_days(root: &Path, rel: &str) -> FileDays {
    let p = rel.split('/').fold(root.to_path_buf(), |p, s| p.join(s));
    match std::fs::metadata(&p) {
        Ok(m) => FileDays {
            created: local_day(m.created()),
            modified: local_day(m.modified()),
        },
        Err(_) => FileDays::default(),
    }
}

/// Prints what an edit did, then keeps `dates.json` in step when it exists.
fn print_outcome(dir: &Path, out: &Outcome) -> Result<()> {
    for m in &out.messages {
        println!("{m}");
    }
    if out.touched.is_empty() {
        return Ok(());
    }
    let day = today().context("the local date is outside what dates.json can record")?;
    if let Some(m) =
        dates::after_edit(&edit::root_of(dir), &out.touched, &day).map_err(anyhow::Error::msg)?
    {
        println!("{m}");
    }
    Ok(())
}

fn check(dir: &Path, fix: bool) -> Result<()> {
    let r = edit::check(dir, fix)?;
    println!("{}", r.line);
    if !r.touched.is_empty() {
        let day = today().context("the local date is outside what dates.json can record")?;
        if let Some(m) =
            dates::after_edit(&edit::root_of(dir), &r.touched, &day).map_err(anyhow::Error::msg)?
        {
            println!("{m}");
        }
    }
    if !r.issues.is_empty() {
        println!("issues:");
        for i in &r.issues {
            println!("  {i}");
        }
    }
    for w in &r.warnings {
        println!("warning: {w}");
    }
    match &r.gate {
        Ok(stats) => println!("run gate: accepts this vault ({stats})"),
        Err(violations) => {
            println!(
                "run gate: refuses this vault, {} violation(s):",
                violations.len()
            );
            for v in violations {
                println!("  {v}");
            }
            if !fix {
                println!("`trellis vault check --fix` repairs frontmatter and the manifest.");
            }
            use std::io::Write;
            std::io::stdout().flush().ok();
            std::process::exit(1);
        }
    }
    Ok(())
}

fn search(dir: &Path, query: &str, limit: usize) -> Result<()> {
    let v = Vault::open(dir)?;
    let idx = Index::build(&v);
    if !idx.mode.include_body {
        println!("search reduced to titles and paths: the vault is over Apex's body-index limits");
    } else if !idx.mode.include_trigrams {
        println!("fuzzy search reduced: the vault is over Apex's trigram limit");
    }
    let hits = idx.search(query, limit);
    if hits.is_empty() {
        println!("no matches for \"{query}\"");
    }
    for h in hits {
        println!("{:>8.4}  {}  \"{}\"", h.score, h.path, h.title);
    }
    Ok(())
}

fn graph(dir: &Path, arg: &str) -> Result<()> {
    let s = edit::Session::open(dir)?;
    let i = s.resolve(arg)?;
    let (v, g): (&Vault, &Graph) = (&s.vault, &s.graph);
    let n = &v.notes[i];
    let label = |j: usize| format!("{} \"{}\"", v.notes[j].path, v.notes[j].title);
    println!("note:     {}", label(i));
    match g.depth[i] {
        Some(d) => println!("depth:    {d}"),
        None => println!("depth:    none (under a missing parent, or on a cycle)"),
    }
    println!(
        "lineage:  {}",
        g.lineage(i)
            .iter()
            .map(|&j| v.notes[j].path.as_str())
            .collect::<Vec<_>>()
            .join(" > ")
    );
    if g.parents[i].is_none()
        && let Some(t) = n.parent_target()
    {
        println!("parent:   [[{t}]] does not resolve");
    }
    let list = |name: &str, items: Vec<String>| {
        if items.is_empty() {
            println!("{name:<9} (none)");
        } else {
            println!("{name}");
            for it in items {
                println!("  {it}");
            }
        }
    };
    list(
        "children:",
        g.children[i].iter().map(|&c| label(c)).collect(),
    );
    list(
        "refs out:",
        g.refs_out[i]
            .iter()
            .map(|&(t, w)| format!("{} x{w}", label(t)))
            .collect(),
    );
    list(
        "refs in:",
        g.refs_in[i]
            .iter()
            .map(|&(t, w)| format!("{} x{w}", label(t)))
            .collect(),
    );
    let mine: Vec<String> = g
        .issues
        .iter()
        .filter(|x| mentions(x, &n.path))
        .map(ToString::to_string)
        .collect();
    if !mine.is_empty() {
        list("issues:", mine);
    }
    Ok(())
}

fn mentions(i: &Issue, path: &str) -> bool {
    match i {
        Issue::MissingParent { path: p, .. } | Issue::Disconnected { path: p } => p == path,
        Issue::Cycle { paths, .. } | Issue::DuplicateAlias { paths, .. } => {
            paths.iter().any(|p| p == path)
        }
    }
}

fn dates_cmd(dir: &Path, seed: bool) -> Result<()> {
    let v = Vault::open(dir)?;
    let file = match dates::load(&v.root) {
        State::Invalid(e) => {
            anyhow::bail!("dates.json does not validate, and is never overwritten: {e}")
        }
        State::Absent if !seed => {
            println!(
                "no dates.json in {}; `trellis vault dates --seed` creates one",
                v.root.display()
            );
            return Ok(());
        }
        State::Absent => None,
        State::Valid(f) => Some(f),
    };
    if seed {
        let mut doc = file.as_ref().map(|f| f.doc.clone()).unwrap_or_default();
        let root = v.root.clone();
        let (s, r, d, failures) = dates::refresh(&v, &mut doc, &|p| file_days(&root, p));
        for f in &failures {
            println!("skipped: {f}");
        }
        let wrote = match file {
            Some(mut f) => {
                f.doc = doc;
                f.save().map_err(anyhow::Error::msg)?
            }
            None => {
                dates::validate(&doc).map_err(anyhow::Error::msg)?;
                dates::atomic_write(&dates::path(&v.root), &dates::to_bytes(&doc), None)
                    .context("writing dates.json")?;
                true
            }
        };
        println!(
            "dates.json: {s} seeded, {r} reconciled, {d} dropped{}",
            if wrote { "" } else { "; unchanged" }
        );
        return Ok(());
    }
    let Some(f) = file else {
        return Ok(());
    };
    let (rows, stale) = dates::coverage(&v, &f.doc);
    for c in rows {
        if c.has_entry {
            println!(
                "{}  created={}  lines={} matching={}",
                c.path,
                c.created.as_deref().unwrap_or("unknown"),
                c.lines,
                c.matching
            );
        } else {
            println!("{}  no entry", c.path);
        }
    }
    for s in stale {
        println!("{s}  entry for a note that does not exist");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::{CommandFactory, Parser};

    fn parse(argv: &[&str]) -> VaultArgs {
        match crate::Cli::try_parse_from(argv).unwrap().command {
            Some(crate::Command::Vault(a)) => a,
            _ => panic!("not a vault command"),
        }
    }

    #[test]
    fn the_vault_subcommand_parses_under_the_real_cli() {
        // `reparent --root` once collided with the global `--root <PATH>` and
        // panicked at parse time, in the binary, not in any library test.
        crate::Cli::command().debug_assert();
        let a = parse(&["trellis", "vault", "reparent", "a", "--no-parent"]);
        assert!(matches!(
            a.action,
            VaultAction::Reparent {
                no_parent: true,
                parent: None,
                ..
            }
        ));
        let a = parse(&[
            "trellis", "--root", ".", "vault", "--vault", "v", "search", "two", "words",
        ]);
        assert_eq!(a.vault.as_deref(), Some(Path::new("v")));
        assert!(
            matches!(a.action, VaultAction::Search { ref query, .. } if query == &["two", "words"])
        );
    }

    #[test]
    fn no_vault_configured_is_an_error_that_says_how_to_set_one() {
        let d = tempfile::tempdir().unwrap();
        let e = vault_dir(d.path(), None).unwrap_err().to_string();
        assert!(
            e.contains("--vault") && e.contains("[context] vault"),
            "{e}"
        );
    }

    #[test]
    fn a_local_day_is_a_calendar_day() {
        let d = local_day(Ok(std::time::SystemTime::now())).unwrap();
        assert!(trellis_vault::dates::normalize_day(&d).is_some(), "{d}");
        assert_eq!(today().as_deref().map(str::len), Some(10));
    }
}
