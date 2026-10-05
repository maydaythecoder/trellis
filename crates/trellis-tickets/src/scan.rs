//! `trellis-tickets scan [<path>]` (ADR 0040 decision 4, #80).
//!
//! Finds G4's stub markers in Rust source and proposes an `origin:code`
//! ticket for each, printed as `file:line`. It files nothing: this module
//! has no way to reach GitHub, and `scan` takes no closure that could.
//!
//! The markers are G4's own: the pattern in `scripts/gates/no-stubs.sh` is
//! compiled into the binary and read from there, so the two cannot drift
//! apart. So are G4's exclusions: only `.rs` files, none under a `tests/`,
//! `examples/`, `benches/` or `fixtures/` directory or named `*_test.rs`,
//! and no line at or below a file's first column-0 `#[cfg(test)]`.
//!
//! G4 reads the lines a change added (a git diff); `scan` reads the whole
//! tree, and does not spawn git to do it (ADR 0040 decision 6). A diff
//! never shows `target/` or a dot-directory, so the walk skips those too,
//! and it does not follow symbolic links.

use std::path::{Path, PathBuf};

/// G4, as committed.
const NO_STUBS_SH: &str = include_str!("../../../scripts/gates/no-stubs.sh");

/// `scan` could not read G4's markers or the tree.
#[derive(Debug, thiserror::Error)]
pub enum ScanError {
    #[error("scripts/gates/no-stubs.sh: {0}")]
    Markers(String),
    #[error("reading {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{}: not a directory or a `.rs` file", path.display())]
    NotRust { path: PathBuf },
}

/// One marker found: a proposed `origin:code` ticket.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Proposal {
    /// Relative to the scanned root, `/`-separated.
    pub path: String,
    /// 1-based.
    pub line: usize,
    /// The first of G4's markers the line contains.
    pub marker: String,
    /// The line, trimmed.
    pub text: String,
}

impl Proposal {
    /// `file:line: proposed origin:code ticket (marker `m`): text`. The line
    /// comes from a file anyone can write, so it is escaped.
    pub fn text(&self) -> String {
        format!(
            "{}:{}: proposed origin:code ticket (marker `{}`): {}\n",
            crate::printable(&self.path),
            self.line,
            crate::printable(&self.marker),
            crate::printable(&self.text)
        )
    }
}

/// G4's markers: the alternatives of the `grep -E '...'` pattern in
/// `scripts/gates/no-stubs.sh`, each as the literal text it matches.
///
/// An alternative that is not a literal (an unescaped `.`, `*`, `[` and so
/// on) is refused rather than matched as text: it would match lines G4
/// does not, or miss lines G4 catches.
pub fn g4_markers() -> Result<Vec<String>, ScanError> {
    markers_from(NO_STUBS_SH)
}

pub(crate) fn markers_from(script: &str) -> Result<Vec<String>, ScanError> {
    const OPEN: &str = "grep -E '";
    let mut found = script.lines().filter_map(|l| {
        let at = l.find(OPEN)?;
        let rest = &l[at + OPEN.len()..];
        rest.find('\'').map(|end| &rest[..end])
    });
    let pattern = found
        .next()
        .ok_or_else(|| ScanError::Markers("no `grep -E '...'` pattern".to_string()))?;
    if found.next().is_some() {
        return Err(ScanError::Markers(
            "more than one `grep -E` pattern; which one is G4's is not known".to_string(),
        ));
    }
    let mut out = Vec::new();
    for alt in pattern.split('|') {
        let mut lit = String::new();
        let mut chars = alt.chars();
        while let Some(c) = chars.next() {
            match c {
                '\\' => match chars.next() {
                    Some(e) if ".[]{}()*+?^$|\\".contains(e) => lit.push(e),
                    _ => {
                        return Err(ScanError::Markers(format!(
                            "`{alt}` has an escape that is not a literal character"
                        )));
                    }
                },
                '.' | '[' | ']' | '{' | '}' | '(' | ')' | '*' | '+' | '?' | '^' | '$' => {
                    return Err(ScanError::Markers(format!(
                        "`{alt}` is a pattern, not a literal"
                    )));
                }
                _ => lit.push(c),
            }
        }
        if lit.is_empty() {
            return Err(ScanError::Markers(
                "an empty alternative would match every line".to_string(),
            ));
        }
        out.push(lit);
    }
    Ok(out)
}

/// G4's `(^|/)(tests|examples|benches|fixtures)/|_test\.rs$`, over a
/// `/`-separated path.
fn excluded(rel: &str) -> bool {
    let mut parts: Vec<&str> = rel.split('/').collect();
    let file = parts.pop().unwrap_or("");
    file.ends_with("_test.rs")
        || parts
            .iter()
            .any(|d| matches!(*d, "tests" | "examples" | "benches" | "fixtures"))
}

