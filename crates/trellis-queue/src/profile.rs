//! Agent profiles: `.trellis/agents/<name>.toml` (ADR 0026 §4).
//!
//! The file is the committed definition. What changes at run time — a pause
//! by a person or by the budget, a budget raise — is an overlay in the git
//! directory, because writing to the tracked file would dirty the worktree
//! every run needs clean (ADR 0009).

use crate::files::{read_json_opt, replace_locked};
use crate::{Queue, QueueError};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

pub const DIR: &str = "agents";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ControlFile {
    pub mode: Option<String>,
    pub approve: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillsFile {
    /// Absent means every skill; present means only these.
    pub allow: Option<Vec<String>>,
    #[serde(default)]
    pub deny: Vec<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BudgetFile {
    pub monthly_tokens: Option<u64>,
    pub lifetime_tokens: Option<u64>,
    pub warn_at: Option<f64>,
}

/// What a profile file says. Unknown keys are refused: a misspelt
/// `[budgett]` silently meaning "no budget" is the failure this prevents.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileFile {
    pub role: Option<String>,
    /// A file under `.trellis/`, prepended to the task.
    pub instructions: Option<String>,
    pub model: Option<String>,
    /// A process agent (ADR 0026 §11): argv, run in the worktree.
    pub command: Option<Vec<String>>,
    pub timeout_s: Option<u64>,
    pub reports_to: Option<String>,
    #[serde(default)]
    pub paused: bool,
    /// The tools this profile's runs may call: intersected with the set the
    /// repository enabled and trusted (ADR 0023), so it can only narrow.
    /// Names are checked against the repository's tools by the binary, which
    /// is the side that knows the custom ones.
    pub tools: Option<Vec<String>>,
    pub control: Option<ControlFile>,
    pub skills: Option<SkillsFile>,
    pub budget: Option<BudgetFile>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Profile {
    pub name: String,
    pub file: ProfileFile,
}

pub const DEFAULT_WARN_AT: f64 = 0.8;
pub const DEFAULT_TIMEOUT_S: u64 = 1800;

impl Profile {
    pub fn is_process(&self) -> bool {
        self.file.command.is_some()
    }

    pub fn warn_at(&self) -> f64 {
        self.file
            .budget
            .as_ref()
            .and_then(|b| b.warn_at)
            .unwrap_or(DEFAULT_WARN_AT)
    }

    /// The instructions file, resolved under `.trellis/`.
    pub fn instructions_path(&self, root: &Path) -> Option<PathBuf> {
        self.file
            .instructions
            .as_ref()
            .map(|p| root.join(".trellis").join(p))
    }

    /// Whether a skill may be disclosed to this profile's runs.
    pub fn allows_skill(&self, name: &str) -> bool {
        let Some(s) = &self.file.skills else {
            return true;
        };
        if s.deny.iter().any(|d| d == name) {
            return false;
        }
        s.allow.as_ref().is_none_or(|a| a.iter().any(|x| x == name))
    }
}

pub fn valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'_')
}

fn relative_inside(p: &str) -> bool {
    let path = Path::new(p);
    !p.is_empty()
        && path
            .components()
            .all(|c| matches!(c, Component::Normal(_) | Component::CurDir))
}

pub fn parse(name: &str, text: &str) -> Result<Profile, QueueError> {
    let file: ProfileFile =
        toml::from_str(text).map_err(|e| QueueError::Invalid(format!("agent `{name}`: {e}")))?;
    Ok(Profile {
        name: name.to_string(),
        file,
    })
}

