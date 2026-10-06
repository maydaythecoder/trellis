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

use crate::{config, plain};
use anyhow::{Context, Result};
use clap::Subcommand;
use std::path::{Path, PathBuf};
use trellis_vault::dates::{self, FileDays, State};
use trellis_vault::edit::{self, Outcome};
use trellis_vault::search::{Index, search_results};
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
    /// Apex's search-box ranking: BM25 over title, path and body with a
    /// trigram fallback, plus exact, prefix, substring and subsequence
    /// matches on title, filename and path.
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
fn today() -> String {
    jiff::Zoned::now().date().to_string()
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
    // Edit messages name notes by path and title, which are untrusted (#461).
    for m in &out.messages {
        println!("{}", plain(m));
    }
    sync_dates(dir, &out.touched)
}

/// Keeps `dates.json` in step with what an edit touched, when it exists.
fn sync_dates(dir: &Path, touched: &dates::Touched) -> Result<()> {
    if touched.is_empty() {
        return Ok(());
    }
    if let Some(m) = dates::after_edit(&trellis_vault::notes_root(dir), touched, &today())
        .map_err(anyhow::Error::msg)?
    {
        // Several lines, each naming a note: escaped one line at a time.
        for l in m.split('\n') {
            println!("{}", plain(l));
        }
    }
    Ok(())
}

