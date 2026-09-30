//! Reading the standard's inputs out of an issue body (standard §4, intake).
//!
//! The standard reads frontmatter; these issues carry the same facts in
//! their Markdown (`docs/TICKETING.md`, "How this repository applies it").
//! A field that is present but cannot be read comes back as `Err(reason)`,
//! never as a best guess: intake failure is a hard fail (§4), and the rules
//! turn it into an abstention that names what could not be read.

/// One entry of the `**Files:**` line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FileRef {
    pub(crate) path: String,
    /// A directory names an unknown number of files.
    pub(crate) is_dir: bool,
}

impl FileRef {
    /// Whether this entry is `prefix` or lies under it. A directory entry
    /// that contains `prefix` counts too: `crates/trellis-checks` names the
    /// checks crate even without a file inside it.
    pub(crate) fn under(&self, prefix: &str) -> bool {
        self.path == prefix
            || self
                .path
                .strip_prefix(prefix)
                .is_some_and(|rest| rest.starts_with('/'))
    }

    pub(crate) fn file_name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

const FILES_PREFIX: &str = "**Files:**";

/// The paths on the body's `**Files:**` line, in the order written.
///
/// Entries are separated by `;` or `,`; a parenthetical is commentary and a
/// word after the path (`docs/X.md §4`) is a locator within it. An entry
/// whose first word is not a path (`if built: crates/...`, `no crate files`)
/// makes the whole line unreadable, because the files it stands for are not
/// known. `src/...`, `tests/...` after a `crates/<name>/...` entry continue
/// that crate, as issue #76 writes its own line.
pub(crate) fn files(body: &str) -> Result<Vec<FileRef>, String> {
    let line = body
        .lines()
        .find_map(|l| l.trim_start().strip_prefix(FILES_PREFIX))
        .ok_or_else(|| "the body has no `**Files:**` line".to_string())?;

    let mut out: Vec<FileRef> = Vec::new();
    let mut crate_root: Option<String> = None;
    for segment in top_level_segments(line) {
        let Some(word) = segment.split_whitespace().next() else {
            continue;
        };
        if !looks_like_path(word) {
            return Err(format!(
                "`{}` on the `**Files:**` line is not a file path",
                segment.trim()
            ));
        }
        let mut path = word.trim_end_matches('/').to_string();
        let continues_crate = ["src/", "tests/", "benches/", "examples/"]
            .iter()
            .any(|p| word.starts_with(p));
        if continues_crate && let Some(root) = &crate_root {
            path = format!("{root}/{path}");
        }
        if let Some(root) = crate_of(&path) {
            crate_root = Some(root);
        }
        let last = path.rsplit('/').next().unwrap_or(&path);
        let is_dir = word.ends_with('/') || !last.contains('.');
        if !out.iter().any(|f| f.path == path) {
            out.push(FileRef { path, is_dir });
        }
    }
    Ok(out)
}

/// `crates/<name>` for a path inside a crate.
pub(crate) fn crate_of(path: &str) -> Option<String> {
    let mut parts = path.split('/');
    match (parts.next(), parts.next()) {
        (Some("crates"), Some(name)) if !name.is_empty() => Some(format!("crates/{name}")),
        _ => None,
    }
}

/// Split on `;` and `,` outside parentheses, dropping the parentheticals and
/// backticks. Commas inside a parenthetical are commentary, not separators.
fn top_level_segments(line: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    for c in line.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth = depth.saturating_sub(1),
            ';' | ',' if depth == 0 => segments.push(std::mem::take(&mut current)),
            '`' => {}
            _ if depth == 0 => current.push(c),
            _ => {}
        }
    }
    segments.push(current);
    segments.retain(|s| !s.trim().is_empty());
    segments
}

fn looks_like_path(word: &str) -> bool {
    (word.contains('/') || word.contains('.')) && !word.ends_with(':') && !word.starts_with('§')
}

/// The text under `## <name>`, up to the next `## ` heading or the
/// generated triage block. `None` when the heading is absent.
pub(crate) fn section<'a>(body: &'a str, name: &str) -> Option<Vec<&'a str>> {
    let mut lines = body.lines();
    lines.find(|l| is_heading(l, name))?;
    Some(
        lines
            .take_while(|l| !l.starts_with("## ") && !l.starts_with(TRIAGE_START))
            .collect(),
    )
}

fn is_heading(line: &str, name: &str) -> bool {
    line.strip_prefix("## ")
        .is_some_and(|rest| rest.trim_end() == name)
}

