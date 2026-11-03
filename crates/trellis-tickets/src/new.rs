//! `trellis-tickets new`, less the spawns (ADR 0040 decision 4, #79).
//!
//! Filing is the one write `new` makes, so it is a closure: the binary
//! passes one that runs `gh api` (a REST `POST`), and the tests pass one that
//! records what it was given. Reading blockers is the same closure `triage`
//! takes. Nothing here reaches GitHub on its own.
//!
//! The body is refused before anything is filed unless G0
//! (`scripts/gates/ticket-ready.sh`) would find a `## Done when` and a fenced
//! `## Verify` in it, and every line of that Verify block passes G6's line
//! checks (`scripts/gates/ticket-verify.sh`). The failure strings are the
//! scripts', so a refusal here reads the same as the gate would later.
//!
//! The labels are what the rules compute over the body, never a default: a
//! dimension whose rule abstains gets no label and is printed as
//! `abstained`, and the triage route that follows from it (`needs-human`)
//! is applied as computed.

use crate::labels::Origin;
use crate::report::{Line, TriageError, Value, blocker_states, line_text, printable, report};
use crate::{State, TRIAGE_VERSION, Ticket, gate};

/// `new` could not produce, or could not file, the issue.
#[derive(Debug, thiserror::Error)]
pub enum NewError {
    /// The title or body would not make a workable ticket; nothing was
    /// filed. Each failure quotes the input through `printable`.
    #[error("refusing to file:\n  - {}", .failures.join("\n  - "))]
    Refused { failures: Vec<String> },
    /// A `blocked_by` issue could not be read, so triage could not run;
    /// nothing was filed.
    #[error(transparent)]
    Triage(#[from] TriageError),
    /// The create step failed.
    #[error("filing the issue: {reason}")]
    Create { reason: String },
}

/// What `new` would file: the checked title and body, every rule's line,
/// and the labels the decided lines give.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewPlan {
    pub title: String,
    pub body: String,
    /// One line per dimension, in `triage`'s order.
    pub lines: Vec<(&'static str, Line)>,
    /// The labels applied: each decided dimension, the decided flags, and
    /// the triage route. `ready` is not among them; `sync-ready.py` owns
    /// that label.
    pub labels: Vec<&'static str>,
}

/// What `new` did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewOutcome {
    pub plan: NewPlan,
    /// What the create step printed (for `gh`, the new issue's URL), or
    /// `None` on a dry run, where nothing was filed.
    pub created: Option<String>,
}

/// G6's shell-composition patterns.
const G6_COMPOSITION: [&str; 7] = ["$(", "`", "|", ";", "&", ">", "<"];

/// The lines G6 would run: its awk
/// `/^## Verify/{v=1; next} v && /^```/{ if (in_) exit; in_=1; next } in_{print}`,
/// read line for line. The block is the first fence after the first
/// `## Verify` heading, wherever it sits, not only inside that section.
fn g6_block(body: &str) -> Vec<&str> {
    let (mut after_heading, mut inside) = (false, false);
    let mut out = Vec::new();
    for line in body.lines() {
        if line.starts_with("## Verify") {
            after_heading = true;
            continue;
        }
        if after_heading && line.starts_with("```") {
            if inside {
                break;
            }
            inside = true;
            continue;
        }
        if inside {
            out.push(line);
        }
    }
    out
}

/// G6's check of one line: `None` when G6 would run it (or skip it as blank
/// or a comment), otherwise its failure. G6 stops at a line's first failing
/// check, so only that one is reported.
fn g6_line(line: &str) -> Option<String> {
    // `${line#"${line%%[![:space:]]*}"}`: POSIX [:space:] is these six.
    let l = line.trim_start_matches([' ', '\t', '\n', '\u{b}', '\u{c}', '\r']);
    if l.is_empty() || l.starts_with('#') {
        return None;
    }
    let shown = printable(l);
    // G6's allowed first words (`ticket-verify.sh`'s `case`). Nothing here
    // runs them; the line is only compared against them.
    let allowed = ["cargo ", "git ", "scripts/", "py -3 qa/", "python3 qa/"];
    if !allowed.iter().any(|p| l.starts_with(p)) {
        return Some(format!(
            "Verify line not allowed (cargo, git, scripts/, py -3 qa/, python3 qa/ only): {shown}"
        ));
    }
    if G6_COMPOSITION.iter().any(|p| l.contains(p)) {
        return Some(format!(
            "Verify line uses shell composition; put it in a script under scripts/: {shown}"
        ));
    }
    if l.contains("..") {
        return Some(format!("Verify line contains '..': {shown}"));
    }
    None
}

