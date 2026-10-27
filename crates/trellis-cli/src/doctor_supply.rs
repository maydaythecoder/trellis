//! `doctor`'s `lockfile:` line: advisories and yanked crates in the
//! repository's `Cargo.lock`, as cargo-audit reports them, under the
//! repository's own `deny.toml` ignore list (ADR 0057).
//!
//! The line exists because `main` went red on 2026-10-24 with no code change:
//! its lock had aged against the RustSec database (tokio 1.53.1,
//! RUSTSEC-2026-0386; clipboard-win 5.4.1, yanked), and the first thing that
//! said so was CI. `doctor` is where a person checks a machine before a run,
//! so it is where that should read first.
//!
//! It reports; it does not rewrite the lock. A lockfile update is its own
//! commit (CLAUDE.md, "Dependency pinning"), made by a person.

use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The binary `cargo audit` dispatches to. Looked up on `PATH` and spawned
/// by its path, rather than as `cargo audit`, so that "not on PATH" means
/// the tool, not cargo, and means `PATH` alone: std's Windows resolver also
/// searches the parent's `PATH` and the system directories, which would
/// make the absent case depend on the host.
pub(crate) const TOOL: &str = "cargo-audit";

/// How long the whole check may take, its network work included: cargo-audit
/// fetches rustsec/advisory-db through a git library and refreshes the
/// crates.io index for the yanked check, together tens of seconds on a cold
/// cache; `doctor` must still end.
pub(crate) const BOUND: Duration = Duration::from_secs(90);

/// What CI runs, named when `doctor` could not, so the line still says
/// where the answer is.
const CI_COMMANDS: &str = "CI runs `cargo deny --locked check` and `cargo audit`";

/// One crate the database objects to. `kind` is cargo-audit's word for it
/// (`vulnerability`, `unsound`, `unmaintained`, `notice`, `yanked`); `id`
/// is the advisory, which a yanked version does not have.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Finding {
    pub(crate) krate: String,
    pub(crate) version: String,
    pub(crate) kind: String,
    pub(crate) id: Option<String>,
}

/// What the check found, or why there is no answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Report {
    /// Nothing the policy objects to. `database` is the date the RustSec
    /// database was last updated, when cargo-audit reported one: a clean
    /// verdict is only as current as the database it was read from.
    /// `ignored` is how many advisories `deny.toml` accepted.
    Clean {
        database: Option<String>,
        ignored: usize,
    },
    Findings {
        found: Vec<Finding>,
        ignored: usize,
    },
    /// The tool was absent, timed out, or answered in a shape this parser
    /// does not read. Never rendered as clean.
    NotChecked(String),
}

/// The `lockfile:` line for `root`, or `None` when there is no `Cargo.lock`
/// there. A Python repository gets no line: a key that is always "not
/// applicable" teaches the reader to skip the column.
pub(crate) fn line(root: &Path) -> Option<String> {
    let lock = root.join("Cargo.lock");
    if !lock.is_file() {
        return None;
    }
    let ignored = deny_ignores(&root.join("deny.toml"));
    Some(render(&run(&lock, &ignored)))
}

/// `cargo-audit audit --json --file <lock>`, bounded, keys scrubbed, then
/// the repository's ignore list applied.
fn run(lock: &Path, ignored: &[String]) -> Report {
    let args = [
        OsStr::new("audit"),
        OsStr::new("--json"),
        OsStr::new("--file"),
        lock.as_os_str(),
    ];
    let Some(tool) = on_path(TOOL) else {
        return Report::NotChecked(format!("{TOOL} is not on PATH"));
    };
    match output_within(&tool, &args, BOUND) {
        Ok(out) => match parse(&out.stdout) {
            Ok(parsed) => policy(parsed.findings, ignored, parsed.database),
            Err(why) => {
                // cargo-audit writes its own errors (no lockfile, no
                // database, no network) to stderr; that line beats a
                // parser's complaint about an empty stdout.
                let stderr = error_line(&out.stderr);
                Report::NotChecked(if stderr.is_empty() {
                    format!("{TOOL} answered in a shape doctor does not read ({why})")
                } else {
                    format!("{TOOL}: {stderr}")
                })
            }
        },
        Err(why) => Report::NotChecked(why),
    }
}

