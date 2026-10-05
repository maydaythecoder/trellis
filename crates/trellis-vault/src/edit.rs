//! Editing a vault the way Apex's app does, as commands a human runs
//! (ADR 0022 §3).
//!
//! Every write:
//! - goes through `composeRaw` or `createNoteRaw`, so a note written here
//!   has exactly two frontmatter keys;
//! - happens only when a file's bytes change;
//! - keeps `manifest.json` equal to the files on disk and sorted;
//! - prunes `layout.json` keys for deleted notes and leaves every other key;
//! - never opens `annotations.json`.
//!
//! `dates.json` is kept in step by the caller through
//! [`crate::dates::after_edit`], with the [`Touched`] every operation
//! returns, because "today" is a local-time question this crate does not
//! answer.

use crate::VaultError;
use crate::dates::Touched;
use crate::graph::{Graph, Issue, Resolved};
use crate::js;
use crate::reader::{Vault, file_of, io, note_paths, refuse_link};
use crate::refs::{self, alias_from_inner, link_markup, normalize_key, rewrite_links, slugify};
use crate::validate::{self, Stats};
use std::path::Path;

/// `escapeYaml`.
fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn parent_line(parent: Option<&str>) -> String {
    match parent {
        Some(p) => format!("parent: \"{}\"", escape(p)),
        None => "parent: null".to_string(),
    }
}

/// `composeRaw`: the form every rewrite takes.
pub fn compose_raw(title: &str, parent: Option<&str>, body: &str) -> String {
    let nl = if body.ends_with('\n') { "" } else { "\n" };
    format!(
        "---\ntitle: \"{}\"\n{}\n---\n\n{body}{nl}",
        escape(title),
        parent_line(parent)
    )
}

/// `createNoteRaw` with an empty body.
pub fn create_raw(title: &str, parent: Option<&str>) -> String {
    format!(
        "---\ntitle: \"{}\"\n{}\n---\n\n",
        escape(title),
        parent_line(parent)
    )
}

/// What an operation did.
#[derive(Debug, Clone, Default)]
pub struct Outcome {
    pub touched: Touched,
    /// One line per thing worth saying: what was written, what was left.
    pub messages: Vec<String>,
}

/// A loaded vault with its index, for resolving arguments and planning
/// writes against one consistent snapshot.
pub struct Session {
    pub vault: Vault,
    pub graph: Graph,
}

impl Session {
    pub fn open(dir: &Path) -> Result<Session, VaultError> {
        let vault = Vault::open(dir)?;
        // Here, before any operation writes: `create` and `rm` touch a note
        // before the manifest and layout are reached.
        for f in ["manifest.json", "layout.json"] {
            refuse_link(&vault.root.join(f), "workspace metadata")?;
        }
        let graph = Graph::build(&vault);
        Ok(Session { vault, graph })
    }

    /// A note named on the command line, in any form Apex resolves.
    pub fn resolve(&self, arg: &str) -> Result<usize, VaultError> {
        match self.graph.resolver.resolve(arg) {
            Resolved::Note(i) => Ok(i),
            Resolved::Missing => Err(VaultError::UnknownStem(arg.to_string())),
            Resolved::Ambiguous(c) => Err(VaultError::AmbiguousStem {
                stem: arg.to_string(),
                paths: c
                    .iter()
                    .map(|&i| self.vault.notes[i].path.clone())
                    .collect(),
            }),
        }
    }

    /// `[[key]]` for a note, as a parent or link target.
    fn link_to(&self, i: usize) -> String {
        format!("[[{}]]", self.graph.resolver.key_for(&self.vault, i))
    }

    /// `frontmatterValuesForNote`: the parent a rewrite keeps. A resolved
    /// parent is written by its key; an unresolved one keeps its reference
    /// rather than being silently dropped.
    fn parent_of(&self, i: usize) -> Option<String> {
        match self.graph.parents[i] {
            Some(p) => Some(self.link_to(p)),
            None => self.vault.notes[i]
                .parent_target()
                .map(|t| format!("[[{t}]]")),
        }
    }

    fn root(&self) -> &Path {
        &self.vault.root
    }
}

fn refused(msg: impl Into<String>) -> VaultError {
    VaultError::Refused(msg.into())
}

