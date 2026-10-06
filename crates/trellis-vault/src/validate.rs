//! The run gate: the format rules of ADR 0017, checked all at once.
//!
//! Strict about the schema — two keys, a canonical parent form, a correct
//! manifest — and lenient about everything ADR 0022 §1 made additive: the
//! `notes/` root, dotfiles, the null forms, and resolution through Apex's
//! aliases. Every message is the one ADR 0017 shipped.

use crate::VaultError;
use crate::graph::{Resolved, Resolver};
use crate::js;
use crate::reader::{
    checked_notes_root, file_of, is_null_parent, note_paths, read_text, refuse_link, unquote,
};
use crate::refs::{has_md_ext, strip_md};
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::Path;

/// One note, parsed.
#[derive(Debug)]
pub(crate) struct Note {
    /// Relative to the vault, `/`-separated on every platform, so a path in
    /// the compiled text does not depend on where it was compiled.
    pub(crate) path: String,
    pub(crate) stem: String,
    pub(crate) title: String,
    /// The parent's reference, e.g. `root` for `"[[root]]"`.
    pub(crate) parent: Option<String>,
    /// LF-normalised.
    pub(crate) body: String,
}

/// Counts over a vault, reported whether or not it is valid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    pub notes: usize,
    pub parentless: usize,
    /// Has a parent or a child. A note that is neither is reachable only by
    /// link, which is legal but worth seeing.
    pub connected: usize,
    pub manifest_missing: usize,
    pub manifest_extra: usize,
}

impl fmt::Display for Stats {
    /// The TypeScript twin's `formatStats`, byte for byte.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "notes={} parentless={} connected={} manifest_missing={} manifest_extra={}",
            self.notes, self.parentless, self.connected, self.manifest_missing, self.manifest_extra
        )
    }
}

/// A parsed vault, valid or not.
pub(crate) struct Parsed {
    /// Sorted by path.
    pub(crate) notes: Vec<Note>,
    /// Stem to indices into `notes`, in path order. The focus of a run
    /// resolves here and only here (ADR 0022 §1).
    pub(crate) by_stem: BTreeMap<String, Vec<usize>>,
    /// Parents and body links resolve here, as Apex resolves them.
    pub(crate) resolver: Resolver,
    /// Resolved parent per note; `None` for a root or an unresolved parent.
    pub(crate) parents: Vec<Option<usize>>,
    pub(crate) violations: Vec<String>,
    pub(crate) stats: Stats,
}

impl Parsed {
    pub(crate) fn lookup(&self, stem: &str) -> &[usize] {
        self.by_stem.get(stem).map(Vec::as_slice).unwrap_or(&[])
    }
}

/// Checks a vault and returns its counts.
///
/// Every violation is collected before returning. An I/O failure while
/// reading a file that exists is an error of its own, not a violation: the
/// vault may be fine and the disk not.
pub fn validate(root: &Path) -> Result<Stats, VaultError> {
    let p = parse(root)?;
    if p.violations.is_empty() {
        Ok(p.stats)
    } else {
        Err(VaultError::Invalid {
            violations: p.violations,
            stats: p.stats,
        })
    }
}

/// Node's `path.basename(p, ".md")`: the suffix is kept when it is the whole
/// name. Case-insensitive, as the walk is.
fn stem_of(rel: &str) -> String {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    match strip_md(name) {
        s if !s.is_empty() => s.to_string(),
        _ => name.to_string(),
    }
}

/// `^([^:]+):\s*(.*)$` on one line, without the `m` or `s` flags.
fn split_field(line: &str) -> Option<(&str, &str)> {
    let colon = line.find(':')?;
    if colon == 0 {
        return None;
    }
    let value = line[colon + 1..].trim_start_matches(js::is_ws);
    // `.` does not match a line terminator, and `$` is end of input.
    if value.chars().any(js::is_line_terminator) {
        return None;
    }
    Some((&line[..colon], value))
}

/// `^"\[\[([^\]]+)\]\]"$`, returning the captured stem.
fn parent_link(raw: &str) -> Option<&str> {
    let inner = raw.strip_prefix("\"[[")?.strip_suffix("]]\"")?;
    if inner.is_empty() || inner.contains(']') {
        return None;
    }
    Some(inner)
}