/// Every reason the title and body would not make a workable ticket.
fn refusals(title: &str, body: &str) -> Vec<String> {
    let mut out = Vec::new();
    // GitHub refuses an issue with an empty title; refusing it here says so
    // before anything is spawned.
    if title.trim().is_empty() {
        out.push("the title is empty".to_string());
    }
    if title.chars().any(char::is_control) {
        out.push(format!(
            "the title contains a control character: {}",
            printable(title)
        ));
    }
    // G0 strips carriage returns (#205) and G6 does not: a CRLF Verify line
    // passes G6's checks and then runs with `\r` on its last argument. The
    // file is the user's, so it is not rewritten here; they save it as LF.
    if body.contains('\r') {
        out.push(
            "the body has carriage returns (CRLF line endings); G6 would run its Verify lines with them attached, so save it with LF endings"
                .to_string(),
        );
    }
    if !gate::has_done_when(body) {
        out.push("no '## Done when' section".to_string());
    }
    let block = g6_block(body);
    // G6's `[ -n "$cmd" ]`: a block of only empty lines is no command.
    if !gate::has_fenced_verify(body) || block.iter().all(|l| l.is_empty()) {
        out.push("no fenced command under '## Verify'".to_string());
    }
    out.extend(block.into_iter().filter_map(g6_line));
    out
}

/// Runs every rule over the body and collects the labels to apply.
///
/// `fetch` reads each issue the body's `blocked_by` names, as
/// [`crate::triage_with`] does, and nothing else.
pub fn plan_new(
    title: &str,
    body: &str,
    mut fetch: impl FnMut(u64) -> Result<String, String>,
) -> Result<NewPlan, NewError> {
    let failures = refusals(title, body);
    if !failures.is_empty() {
        return Err(NewError::Refused { failures });
    }
    // `origin:` is not in the text; §2's Sources table has the creation path
    // record it, and `new` is the Manual row. Without it the origin rule
    // abstains on every issue `new` files and each one routes to a person.
    let t = Ticket {
        title: title.to_string(),
        body: body.to_string(),
        labels: vec![Origin::Manual.label().to_string()],
        state: State::Open,
    };
    let blockers = blocker_states(body, &mut fetch)?;
    let r = report(0, &t, &blockers);
    let lines: Vec<(&'static str, Line)> = r
        .lines()
        .into_iter()
        .map(|(name, line)| (name, line.clone()))
        .collect();
    let mut labels = Vec::new();
    for (_, line) in &lines {
        match line {
            Line::Decided {
                value: Value::Label(l),
                ..
            } => labels.push(*l),
            Line::Decided {
                value: Value::Flags(f),
                ..
            } => labels.extend(f.iter().copied()),
            Line::Decided {
                value: Value::Route { route, .. },
                ..
            } => labels.push(*route),
            // `ready` is a triage-block field and `sync-ready.py`'s label.
            Line::Decided {
                value: Value::Ready { .. },
                ..
            } => {}
            Line::Abstained { .. } => {}
        }
    }
    Ok(NewPlan {
        title: title.to_string(),
        body: body.to_string(),
        lines,
        labels,
    })
}

/// A word as a POSIX shell would read it back: unquoted when every
/// character is inert, otherwise single-quoted. `!` is quoted, so a flag
/// label such as `!needs-adr` pasted into bash is not a history expansion.
fn shell_word(s: &str) -> String {
    let inert = |c: char| c.is_ascii_alphanumeric() || "-_./:=@,+%".contains(c);
    if !s.is_empty() && s.chars().all(inert) {
        s.to_string()
    } else {
        format!("'{}'", s.replace('\'', r"'\''"))
    }
}

impl NewPlan {
    /// The `gh api` argv (a REST `POST`, gh's default once a field is
    /// given) that files this plan. The body is not in it: it goes on stdin
    /// (`-F body=@-`), so what `gh` files is the body that was checked, not
    /// whatever the file holds by the time `gh` would read it. The title
    /// and labels are `-f`, which gh sends as given: no `@file` read, no
    /// `{owner}` fill. `--jq .html_url` prints the new issue's URL.
    pub fn gh_create_args(&self) -> Vec<String> {
        let mut args: Vec<String> = vec![
            "api".to_string(),
            "repos/{owner}/{repo}/issues".to_string(),
            "-f".to_string(),
            format!("title={}", self.title),
            "-F".to_string(),
            "body=@-".to_string(),
        ];
        for l in &self.labels {
            args.push("-f".to_string());
            args.push(format!("labels[]={l}"));
        }
        args.push("--jq".to_string());
        args.push(".html_url".to_string());
        args
    }

    /// The abstained lines that would have been labels, by dimension name.
    pub fn abstained(&self) -> Vec<&'static str> {
        self.lines
            .iter()
            .filter(|(name, line)| *name != "ready" && matches!(line, Line::Abstained { .. }))
            .map(|(name, _)| *name)
            .collect()
    }

    /// Everything that will be filed, before it is: each rule's line, the
    /// labels, the argv, and the body that goes on stdin.
    pub fn text(&self) -> String {
        let mut out = format!(
            "new issue: {} (triage_version {TRIAGE_VERSION})\n",
            printable(&self.title)
        );
        for (name, line) in &self.lines {
            out.push_str(&line_text(name, line));
        }
        if self.labels.is_empty() {
            out.push_str("labels: (none)\n");
        } else {
            out.push_str(&format!("labels: {}\n", self.labels.join(" ")));
        }
        let abstained = self.abstained();
        if !abstained.is_empty() {
            out.push_str(&format!(
                "abstained: {}; no label is applied for {}\n",
                abstained.join(", "),
                if abstained.len() == 1 { "it" } else { "them" }
            ));
        }
        let argv: Vec<String> = std::iter::once("gh".to_string())
            .chain(self.gh_create_args().iter().map(|a| shell_word(a)))
            .collect();
        out.push_str(&printable(&argv.join(" ")));
        out.push_str("\nbody, on stdin:\n");
        for line in self.body.lines() {
            if line.is_empty() {
                out.push_str("  |\n");
            } else {
                out.push_str(&format!("  | {}\n", printable(line)));
            }
        }
        out
    }
}

