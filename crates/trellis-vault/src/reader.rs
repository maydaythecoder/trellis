//! Reading a vault the way Apex reads one: `noteParser.ts` for a note,
//! `lib.rs#walk_markdown_files` for the folder.
//!
//! Lenient where Apex is lenient: a missing title falls back, `~` is null,
//! and a stray key is kept rather than refused. The run gate's strictness is
//! layered on top in `validate.rs`, not built into this reader (ADR 0022
//! §1).

use crate::refs::{self, strip_md};
use crate::{VaultError, js};
use std::path::{Path, PathBuf};

/// The notes root: `<dir>/notes` when that is a directory, else `<dir>`.
pub fn notes_root(dir: &Path) -> PathBuf {
    let notes = dir.join("notes");
    // `symlink_metadata`, so a `notes` symlink is not followed out of the
    // vault; Apex's `is_dir` would follow it, and then its walk would refuse
    // every entry anyway.
    match std::fs::symlink_metadata(&notes) {
        Ok(m) if m.is_dir() => notes,
        _ => dir.to_path_buf(),
    }
}

pub(crate) fn io(path: &Path) -> impl FnOnce(std::io::Error) -> VaultError + '_ {
    move |source| VaultError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Every `.md` file under `dir`, relative and `/`-separated, unsorted.
///
/// Dotfiles and dot-directories are skipped, and so is every symlink: a link
/// out of the vault would put a file in front of the model that no manifest
/// entry could name. The extension matches case-insensitively, as Apex's
/// `is_markdown_path` does.
pub(crate) fn walk(dir: &Path, rel: &str, out: &mut Vec<String>) -> Result<(), VaultError> {
    for entry in std::fs::read_dir(dir).map_err(io(dir))? {
        let entry = entry.map_err(io(dir))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let ft = entry.file_type().map_err(io(&entry.path()))?;
        let child = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        if ft.is_dir() {
            walk(&entry.path(), &child, out)?;
        } else if ft.is_file() && refs::has_md_ext(&name) {
            out.push(child);
        }
    }
    Ok(())
}

/// Every note path under the root, in JavaScript string order.
pub(crate) fn note_paths(root: &Path) -> Result<Vec<String>, VaultError> {
    let mut paths = Vec::new();
    walk(root, "", &mut paths)?;
    paths.sort_by(|a, b| js::cmp(a, b));
    Ok(paths)
}

pub(crate) fn file_of(root: &Path, rel: &str) -> PathBuf {
    rel.split('/').fold(root.to_path_buf(), |p, s| p.join(s))
}

/// Reads as UTF-8 with replacement, BOM stripped, line endings LF.
pub(crate) fn read_text(path: &Path) -> Result<String, VaultError> {
    let bytes = std::fs::read(path).map_err(io(path))?;
    Ok(js::normalize(&String::from_utf8_lossy(&bytes)))
}

/// One note as Apex parses it.
#[derive(Debug, Clone)]
pub struct Note {
    pub path: String,
    /// `path` without `.md`.
    pub path_no_ext: String,
    pub basename: String,
    /// Frontmatter `title`, else the first `# H1`, else the basename.
    pub title: String,
    pub has_frontmatter: bool,
    pub has_title: bool,
    pub has_parent: bool,
    /// The `parent` value with null forms mapped to `None`, e.g. `[[root]]`.
    pub parent_ref: Option<String>,
    /// The file's text, BOM stripped and LF-normalised.
    pub raw: String,
    /// The text after the frontmatter with its leading blank lines removed:
    /// what Apex edits, hashes for `dates.json`, and rewrites.
    pub body: String,
    /// Bytes on disk, for the size warnings.
    pub bytes: u64,
}

impl Note {
    /// `cleanWikiRef(parentRef)`: the parent's target, or `None`.
    pub fn parent_target(&self) -> Option<String> {
        self.parent_ref.as_deref().and_then(refs::clean_wiki_ref)
    }
}

/// `splitMarkdown`: `(has_frontmatter, frontmatter_raw, body)`.
pub(crate) fn split_markdown(raw: &str) -> (bool, &str, &str) {
    if !raw.starts_with("---") {
        return (false, "", raw);
    }
    let Some(end) = raw.get(3..).and_then(|r| r.find("\n---")).map(|i| i + 3) else {
        return (false, "", raw);
    };
    let fm = &raw[3..end];
    let fm = fm.strip_prefix('\n').unwrap_or(fm);
    let fm = fm.trim_end_matches(js::is_ws);
    (true, fm, strip_leading_blank(&raw[end + 4..]))
}