/// The per-note schema check. `None` means the note fails it, with every
/// reason pushed to `violations`. `check --fix` rewrites exactly the notes
/// this rejects.
pub(crate) fn parse_note(rel: &str, text: &str, violations: &mut Vec<String>) -> Option<Note> {
    let lines: Vec<&str> = text.split('\n').collect();
    if lines[0] != "---" {
        violations.push(format!("{rel}: missing opening frontmatter delimiter"));
        return None;
    }
    let Some(close) = lines
        .iter()
        .skip(1)
        .position(|l| *l == "---")
        .map(|i| i + 1)
    else {
        violations.push(format!("{rel}: missing closing frontmatter delimiter"));
        return None;
    };

    let mut title: Option<String> = None;
    let mut parent: Option<String> = None;
    let mut ok = true;
    for line in &lines[1..close] {
        let Some((key, value)) = split_field(line) else {
            violations.push(format!(
                "{rel}: unparseable frontmatter line: {}",
                serde_json::to_string(line).unwrap_or_default()
            ));
            ok = false;
            continue;
        };
        let key = js::trim(key);
        let slot = match key {
            "title" => &mut title,
            "parent" => &mut parent,
            _ => {
                violations.push(format!("{rel}: unexpected frontmatter key \"{key}\""));
                ok = false;
                continue;
            }
        };
        if slot.is_some() {
            violations.push(format!("{rel}: duplicate frontmatter key \"{key}\""));
            ok = false;
            continue;
        }
        *slot = Some(js::trim(value).to_string());
    }
    for (key, v) in [("title", &title), ("parent", &parent)] {
        if v.is_none() {
            violations.push(format!("{rel}: missing frontmatter key \"{key}\""));
            ok = false;
        }
    }
    if !ok {
        return None;
    }
    let (title, raw_parent) = (title?, parent?);
    // A quoted title is a JSON string, so `"A \"B\""` means what it says. One
    // that does not parse keeps its inner text rather than failing the vault.
    let title = if title.len() >= 2 && title.starts_with('"') && title.ends_with('"') {
        unquote(&title)
    } else {
        title
    };

    // Apex's null forms are values, not keys, so the gate accepts them all.
    let parent = if is_null_parent(&unquote(&raw_parent)) {
        None
    } else {
        match parent_link(&raw_parent) {
            Some(s) => Some(js::trim(s).to_string()),
            None => {
                violations.push(format!(
                    "{rel}: parent must be null or \"[[stem]]\", got {raw_parent}"
                ));
                return None;
            }
        }
    };

    Some(Note {
        path: rel.to_string(),
        stem: stem_of(rel),
        title,
        parent,
        body: lines[close + 1..].join("\n"),
    })
}