/// Writes `raw` to `rel` unless the file already holds exactly those bytes.
fn write_note(root: &Path, rel: &str, raw: &str) -> Result<bool, VaultError> {
    let p = file_of(root, rel);
    if std::fs::read(&p).is_ok_and(|b| b == raw.as_bytes()) {
        return Ok(false);
    }
    std::fs::write(&p, raw).map_err(io(&p))?;
    Ok(true)
}

/// Apex's `write_manifest_file` format: pretty, sorted, trailing newline.
pub fn manifest_text(paths: &[String]) -> String {
    let mut sorted = paths.to_vec();
    sorted.sort_by(|a, b| js::cmp(a, b));
    format!(
        "{}\n",
        serde_json::to_string_pretty(&sorted).unwrap_or_else(|_| "[]".into())
    )
}

/// Rewrites `manifest.json` when it does not match the files on disk.
/// A manifest that is already right is left in whatever format it has.
fn sync_manifest(root: &Path) -> Result<bool, VaultError> {
    let paths = note_paths(root)?;
    let state = validate::manifest_state(root, &paths, &mut Vec::new())?;
    if !state.wrong {
        return Ok(false);
    }
    let p = root.join("manifest.json");
    std::fs::write(&p, manifest_text(&paths)).map_err(io(&p))?;
    Ok(true)
}

/// Removes `layout.json` keys naming no note. `None` when there is no
/// layout; an unparseable one is left alone and reported, because Apex's
/// answer to that is to replace it with `{}` and lose every position.
fn prune_layout(root: &Path) -> Result<Option<Result<usize, String>>, VaultError> {
    let p = root.join("layout.json");
    let text = match std::fs::read_to_string(&p) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(io(&p)(e)),
    };
    let Ok(serde_json::Value::Object(mut map)) = serde_json::from_str::<serde_json::Value>(&text)
    else {
        return Ok(Some(Err(
            "layout.json is not a JSON object; left untouched".to_string(),
        )));
    };
    let live = note_paths(root)?;
    let before = map.len();
    map.retain(|k, _| live.iter().any(|l| l == k));
    let removed = before - map.len();
    if removed > 0 {
        let out = format!(
            "{}\n",
            serde_json::to_string_pretty(&serde_json::Value::Object(map)).unwrap_or_default()
        );
        std::fs::write(&p, out).map_err(io(&p))?;
    }
    Ok(Some(Ok(removed)))
}

/// Manifest and layout after a change to the set of notes.
fn after_set_change(root: &Path, out: &mut Outcome) -> Result<(), VaultError> {
    if sync_manifest(root)? {
        out.messages.push("manifest.json updated".into());
    }
    match prune_layout(root)? {
        Some(Ok(n)) if n > 0 => out
            .messages
            .push(format!("layout.json: {n} stale key(s) removed")),
        Some(Err(e)) => out.messages.push(e),
        _ => {}
    }
    Ok(())
}

/// `getAvailableDisplayTitle`.
fn available_title(title: &str, s: &Session) -> String {
    let taken: Vec<String> = s
        .vault
        .notes
        .iter()
        .map(|n| normalize_key(&n.title))
        .collect();
    let base = match js::trim(title) {
        "" => "Untitled note".to_string(),
        t => t.to_string(),
    };
    let mut candidate = base.clone();
    let mut suffix = 2;
    while taken.contains(&normalize_key(&candidate)) {
        candidate = format!("{base} {suffix}");
        suffix += 1;
    }
    candidate
}

/// `getAvailableNewNotePath`: slug, then `-2`, `-3`, …, compared without
/// case, at the root.
fn available_path(title: &str, s: &Session) -> String {
    let base = match slugify(title) {
        b if b.is_empty() => "untitled".to_string(),
        b => b,
    };
    let taken: Vec<String> = s
        .vault
        .notes
        .iter()
        .map(|n| n.path.to_lowercase())
        .collect();
    let mut suffix = 0;
    loop {
        let name = if suffix == 0 {
            format!("{base}.md")
        } else {
            format!("{base}-{}.md", suffix + 1)
        };
        if !taken.contains(&name.to_lowercase()) {
            return name;
        }
        suffix += 1;
    }
}

