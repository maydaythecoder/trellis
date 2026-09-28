//! `grep`, `find` and `ls`: read-only views of the repository (ADR 0023).
//!
//! Git does the matching where it can. `git grep` rather than a regex crate,
//! so a pattern means what it means to every developer who has typed one; and
//! `git ls-files` rather than a directory walk, so what `find` lists is what
//! git considers part of the repository, ignored files excluded.

use crate::exec::{self, OUTPUT_CAP, Spec};
use crate::tools::ToolResult;
use std::path::Path;
use std::time::Duration;

/// How long a listing may take. Generous for a large repository and still
/// short of a budget's wall clock.
const LISTING_TIMEOUT: Duration = Duration::from_secs(60);

/// How many paths `find` and `ls` name before they say how many they left out.
const MAX_LISTED: usize = 500;

pub(crate) async fn grep(root: &Path, pattern: &str, path: Option<&str>) -> ToolResult {
    if pattern.is_empty() {
        return ToolResult::Error("grep needs a non-empty pattern".into());
    }
    let mut argv: Vec<String> = ["git", "grep", "-n", "-I", "--no-color", "--untracked", "-e"]
        .iter()
        .map(|s| s.to_string())
        .collect();
    // After `-e`, so a pattern that starts with `-` is a pattern and not an
    // option; the path after `--` for the same reason.
    argv.push(pattern.to_string());
    argv.push("--".into());
    if let Some(p) = path.filter(|p| !p.is_empty()) {
        argv.push(p.to_string());
    }
    let r = exec::run(Spec {
        argv: &argv,
        cwd: root,
        env: &[],
        stdin: None,
        timeout: LISTING_TIMEOUT,
        cap: OUTPUT_CAP,
        head: false,
    })
    .await;
    match r {
        Err(e) => ToolResult::Error(format!("grep could not run git: {e}")),
        Ok(c) if c.timed_out => ToolResult::Error("grep timed out".into()),
        // git grep: 0 found, 1 found nothing, anything else is a failure.
        Ok(c) if c.code == Some(0) => {
            // git does not search its own directory, and a line from one
            // would be a credential's way into the trail; belt and braces
            // for a nested `.GIT` on a case-insensitive filesystem. The path
            // ends at the first `:`, which a Windows path cannot contain.
            let out = c.stdout.render();
            let kept: Vec<&str> = out
                .lines()
                .filter(|l| {
                    !crate::orchestrator::names_git_dir(l.split_once(':').map_or(*l, |(p, _)| p))
                })
                .collect();
            if kept.is_empty() {
                ToolResult::Output(format!("no match for `{pattern}`"))
            } else {
                ToolResult::Output(kept.join("\n"))
            }
        }
        Ok(c) if c.code == Some(1) && c.stderr.is_empty() => {
            ToolResult::Output(format!("no match for `{pattern}`"))
        }
        Ok(c) => ToolResult::Error(format!("grep failed: {}", c.stderr.render().trim())),
    }
}

pub(crate) async fn find(root: &Path, glob: &str) -> ToolResult {
    if glob.is_empty() {
        return ToolResult::Error("find needs a glob, e.g. `**/*.py`".into());
    }
    let argv: Vec<String> = [
        "git",
        "ls-files",
        "-z",
        "--cached",
        "--others",
        "--exclude-standard",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    // Not capped at the tail: the list is filtered after reading, and a tail
    // of it would be an arbitrary subset. A file list is bounded by the
    // repository, so the cap here is generous.
    let r = exec::run(Spec {
        argv: &argv,
        cwd: root,
        env: &[],
        stdin: None,
        timeout: LISTING_TIMEOUT,
        cap: 64 << 20,
        head: false,
    })
    .await;
    let c = match r {
        Err(e) => return ToolResult::Error(format!("find could not run git: {e}")),
        Ok(c) if c.code != Some(0) => {
            return ToolResult::Error(format!(
                "find: git ls-files failed: {}",
                c.stderr.render().trim()
            ));
        }
        Ok(c) => c,
    };
    let listed = c.stdout.render();
    let pat = glob.replace('\\', "/");
    let pat = pat.trim_start_matches("./");
    let mut hits: Vec<&str> = listed
        .split('\0')
        .filter(|p| !p.is_empty() && glob_match(pat, p) && !crate::orchestrator::names_git_dir(p))
        .collect();
    hits.sort_unstable();
    hits.dedup();
    ToolResult::Output(name_list(&hits, &format!("no file matches `{glob}`")))
}

pub(crate) fn ls(full: &Path, rel: &str) -> ToolResult {
    let rd = match std::fs::read_dir(full) {
        Ok(rd) => rd,
        Err(e) => return ToolResult::Error(format!("cannot list {rel}: {e}")),
    };
    let mut names: Vec<String> = rd
        .filter_map(Result::ok)
        // By any spelling: `.GIT` on a case-insensitive filesystem is it.
        .filter(|e| !crate::orchestrator::names_git_dir(&e.file_name().to_string_lossy()))
        .map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                format!("{name}/")
            } else {
                name
            }
        })
        .collect();
    names.sort_unstable();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    ToolResult::Output(name_list(&refs, &format!("{rel} is empty")))
}