fn check(dir: &Path, fix: bool) -> Result<()> {
    let r = edit::check(dir, fix)?;
    println!("{}", plain(&r.line));
    sync_dates(dir, &r.touched)?;
    if !r.issues.is_empty() {
        println!("issues:");
        for i in &r.issues {
            println!("  {}", plain(&i.to_string()));
        }
    }
    for w in &r.warnings {
        println!("warning: {}", plain(&w.to_string()));
    }
    match &r.gate {
        Ok(stats) => println!("run gate: accepts this vault ({stats})"),
        Err(violations) => {
            println!(
                "run gate: refuses this vault, {} violation(s):",
                violations.len()
            );
            for v in violations {
                println!("  {}", plain(&v.to_string()));
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
    for l in search_lines(dir, query, limit)? {
        println!("{l}");
    }
    Ok(())
}

/// What `trellis vault search` prints, one string per line.
fn search_lines(dir: &Path, query: &str, limit: usize) -> Result<Vec<String>> {
    let v = Vault::open(dir)?;
    let idx = Index::build(&v);
    let mut lines = Vec::new();
    if !idx.mode.include_body {
        lines.push(
            "search reduced to titles and paths: the vault is over Apex's body-index limits"
                .to_string(),
        );
    } else if !idx.mode.include_trigrams {
        lines.push("fuzzy search reduced: the vault is over Apex's trigram limit".to_string());
    }
    // `searchResults.ts`, the order Apex's search box shows, not the
    // index's own (ADR 0022 §4).
    let hits = search_results(&v, &idx, query, limit);
    if hits.is_empty() {
        lines.push(format!("no matches for \"{query}\""));
    }
    // A note is untrusted content (#461): its path and title go through
    // `plain`, so a control sequence in either cannot repaint the terminal.
    for h in hits {
        lines.push(format!(
            "{:>8.4}  {}  \"{}\"",
            h.score,
            plain(&h.path),
            plain(&h.title)
        ));
    }
    Ok(lines)
}

fn graph(dir: &Path, arg: &str) -> Result<()> {
    let s = edit::Session::open(dir)?;
    let i = s.resolve(arg)?;
    let (v, g): (&Vault, &Graph) = (&s.vault, &s.graph);
    let n = &v.notes[i];
    // Paths, titles, link targets and issues come from the notes, which
    // are untrusted (#461): every one goes through `plain` before printing.
    let label = |j: usize| {
        format!(
            "{} \"{}\"",
            plain(&v.notes[j].path),
            plain(&v.notes[j].title)
        )
    };
    println!("note:     {}", label(i));
    match g.depth[i] {
        Some(d) => println!("depth:    {d}"),
        None => println!("depth:    none (under a missing parent, or on a cycle)"),
    }
    println!(
        "lineage:  {}",
        g.lineage(i)
            .iter()
            .map(|&j| plain(&v.notes[j].path))
            .collect::<Vec<_>>()
            .join(" > ")
    );
    if g.parents[i].is_none()
        && let Some(t) = n.parent_target()
    {
        println!("parent:   [[{}]] does not resolve", plain(&t));
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
        .map(|x| plain(&x.to_string()))
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

impl VaultArgs {
    /// `vault init`, the one action that makes sense outside a repository:
    /// it creates a vault, which a person may do before there is anything
    /// to put it in (ADR 0022 §3, integrated).
    pub(crate) fn is_init(&self) -> bool {
        matches!(self.action, VaultAction::Init { .. })
    }
}

/// The vault note a run is given, and what `[context] dates` did to it.
///
/// With `dates` off this is `trellis_vault::compile`, byte for byte. With it
/// on, `dates.json` at the notes root annotates the headings. Absent is not
/// an error: the note compiles plain and the second value says why. Present
/// and invalid stops the run before the snapshot, like an invalid vault: a
/// file the project configured and the model silently did not get is the
/// failure ADR 0025 §1 names.
pub(crate) fn compile_for_run(
    dir: &Path,
    stem: &str,
    dates_on: bool,
) -> Result<(trellis_vault::Compiled, Option<String>)> {
    if !dates_on {
        return Ok((
            trellis_vault::compile(dir, stem, trellis_vault::DEFAULT_CAP)?,
            None,
        ));
    }
    match dates::load(&trellis_vault::notes_root(dir)) {
        State::Absent => Ok((
            trellis_vault::compile(dir, stem, trellis_vault::DEFAULT_CAP)?,
            Some("dates on, but no dates.json; headings not dated".to_string()),
        )),
        State::Invalid(e) => anyhow::bail!(
            "[context] dates = true and dates.json does not validate: {e}. Fix it with \
             `trellis vault dates`, or turn dates off"
        ),
        State::Valid(f) => Ok((
            trellis_vault::compile_with_dates(dir, stem, trellis_vault::DEFAULT_CAP, Some(&f.doc))?,
            Some("headings dated from dates.json".to_string()),
        )),
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
            println!("skipped: {}", plain(&f.to_string()));
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
        println!("{}", plain(&coverage_row(&c)));
    }
    for s in stale {
        println!("{}  entry for a note that does not exist", plain(&s));
    }
    Ok(())
}

/// One `trellis vault dates` row. A note over the line limit has 0 lines
/// and 0 matching only because it was not read, so it says why, in the
/// words `--seed` uses when it skips the same note.
fn coverage_row(c: &dates::Coverage) -> String {
    if !c.has_entry {
        return format!("{}  no entry", c.path);
    }
    let created = c.created.as_deref().unwrap_or("unknown");
    if c.too_long {
        format!(
            "{}  created={created}  {}",
            c.path,
            dates::DatesError::NoteTooLong
        )
    } else {
        format!(
            "{}  created={created}  lines={} matching={}",
            c.path, c.lines, c.matching
        )
    }
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

    /// The risk: an over-limit note printed `lines=0 matching=0`, the same
    /// row as an empty one, while `--seed` skipped it as too long.
    #[test]
    fn a_dates_row_for_a_note_over_the_line_limit_says_so() {
        let row = |too_long| dates::Coverage {
            path: "big.md".to_string(),
            created: Some("2026-10-05".to_string()),
            lines: 0,
            matching: 0,
            has_entry: true,
            too_long,
        };
        let long = coverage_row(&row(true));
        assert!(
            long.contains(&dates::DatesError::NoteTooLong.to_string()),
            "{long}"
        );
        assert!(!long.contains("lines=0"), "{long}");
        assert_eq!(
            coverage_row(&row(false)),
            "big.md  created=2026-10-05  lines=0 matching=0"
        );
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

    fn fixture_vault() -> tempfile::TempDir {
        let src =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../trellis-vault/tests/fixtures/vault");
        let d = tempfile::tempdir().unwrap();
        for e in std::fs::read_dir(src).unwrap() {
            let e = e.unwrap();
            std::fs::copy(e.path(), d.path().join(e.file_name())).unwrap();
        }
        d
    }

    #[test]
    fn dates_off_or_absent_compiles_the_golden_and_on_with_a_file_dates_the_heading() {
        // The risk: `[context] dates` changing what the model is shown when
        // it is off, or when there is nothing to date from. The golden is
        // the specification of the selection rule (ADR 0017).
        let golden = std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../trellis-vault/tests/fixtures/expected-task-a.md"),
        )
        .unwrap();
        let d = fixture_vault();
        let (c, note) = compile_for_run(d.path(), "task-a", true).unwrap();
        assert_eq!(c.text, golden);
        assert!(note.unwrap().contains("no dates.json"));

        let mut doc = dates::Document::default();
        doc.notes.insert(
            "task-a.md".into(),
            dates::NoteDates {
                created: dates::Stamp {
                    day: Some("2026-03-01".into()),
                    estimated: false,
                },
                lines: Vec::new(),
            },
        );
        std::fs::write(d.path().join("dates.json"), dates::to_bytes(&doc)).unwrap();
        let (c, _) = compile_for_run(d.path(), "task-a", false).unwrap();
        assert_eq!(c.text, golden, "off ignores a dates.json that is present");
        let (c, note) = compile_for_run(d.path(), "task-a", true).unwrap();
        assert!(
            c.text
                .contains("### Task A (task-a.md, created 2026-03-01) [target]"),
            "{}",
            c.text
        );
        assert!(note.unwrap().contains("dated from dates.json"));

        // Configured and invalid stops the run rather than sending plain
        // headings the configuration said would be dated.
        std::fs::write(d.path().join("dates.json"), "{\"version\":2}").unwrap();
        let e = compile_for_run(d.path(), "task-a", true).unwrap_err();
        assert!(e.to_string().contains("does not validate"), "{e}");
    }

    fn vault_of(notes: &[(&str, &str, &str)]) -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        for (path, title, body) in notes {
            std::fs::write(
                d.path().join(path),
                format!("---\ntitle: \"{title}\"\n---\n{body}\n"),
            )
            .unwrap();
        }
        d
    }

    /// The position of the first printed line naming `path`.
    fn line_of(lines: &[String], path: &str) -> Option<usize> {
        lines
            .iter()
            .position(|l| l.contains(&format!("  {path}  ")))
    }

    /// The risk: the command ranking with the index alone, which never
    /// finds a note whose title holds the query only as a subsequence,
    /// where Apex's search box does (`searchResults.ts`).
    #[test]
    fn vault_search_finds_a_subsequence_only_title_match() {
        let d = vault_of(&[
            ("retry-policy.md", "Retry policy", "Backoff doubles."),
            ("other.md", "Other", "Nothing here."),
        ]);
        let v = Vault::open(d.path()).unwrap();
        assert!(
            Index::build(&v).search("rtp", 12).is_empty(),
            "the index alone must not find it, or this test proves nothing"
        );
        let lines = search_lines(d.path(), "rtp", 12).unwrap();
        assert!(line_of(&lines, "retry-policy.md").is_some(), "{lines:?}");
    }

    /// The risk: the command printing the index's order, where a body that
    /// repeats the query outranks a note whose title is the query.
    #[test]
    fn vault_search_ranks_an_exact_title_first() {
        let d = vault_of(&[
            ("retry.md", "Retry", "Backoff doubles."),
            (
                "policy.md",
                "Retry policy",
                &"retry retry retry retry retry retry retry retry. ".repeat(4),
            ),
        ]);
        let v = Vault::open(d.path()).unwrap();
        let hits = Index::build(&v).search("retry", 12);
        let paths: Vec<&str> = hits.iter().map(|h| h.path.as_str()).collect();
        assert_eq!(
            paths,
            ["policy.md", "retry.md"],
            "the index must rank B first, or this test proves nothing"
        );
        let lines = search_lines(d.path(), "retry", 12).unwrap();
        let (a, b) = (line_of(&lines, "retry.md"), line_of(&lines, "policy.md"));
        assert!(a.is_some() && b.is_some() && a < b, "{lines:?}");
    }

    #[test]
    fn a_local_day_is_a_calendar_day() {
        let d = local_day(Ok(std::time::SystemTime::now())).unwrap();
        assert!(trellis_vault::dates::normalize_day(&d).is_some(), "{d}");
        assert_eq!(today().len(), 10);
    }
}