/// Checks one profile on its own, and the set for `reports_to`.
pub fn validate(set: &BTreeMap<String, Profile>) -> Result<(), QueueError> {
    let bad = |n: &str, why: String| Err(QueueError::Invalid(format!("agent `{n}`: {why}")));
    for (n, p) in set {
        if !valid_name(n) {
            return bad(
                n,
                "a name is lowercase ASCII letters, digits, `-` and `_`".into(),
            );
        }
        let f = &p.file;
        if f.model.is_some() && f.command.is_some() {
            return bad(
                n,
                "`model` and `command` are exclusive: a profile is driven by trellis's \
                 loop or by an external process, not both"
                    .into(),
            );
        }
        if let Some(c) = &f.command
            && (c.is_empty() || c[0].trim().is_empty())
        {
            return bad(n, "`command` names no program".into());
        }
        if f.command.is_some() && f.budget.is_some() {
            // An unenforceable limit reads as a limit that exists.
            return bad(
                n,
                "a process agent cannot have a token budget: trellis does not see \
                 its tokens, so the limit could never be enforced (ADR 0026 §4)"
                    .into(),
            );
        }
        if f.command.is_some() && f.tools.is_some() {
            // The same reason as the budget: a process is not driven through
            // trellis's tool set, so the list could restrict nothing, and a
            // restriction that exists only on paper reads as one that holds.
            return bad(
                n,
                "a process agent cannot have a `tools` list: it does not call trellis's \
                 tools, so the list could never be enforced (ADR 0026 §4)"
                    .into(),
            );
        }
        if f.command.is_none() && f.timeout_s.is_some() {
            return bad(
                n,
                "`timeout_s` applies to a process agent (`command`); a model run is \
                 bounded by [budget]"
                    .into(),
            );
        }
        if f.timeout_s == Some(0) {
            return bad(n, "`timeout_s` of 0 would kill every run at once".into());
        }
        if let Some(i) = &f.instructions
            && !relative_inside(i)
        {
            return bad(
                n,
                format!("instructions `{i}` must be a relative path under .trellis/"),
            );
        }
        if let Some(c) = &f.control
            && let Some(m) = &c.mode
            && m != "auto"
            && m != "approve"
        {
            return bad(
                n,
                format!("[control] mode = \"{m}\" is not a mode; use \"auto\" or \"approve\""),
            );
        }
        if let Some(b) = &f.budget
            && let Some(w) = b.warn_at
            && !(w > 0.0 && w <= 1.0)
        {
            return bad(n, format!("[budget] warn_at = {w} is outside (0, 1]"));
        }
        if let Some(r) = &f.reports_to {
            if r == n {
                return bad(n, "reports to itself".into());
            }
            if !set.contains_key(r) {
                return bad(n, format!("reports_to `{r}`, which is not a profile"));
            }
        }
    }
    // No cycles (Paperclip item 4). Each chain is walked at most set.len()
    // steps; a longer walk has revisited a profile.
    for n in set.keys() {
        let mut seen = vec![n.as_str()];
        let mut cur = n.as_str();
        while let Some(next) = set.get(cur).and_then(|p| p.file.reports_to.as_deref()) {
            if seen.contains(&next) {
                seen.push(next);
                return bad(
                    n,
                    format!("reports_to forms a cycle: {}", seen.join(" -> ")),
                );
            }
            seen.push(next);
            cur = next;
        }
    }
    Ok(())
}

/// Every profile in `<root>/.trellis/agents/`, validated as a set.
pub fn load_all(root: &Path) -> Result<BTreeMap<String, Profile>, QueueError> {
    load_dir(&root.join(".trellis").join(DIR))
}

pub fn load_dir(dir: &Path) -> Result<BTreeMap<String, Profile>, QueueError> {
    let mut set = BTreeMap::new();
    let rd = match std::fs::read_dir(dir) {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(set),
        Err(e) => return Err(crate::files::io(dir)(e)),
    };
    for e in rd.filter_map(Result::ok) {
        let path = e.path();
        let Some(name) = path
            .file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(".toml"))
        else {
            continue;
        };
        let text = std::fs::read_to_string(&path).map_err(crate::files::io(&path))?;
        set.insert(name.to_string(), parse(name, &text)?);
    }
    validate(&set)?;
    Ok(set)
}