fn name_list(names: &[&str], empty: &str) -> String {
    if names.is_empty() {
        return empty.to_string();
    }
    let mut out = names
        .iter()
        .take(MAX_LISTED)
        .copied()
        .collect::<Vec<_>>()
        .join("\n");
    if names.len() > MAX_LISTED {
        out.push_str(&format!(
            "\n...and {} more, not listed. Narrow the pattern.",
            names.len() - MAX_LISTED
        ));
    }
    out
}

/// A glob over `/`-separated paths: `*` any run within one segment, `**` any
/// run of whole segments (including none), `?` one character in a segment.
/// Everything else is literal. No brackets or braces: a glob whose meaning a
/// reader has to work out is a worse tool than two simple ones.
pub(crate) fn glob_match(pat: &str, path: &str) -> bool {
    let p: Vec<&str> = pat.split('/').collect();
    let s: Vec<&str> = path.split('/').collect();
    segments(&p, &s)
}

fn segments(p: &[&str], s: &[&str]) -> bool {
    match p.split_first() {
        None => s.is_empty(),
        Some((&"**", rest)) => (0..=s.len()).any(|i| segments(rest, &s[i..])),
        Some((head, rest)) => match s.split_first() {
            Some((seg, srest)) => segment(head.as_bytes(), seg.as_bytes()) && segments(rest, srest),
            None => false,
        },
    }
}

fn segment(p: &[u8], s: &[u8]) -> bool {
    match p.split_first() {
        None => s.is_empty(),
        Some((b'*', rest)) => (0..=s.len()).any(|i| segment(rest, &s[i..])),
        Some((b'?', rest)) => !s.is_empty() && segment(rest, &s[utf8_len(s[0]).min(s.len())..]),
        Some((c, rest)) => s.first() == Some(c) && segment(rest, &s[1..]),
    }
}

/// Byte length of the UTF-8 character that starts with `b`, so `?` means one
/// character to a user whose file names are not ASCII.
fn utf8_len(b: u8) -> usize {
    match b {
        0xF0..=0xFF => 4,
        0xE0..=0xEF => 3,
        0xC0..=0xDF => 2,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_star_stays_inside_one_directory_and_a_double_star_crosses_them() {
        assert!(glob_match("*.py", "a.py"));
        assert!(!glob_match("*.py", "app/a.py"), "`*` does not cross `/`");
        assert!(glob_match("**/*.py", "a.py"), "`**` may match nothing");
        assert!(glob_match("**/*.py", "app/deep/a.py"));
        assert!(glob_match("app/**", "app/deep/a.py"));
        assert!(!glob_match("app/**/*.py", "lib/a.py"));
        assert!(glob_match("app/?.py", "app/a.py"));
        assert!(!glob_match("app/?.py", "app/ab.py"));
        assert!(glob_match("tests/test_*.py", "tests/test_x.py"));
        assert!(glob_match("?.py", "é.py"), "one character, not one byte");
    }

    #[test]
    fn a_long_listing_says_how_many_it_left_out() {
        let names: Vec<String> = (0..600).map(|i| format!("f{i}")).collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        let out = name_list(&refs, "none");
        assert!(out.contains("...and 100 more"), "{out}");
    }
}