/// `trellis vault new`. Returns the new note's path.
pub fn create(
    dir: &Path,
    title: &str,
    parent: Option<&str>,
) -> Result<(String, Outcome), VaultError> {
    let s = Session::open(dir)?;
    let parent_ref = parent
        .map(|p| s.resolve(p).map(|i| s.link_to(i)))
        .transpose()?;
    let title = available_title(title, &s);
    let path = available_path(&title, &s);
    let file = file_of(s.root(), &path);
    // `create_new`: a non-note file of the same name (a dotfile's twin, a
    // file the walk skipped) is never overwritten.
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&file)
        .map_err(io(&file))?;
    use std::io::Write;
    f.write_all(create_raw(&title, parent_ref.as_deref()).as_bytes())
        .map_err(io(&file))?;
    let mut out = Outcome::default();
    out.messages.push(format!("created {path} (\"{title}\")"));
    out.touched.created.push(path.clone());
    after_set_change(s.root(), &mut out)?;
    Ok((path, out))
}

/// `trellis vault reparent`. `parent: None` makes the note a root.
pub fn reparent(dir: &Path, note: &str, parent: Option<&str>) -> Result<Outcome, VaultError> {
    let s = Session::open(dir)?;
    let i = s.resolve(note)?;
    let parent_ref = match parent {
        None => None,
        Some(p) => {
            let p = s.resolve(p)?;
            if s.graph.is_descendant_or_self(p, i) {
                return Err(refused(format!(
                    "{} cannot be the parent of {}: it is the note itself or one of its descendants",
                    s.vault.notes[p].path, s.vault.notes[i].path
                )));
            }
            Some(s.link_to(p))
        }
    };
    let n = &s.vault.notes[i];
    let raw = compose_raw(&n.title, parent_ref.as_deref(), &n.body);
    let mut out = Outcome::default();
    if write_note(s.root(), &n.path, &raw)? {
        out.touched.modified.push(n.path.clone());
        out.messages.push(format!(
            "{}: parent {}",
            n.path,
            parent_ref.as_deref().unwrap_or("null")
        ));
    } else {
        out.messages.push(format!("{}: already so", n.path));
    }
    Ok(out)
}

/// `trellis vault link`: appends `[[to]]` to `from`'s body.
pub fn link(dir: &Path, from: &str, to: &str) -> Result<Outcome, VaultError> {
    let s = Session::open(dir)?;
    let (f, t) = (s.resolve(from)?, s.resolve(to)?);
    if f == t {
        return Err(refused("a note cannot link to itself"));
    }
    let src = &s.vault.notes[f];
    let mut out = Outcome::default();
    // `hasBodyReference`.
    if refs::graph_links(&src.body)
        .iter()
        .any(|l| s.graph.resolver.resolve(&l.target) == Resolved::Note(t))
    {
        out.messages.push(format!(
            "{} already links to {}",
            src.path, s.vault.notes[t].path
        ));
        return Ok(out);
    }
    // `appendBodyReference`.
    let trimmed = src.body.trim_end_matches(js::is_ws);
    let sep = if trimmed.is_empty() { "" } else { "\n\n" };
    let body = format!("{trimmed}{sep}{}\n", s.link_to(t));
    let raw = compose_raw(&src.title, s.parent_of(f).as_deref(), &body);
    write_note(s.root(), &src.path, &raw)?;
    out.touched.modified.push(src.path.clone());
    out.messages
        .push(format!("{}: linked to {}", src.path, s.vault.notes[t].path));
    Ok(out)
}