/// Every `.rs` file under `dir`, sorted, as `(relative path, full path)`.
fn rust_files(root: &Path, dir: &Path, out: &mut Vec<(String, PathBuf)>) -> Result<(), ScanError> {
    let io = |source| ScanError::Io {
        path: dir.to_path_buf(),
        source,
    };
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .map_err(io)?
        .collect::<Result<_, _>>()
        .map_err(io)?;
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let name = e.file_name();
        let name = name.to_string_lossy();
        // `DirEntry::file_type` does not follow a symbolic link, so a link
        // is neither walked into nor read.
        let kind = e.file_type().map_err(|source| ScanError::Io {
            path: e.path(),
            source,
        })?;
        if kind.is_dir() {
            if name.starts_with('.') || name == "target" {
                continue;
            }
            rust_files(root, &e.path(), out)?;
        } else if kind.is_file() && name.ends_with(".rs") {
            let path = e.path();
            let rel = path
                .strip_prefix(root)
                .unwrap_or(&path)
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.push((rel, path));
        }
    }
    Ok(())
}

/// The proposals in one file's text, above its first column-0
/// `#[cfg(test)]`.
fn scan_text(rel: &str, text: &str, markers: &[String]) -> Vec<Proposal> {
    text.lines()
        .take_while(|l| !l.starts_with("#[cfg(test)]"))
        .enumerate()
        .filter_map(|(i, l)| {
            let m = markers.iter().find(|m| l.contains(m.as_str()))?;
            Some(Proposal {
                path: rel.to_string(),
                line: i + 1,
                marker: m.clone(),
                text: l.trim().to_string(),
            })
        })
        .collect()
}

/// Every proposal under `root`, a directory or one `.rs` file, in path
/// then line order. Reads files; writes nothing and reaches nothing else.
pub fn scan(root: &Path) -> Result<Vec<Proposal>, ScanError> {
    let markers = g4_markers()?;
    let mut files = Vec::new();
    let meta = std::fs::metadata(root).map_err(|source| ScanError::Io {
        path: root.to_path_buf(),
        source,
    })?;
    if meta.is_dir() {
        rust_files(root, root, &mut files)?;
    } else if root.extension().is_some_and(|e| e == "rs") {
        let name = root.file_name().map_or_else(
            || root.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        );
        files.push((name, root.to_path_buf()));
    } else {
        return Err(ScanError::NotRust {
            path: root.to_path_buf(),
        });
    }
    let mut out = Vec::new();
    for (rel, path) in files {
        // The whole path as given, not `rel`: a root that is itself under
        // `tests/` (or is one file in it) is excluded as G4 would exclude it.
        let given = path
            .components()
            .map(|c| c.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/");
        if excluded(&given) {
            continue;
        }
        let bytes = std::fs::read(&path).map_err(|source| ScanError::Io {
            path: path.clone(),
            source,
        })?;
        // The markers are ASCII, so a stray non-UTF-8 byte elsewhere in the
        // file changes no match; G4's grep reads bytes the same way.
        out.extend(scan_text(&rel, &String::from_utf8_lossy(&bytes), &markers));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pattern_alternative_is_refused_not_matched_as_text() {
        assert!(markers_from("x | grep -E 'TODO|a.b' || true").is_err());
        assert!(markers_from("no pattern here").is_err());
        assert_eq!(
            markers_from(r#"grep -E 'TODO|todo!\(' "#).unwrap(),
            ["TODO", "todo!("]
        );
    }

    #[test]
    fn exclusions_are_directory_components_and_the_test_suffix() {
        assert!(excluded("crates/a/tests/x.rs"));
        assert!(excluded("examples/x.rs"));
        assert!(excluded("src/parse_test.rs"));
        assert!(!excluded("src/testsuite.rs"));
        assert!(!excluded("src/tests.rs"));
    }

    fn tree(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!(
            "trellis-tickets-scan-unit-{name}-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        for rel in ["crates/a/tests/it.rs", "crates/a/src/lib.rs"] {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(&p, "// FIXME later\n").unwrap();
        }
        std::fs::write(root.join("notes.txt"), "// FIXME later\n").unwrap();
        root
    }

    #[test]
    fn scan_excludes_a_root_that_is_a_tests_directory_or_a_file_in_one() {
        // Relative to the root, `tests/it.rs` is just `it.rs`; G4 would
        // still skip it, so scan must not propose it.
        let root = tree("rooted");
        assert!(scan(&root.join("crates/a/tests")).unwrap().is_empty());
        assert!(scan(&root.join("crates/a/tests/it.rs")).unwrap().is_empty());
        // The same marker in production source is still found.
        assert_eq!(scan(&root.join("crates/a/src/lib.rs")).unwrap().len(), 1);
        assert_eq!(scan(&root.join("crates/a")).unwrap().len(), 1);
        std::fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn scan_refuses_a_root_that_is_a_file_but_not_rust() {
        // `0 proposal(s)` for `scan notes.txt` would read as a clean scan.
        let root = tree("notrust");
        let got = scan(&root.join("notes.txt"));
        assert!(matches!(got, Err(ScanError::NotRust { .. })), "{got:?}");
        std::fs::remove_dir_all(&root).unwrap();
    }
}