// ---------------------------------------------------------------------------
// The overlay: run-time state in the git directory
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PausedBy {
    Human,
    Budget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Window {
    Monthly,
    Lifetime,
}

impl Window {
    pub fn as_str(self) -> &'static str {
        match self {
            Window::Monthly => "monthly",
            Window::Lifetime => "lifetime",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pause {
    pub by: PausedBy,
    pub at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// For a budget pause: the window that tripped, and for a monthly one
    /// the month, so a rollover can lift it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub window: Option<Window>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Raise {
    pub window: Window,
    /// `YYYY-MM` for a monthly raise: it applies to that month only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub month: Option<String>,
    pub tokens: u64,
    pub at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Overlay {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub paused: Option<Pause>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub raises: Vec<Raise>,
}

impl Overlay {
    /// The limit for a window after raises. `None` when no limit is set.
    pub fn limit(&self, base: Option<u64>, window: Window, month: &str) -> Option<u64> {
        let base = base?;
        let extra: u64 = self
            .raises
            .iter()
            .filter(|r| {
                r.window == window
                    && (window == Window::Lifetime || r.month.as_deref() == Some(month))
            })
            .map(|r| r.tokens)
            .sum();
        Some(base.saturating_add(extra))
    }
}

fn overlay_path(q: &Queue, name: &str) -> PathBuf {
    q.trellis_dir().join(DIR).join(format!("{name}.json"))
}

pub fn overlay(q: &Queue, name: &str) -> Result<Overlay, QueueError> {
    Ok(read_json_opt(&overlay_path(q, name))?.unwrap_or_default())
}

/// Changes a profile's overlay under its lock.
pub fn update_overlay<R>(
    q: &Queue,
    name: &str,
    f: impl FnOnce(&mut Overlay) -> R,
) -> Result<R, QueueError> {
    replace_locked(
        &q.locks(),
        &format!("agent-{name}"),
        &overlay_path(q, name),
        |cur: Option<Overlay>| {
            let mut o = cur.unwrap_or_default();
            let r = f(&mut o);
            Ok((o, r))
        },
    )
}

/// Why a profile may not run now, from its file and its overlay alone.
pub fn paused_reason(p: &Profile, o: &Overlay) -> Option<String> {
    if p.file.paused {
        return Some(format!(
            "agent `{}` is paused in .trellis/agents/{}.toml",
            p.name, p.name
        ));
    }
    o.paused.as_ref().map(|pause| match pause.by {
        PausedBy::Human => format!(
            "agent `{}` is paused{}",
            p.name,
            pause
                .reason
                .as_deref()
                .map(|r| format!(": {}", crate::plain(r)))
                .unwrap_or_default()
        ),
        PausedBy::Budget => format!(
            "agent `{}` is paused by its {} token budget; see `trellis queue list` \
             approvals and `trellis queue approve <request> --raise <tokens>`",
            p.name,
            pause.window.map(Window::as_str).unwrap_or("token")
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(files: &[(&str, &str)]) -> Result<BTreeMap<String, Profile>, QueueError> {
        let mut s = BTreeMap::new();
        for (n, t) in files {
            s.insert(n.to_string(), parse(n, t)?);
        }
        validate(&s)?;
        Ok(s)
    }

    fn err(files: &[(&str, &str)]) -> String {
        set(files).unwrap_err().to_string()
    }

    #[test]
    fn a_full_profile_parses() {
        let s = set(&[
            ("lead", "role = \"lead\"\n"),
            (
                "reviewer",
                r#"
role = "reviewer"
instructions = "agents/reviewer.md"
model = "scripted:x.json"
reports_to = "lead"
tools = ["read_file"]
[control]
mode = "approve"
approve = ["edit"]
[skills]
allow = ["a", "b"]
deny = ["b"]
[budget]
monthly_tokens = 1000
lifetime_tokens = 5000
warn_at = 0.5
"#,
            ),
        ])
        .unwrap();
        let r = &s["reviewer"];
        assert_eq!(r.warn_at(), 0.5);
        assert!(r.allows_skill("a") && !r.allows_skill("b") && !r.allows_skill("c"));
        assert_eq!(s["lead"].warn_at(), DEFAULT_WARN_AT);
    }

    #[test]
    fn reports_to_may_not_form_a_cycle() {
        // Paperclip item 4. A cycle has no top, and anything that walks the
        // chain — escalation, rollups — would never end.
        let e = err(&[
            ("a", "reports_to = \"b\""),
            ("b", "reports_to = \"c\""),
            ("c", "reports_to = \"a\""),
        ]);
        assert!(e.contains("cycle"), "{e}");
        assert!(err(&[("a", "reports_to = \"a\"")]).contains("itself"));
        assert!(err(&[("a", "reports_to = \"ghost\"")]).contains("not a profile"));
    }

    #[test]
    fn what_could_not_be_enforced_or_would_be_misread_is_refused() {
        assert!(err(&[("p", "model = \"x\"\ncommand = [\"y\"]")]).contains("exclusive"));
        assert!(
            err(&[("p", "command = [\"y\"]\n[budget]\nmonthly_tokens = 5")])
                .contains("cannot have a token budget")
        );
        assert!(
            err(&[("p", "command = [\"y\"]\ntools = [\"read_file\"]")])
                .contains("cannot have a `tools` list")
        );
        assert!(err(&[("p", "[budgett]\nx = 1")]).contains("unknown field"));
        assert!(err(&[("p", "[control]\nmode = \"yolo\"")]).contains("not a mode"));
        assert!(err(&[("p", "[budget]\nwarn_at = 1.5")]).contains("outside"));
        assert!(err(&[("p", "instructions = \"../../etc/passwd\"")]).contains("relative"));
        assert!(err(&[("Bad", "")]).contains("lowercase"));
    }

    #[test]
    fn a_monthly_raise_applies_to_its_month_only() {
        let o = Overlay {
            paused: None,
            raises: vec![
                Raise {
                    window: Window::Monthly,
                    month: Some("2026-09".into()),
                    tokens: 100,
                    at: "x".into(),
                },
                Raise {
                    window: Window::Lifetime,
                    month: None,
                    tokens: 7,
                    at: "x".into(),
                },
            ],
        };
        assert_eq!(o.limit(Some(1000), Window::Monthly, "2026-09"), Some(1100));
        assert_eq!(o.limit(Some(1000), Window::Monthly, "2026-10"), Some(1000));
        assert_eq!(o.limit(Some(10), Window::Lifetime, "2026-10"), Some(17));
        assert_eq!(o.limit(None, Window::Monthly, "2026-09"), None);
    }
}