/// `/^[A-Za-z]:/`
fn has_drive(e: &str) -> bool {
    let b = e.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

pub(crate) fn parse(dir: &Path) -> Result<Parsed, VaultError> {
    let mut violations = Vec::new();
    if !dir.is_dir() {
        violations.push(format!("{}: not a directory", dir.display()));
        return Ok(Parsed {
            notes: Vec::new(),
            by_stem: BTreeMap::new(),
            resolver: Resolver::new([]),
            parents: Vec::new(),
            violations,
            stats: Stats::default(),
        });
    }
    let root = checked_notes_root(dir)?;

    // Sorted before parsing, not only after, so violations are reported in an
    // order that does not depend on the filesystem's directory order.
    let rel_paths = note_paths(&root)?;

    let mut notes = Vec::new();
    for rel in &rel_paths {
        let text = read_text(&file_of(&root, rel))?;
        if let Some(n) = parse_note(rel, &text, &mut violations) {
            notes.push(n);
        }
    }

    let mut by_stem: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, n) in notes.iter().enumerate() {
        by_stem.entry(n.stem.clone()).or_default().push(i);
    }
    let resolver = Resolver::new(notes.iter().map(|n| (n.path.as_str(), n.title.as_str())));

    // Every parent resolves to exactly one note.
    let mut parents = vec![None; notes.len()];
    let mut has_child: HashSet<usize> = HashSet::new();
    for (i, n) in notes.iter().enumerate() {
        let Some(parent) = &n.parent else { continue };
        match resolver.resolve(parent) {
            Resolved::Missing => violations.push(format!(
                "{}: parent [[{parent}]] does not resolve to any note",
                n.path
            )),
            Resolved::Note(one) => {
                parents[i] = Some(one);
                has_child.insert(one);
            }
            Resolved::Ambiguous(many) => violations.push(format!(
                "{}: parent [[{parent}]] is ambiguous: {}",
                n.path,
                many.iter()
                    .map(|&i| notes[i].path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    // Cycles: follow resolved parents, and report each cycle once, keyed by
    // its sorted member set — every note on a cycle would otherwise report it.
    let mut reported: HashSet<String> = HashSet::new();
    for start in 0..notes.len() {
        let mut seen: Vec<usize> = Vec::new();
        let mut cur = Some(start);
        while let Some(c) = cur {
            if notes[c].parent.is_none() {
                break;
            }
            if let Some(idx) = seen.iter().position(|&s| s == c) {
                let mut members: Vec<&str> = seen[idx..]
                    .iter()
                    .map(|&m| notes[m].path.as_str())
                    .collect();
                members.sort_by(|a, b| js::cmp(a, b));
                if reported.insert(members.join("|")) {
                    violations.push(format!("parent cycle: {}", members.join(" -> ")));
                }
                break;
            }
            seen.push(c);
            cur = parents[c];
        }
    }

    let m = manifest_state(&root, &rel_paths, &mut violations)?;

    let stats = Stats {
        notes: notes.len(),
        parentless: notes.iter().filter(|n| n.parent.is_none()).count(),
        connected: (0..notes.len())
            .filter(|&i| notes[i].parent.is_some() || has_child.contains(&i))
            .count(),
        manifest_missing: m.missing,
        manifest_extra: m.extra,
    };
    Ok(Parsed {
        notes,
        by_stem,
        resolver,
        parents,
        violations,
        stats,
    })
}

/// What the manifest says, compared with the files on disk.
pub(crate) struct ManifestState {
    pub(crate) missing: usize,
    pub(crate) extra: usize,
    /// Anything wrong at all: absent, unparseable, unsorted, a duplicate, a
    /// bad entry, or a set that differs from disk. `--fix` rewrites it then.
    pub(crate) wrong: bool,
}

/// The manifest must be the sorted set of `.md` paths on disk.
///
/// A manifest that is absent counts every note as missing; one that is not a
/// string array counts nothing, because there is no set to compare.
pub(crate) fn manifest_state(
    root: &Path,
    rel_paths: &[String],
    violations: &mut Vec<String>,
) -> Result<ManifestState, VaultError> {
    let before = violations.len();
    let path = root.join("manifest.json");
    // An error, not a violation: `sync_manifest` discards the violations and
    // would write through the link. Before `exists`, which follows a link,
    // so a dangling one is not "missing" either.
    refuse_link(&path, "workspace metadata")?;
    if !path.exists() {
        violations.push("manifest.json: missing".to_string());
        return Ok(ManifestState {
            missing: rel_paths.len(),
            extra: 0,
            wrong: true,
        });
    }
    let text = read_text(&path)?;
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            violations.push(format!("manifest.json: invalid JSON ({e})"));
            return Ok(ManifestState {
                missing: 0,
                extra: 0,
                wrong: true,
            });
        }
    };
    let entries: Option<Vec<&str>> = value
        .as_array()
        .and_then(|a| a.iter().map(serde_json::Value::as_str).collect());
    let Some(entries) = entries else {
        violations.push("manifest.json: must be an array of strings".to_string());
        return Ok(ManifestState {
            missing: 0,
            extra: 0,
            wrong: true,
        });
    };

    let mut sorted = entries.clone();
    sorted.sort_by(|a, b| js::cmp(a, b));
    if entries != sorted {
        violations.push("manifest.json: entries are not sorted".to_string());
    }

    // Insertion order kept, so "lists X, which does not exist" comes out in
    // manifest order as it does in the twin.
    let mut seen: Vec<&str> = Vec::new();
    for &e in &entries {
        if seen.contains(&e) {
            violations.push(format!("manifest.json: duplicate entry {e}"));
        } else {
            seen.push(e);
        }
        if !has_md_ext(e) {
            violations.push(format!("manifest.json: entry is not a .md path: {e}"));
        }
        if e.contains('\\') {
            violations.push(format!("manifest.json: entry uses backslashes: {e}"));
        }
        if e.starts_with('/') || has_drive(e) || e.split('/').any(|s| s == "..") {
            violations.push(format!(
                "manifest.json: entry is not a relative path inside the vault: {e}"
            ));
        }
    }

    let mut missing = 0;
    let mut extra = 0;
    for p in rel_paths {
        if !seen.contains(&p.as_str()) {
            missing += 1;
            violations.push(format!("manifest.json: missing {p}"));
        }
    }
    for e in &seen {
        if !rel_paths.iter().any(|p| p == e) {
            extra += 1;
            violations.push(format!("manifest.json: lists {e}, which does not exist"));
        }
    }
    Ok(ManifestState {
        missing,
        extra,
        wrong: violations.len() > before,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stem_keeps_md_when_it_is_the_whole_name() {
        // Node's `path.basename(".md", ".md")` is ".md", not "".
        assert_eq!(stem_of("a/.md"), ".md");
        assert_eq!(stem_of("a/b.MD"), "b");
    }

    #[test]
    fn a_parent_link_is_one_non_empty_bracket_free_stem() {
        assert_eq!(parent_link("\"[[a]]\""), Some("a"));
        assert_eq!(parent_link("\"[[]]\""), None);
        assert_eq!(parent_link("\"[[a]b]]\""), None);
    }

    #[test]
    fn the_gate_decodes_only_a_double_quoted_title() {
        // ADR 0017's parser reads a double-quoted title as a JSON string and
        // leaves every other form as written.
        let title = |t: &str| {
            let raw = format!("---\ntitle: {t}\nparent: null\n---\nbody");
            parse_note("a.md", &raw, &mut Vec::new()).unwrap().title
        };
        assert_eq!(title("\"A \\\"B\\\"\""), "A \"B\"");
        assert_eq!(title("'Single'"), "'Single'");
        assert_eq!(title("x\""), "x\"");
    }

    #[test]
    fn a_manifest_entry_outside_the_vault_is_named_for_each_form() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(
            d.path().join("manifest.json"),
            r#"["../a.md","/a.md","C:a.md"]"#,
        )
        .unwrap();
        let mut v = Vec::new();
        manifest_state(d.path(), &[], &mut v).unwrap();
        for e in ["../a.md", "/a.md", "C:a.md"] {
            let want = format!("manifest.json: entry is not a relative path inside the vault: {e}");
            assert!(v.contains(&want), "{e}: {v:?}");
        }
    }
}
