//! One pure function per dimension (standard §5 and §6).
//!
//! The invariant every rule holds: it abstains only when an input it cannot
//! read could change its answer. An abstention that fired on every ticket
//! would carry no information; a value chosen when the input could flip it
//! is the "wrong label that looks authoritative" §1 forbids. Where the
//! standard names a signal an issue does not carry (a diff, a path table for
//! rollback), the rule string or `docs/TICKETING.md` says so.

use std::collections::BTreeMap;

use crate::labels::{Area, Effort, Flag, Kind, Origin, Risk, Route, Surface};
use crate::parse::{self, FileRef};
use crate::{Outcome, State, Ticket};

fn decide<T>(value: T, rule: &'static str) -> Outcome<T> {
    Outcome::Decided { value, rule }
}

fn abstain<T>(rule: &'static str, reason: impl Into<String>) -> Outcome<T> {
    Outcome::Abstained {
        rule,
        reason: reason.into(),
    }
}

/// §5's area table, in its order, then the entries this repository adds
/// (`docs/TICKETING.md`): the crate and directory whose names are the area's,
/// and the owner's mapping of this repository's other crates onto the same
/// twelve areas (#194, 2026-10-04: no new area values). Every other path
/// maps to nothing, and a ticket naming one abstains.
const AREA_PATHS: [(&str, Area); 21] = [
    ("crates/trellis-context", Area::Context),
    ("crates/trellis-model", Area::Model),
    ("crates/trellis-checks", Area::Checks),
    ("crates/trellis-runtime", Area::Runtime),
    ("crates/trellis-skills", Area::Skills),
    ("crates/trellis-agents", Area::Agents),
    ("protocol", Area::Protocol),
    ("crates/trellis-protocol", Area::Protocol),
    ("tui", Area::Tui),
    ("cmd/trellis-tui", Area::Tui),
    ("crates/trellis-cli", Area::Cli),
    ("docs", Area::Docs),
    ("crates/trellis-index", Area::Graph),
    ("crates/trellis-loop", Area::Runtime),
    ("crates/trellis-vcs", Area::Runtime),
    ("crates/trellis-vault", Area::Context),
    ("crates/trellis-queue", Area::Agents),
    ("crates/trellis-trail", Area::Protocol),
    ("crates/trellis-core", Area::Build),
    ("scripts", Area::Build),
    (".github", Area::Build),
];

fn area_of(file: &FileRef) -> Option<Area> {
    AREA_PATHS
        .iter()
        .find(|(prefix, _)| file.under(prefix))
        .map(|(_, a)| *a)
}

/// The distinct areas of the files that map to one, in the order listed.
fn mapped_areas(files: &[FileRef]) -> Vec<Area> {
    let mut out = Vec::new();
    for a in files.iter().filter_map(area_of) {
        if !out.contains(&a) {
            out.push(a);
        }
    }
    out
}

fn files_or<T>(t: &Ticket, rule: &'static str) -> Result<Vec<FileRef>, Outcome<T>> {
    match parse::files(&t.body) {
        Ok(f) if f.is_empty() => Err(abstain(rule, "the `**Files:**` line names no files")),
        Ok(f) => Ok(f),
        Err(reason) => Err(abstain(rule, reason)),
    }
}