/// `trellis vault retitle`: the title changes, the file does not, and every
/// body link that resolved to the note becomes `[[New Title]]`, keeping its
/// label.
pub fn retitle(dir: &Path, note: &str, title: &str) -> Result<Outcome, VaultError> {
    let s = Session::open(dir)?;
    let i = s.resolve(note)?;
    let title = js::trim(title);
    if title.is_empty() {
        return Err(refused("a title cannot be empty"));
    }
    let mut out = Outcome::default();
    if s.vault.notes[i].title == title {
        out.messages
            .push(format!("{}: title unchanged", s.vault.notes[i].path));
        return Ok(out);
    }
    let key = normalize_key(title);
    let new_keys = refs::alias_keys(&s.vault.notes[i].path, title);
    let other = s
        .vault
        .notes
        .iter()
        .enumerate()
        .find(|(j, n)| *j != i && refs::alias_keys(&n.path, &n.title).contains(&key));
    let collides = other.is_some();
    // A child whose `parent` reached this note only through the old title
    // (or its slug) would be orphaned by the new one. Rewritten as
    // `[[New Title]]`, as body links are, unless that title is also another
    // note's key; then by the key `link_to` would write. A reference by
    // path or basename still resolves and is left as written.
    let reparent = |j: usize| -> Option<String> {
        if s.graph.parents[j] != Some(i) {
            return None;
        }
        let t = s.vault.notes[j].parent_target()?;
        if new_keys.contains(&normalize_key(&t)) {
            return None;
        }
        Some(if collides {
            s.link_to(i)
        } else {
            format!("[[{title}]]")
        })
    };
    if let Some(other) = other {
        out.messages.push(format!(
            "warning: \"{title}\" is also a key of {}; [[{title}]] will resolve to neither",
            other.1.path
        ));
    }
    let (mut links, mut parents) = (0, 0);
    for (j, n) in s.vault.notes.iter().enumerate() {
        let (body, count) = rewrite_links(&n.body, |l| {
            (s.graph.resolver.resolve(&l.parsed.target) == Resolved::Note(i))
                .then(|| link_markup(title, alias_from_inner(l.inner)))
        });
        let new_parent = reparent(j);
        if j != i && count == 0 && new_parent.is_none() {
            continue;
        }
        links += count;
        parents += usize::from(new_parent.is_some());
        let t = if j == i { title } else { n.title.as_str() };
        let parent = new_parent.or_else(|| s.parent_of(j));
        let raw = compose_raw(t, parent.as_deref(), &body);
        if write_note(s.root(), &n.path, &raw)? {
            out.touched.modified.push(n.path.clone());
        }
    }
    out.messages.push(format!(
        "{}: retitled \"{title}\"; {links} link(s) and {parents} parent reference(s) rewritten",
        s.vault.notes[i].path
    ));
    Ok(out)
}

/// `trellis vault rm`: the file is deleted; links to it become their label;
/// its children become roots, as Apex's code does (its docs say children
/// keep a broken parent). Git is the undo.
pub fn remove(dir: &Path, note: &str) -> Result<Outcome, VaultError> {
    let s = Session::open(dir)?;
    let i = s.resolve(note)?;
    let gone = s.vault.notes[i].path.clone();
    let file = file_of(s.root(), &gone);
    std::fs::remove_file(&file).map_err(io(&file))?;
    let mut out = Outcome::default();
    out.touched.deleted.push(gone.clone());
    out.messages.push(format!("deleted {gone}"));
    let (mut links, mut orphans) = (0, 0);
    for (j, n) in s.vault.notes.iter().enumerate() {
        if j == i {
            continue;
        }
        let (body, count) = rewrite_links(&n.body, |l| {
            (s.graph.resolver.resolve(&l.parsed.target) == Resolved::Note(i))
                .then(|| l.parsed.label.clone())
        });
        let orphaned = s.graph.parents[j] == Some(i);
        if count == 0 && !orphaned {
            continue;
        }
        let parent = if orphaned { None } else { s.parent_of(j) };
        let raw = compose_raw(&n.title, parent.as_deref(), &body);
        if write_note(s.root(), &n.path, &raw)? {
            out.touched.modified.push(n.path.clone());
        }
        links += count;
        orphans += usize::from(orphaned);
    }
    if links > 0 || orphans > 0 {
        out.messages.push(format!(
            "{links} link(s) turned to text; {orphans} child note(s) now have parent null"
        ));
    }
    after_set_change(s.root(), &mut out)?;
    Ok(out)
}

/// `trellis vault init`: a directory with an empty manifest and layout.
pub fn init(dir: &Path) -> Result<Outcome, VaultError> {
    std::fs::create_dir_all(dir).map_err(io(dir))?;
    for f in ["manifest.json", "layout.json"] {
        if dir.join(f).exists() {
            return Err(refused(format!(
                "{} already exists; not a new vault",
                dir.join(f).display()
            )));
        }
    }
    for (f, body) in [("manifest.json", "[]\n"), ("layout.json", "{}\n")] {
        let p = dir.join(f);
        std::fs::write(&p, body).map_err(io(&p))?;
    }
    let mut out = Outcome::default();
    out.messages.push(format!(
        "initialised {} (manifest.json, layout.json)",
        dir.display()
    ));
    Ok(out)
}