/// The verdict `deny.toml` gives: every advisory is a finding unless its id
/// is in `[advisories].ignore`, and a yanked version is a finding always
/// (it has no id to ignore). This is cargo-deny's default policy (its
/// `version = 2`; `deny.toml` sets no `version` key), which CI applies;
/// cargo-audit on its own fails only on vulnerabilities.
///
/// `ignored` counts advisories, not entries: one ignored advisory matched
/// at two locked versions is one.
pub(crate) fn policy(all: Vec<Finding>, ignored: &[String], database: Option<String>) -> Report {
    let (kept, dropped): (Vec<Finding>, Vec<Finding>) = all
        .into_iter()
        .partition(|f| !f.id.as_ref().is_some_and(|id| ignored.contains(id)));
    let mut ids: Vec<&str> = dropped.iter().filter_map(|f| f.id.as_deref()).collect();
    ids.sort_unstable();
    ids.dedup();
    if kept.is_empty() {
        Report::Clean {
            database,
            ignored: ids.len(),
        }
    } else {
        Report::Findings {
            found: kept,
            ignored: ids.len(),
        }
    }
}

/// `name` in a directory on this process's `PATH`, as a regular file, with
/// the executable suffix Windows needs. `PATH` only, on purpose (see
/// [`TOOL`]).
fn on_path(name: &str) -> Option<PathBuf> {
    on_path_in(&std::env::var_os("PATH")?, name)
}

/// [`on_path`] against a given `PATH` value, so a test can hand it one.
fn on_path_in(path: &OsStr, name: &str) -> Option<PathBuf> {
    let file = if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    };
    std::env::split_paths(path)
        // A relative entry (`.`) would resolve the tool from the working
        // directory, which under `doctor` is the repository, where model
        // writes land.
        .filter(|d| d.is_absolute())
        .map(|d| d.join(&file))
        .find(|p| p.is_file())
}

/// The advisory ids `deny.toml` ignores, in either form cargo-deny takes:
/// `"RUSTSEC-..."` or `{ id = "RUSTSEC-...", reason = "..." }`. No file, or
/// one that does not parse, ignores nothing: the line then reports more,
/// never less, and the file's own problem is cargo-deny's to name.
pub(crate) fn deny_ignores(deny: &Path) -> Vec<String> {
    let Ok(text) = std::fs::read_to_string(deny) else {
        return Vec::new();
    };
    let Ok(v) = text.parse::<toml::Table>() else {
        return Vec::new();
    };
    v.get("advisories")
        .and_then(|a| a.get("ignore"))
        .and_then(|i| i.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|i| match i {
                    toml::Value::String(s) => Some(s.clone()),
                    toml::Value::Table(t) => {
                        t.get("id").and_then(|id| id.as_str()).map(str::to_string)
                    }
                    _ => None,
                })
                .collect()
        })
        .unwrap_or_default()
}

#[derive(Debug)]
struct Captured {
    stdout: String,
    stderr: String,
}

/// Runs `program args...` and returns what it wrote, or why it was not run
/// to completion. The process is killed at `bound`: std has no timed wait,
/// so this polls `try_wait`, as `services::reap` does. The pipes are read
/// under the same bound: a grandchild that inherited them and outlived the
/// child would otherwise hold `doctor` after the child has exited.
///
/// `program` is a path (see [`TOOL`]); the name on the error lines is its
/// file stem, so a reader sees `cargo-audit`, not where it was found.
fn output_within(program: &Path, args: &[&OsStr], bound: Duration) -> Result<Captured, String> {
    let name = program.file_stem().map_or_else(
        || program.display().to_string(),
        |s| s.to_string_lossy().into_owned(),
    );
    let mut c = Command::new(program);
    c.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    trellis_core::secrets::scrub(&mut c);
    let mut child = match c.spawn() {
        Ok(child) => child,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(format!("{name} is not on PATH"));
        }
        Err(e) => return Err(format!("{name} could not be started: {e}")),
    };
    // Both pipes are drained on their own threads: a child that fills one
    // while this thread waits on the other would block forever.
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let deadline = Instant::now() + bound;
    let timed_out = |what: &str| format!("{name} {what} within {} s", bound.as_secs());
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(timed_out("did not finish"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => {
                let _ = child.kill();
                return Err(format!("{name} could not be waited for: {e}"));
            }
        }
    }
    // The child has exited; what can still hold the pipes open is a
    // grandchild that inherited them.
    let remaining = |now: Instant| deadline.saturating_duration_since(now);
    let unclosed = || timed_out("exited but its output was not closed");
    let stdout = stdout
        .recv_timeout(remaining(Instant::now()))
        .map_err(|_| unclosed())?;
    let stderr = stderr
        .recv_timeout(remaining(Instant::now()))
        .map_err(|_| unclosed())?;
    Ok(Captured { stdout, stderr })
}