/// `area:`, from the `**Files:**` line (§5). Zero files, a file under no
/// path in the table, or more than two areas abstains: spanning areas
/// usually means the ticket is a story.
pub fn area(t: &Ticket) -> Outcome<Area> {
    const RULE: &str = "§5 area: from file paths";
    let files = match files_or(t, RULE) {
        Ok(f) => f,
        Err(o) => return o,
    };
    if let Some(unmapped) = files.iter().find(|f| area_of(f).is_none()) {
        return abstain(
            RULE,
            format!("`{}` is under no path in the area table", unmapped.path),
        );
    }
    let areas = mapped_areas(&files);
    match areas.as_slice() {
        [one] => decide(*one, RULE),
        // Two areas are allowed (they make risk medium); the standard does
        // not say which one labels the ticket, so the first-listed file's
        // does, as the issues list the file the work mainly changes first.
        [first, _] => decide(
            *first,
            "§5 area: from file paths, first-listed of two areas",
        ),
        _ => abstain(
            RULE,
            format!(
                "files span {} areas ({}): probably a story",
                areas.len(),
                areas
                    .iter()
                    .map(|a| a.label())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        ),
    }
}

fn title_words(title: &str) -> Vec<String> {
    title
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect()
}

/// Whether every listed file satisfies `pred`: `None` when a directory
/// entry leaves it unknown and no listed file already fails it.
fn only(files: &[FileRef], pred: impl Fn(&FileRef) -> bool) -> Option<bool> {
    if files.iter().any(|f| !f.is_dir && !pred(f)) {
        return Some(false);
    }
    if files.iter().any(|f| f.is_dir) {
        return None;
    }
    Some(true)
}

fn is_chore_file(f: &FileRef) -> bool {
    matches!(f.file_name(), "Cargo.toml" | "go.mod") || f.under(".github/workflows")
}

/// `kind:`, by §5's table, first matching row wins.
pub fn kind(t: &Ticket) -> Outcome<Kind> {
    const FILES_RULE: &str = "§5 kind: only tests, docs or build files touched";
    let words = title_words(&t.title);
    let Some(first) = words.first() else {
        return abstain("§4 intake", "the title is empty");
    };
    if t.has_label("type:story") {
        return abstain(
            "§5 kind",
            "a story is not classified as a ticket; its tickets are",
        );
    }
    if matches!(first.as_str(), "fix" | "repair" | "correct") {
        return decide(Kind::Bug, "§5 kind: title starts with fix/repair/correct");
    }
    if t.has_label(Origin::Check.label()) {
        return decide(Kind::Bug, "§5 kind: source: check");
    }

    let files = match parse::files(&t.body) {
        Ok(f) => f,
        Err(reason) => {
            return abstain(
                FILES_RULE,
                format!(
                    "{reason}, so whether only tests, docs or build files are touched is unknown"
                ),
            );
        }
    };
    type Row = (fn(&FileRef) -> bool, Kind, &'static str, &'static str);
    let rows: [Row; 3] = [
        (
            |f| f.path.ends_with("_test.rs") || f.path.ends_with("_test.go"),
            Kind::Test,
            "§5 kind: only *_test.rs / *_test.go touched",
            "test files",
        ),
        (
            |f| f.path.ends_with(".md"),
            Kind::Docs,
            "§5 kind: only *.md touched",
            "Markdown",
        ),
        (
            is_chore_file,
            Kind::Chore,
            "§5 kind: only Cargo.toml, go.mod or CI files touched",
            "manifests and CI files",
        ),
    ];
    if !files.is_empty() {
        for (pred, kind, rule, what) in rows {
            match only(&files, pred) {
                Some(true) => return decide(kind, rule),
                Some(false) => {}
                None => {
                    let dir = files.iter().find(|f| f.is_dir).map_or("", |f| &f.path);
                    return abstain(
                        FILES_RULE,
                        format!("`{dir}` is a directory, so whether only {what} change is unknown"),
                    );
                }
            }
        }
    }

    const REFACTOR: [&str; 5] = ["rename", "extract", "inline", "move", "simplify"];
    if words.iter().any(|w| REFACTOR.contains(&w.as_str())) {
        return decide(
            Kind::Refactor,
            "§5 kind: title contains rename/extract/inline/move/simplify",
        );
    }
    if t.has_label("type:spike") {
        return decide(Kind::Spike, "§5 kind: type is spike");
    }
    decide(Kind::Feature, "§5 kind: otherwise")
}

/// A crate other than the `trellis` binary's, whose `src/` holds a library
/// API. `trellis-cli` is binary-only (it has no `[lib]` target).
fn in_library_src(f: &FileRef) -> bool {
    let Some(root) = parse::crate_of(&f.path) else {
        return f.is_dir && f.path == "crates";
    };
    if root == "crates/trellis-cli" {
        return false;
    }
    f.under(&format!("{root}/src")) || (f.is_dir && f.path == root)
}

fn is_command_definition(f: &FileRef) -> bool {
    f.under("cli")
        || f.under("crates/trellis-cli/src")
        || (f.is_dir && (f.path == "crates/trellis-cli" || f.path == "crates"))
}

/// `surface:`, by §5's table, first matching row wins.
///
/// The `surface:public` row needs "a `pub fn` or `pub struct` diff in a lib
/// crate", and an issue has no diff. A file in a library crate's `src/`
/// could be either, so the rule abstains on it rather than calling it
/// internal.
pub fn surface(t: &Ticket) -> Outcome<Surface> {
    const RULE: &str = "§5 surface";
    let files = match files_or(t, RULE) {
        Ok(f) => f,
        Err(o) => return o,
    };
    if files.iter().any(|f| f.under("protocol")) {
        return decide(Surface::Protocol, "§5 surface: protocol/** touched");
    }
    if let Some(lib) = files.iter().find(|f| in_library_src(f)) {
        return abstain(
            "§5 surface: a pub fn or pub struct diff in a lib crate",
            format!(
                "`{}` is in a library crate's src/; whether it changes a pub fn or pub struct needs the diff",
                lib.path
            ),
        );
    }
    if files.iter().any(is_command_definition) {
        return decide(
            Surface::Cli,
            "§5 surface: a command definition (crates/trellis-cli/src) touched",
        );
    }
    decide(Surface::Internal, "§5 surface: otherwise")
}

/// Where rollback, worktree teardown and session persistence live, by the
/// owner's table (#196, 2026-10-04): a directory or a file. `session*.rs`
/// under `crates/trellis-cli/src` is matched by name in [`touches_persistence`].
const PERSISTENCE_PATHS: [&str; 5] = [
    "crates/trellis-vcs",
    "crates/trellis-trail",
    "crates/trellis-loop/src/integrate.rs",
    "crates/trellis-loop/src/partition.rs",
    "crates/trellis-loop/src/supervise.rs",
];

/// Whether `f` is in [`PERSISTENCE_PATHS`] or is a `session*.rs` in the CLI.
/// A directory entry counts when it contains one of them: it names an
/// unknown set of files, and the safe reading is that the set includes it.
fn touches_persistence(f: &FileRef) -> bool {
    let contains = |p: &str| {
        f.under(p) || (f.is_dir && p.starts_with(&format!("{}/", f.path.trim_end_matches('/'))))
    };
    PERSISTENCE_PATHS.iter().any(|p| contains(p))
        || (f.path.starts_with("crates/trellis-cli/src/session") && f.path.ends_with(".rs"))
        || contains("crates/trellis-cli/src/session.rs")
}

/// `risk:` — the important one (§5).
///
/// High triggers are checked on the files themselves, so a checks file
/// makes the ticket high even when `area` abstained on another file.
/// "Touches rollback, worktree teardown, or session persistence" is read
/// from the owner's path table, [`PERSISTENCE_PATHS`]. `unsafe` cannot enter this workspace
/// without a `Cargo.toml` edit (`unsafe_code = "forbid"`), so a manifest in
/// the files abstains instead. An on-disk format change is expected to carry
/// `!breaking`.
pub fn risk(t: &Ticket) -> Outcome<Risk> {
    const RULE: &str = "§5 risk";
    if t.has_label(Flag::Breaking.label()) || t.has_label(Flag::Security.label()) {
        return decide(Risk::High, "§5 risk:high: !breaking or !security set");
    }
    let files = match files_or(t, RULE) {
        Ok(f) => f,
        Err(o) => return o,
    };
    if files.iter().any(|f| f.under("crates/trellis-checks")) {
        return decide(Risk::High, "§5 risk:high: area:checks, always");
    }
    if files
        .iter()
        .any(|f| f.under("protocol") || f.under("crates/trellis-protocol"))
    {
        return decide(Risk::High, "§5 risk:high: area:protocol");
    }
    if files.iter().any(touches_persistence) {
        return decide(
            Risk::High,
            "§5 risk:high: touches rollback, worktree teardown or session persistence (#196's path table)",
        );
    }
    if let Some(manifest) = files.iter().find(|f| f.file_name() == "Cargo.toml") {
        return abstain(
            "§5 risk:high: introduces unsafe",
            format!(
                "`{}` can lift the workspace's `unsafe_code = \"forbid\"`; whether the change introduces unsafe needs the diff",
                manifest.path
            ),
        );
    }

    if files.len() > 5 {
        return decide(Risk::Medium, "§5 risk:medium: more than five files");
    }
    let surface = surface(t);
    if surface.decided().is_some_and(|s| *s != Surface::Internal) {
        return decide(Risk::Medium, "§5 risk:medium: surface above internal");
    }
    if mapped_areas(&files).len() >= 2 {
        return decide(Risk::Medium, "§5 risk:medium: files span two areas");
    }
    if let Outcome::Abstained { reason, .. } = surface {
        return abstain(RULE, format!("surface abstained: {reason}"));
    }
    if let Outcome::Abstained { reason, .. } = area(t) {
        return abstain(RULE, format!("area abstained: {reason}"));
    }
    if let Some(dir) = files.iter().find(|f| f.is_dir) {
        return abstain(
            RULE,
            format!(
                "`{}` is a directory, so whether more than five files change is unknown",
                dir.path
            ),
        );
    }
    decide(
        Risk::Low,
        "§5 risk:low: no trigger in files, labels or surface",
    )
}

/// `effort:`, by §5's table: 1-2 files and 0-1 new tests is `s`, 6 or more
/// files is `l`, anything between is `m`. New tests are counted from the
/// Verify block's `verify-tests.sh` lines, the one count an issue records.
pub fn effort(t: &Ticket) -> Outcome<Effort> {
    const RULE: &str = "§5 effort: from files and new tests";
    let files = match files_or(t, RULE) {
        Ok(f) => f,
        Err(o) => return o,
    };
    if files.len() >= 6 {
        return decide(Effort::L, "§5 effort: 6+ files, split it");
    }
    if let Some(dir) = files.iter().find(|f| f.is_dir) {
        return abstain(
            RULE,
            format!("`{}` is a directory; its file count is unknown", dir.path),
        );
    }
    let tests = parse::new_tests(&parse::verify_commands(&t.body));
    if files.len() >= 3 || tests >= 2 {
        decide(Effort::M, "§5 effort: 3-5 files or 2+ new tests")
    } else {
        decide(Effort::S, "§5 effort: 1-2 files, 0-1 new tests")
    }
}

/// `origin:` is where the ticket came from, which the path that created it
/// records (§2's Sources table: `new` is manual, `scan` is code, ...). It is
/// not in the text, so the rule reads the one recorded `origin:` label.
pub fn origin(t: &Ticket) -> Outcome<Origin> {
    const RULE: &str = "§2 origin: recorded when the ticket is created";
    let recorded: Vec<&String> = t
        .labels
        .iter()
        .filter(|l| l.starts_with("origin:"))
        .collect();
    match recorded.as_slice() {
        [] => abstain(RULE, "no `origin:` label is recorded"),
        [one] => match Origin::from_label(one) {
            Some(o) => decide(o, RULE),
            None => abstain(RULE, format!("`{one}` is not an origin in §3")),
        },
        many => abstain(
            RULE,
            format!(
                "{} `origin:` labels; a dimension holds one value",
                many.len()
            ),
        ),
    }
}

/// Whether `!needs-adr` applies (§5), with the reason it cannot be told.
pub(crate) fn needs_adr(t: &Ticket) -> Result<bool, String> {
    let surface = surface(t);
    if matches!(surface.decided(), Some(Surface::Protocol | Surface::Public)) {
        return Ok(true);
    }
    let risk = risk(t);
    let kind = kind(t);
    if risk.decided() == Some(&Risk::High) && kind.decided().is_some_and(|k| *k != Kind::Bug) {
        return Ok(true);
    }
    // Nothing has set it. Anything still unknown could.
    if let Outcome::Abstained { reason, .. } = surface {
        return Err(format!("surface abstained: {reason}"));
    }
    if let Outcome::Abstained { reason, .. } = risk {
        return Err(format!("risk abstained: {reason}"));
    }
    if risk.decided() == Some(&Risk::High)
        && let Outcome::Abstained { reason, .. } = kind
    {
        return Err(format!("risk is high and kind abstained: {reason}"));
    }
    if let Ok(files) = parse::files(&t.body)
        && let Some(m) = files
            .iter()
            .find(|f| matches!(f.file_name(), "Cargo.toml" | "Cargo.lock"))
    {
        return Err(format!(
            "`{}` is in the files; whether a dependency is added, removed or version-changed needs the diff",
            m.path
        ));
    }
    Ok(false)
}

/// The flags (§3, §5).
///
/// - `!breaking`, `!security` and `!blocked-ext` have no derivation in the
///   standard, which uses them as inputs; they are carried from the labels.
/// - `!needs-adr` is derived by §5's list.
/// - `!agent-ok` needs all six of §5's conditions shown, and rule 2 ("no
///   Done when line needing judgment") has no deterministic check here. The
///   flag asserts the six were shown, so not shown is its defined state:
///   absent. The rules therefore never set it (docs/TICKETING.md).
pub fn flags(t: &Ticket) -> Outcome<Vec<Flag>> {
    const RULE: &str = "§5 flags: !breaking, !security, !blocked-ext as recorded; !needs-adr by §5; !agent-ok never set, rule 2 has no deterministic check here";
    let mut out: Vec<Flag> = [Flag::Breaking, Flag::Security, Flag::BlockedExt]
        .into_iter()
        .filter(|f| t.has_label(f.label()))
        .collect();
    match needs_adr(t) {
        Ok(true) => out.push(Flag::NeedsAdr),
        Ok(false) => {}
        Err(reason) => return abstain("§5 !needs-adr", reason),
    }
    out.sort();
    decide(out, RULE)
}

/// The ready gate's verdict: every failure by name, empty when ready.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Readiness {
    pub failures: Vec<String>,
}