/// Apex's size warnings, at its own `>=` thresholds.
const WARN_NOTES: usize = 1000;
const WARN_TOTAL: u64 = 50 * 1024 * 1024;
const WARN_NOTE: u64 = 2 * 1024 * 1024;

/// What `trellis vault check` reports.
#[derive(Debug)]
pub struct Report {
    /// SKILL.md's line: `notes=N parentless=N connected=N manifest_missing=N
    /// manifest_extra=N fixes=[...]`.
    pub line: String,
    pub issues: Vec<Issue>,
    pub warnings: Vec<String>,
    /// The run gate's answer: its counts, or every violation.
    pub gate: Result<Stats, Vec<String>>,
    pub touched: Touched,
}

impl Report {
    /// Whether `trellis run` would accept this vault.
    pub fn gate_ok(&self) -> bool {
        self.gate.is_ok()
    }
}

/// Rewrites the notes the gate rejects for their frontmatter, then the
/// manifest and layout if wrong. Nothing already correct is touched: a
/// check that dirtied a clean vault would dirty the worktree a run needs
/// clean (ADR 0022 §1).
fn fix(dir: &Path) -> Result<(Vec<String>, Touched), VaultError> {
    let s = Session::open(dir)?;
    let mut fixes = Vec::new();
    let mut touched = Touched::default();
    for (i, n) in s.vault.notes.iter().enumerate() {
        if validate::parse_note(&n.path, &n.raw, &mut Vec::new()).is_some() {
            continue;
        }
        let raw = compose_raw(&n.title, s.parent_of(i).as_deref(), &n.body);
        if write_note(s.root(), &n.path, &raw)? {
            fixes.push(format!("frontmatter:{}", n.path));
            touched.modified.push(n.path.clone());
        }
    }
    if sync_manifest(s.root())? {
        fixes.push("manifest.json".into());
    }
    if let Some(Ok(n)) = prune_layout(s.root())?
        && n > 0
    {
        fixes.push("layout.json".into());
    }
    Ok((fixes, touched))
}

/// `trellis vault check [--fix]`.
pub fn check(dir: &Path, apply_fixes: bool) -> Result<Report, VaultError> {
    let (fixes, touched) = if apply_fixes {
        fix(dir)?
    } else {
        (Vec::new(), Touched::default())
    };
    let s = Session::open(dir)?;
    let paths: Vec<String> = s.vault.notes.iter().map(|n| n.path.clone()).collect();
    let manifest = validate::manifest_state(s.root(), &paths, &mut Vec::new())?;
    let (parentless, connected) = s.graph.counts(&s.vault);
    let line = format!(
        "notes={} parentless={parentless} connected={connected} manifest_missing={} manifest_extra={} fixes=[{}]",
        s.vault.notes.len(),
        manifest.missing,
        manifest.extra,
        fixes.join(",")
    );

    let mut warnings = Vec::new();
    let total: u64 = s.vault.notes.iter().map(|n| n.bytes).sum();
    if s.vault.notes.len() >= WARN_NOTES {
        warnings.push(format!(
            "large vault: {} notes (Apex warns at {WARN_NOTES})",
            s.vault.notes.len()
        ));
    }
    if total >= WARN_TOTAL {
        warnings.push(format!(
            "large vault: {total} bytes of notes (Apex warns at 50 MiB)"
        ));
    }
    for n in &s.vault.notes {
        if n.bytes >= WARN_NOTE {
            warnings.push(format!(
                "large note: {} is {} bytes (Apex warns at 2 MiB)",
                n.path, n.bytes
            ));
        }
    }

    let gate = match validate::validate(dir) {
        Ok(stats) => Ok(stats),
        Err(VaultError::Invalid { violations, .. }) => Err(violations),
        Err(e) => return Err(e),
    };
    Ok(Report {
        line,
        issues: s.graph.issues.clone(),
        warnings,
        gate,
        touched,
    })
}