/// `new`: check the body, compute its labels, show the plan through `show`,
/// then file it through `create` unless `dry_run`.
///
/// `show` is always called before `create`, so the plan is printed before
/// anything is filed. `create` receives [`NewPlan::gh_create_args`] and the
/// body for stdin; it is called at most once, and never on a dry run or a
/// refused body.
pub fn new_with(
    title: &str,
    body: &str,
    dry_run: bool,
    fetch: impl FnMut(u64) -> Result<String, String>,
    mut show: impl FnMut(&str),
    create: impl FnOnce(&[String], &str) -> Result<String, String>,
) -> Result<NewOutcome, NewError> {
    let plan = plan_new(title, body, fetch)?;
    show(&plan.text());
    if dry_run {
        show("dry run: nothing filed\n");
        return Ok(NewOutcome {
            plan,
            created: None,
        });
    }
    let created =
        create(&plan.gh_create_args(), &plan.body).map_err(|reason| NewError::Create { reason })?;
    Ok(NewOutcome {
        plan,
        created: Some(created),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn g6_block_is_the_first_fence_after_the_heading_wherever_it_sits() {
        // G6's awk does not stop at the next heading; reading only the
        // section would check a block G6 never runs, or miss the one it does.
        let body = "## Verify\n\n## Notes\n```\ncargo test\n```\n```\nbash x\n```\n";
        assert_eq!(g6_block(body), ["cargo test"]);
    }

    #[test]
    fn g6_line_reports_its_first_failing_check_as_the_script_does() {
        assert_eq!(g6_line("  # a comment"), None);
        assert_eq!(g6_line("   "), None);
        assert_eq!(g6_line("cargo test -p x"), None);
        assert!(
            g6_line("bash scripts/x.sh")
                .unwrap()
                .contains("not allowed")
        );
        assert!(
            g6_line("cargo test | tail")
                .unwrap()
                .contains("composition")
        );
        assert!(g6_line("scripts/../rm").unwrap().contains("'..'"));
    }

    #[test]
    fn shell_word_quotes_what_bash_would_expand() {
        assert_eq!(shell_word("area:cli"), "area:cli");
        assert_eq!(shell_word("!needs-adr"), "'!needs-adr'");
        assert_eq!(shell_word("it's"), r"'it'\''s'");
        assert_eq!(shell_word(""), "''");
    }
}