impl Readiness {
    pub fn is_ready(&self) -> bool {
        self.failures.is_empty()
    }
}

/// `ready:` (§5): every condition, each failure reported by name.
///
/// `blockers` is the state of each issue named in `blocked_by`; the caller
/// looks them up. A blocker missing from it abstains, since an unknown
/// dependency is not a satisfied one. "Status is draft or todo" is read as
/// the issue being open. "The ADR exists and is accepted" cannot be read
/// from an issue, so `!needs-adr` always fails the gate by name.
pub fn ready(t: &Ticket, blockers: &BTreeMap<u64, State>) -> Outcome<Readiness> {
    const RULE: &str = "§5 ready";
    let mut failures = Vec::new();
    if t.state == State::Closed {
        failures.push("issue is CLOSED".to_string());
    }
    match parse::section(&t.body, "Done when") {
        None => failures.push("no '## Done when' section".to_string()),
        Some(lines) if lines.iter().all(|l| l.trim().is_empty()) => {
            failures.push("'## Done when' is empty".to_string());
        }
        Some(_) => {}
    }
    if parse::verify_commands(&t.body).is_empty() {
        failures.push("no verify command".to_string());
    }
    let words = parse::decision_words(&t.title, &t.body);
    if !words.is_empty() {
        failures.push(format!("decision words: {}", words.join(", ")));
    }
    match effort(t) {
        Outcome::Decided {
            value: Effort::L, ..
        } => failures.push("effort:l".to_string()),
        Outcome::Decided { .. } => {}
        Outcome::Abstained { reason, .. } => {
            return abstain(RULE, format!("effort abstained: {reason}"));
        }
    }
    let blocked = match parse::blocked_by(&t.body) {
        Ok(b) => b,
        Err(reason) => return abstain("§4 intake", reason),
    };
    for n in blocked {
        match blockers.get(&n) {
            Some(State::Closed) => {}
            Some(State::Open) => failures.push(format!("blocked by #{n} (OPEN)")),
            None => return abstain(RULE, format!("the state of blocker #{n} was not supplied")),
        }
    }
    match needs_adr(t) {
        Ok(true) => failures.push("needs ADR".to_string()),
        Ok(false) => {}
        Err(reason) => return abstain(RULE, format!("!needs-adr unknown: {reason}")),
    }
    decide(Readiness { failures }, RULE)
}