/// `.replace(/^\s*\n/, "")`: the longest whitespace prefix that ends in a
/// newline.
fn strip_leading_blank(s: &str) -> &str {
    let ws = s.len() - s.trim_start_matches(js::is_ws).len();
    match s[..ws].rfind('\n') {
        Some(i) => &s[i + 1..],
        None => s,
    }
}

/// `^([A-Za-z0-9_-]+):\s*(.*)$` on one line.
pub(crate) fn key_line(line: &str) -> Option<(&str, &str)> {
    let colon = line.find(':')?;
    let key = &line[..colon];
    if key.is_empty()
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return None;
    }
    let value = line[colon + 1..].trim_start_matches(js::is_ws);
    if value.chars().any(js::is_line_terminator) {
        return None;
    }
    Some((key, value))
}

/// `stripQuotes`, with one difference: a double-quoted value is decoded as a
/// JSON string, as ADR 0017's parser does, so `"A \"B\""` — which is what
/// Apex's own `escapeYaml` writes — reads back as `A "B"`.
pub(crate) fn unquote(value: &str) -> String {
    let v = value;
    if v.len() >= 2 && v.starts_with('"') && v.ends_with('"') {
        return serde_json::from_str::<String>(v).unwrap_or_else(|_| v[1..v.len() - 1].to_string());
    }
    if v.len() >= 2 && v.starts_with('\'') && v.ends_with('\'') {
        return v[1..v.len() - 1].to_string();
    }
    v.to_string()
}

/// The null forms of `normalizeParentRef`.
pub(crate) fn is_null_parent(value: &str) -> bool {
    matches!(
        js::trim(value).to_lowercase().as_str(),
        "" | "null" | "undefined" | "~"
    )
}

/// `body.match(/^#\s+(.+)$/m)`, trimmed. `Some("")` when the first match
/// captures only whitespace, which Apex then treats as no heading.
fn first_heading(body: &str) -> Option<String> {
    let mut starts = vec![0];
    for (i, c) in body.char_indices() {
        if js::is_line_terminator(c) {
            starts.push(i + c.len_utf8());
        }
    }
    for s in starts {
        let Some(rest) = body[s..].strip_prefix('#') else {
            continue;
        };
        let ws: Vec<char> = rest.chars().take_while(|&c| js::is_ws(c)).collect();
        if ws.is_empty() {
            continue;
        }
        let ws_bytes: usize = ws.iter().map(|c| c.len_utf8()).sum();
        let after = &rest[ws_bytes..];
        if after
            .chars()
            .next()
            .is_some_and(|c| !js::is_line_terminator(c))
        {
            let line = after.split(js::is_line_terminator).next().unwrap_or("");
            return Some(js::trim(line).to_string());
        }
        // Greedy `\s+` overshot; backtracking can hand `.+` a trailing run of
        // non-terminator whitespace. Whatever it captures trims to "".
        let ok = (2..=ws.len()).any(|j| {
            !js::is_line_terminator(ws[j - 1]) && (j == ws.len() || js::is_line_terminator(ws[j]))
        });
        if ok {
            return Some(String::new());
        }
    }
    None
}

/// `noteParser.ts#parseNote` over normalised text.
pub(crate) fn parse_note(path: &str, raw: &str, bytes: u64) -> Note {
    let (has_fm, fm, body) = split_markdown(raw);
    let mut title_v: Option<String> = None;
    let mut parent_v: Option<String> = None;
    if !fm.is_empty() {
        for line in fm.split('\n') {
            if let Some((k, v)) = key_line(line) {
                // Later duplicates win, as assignment into `values` does.
                let v = unquote(js::trim(v));
                match k {
                    "title" => title_v = Some(v),
                    "parent" => parent_v = Some(v),
                    _ => {}
                }
            }
        }
    }
    let path_no_ext = strip_md(path).to_string();
    let basename = path_no_ext
        .rsplit('/')
        .next()
        .filter(|b| !b.is_empty())
        .unwrap_or(&path_no_ext)
        .to_string();
    let title = title_v
        .clone()
        .filter(|t| !t.is_empty())
        .or_else(|| first_heading(body).filter(|h| !h.is_empty()))
        .unwrap_or_else(|| basename.clone());
    let parent_ref = parent_v
        .as_deref()
        .filter(|p| !is_null_parent(p))
        .map(|p| js::trim(p).to_string());
    Note {
        path: path.to_string(),
        path_no_ext,
        basename,
        title,
        has_frontmatter: has_fm,
        has_title: title_v.is_some(),
        has_parent: parent_v.is_some(),
        parent_ref,
        raw: raw.to_string(),
        body: body.to_string(),
        bytes,
    }
}