/// Reads `pipe` to its end on its own thread and sends what it held.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> std::sync::mpsc::Receiver<String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut text = String::new();
        if let Some(mut r) = pipe {
            let mut raw = Vec::new();
            let _ = r.read_to_end(&mut raw);
            text = String::from_utf8_lossy(&raw).into_owned();
        }
        let _ = tx.send(text);
    });
    rx
}

/// cargo-audit's own reason: its `error:` line when there is one, since a
/// `warning:` about a locked cache directory can come first, else the first
/// non-empty line.
fn error_line(stderr: &str) -> String {
    let lines = || stderr.lines().map(str::trim).filter(|l| !l.is_empty());
    lines()
        .find(|l| l.starts_with("error"))
        .or_else(|| lines().next())
        .unwrap_or("")
        .to_string()
}

/// cargo-audit's `--json` report, read once.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Parsed {
    /// Everything in the report, before any policy: `vulnerabilities.list[]`
    /// as kind `vulnerability`, and every entry under `warnings.<kind>[]` as
    /// that kind (`unsound`, `unmaintained`, `notice`, `yanked`, and whatever
    /// a later cargo-audit adds).
    pub(crate) findings: Vec<Finding>,
    /// The day the advisory database was last updated, from
    /// `database.last-updated` (an RFC 3339 timestamp; the date is enough).
    pub(crate) database: Option<String>,
}

/// The shape is what cargo-audit 0.22.2 writes (the `rustsec` crate's
/// `Report`). A report missing `vulnerabilities.list`, or with `warnings`
/// or a `warnings.<kind>` in another shape, is an error, never an empty
/// list: a report that parsed to nothing would print as clean.
pub(crate) fn parse(json: &str) -> Result<Parsed, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("not a JSON report: {e}"))?;
    let vulns = v
        .pointer("/vulnerabilities/list")
        .and_then(|l| l.as_array())
        .ok_or("no `vulnerabilities.list` in the report")?;
    let mut findings = Vec::new();
    for item in vulns {
        let (krate, version) =
            package(item.get("package")).ok_or("a vulnerability names no package")?;
        let id = item
            .pointer("/advisory/id")
            .and_then(|s| s.as_str())
            .ok_or("a vulnerability names no advisory id")?;
        findings.push(Finding {
            krate,
            version,
            kind: "vulnerability".into(),
            id: Some(id.to_string()),
        });
    }
    // `warnings` is keyed by kind; a report with none of a kind omits the
    // key, so a missing kind is "none", not "unreadable".
    if let Some(warnings) = v.get("warnings") {
        let warnings = warnings.as_object().ok_or("`warnings` is not an object")?;
        for (kind, list) in warnings {
            let items = list
                .as_array()
                .ok_or_else(|| format!("`warnings.{kind}` is not a list"))?;
            for item in items {
                let (krate, version) =
                    package(item.get("package")).ok_or("a warning names no package")?;
                findings.push(Finding {
                    krate,
                    version,
                    kind: kind.clone(),
                    id: item
                        .pointer("/advisory/id")
                        .and_then(|s| s.as_str())
                        .map(str::to_string),
                });
            }
        }
    }
    let database = v
        .pointer("/database/last-updated")
        .and_then(|s| s.as_str())
        .map(|stamp| stamp.split('T').next().unwrap_or(stamp).to_string());
    Ok(Parsed { findings, database })
}