/// The route and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Triage {
    pub route: Route,
    pub reasons: Vec<String>,
}

/// `triage:` (§6).
///
/// Any dimension that abstained, an agent's ticket, or `risk:high` routes to
/// a person. Otherwise an open blocker or `!blocked-ext` is `blocked`, and a
/// ready ticket is `auto`. A ticket that is none of those (not ready, and
/// not blocked) fits no row of §6's table, so it too goes to a person, with
/// its failures as the reason. Triage itself abstains only on intake it
/// cannot read, or a blocker whose state was not supplied.
pub fn triage(t: &Ticket, blockers: &BTreeMap<u64, State>) -> Outcome<Triage> {
    if t.title.trim().is_empty() {
        return abstain("§4 intake", "the title is empty");
    }
    if t.has_label("type:story") {
        return abstain(
            "§6 triage",
            "a story is not triaged as a ticket; its tickets are",
        );
    }
    let blocked = match parse::blocked_by(&t.body) {
        Ok(b) => b,
        Err(reason) => return abstain("§4 intake", reason),
    };
    let mut open_blockers = Vec::new();
    for n in blocked {
        match blockers.get(&n) {
            Some(State::Closed) => {}
            Some(State::Open) => open_blockers.push(n),
            None => {
                return abstain(
                    "§6 triage",
                    format!("the state of blocker #{n} was not supplied"),
                );
            }
        }
    }

    fn why<T>(o: &Outcome<T>) -> Option<&str> {
        match o {
            Outcome::Abstained { reason, .. } => Some(reason),
            Outcome::Decided { .. } => None,
        }
    }
    let risk = risk(t);
    let origin = origin(t);
    let flags = flags(t);
    let mut human: Vec<String> = [
        ("area", why(&area(t)).map(str::to_string)),
        ("kind", why(&kind(t)).map(str::to_string)),
        ("surface", why(&surface(t)).map(str::to_string)),
        ("risk", why(&risk).map(str::to_string)),
        ("effort", why(&effort(t)).map(str::to_string)),
        ("origin", why(&origin).map(str::to_string)),
        ("flags", why(&flags).map(str::to_string)),
    ]
    .into_iter()
    .filter_map(|(name, reason)| reason.map(|r| format!("{name} abstained: {r}")))
    .collect();
    if origin.decided() == Some(&Origin::Agent) {
        human.push("origin:agent: an agent's ticket is reviewed before promotion (§2)".to_string());
    }
    if risk.decided() == Some(&Risk::High) {
        human.push(format!("risk:high ({})", risk.rule()));
    }
    if !human.is_empty() {
        return decide(
            Triage {
                route: Route::NeedsHuman,
                reasons: human,
            },
            "§6 needs-human: a dimension abstained, source: agent, or risk:high",
        );
    }

    let mut waiting: Vec<String> = open_blockers
        .iter()
        .map(|n| format!("blocked by #{n} (OPEN)"))
        .collect();
    if flags
        .decided()
        .is_some_and(|f| f.contains(&Flag::BlockedExt))
    {
        waiting.push("!blocked-ext: waiting on something outside the repo".to_string());
    }
    if !waiting.is_empty() {
        return decide(
            Triage {
                route: Route::Blocked,
                reasons: waiting,
            },
            "§6 blocked: a blocked_by is unresolved, or !blocked-ext",
        );
    }

    match ready(t, blockers) {
        Outcome::Decided { value, .. } if value.is_ready() => decide(
            Triage {
                route: Route::Auto,
                reasons: vec!["classified confidently and ready".to_string()],
            },
            "§6 auto: classified confidently, ready: true",
        ),
        Outcome::Decided { value, .. } => decide(
            Triage {
                route: Route::NeedsHuman,
                reasons: value
                    .failures
                    .iter()
                    .map(|f| format!("not ready: {f}"))
                    .collect(),
            },
            "§6: not ready and not blocked fits no auto row, so a person decides",
        ),
        Outcome::Abstained { reason, .. } => {
            abstain("§6 triage", format!("ready abstained: {reason}"))
        }
    }
}
