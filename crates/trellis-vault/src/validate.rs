//! The format rules, checked all at once.

use crate::VaultError;
use crate::js;
use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};

/// One note, parsed.
#[derive(Debug)]
pub(crate) struct Note {
    /// Relative to the vault, `/`-separated on every platform, so a path in
    /// the compiled text does not depend on where it was compiled.
    pub(crate) path: String,
    pub(crate) stem: String,
    pub(crate) title: String,
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
    /// Stem to indices into `notes`, in path order.
    pub(crate) by_stem: BTreeMap<String, Vec<usize>>,
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

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> VaultError + '_ {
    move |source| VaultError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Reads as UTF-8 with replacement, as Node's `readFileSync(_, "utf8")` does.
fn read_text(path: &Path) -> Result<String, VaultError> {
    let bytes = std::fs::read(path).map_err(io(path))?;
    Ok(js::normalize(&String::from_utf8_lossy(&bytes)))
}

/// Every `.md` file under `dir`, relative and `/`-separated.
///
/// `file_type()` does not follow symlinks, which matches Node's
/// `readdir(withFileTypes)`: a symlinked note or directory is not walked. A
/// link out of the vault would otherwise put a file in front of the model
/// that no manifest entry could name.
fn walk(dir: &Path, rel: &str, out: &mut Vec<String>) -> Result<(), VaultError> {
    for entry in std::fs::read_dir(dir).map_err(io(dir))? {
        let entry = entry.map_err(io(dir))?;
        let ft = entry.file_type().map_err(io(&entry.path()))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        let child = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if ft.is_dir() {
            walk(&entry.path(), &child, out)?;
        } else if ft.is_file() && name.ends_with(".md") {
            out.push(child);
        }
    }
    Ok(())
}

/// Node's `path.basename(p, ".md")`: the suffix is kept when it is the whole
/// name.
fn stem_of(rel: &str) -> String {
    let name = rel.rsplit('/').next().unwrap_or(rel);
    match name.strip_suffix(".md") {
        Some(s) if !s.is_empty() => s.to_string(),
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

fn parse_note(rel: &str, text: &str, violations: &mut Vec<String>) -> Option<Note> {
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
    let (mut title, raw_parent) = (title?, parent?);

    // A quoted title is a JSON string, so `"A \"B\""` means what it says. One
    // that does not parse keeps its inner text rather than failing the vault.
    if title.len() >= 2 && title.starts_with('"') && title.ends_with('"') {
        title = serde_json::from_str::<String>(&title)
            .unwrap_or_else(|_| title[1..title.len() - 1].to_string());
    }

    let parent = if raw_parent == "null" {
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

pub(crate) fn parse(root: &Path) -> Result<Parsed, VaultError> {
    let mut violations = Vec::new();
    if !root.is_dir() {
        violations.push(format!("{}: not a directory", root.display()));
        return Ok(Parsed {
            notes: Vec::new(),
            by_stem: BTreeMap::new(),
            violations,
            stats: Stats::default(),
        });
    }

    let mut rel_paths = Vec::new();
    walk(root, "", &mut rel_paths)?;
    // Sorted before parsing, not only after, so violations are reported in an
    // order that does not depend on the filesystem's directory order.
    rel_paths.sort_by(|a, b| js::cmp(a, b));

    let mut notes = Vec::new();
    for rel in &rel_paths {
        let file: PathBuf = rel.split('/').fold(root.to_path_buf(), |p, s| p.join(s));
        let text = read_text(&file)?;
        if let Some(n) = parse_note(rel, &text, &mut violations) {
            notes.push(n);
        }
    }

    let mut by_stem: BTreeMap<String, Vec<usize>> = BTreeMap::new();
    for (i, n) in notes.iter().enumerate() {
        by_stem.entry(n.stem.clone()).or_default().push(i);
    }
    let lookup = |stem: &str| by_stem.get(stem).map(Vec::as_slice).unwrap_or(&[]);

    // Every parent resolves to exactly one note.
    let mut has_child: HashSet<usize> = HashSet::new();
    for n in &notes {
        let Some(parent) = &n.parent else { continue };
        match lookup(parent) {
            [] => violations.push(format!(
                "{}: parent [[{parent}]] does not resolve to any note",
                n.path
            )),
            [one] => {
                has_child.insert(*one);
            }
            many => violations.push(format!(
                "{}: parent [[{parent}]] is ambiguous: {}",
                n.path,
                many.iter()
                    .map(|&i| notes[i].path.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            )),
        }
    }

    // Cycles: follow unique parents, and report each cycle once, keyed by its
    // sorted member set — every note on a cycle would otherwise report it.
    let mut reported: HashSet<String> = HashSet::new();
    for start in 0..notes.len() {
        let mut seen: Vec<usize> = Vec::new();
        let mut cur = Some(start);
        while let Some(c) = cur {
            let Some(parent) = &notes[c].parent else {
                break;
            };
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
            cur = match lookup(parent) {
                [one] => Some(*one),
                _ => None,
            };
        }
    }

    let (manifest_missing, manifest_extra) = check_manifest(root, &rel_paths, &mut violations)?;

    let stats = Stats {
        notes: notes.len(),
        parentless: notes.iter().filter(|n| n.parent.is_none()).count(),
        connected: (0..notes.len())
            .filter(|&i| notes[i].parent.is_some() || has_child.contains(&i))
            .count(),
        manifest_missing,
        manifest_extra,
    };
    Ok(Parsed {
        notes,
        by_stem,
        violations,
        stats,
    })
}

/// The manifest must be the sorted set of `.md` paths on disk.
///
/// Returns `(missing, extra)`. A manifest that is absent counts every note as
/// missing; one that is not a string array counts nothing, because there is
/// no set to compare.
fn check_manifest(
    root: &Path,
    rel_paths: &[String],
    violations: &mut Vec<String>,
) -> Result<(usize, usize), VaultError> {
    let path = root.join("manifest.json");
    if !path.exists() {
        violations.push("manifest.json: missing".to_string());
        return Ok((rel_paths.len(), 0));
    }
    let text = read_text(&path)?;
    let value: serde_json::Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            violations.push(format!("manifest.json: invalid JSON ({e})"));
            return Ok((0, 0));
        }
    };
    let entries: Option<Vec<&str>> = value
        .as_array()
        .and_then(|a| a.iter().map(serde_json::Value::as_str).collect());
    let Some(entries) = entries else {
        violations.push("manifest.json: must be an array of strings".to_string());
        return Ok((0, 0));
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
        if !e.ends_with(".md") {
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
    Ok((missing, extra))
}