fn package(p: Option<&serde_json::Value>) -> Option<(String, String)> {
    let p = p?;
    let name = p.get("name")?.as_str()?.to_string();
    let version = p.get("version")?.as_str()?.to_string();
    Some((name, version))
}

/// The report as `doctor` prints it. Crate names, versions and kinds come
/// from the lockfile, the registry and the tool, so they are printed plain,
/// as everything from outside the binary is.
pub(crate) fn render(report: &Report) -> String {
    use crate::plain;
    let ignored_note = |n: usize| {
        if n == 0 {
            String::new()
        } else {
            format!(" · {n} advisory(ies) ignored by deny.toml")
        }
    };
    match report {
        Report::Clean { database, ignored } => {
            let against = match database {
                Some(d) => format!("against the RustSec database of {}", plain(d)),
                None => "database date not reported".to_string(),
            };
            format!(
                "lockfile:   Cargo.lock clean — {TOOL} {against}{}",
                ignored_note(*ignored)
            )
        }
        Report::Findings { found, ignored } => {
            let named: Vec<String> = found
                .iter()
                .map(|f| {
                    let why = match &f.id {
                        Some(id) => format!("{}, {}", plain(id), plain(&f.kind)),
                        None => plain(&f.kind),
                    };
                    format!("{} {} ({why})", plain(&f.krate), plain(&f.version))
                })
                .collect();
            // One `-p name@version` per locked version, each once: a crate
            // can be both yanked and advised against, and a bare name is
            // ambiguous to `cargo update` when the lock holds two versions
            // of it, which a transitive dependency often does.
            let mut pins: Vec<String> = found
                .iter()
                .map(|f| format!("-p {}@{}", plain(&f.krate), plain(&f.version)))
                .collect();
            pins.sort_unstable();
            pins.dedup();
            let update = pins.join(" ");
            format!(
                "lockfile:   {} finding(s) in Cargo.lock — {}{}\n\
                 {:12}fix: `cargo update {update}`, committed on its own (CLAUDE.md); a crate\n\
                 {:12}that stays needs its requirement in Cargo.toml raised first.",
                found.len(),
                named.join(", "),
                ignored_note(*ignored),
                "",
                ""
            )
        }
        Report::NotChecked(why) => format!(
            "lockfile:   Cargo.lock present; advisories not checked — {}\n\
             {:12}({CI_COMMANDS})",
            plain(why),
            ""
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cargo-audit 0.22.2's `--json` report for the lock `main` carried at
    /// b49920e, as run on 2026-10-25 against the database of 2026-10-24 and
    /// trimmed to the keys read here. CI run 40056551866 failed on it: the
    /// tokio advisory is an `unsound` warning to cargo-audit (which exits 0
    /// on it) and an error to `cargo deny` under `deny.toml`; the yanked
    /// crate is a warning to both; atomic-polyfill is in the ignore list.
    const MAIN_B49920E: &str = r#"{
      "database": {"advisory-count": 1352, "last-commit": "21d909e4c8bd4e14c7aea58bf980c881a0053ab1", "last-updated": "2026-10-24T10:30:13+02:00"},
      "lockfile": {"dependency-count": 412},
      "settings": {"target_arch": [], "target_os": [], "severity": null, "ignore": [], "informational_warnings": ["unmaintained", "unsound", "yanked"]},
      "vulnerabilities": {"found": false, "count": 0, "list": []},
      "warnings": {
        "unmaintained": [
          {"kind": "unmaintained", "package": {"name": "atomic-polyfill", "version": "1.0.3", "source": "registry+https://github.com/rust-lang/crates.io-index"},
           "advisory": {"id": "RUSTSEC-2023-0089", "package": "atomic-polyfill", "title": "atomic-polyfill is unmaintained"}}
        ],
        "unsound": [
          {"kind": "unsound", "package": {"name": "tokio", "version": "1.53.1", "source": "registry+https://github.com/rust-lang/crates.io-index"},
           "advisory": {"id": "RUSTSEC-2026-0386", "package": "tokio", "title": "AsyncFd accepts any AsRawFd"},
           "versions": {"patched": [">=1.53.3", ">=1.51.6, <1.52.0"], "unaffected": []}}
        ],
        "yanked": [
          {"kind": "yanked", "package": {"name": "clipboard-win", "version": "5.4.1", "source": "registry+https://github.com/rust-lang/crates.io-index"}, "advisory": null}
        ]
      }
    }"#;

    /// A report with a vulnerability, the kind cargo-audit itself exits 1 on.
    const VULN: &str = r#"{"database": {"last-updated": "2026-10-25T01:00:00Z"},
      "vulnerabilities": {"found": true, "count": 1, "list": [
        {"advisory": {"id": "RUSTSEC-2026-0001", "package": "x"}, "package": {"name": "x", "version": "1.0.0"}}]},
      "warnings": {"yanked": [{"kind": "yanked", "package": {"name": "x", "version": "1.0.0"}}]}}"#;

    const CLEAN: &str = r#"{"database": {"last-updated": "2026-10-25T01:00:00Z"}, "vulnerabilities": {"found": false, "count": 0, "list": []}, "warnings": {}}"#;

    /// The ignore list `deny.toml` carried at b49920e.
    const IGNORED: &[&str] = &["RUSTSEC-2023-0089"];

    fn ignored() -> Vec<String> {
        IGNORED.iter().map(|s| s.to_string()).collect()
    }

    fn findings(json: &str) -> Vec<Finding> {
        parse(json).unwrap().findings
    }

    #[test]
    fn every_kind_of_warning_is_read_and_the_ignore_list_is_the_policy() {
        // The risk: reading only `vulnerabilities`, as cargo-audit's own
        // exit code does, and printing clean for the lock that failed CI.
        let all = findings(MAIN_B49920E);
        assert_eq!(all.len(), 3, "{all:?}");
        let Report::Findings { found, ignored: n } = policy(all, &ignored(), None) else {
            panic!("clean for the lock that failed CI");
        };
        assert_eq!(n, 1);
        assert_eq!(
            found,
            vec![
                Finding {
                    krate: "tokio".into(),
                    version: "1.53.1".into(),
                    kind: "unsound".into(),
                    id: Some("RUSTSEC-2026-0386".into()),
                },
                Finding {
                    krate: "clipboard-win".into(),
                    version: "5.4.1".into(),
                    kind: "yanked".into(),
                    id: None,
                },
            ]
        );
    }

    #[test]
    fn findings_render_each_crate_its_reason_the_ignored_count_and_one_update_command() {
        let parsed = parse(MAIN_B49920E).unwrap();
        assert_eq!(parsed.database.as_deref(), Some("2026-10-24"));
        let text = render(&policy(parsed.findings, &ignored(), parsed.database));
        assert!(
            text.starts_with("lockfile:   2 finding(s) in Cargo.lock — "),
            "{text}"
        );
        assert!(
            text.contains("tokio 1.53.1 (RUSTSEC-2026-0386, unsound)"),
            "{text}"
        );
        assert!(text.contains("clipboard-win 5.4.1 (yanked)"), "{text}");
        assert!(
            text.contains("1 advisory(ies) ignored by deny.toml"),
            "{text}"
        );
        assert!(
            text.contains("`cargo update -p clipboard-win@5.4.1 -p tokio@1.53.1`"),
            "{text}"
        );
        assert!(!text.contains("atomic-polyfill"), "{text}");
    }

    #[test]
    fn a_vulnerability_is_a_finding_and_a_version_named_twice_is_updated_once() {
        let text = render(&policy(findings(VULN), &[], None));
        assert!(
            text.contains("x 1.0.0 (RUSTSEC-2026-0001, vulnerability)"),
            "{text}"
        );
        assert!(text.contains("x 1.0.0 (yanked)"), "{text}");
        assert!(text.contains("`cargo update -p x@1.0.0`"), "{text}");
    }

    #[test]
    fn two_locked_versions_of_one_crate_are_two_pins_and_one_ignored_advisory() {
        // The risk: `cargo update -p syn` refused as ambiguous when the lock
        // holds syn at two versions, as a lock with transitive
        // dependencies often does; and "2 advisories ignored" for one
        // advisory matched twice.
        let syn = |version: &str| Finding {
            krate: "syn".into(),
            version: version.into(),
            kind: "unsound".into(),
            id: Some("RUSTSEC-2026-0002".into()),
        };
        let text = render(&policy(vec![syn("1.0.0"), syn("2.0.0")], &[], None));
        assert!(
            text.contains("`cargo update -p syn@1.0.0 -p syn@2.0.0`"),
            "{text}"
        );
        let Report::Clean { ignored, .. } = policy(
            vec![syn("1.0.0"), syn("2.0.0")],
            &["RUSTSEC-2026-0002".into()],
            None,
        ) else {
            panic!()
        };
        assert_eq!(ignored, 1);
    }

    #[test]
    fn with_no_ignore_list_everything_is_a_finding() {
        // No `deny.toml` means nothing accepted: the line reports more,
        // never less.
        let Report::Findings { found, ignored } = policy(findings(MAIN_B49920E), &[], None) else {
            panic!()
        };
        assert_eq!((found.len(), ignored), (3, 0));
    }

    #[test]
    fn clean_names_the_database_date_it_is_clean_against() {
        // A clean verdict without its date would read as timeless, and the
        // whole reason this line exists is that a lock goes stale by time.
        let parsed = parse(CLEAN).unwrap();
        assert_eq!(parsed.findings, vec![]);
        let text = render(&policy(vec![], &[], parsed.database));
        assert_eq!(
            text,
            "lockfile:   Cargo.lock clean — cargo-audit against the RustSec database of 2026-10-25"
        );
        let only_ignored = render(&policy(
            findings(MAIN_B49920E),
            &["RUSTSEC-2023-0089".into(), "RUSTSEC-2026-0386".into()],
            None,
        ));
        // The yanked crate has no id and so cannot be ignored.
        assert!(only_ignored.contains("1 finding(s)"), "{only_ignored}");
        assert!(
            only_ignored.contains("2 advisory(ies) ignored by deny.toml"),
            "{only_ignored}"
        );
    }

    #[test]
    fn an_unreadable_report_is_not_checked_never_clean() {
        // The risk: an empty stdout (tool crashed before writing) parsing
        // to "no findings" and printing clean.
        assert!(parse("").is_err());
        assert!(parse("{}").is_err());
        assert!(parse(r#"{"vulnerabilities": {"list": [{"package": {"name": "a"}}]}}"#).is_err());
        // A `warnings` in another shape is an error too, not "no warnings":
        // the yanked crate that failed CI lives under `warnings`.
        assert!(parse(r#"{"vulnerabilities": {"list": []}, "warnings": []}"#).is_err());
        assert!(parse(r#"{"vulnerabilities": {"list": []}, "warnings": {"yanked": {}}}"#).is_err());
        let text = render(&Report::NotChecked(
            "cargo-audit: error: no such file".into(),
        ));
        assert!(
            text.starts_with(
                "lockfile:   Cargo.lock present; advisories not checked — cargo-audit: error: no such file"
            ),
            "{text}"
        );
        assert!(text.contains("cargo deny --locked check"), "{text}");
        assert!(!text.contains("clean"), "{text}");
    }

    #[test]
    fn the_tool_s_error_line_is_preferred_over_a_warning_before_it() {
        assert_eq!(
            error_line("warning: directory is locked\nerror: couldn't fetch\n"),
            "error: couldn't fetch"
        );
        assert_eq!(error_line("\n  only a warning  \n"), "only a warning");
        assert_eq!(error_line(""), "");
    }

    #[test]
    fn deny_toml_ignores_are_read_in_both_forms_and_a_missing_file_ignores_nothing() {
        let d = crate::testutil::scratch("doctor-supply-deny");
        let deny = d.join("deny.toml");
        assert_eq!(deny_ignores(&deny), Vec::<String>::new());
        std::fs::write(
            &deny,
            "[advisories]\nignore = [\n  \"RUSTSEC-2023-0089\",\n  { id = \"RUSTSEC-2024-0001\", reason = \"not compiled\" },\n]\n[bans]\nwildcards = \"deny\"\n",
        )
        .unwrap();
        assert_eq!(
            deny_ignores(&deny),
            vec![
                "RUSTSEC-2023-0089".to_string(),
                "RUSTSEC-2024-0001".to_string()
            ]
        );
        std::fs::write(&deny, "this is not toml = = =").unwrap();
        assert_eq!(deny_ignores(&deny), Vec::<String>::new());
    }

    #[test]
    fn a_root_without_a_lockfile_gets_no_line() {
        let d = crate::testutil::scratch("doctor-supply-nolock");
        assert_eq!(line(&d), None);
    }

    #[test]
    fn a_control_character_in_a_crate_name_is_escaped_on_the_line() {
        // Names come from the lockfile, which the repository supplies.
        let text = render(&Report::Findings {
            found: vec![Finding {
                krate: "a\u{1b}[2Jb".into(),
                version: "0.1.0".into(),
                kind: "yanked".into(),
                id: None,
            }],
            ignored: 0,
        });
        assert!(!text.contains('\u{1b}'), "{text}");
    }

    fn args<'a>(a: &'a [&'a str]) -> Vec<&'a OsStr> {
        a.iter().map(OsStr::new).collect()
    }

    /// A shell found on `PATH`, since `output_within` takes a path.
    fn shell(name: &str) -> PathBuf {
        on_path(name).unwrap_or_else(|| panic!("{name} on PATH"))
    }

    #[test]
    fn a_program_that_is_not_on_path_is_named_as_absent() {
        assert_eq!(on_path("trellis-no-such-tool-7f3a"), None);
        let err = output_within(
            Path::new("trellis-no-such-tool-7f3a"),
            &args(&["audit"]),
            Duration::from_secs(5),
        )
        .unwrap_err();
        assert_eq!(err, "trellis-no-such-tool-7f3a is not on PATH");
    }

    #[test]
    fn the_tool_is_looked_up_on_the_given_path_alone() {
        // The risk (Windows): std's resolver also searching the parent's
        // PATH and the system directories, so "not on PATH" would depend
        // on the host rather than on the PATH `doctor` was given. cmd.exe
        // lives in System32, which that resolver finds with no PATH at all;
        // this lookup must not.
        let name = if cfg!(windows) { "cmd" } else { "sh" };
        let empty = crate::testutil::scratch("doctor-supply-empty-path");
        assert_eq!(on_path_in(empty.as_os_str(), name), None);
        let real = std::env::var_os("PATH").expect("PATH is set");
        let found = on_path_in(&real, name).expect("the shell is on PATH");
        assert!(
            found.is_absolute() && found.is_file(),
            "{}",
            found.display()
        );
        // A relative entry is skipped even when the file is there.
        let relative: std::ffi::OsString = ".".into();
        assert_eq!(on_path_in(&relative, name), None);
    }

    #[test]
    fn a_program_that_outlives_the_bound_is_killed_and_named() {
        // The risk: `doctor` hanging on a database fetch that never ends.
        // Each argument is its own word so cmd.exe needs no quoting.
        let (program, a): (&str, &[&str]) = if cfg!(windows) {
            ("cmd", &["/C", "ping", "-n", "30", "127.0.0.1"])
        } else {
            ("sh", &["-c", "sleep 30"])
        };
        let started = Instant::now();
        let err = output_within(&shell(program), &args(a), Duration::from_millis(300)).unwrap_err();
        assert!(err.ends_with("did not finish within 0 s"), "{err}");
        assert!(started.elapsed() < Duration::from_secs(10));
    }

    #[test]
    fn stdout_and_stderr_both_come_back() {
        let (program, a): (&str, &[&str]) = if cfg!(windows) {
            ("cmd", &["/C", "echo", "out"])
        } else {
            ("sh", &["-c", "echo out"])
        };
        let got = output_within(&shell(program), &args(a), Duration::from_secs(10)).unwrap();
        assert!(got.stdout.contains("out"), "{:?}", got.stdout);
        // Something each shell writes to stderr without special characters.
        let (program, a): (&str, &[&str]) = if cfg!(windows) {
            ("cmd", &["/C", "trellis-no-such-command-7f3a"])
        } else {
            ("sh", &["-c", "echo err >&2"])
        };
        let got = output_within(&shell(program), &args(a), Duration::from_secs(10)).unwrap();
        assert!(!got.stderr.trim().is_empty(), "{:?}", got.stderr);
    }
}