/// The command lines of the first fenced block under `## Verify`. Empty
/// when there is no such block or it holds nothing to run.
pub(crate) fn verify_commands(body: &str) -> Vec<String> {
    let Some(lines) = section(body, "Verify") else {
        return Vec::new();
    };
    lines
        .iter()
        .skip_while(|l| !l.starts_with("```"))
        .skip(1)
        .take_while(|l| !l.starts_with("```"))
        .map(|l| l.trim())
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// How many new tests the Verify block names: the `<min>` of each
/// `scripts/tickets/verify-tests.sh <package> <filter> [<min>]` line
/// (default 1, as the script's). It is the one machine-readable count an
/// issue carries; any other Verify command adds nothing to it.
pub(crate) fn new_tests(commands: &[String]) -> u32 {
    commands
        .iter()
        .filter_map(|c| {
            let words: Vec<&str> = c.split_whitespace().collect();
            let at = words.iter().position(|w| w.ends_with("verify-tests.sh"))?;
            Some(
                words
                    .get(at + 3)
                    .and_then(|m| m.parse::<u32>().ok())
                    .unwrap_or(1),
            )
        })
        .sum()
}

pub(crate) const TRIAGE_START: &str = "<!-- triage:start";
const TRIAGE_END: &str = "<!-- triage:end";

/// Every `blocked_by:` entry, as issue numbers.
///
/// Read from the whole body, as G0 (`scripts/gates/ticket-ready.sh`) reads
/// it. An entry that is not a number, a value that is not a `[...]` list,
/// or a triage block that is opened and never closed is malformed intake:
/// a dependency that cannot be looked up is not a satisfied one.
pub(crate) fn blocked_by(body: &str) -> Result<Vec<u64>, String> {
    if let Some(start) = body.find(TRIAGE_START)
        && !body[start..].contains(TRIAGE_END)
    {
        return Err("the triage block is opened and never closed".to_string());
    }
    let mut out = Vec::new();
    for line in body.lines() {
        let Some(value) = line.strip_prefix("blocked_by:") else {
            continue;
        };
        let list = value
            .trim()
            .strip_prefix('[')
            .and_then(|v| v.strip_suffix(']'))
            .ok_or_else(|| format!("`blocked_by:{value}` is not a `[...]` list"))?;
        for entry in list.split(',') {
            let entry = entry.trim().trim_matches('"').trim_matches('\'');
            if entry.is_empty() {
                continue;
            }
            let n = entry
                .parse::<u64>()
                .map_err(|_| format!("blocked_by entry `{entry}` is not an issue number"))?;
            if !out.contains(&n) {
                out.push(n);
            }
        }
    }
    Ok(out)
}

const DECISION_STEMS: [&str; 5] = ["design", "decid", "investigat", "explor", "consider"];

/// The decision words (§5 `!agent-ok` rule 3) in the title and body, sorted
/// and without repeats, each as the whole word that matched.
///
/// The `## Decision` section and the generated triage block are not read:
/// they record decisions already made ("Decided 2026-09-28 by the owner"),
/// and the rule is about decisions the work would still have to make.
pub(crate) fn decision_words(title: &str, body: &str) -> Vec<String> {
    let mut text = String::from(title);
    text.push('\n');
    let mut in_decision = false;
    let mut in_block = false;
    for line in body.lines() {
        if line.starts_with(TRIAGE_START) {
            in_block = true;
        }
        if line.starts_with("## ") {
            in_decision = is_heading(line, "Decision");
        }
        if !in_decision && !in_block {
            text.push_str(line);
            text.push('\n');
        }
        if line.starts_with(TRIAGE_END) {
            in_block = false;
        }
    }

    let words: Vec<String> = text
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    let mut found: Vec<String> = Vec::new();
    for (i, w) in words.iter().enumerate() {
        let hit = if DECISION_STEMS.iter().any(|s| w.starts_with(s)) {
            Some(w.clone())
        } else if w == "figure" && words.get(i + 1).is_some_and(|n| n == "out") {
            Some("figure out".to_string())
        } else {
            None
        };
        if let Some(h) = hit
            && !found.contains(&h)
        {
            found.push(h);
        }
    }
    found.sort();
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_crate_relative_entry_continues_the_crate_before_it() {
        // Issue #76's own line: without the continuation, `src/lib.rs` would
        // be read as a top-level path and map to no area.
        let body = "**Files:** Cargo.toml (workspace members already glob `crates/*`); \
                    crates/trellis-tickets/Cargo.toml, src/lib.rs, src/rules.rs (new)";
        let paths: Vec<String> = files(body).unwrap().into_iter().map(|f| f.path).collect();
        assert_eq!(
            paths,
            [
                "Cargo.toml",
                "crates/trellis-tickets/Cargo.toml",
                "crates/trellis-tickets/src/lib.rs",
                "crates/trellis-tickets/src/rules.rs",
            ]
        );
    }

    #[test]
    fn a_prose_entry_makes_the_files_line_unreadable_rather_than_shorter() {
        // Dropping `if built: ...` would undercount the files and could
        // lower effort and risk on a ticket that touches more.
        let body = "**Files:** docs/adr/0024.md (§5); if built: crates/trellis-loop";
        assert!(files(body).unwrap_err().contains("if built"));
    }

    #[test]
    fn a_blocked_by_entry_that_is_not_a_number_is_malformed_not_skipped() {
        assert!(blocked_by("blocked_by: [75, child-a]").is_err());
        assert_eq!(blocked_by("blocked_by: [75, 76]").unwrap(), [75, 76]);
        assert_eq!(blocked_by("blocked_by: []").unwrap(), Vec::<u64>::new());
    }

    #[test]
    fn verify_tests_lines_count_their_minimum() {
        let cmds = vec![
            "scripts/tickets/verify-tests.sh trellis-tickets sweep_ 3".to_string(),
            "scripts/tickets/verify-tests.sh trellis-tickets scan_".to_string(),
            "git ls-files --error-unmatch docs/TICKETING.md".to_string(),
        ];
        assert_eq!(new_tests(&cmds), 4);
    }
}