/// A vault, read leniently.
#[derive(Debug, Clone)]
pub struct Vault {
    /// The notes root (after the `notes/` rule).
    pub root: PathBuf,
    /// In JavaScript string order of path.
    pub notes: Vec<Note>,
}

impl Vault {
    /// Reads every note under `dir`'s notes root. Fails only on I/O; a
    /// malformed note is still a note.
    pub fn open(dir: &Path) -> Result<Vault, VaultError> {
        let root = notes_root(dir);
        if !root.is_dir() {
            return Err(VaultError::Io {
                path: root.clone(),
                source: std::io::Error::new(std::io::ErrorKind::NotFound, "not a directory"),
            });
        }
        let mut notes = Vec::new();
        for rel in note_paths(&root)? {
            let file = file_of(&root, &rel);
            let bytes = std::fs::metadata(&file).map_err(io(&file))?.len();
            let text = read_text(&file)?;
            notes.push(parse_note(&rel, &text, bytes));
        }
        Ok(Vault { root, notes })
    }

    pub fn index_of(&self, path: &str) -> Option<usize> {
        self.notes.iter().position(|n| n.path == path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(raw: &str) -> Note {
        parse_note("dir/My Note.md", raw, raw.len() as u64)
    }

    #[test]
    fn title_falls_back_to_heading_then_basename() {
        // A note without a title still needs a name everywhere a note is
        // listed; Apex's fallback order is the one its users see.
        assert_eq!(p("---\ntitle: \"T\"\nparent: null\n---\n# H\n").title, "T");
        assert_eq!(
            p("---\nparent: null\n---\n\n# Heading one\n").title,
            "Heading one"
        );
        assert_eq!(p("no frontmatter\n").title, "My Note");
        assert_eq!(p("---\ntitle: \"\"\n---\n#   \n").title, "My Note");
    }

    #[test]
    fn null_forms_and_quotes() {
        for v in ["\"\"", "null", "undefined", "~", "NULL", "'~'"] {
            let n = p(&format!("---\ntitle: x\nparent: {v}\n---\n"));
            assert_eq!(n.parent_ref, None, "{v}");
            assert!(n.has_parent);
        }
        let n = p("---\ntitle: 'Single'\nparent: \"[[a]]\"\n---\n");
        assert_eq!(n.title, "Single");
        assert_eq!(n.parent_target().as_deref(), Some("a"));
        // What escapeYaml writes, read back as meant.
        assert_eq!(p("---\ntitle: \"A \\\"B\\\"\"\n---\n").title, "A \"B\"");
    }

    #[test]
    fn body_is_after_frontmatter_without_leading_blank_lines() {
        let n = p("---\ntitle: T\nparent: null\n---\n\n\nBody\n\nmore\n");
        assert_eq!(n.body, "Body\n\nmore\n");
        assert!(n.has_frontmatter);
        let n = p("---\ntitle: T\nno close\n");
        assert!(!n.has_frontmatter);
    }

    #[test]
    fn a_file_named_notes_is_not_the_notes_root() {
        let d = tempfile::tempdir().unwrap();
        std::fs::write(d.path().join("notes"), "").unwrap();
        assert_eq!(notes_root(d.path()), d.path());
        std::fs::remove_file(d.path().join("notes")).unwrap();
        std::fs::create_dir(d.path().join("notes")).unwrap();
        assert_eq!(notes_root(d.path()), d.path().join("notes"));
    }

    #[test]
    fn a_key_is_letters_digits_underscore_and_dash_only() {
        assert_eq!(key_line("a_b: x"), Some(("a_b", "x")));
        assert_eq!(key_line("a-b: x"), Some(("a-b", "x")));
        for bad in ["a.b: x", "a b: x", ": x"] {
            assert_eq!(key_line(bad), None, "{bad}");
        }
    }

    #[test]
    fn unquote_strips_only_a_matching_pair() {
        assert_eq!(unquote("x\""), "x\"");
        assert_eq!(unquote("x'"), "x'");
        assert_eq!(unquote("'x'"), "x");
        // Not valid JSON: the inner text is kept as written.
        assert_eq!(unquote("\"a\\qb\""), "a\\qb");
    }

    #[test]
    fn the_first_heading_may_be_on_a_later_line_or_only_whitespace() {
        assert_eq!(first_heading("x\n# Head").as_deref(), Some("Head"));
        // `#\s+(.+)$` backtracks so `.+` takes the last space: a heading
        // whose text is blank, which Apex then treats as none.
        assert_eq!(first_heading("#  ").as_deref(), Some(""));
        // A newline cannot be the `.+`.
        assert_eq!(first_heading("# \n"), None);
    }
}
