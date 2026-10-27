//! The decision trail: one JSON document per run, committed to the repo.
//!
//! Future agents read this as context, which is what makes the split
//! between `factual` and `judgment` structural rather than cosmetic. An
//! errant judgment entry filed next to facts propagates indefinitely (S11).
//!
//! It also makes the trail an injection surface: it lives in a repository
//! that may take contributions, so anything read back from it is untrusted
//! input (S10).

pub mod query;
mod store;
mod untrusted;

pub use store::{iso_now, latest, now, slugify, timestamp_now, write};
pub use untrusted::{Integrity, RecordedCheck, UntrustedFactual, UntrustedTrail};

use serde::{Deserialize, Serialize};
use trellis_checks::{AuditMode, Verdict};

/// What happened. Machine-checkable, every entry produced by something that
/// executed.
#[derive(Debug, Clone, Serialize)]
pub struct Factual {
    /// Paths the run proposed to change.
    pub proposed: Vec<String>,
    pub radius: RadiusCounts,
    /// Verdicts, in the order the checks ran. `Verdict` serializes but does
    /// not deserialize, so reading a trail back yields inert
    /// `RecordedCheck` data rather than a live verdict.
    pub checks: Vec<Verdict>,
    /// Which checks ran, in order, and whether each passed.
    ///
    /// `checks` above holds `Verdict`s, which cannot be constructed outside
    /// `trellis-checks` — that seal is what makes Constraint 2 structural, and
    /// it also means a trail written by the CLI cannot carry them. This field
    /// records the same sequence as inert data so the factual section says
    /// what actually ran, rather than being silently empty.
    #[serde(default)]
    pub check_outcomes: Vec<CheckOutcome>,
    pub approved_by: Approval,
    pub mode_counts: ModeCounts,
    /// Prompt size across the run, in **bytes** (v0.2 gate: context fill).
    ///
    /// Bytes rather than tokens because TRELLIS does not tokenize — the
    /// runtime does — and `bytes / 4` under a token-shaped name would be an
    /// invented figure. Absent for a scripted run, which has no prompt.
    #[serde(default)]
    pub context: Option<ContextFill>,
    /// What the agent did, in the order it did it.
    ///
    /// The trail recorded what a run *proposed* and what the checks *said*,
    /// and nothing about what it **did**. So the first question a reader asks
    /// — what happened, in order — could only be answered from a terminal
    /// scrollback that no longer exists, and a run that ended having touched
    /// nothing was indistinguishable from one that never started.
    ///
    /// `#[serde(default)]` so a trail written before this field reads back as
    /// a run with no actions recorded, which is what it is. That is not the
    /// same claim as "a run that took no actions", and `trellis log` says
    /// which of the two it is looking at rather than printing "0 actions" for
    /// both.
    #[serde(default)]
    pub actions: Vec<Action>,
    /// Where the control dial stood for this run (ADR 0018): `auto`, or
    /// `approve (<tools>)`.
    ///
    /// Hands-off and fine-grained runs go through the same loop, checks and
    /// trail, and the only difference is which gates were installed — so
    /// without this a reader cannot tell which kind of run they hold. Omitted
    /// when unknown, so a trail written before the dial existed keeps its
    /// shape and its digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub control: Option<String>,
    /// Completion tokens the runtime adapter reported for the run
    /// (ADR 0026 §8). The budget ledger sums this.
    ///
    /// Completion only. Prompt tokens are `prompt_tokens`, below, and the
    /// two are never summed: a budget over this limits generated tokens,
    /// not billed ones.
    /// `None` for a scripted run and for a process agent, whose spend
    /// trellis cannot see — not a zero, which would read as a free run.
    /// Omitted when absent, like `control`, so older trails keep their digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens: Option<u64>,
    /// Prompt tokens the runtime adapter reported for the run, when it
    /// reported any.
    ///
    /// A separate field rather than a larger `tokens`, so the two figures are
    /// never added into one number: the ledger budgets over `tokens` as
    /// generated output, and prompt tokens folded into it would make that
    /// budget bill for context instead. Nothing in this crate adds them.
    ///
    /// Absent means unknown — a trail written before this field existed, a
    /// scripted run, a process agent, an adapter that did not say — never
    /// zero. Omitted when absent, like `tokens`, so a trail written before
    /// the field keeps its shape and its digest, and `SCHEMA` stays 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prompt_tokens: Option<u64>,
    /// The agent profile the run executed as (ADR 0026 §4), when one did.
    ///
    /// Recorded in the trail rather than joined from the queue: the queue is
    /// per worktree and dies with it, and the trail is what outlives both.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    /// The label a person gave the session the run started from (`/name`
    /// in `trellis session`), when one was set. At most 80 characters, no
    /// control characters; the session refuses anything else.
    ///
    /// A name for finding a run again, and nothing more. It is typed by a
    /// person and is not a fact about what executed, so it is recorded here
    /// only because `factual` is the tamper-evident half, and it is never
    /// rendered into the context a resumed run reads
    /// ([`UntrustedTrail::context_lines`]): a label reaching a prompt would
    /// be instruction-shaped text with no fence around it.
    ///
    /// Omitted when unset, like `agent`, so a trail written without one (or
    /// before the field existed) keeps its bytes and its digest, and
    /// `SCHEMA` stays 1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// What the run was given beyond the task and the repository's own
    /// `.trellis/` files (ADR 0025): context files, the system prompt files
    /// and skills the person forced, each by name and size. Never contents:
    /// a trail is committed, and a user-level file is not the repository's
    /// to publish.
    ///
    /// Two runs from one commit can differ by the user's own files, and this
    /// is what says so afterwards. Omitted when empty, so a trail with none
    /// keeps its shape and its digest.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<Input>,
    /// What the person running the task typed while it ran (ADR 0034):
    /// each steer and follow-up, verbatim up to [`STEER_TEXT_MAX_CHARS`],
    /// with the turn it belongs to.
    ///
    /// Without it a trail shows the model changing course at turn 4 and
    /// not that a person told it to, which reads as the model's own
    /// judgment. Inside `factual`, so under the digest: an edit that puts
    /// words in the user's mouth is as visible as any other. Omitted when
    /// empty, so a trail with no steering keeps its shape and its digest.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub steering: Vec<Steer>,
    /// The person's run controls that took effect (ADR 0058 §6, ADR 0007's
    /// amendment of 2026-10-27): each pause, raised ceiling and withdrawn
    /// message, in the order the loop recorded them.
    ///
    /// Not in `steering`, which says a text reached the model or waited for
    /// a later run; a withdrawn message did neither. Inside `factual`, so
    /// under the digest. Omitted when empty, so a run with no control keeps
    /// its shape and its digest.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub controls: Vec<Control>,
    /// Integrated-check failures that named files changed by two or more
    /// partitions (ADR 0005, ADR 0042): a collision the partition plan did
    /// not predict, and so evidence the graph's recall missed an edge.
    ///
    /// Suspected, not established: the record says which merged files a
    /// failing check's own output named, not that their pairing caused it.
    /// Built by `trellis-loop` from that output alone, never from model
    /// text. Omitted when empty, so a single-agent trail keeps its shape and
    /// its digest.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub suspected_recall_gaps: Vec<SuspectedRecallGap>,
}

/// One integrated-check failure that named files from more than one
/// partition, as `Factual::suspected_recall_gaps` records it.
///
/// Carries no `passed`: it is recorded only for a check that failed, and a
/// failing verdict is already in `check_outcomes`. Nothing here is a
/// correctness claim (ADR 0004).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuspectedRecallGap {
    /// The partitions whose merged files the failure named, ascending.
    pub partitions: Vec<u32>,
    /// Those files, root-relative with `/` separators, in the order merged.
    pub files: Vec<String>,
    /// The check that failed, spelled as `CheckOutcome::kind` spells it
    /// (`Typecheck`, `Tests`). A string rather than `CheckKind`, so a trail
    /// naming a kind this build does not know still reads back.
    pub check: String,
}

/// The most characters of one steer's text a trail keeps (ADR 0034).
///
/// A person can paste a log into the steering box, and a trail is
/// committed; the cut bounds what one message adds to the repository,
/// and [`Steer::len`] keeps the true size so the cut is never silent.
pub const STEER_TEXT_MAX_CHARS: usize = 2_000;

/// Which channel a person's mid-run text arrived on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SteerKind {
    /// Delivered into the running task as user text in the next request.
    Steer,
    /// Set aside for a later run; never delivered into this one.
    FollowUp,
}

/// One message a person sent while a run was in progress, as
/// `Factual::steering` records it.
///
/// User input, not a fact about the code and not an instruction to
/// anyone reading the trail later: read back, it goes through
/// `UntrustedTrail::context_lines`, which fences it (ADR 0007).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Steer {
    /// 1-based, matching `Action::turn`: the turn whose request carried a
    /// steer, or the turn the run had reached when a follow-up arrived.
    pub turn: u32,
    pub kind: SteerKind,
    /// The text as typed, cut at [`STEER_TEXT_MAX_CHARS`] characters.
    pub text: String,
    /// The text's true length in characters, the unit the cut is
    /// measured in. Larger than `text`'s own length exactly when it was cut.
    pub len: u64,
}

impl Steer {
    /// The one way a writer should build a record, so no path can store
    /// more than the cut allows.
    ///
    /// Cut by characters, not bytes: a byte offset at 2,000 can land
    /// inside a multi-byte character and would either panic or store
    /// invalid text.
    pub fn new(turn: u32, kind: SteerKind, text: &str) -> Self {
        Steer {
            turn,
            kind,
            text: text.chars().take(STEER_TEXT_MAX_CHARS).collect(),
            len: text.chars().count() as u64,
        }
    }
}

/// One run control a person sent that took effect, as `Factual::controls`
/// records it.
///
/// Tagged `control` rather than `kind`, because a withdrawal keeps a steering
/// entry's own `kind` and `len`. None of it is a correctness claim, and there
/// is no pass variant (ADR 0004). Read back, every field is data written by
/// whoever last edited the file (ADR 0007): a recorded ceiling is never a
/// budget.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "control", rename_all = "snake_case")]
pub enum Control {
    /// The loop held before model call `turn`, for `paused_ms`, the
    /// `resumed` event's figure.
    Pause { turn: u32, paused_ms: u64 },
    /// A ceiling the budget check before model call `turn` first enforced
    /// at `new`, in the `run budget` event's name and units. One entry per
    /// ceiling that changed.
    Raise {
        turn: u32,
        /// `max_iterations`, `max_wall_clock_ms` or `max_completion_tokens`.
        /// A string, as `SuspectedRecallGap::check` is, so a name this build
        /// does not know still reads back.
        ceiling: String,
        old: u64,
        new: u64,
    },
    /// A message taken back before it reached the model or a later run.
    /// `turn` is the one its steering entry would have carried.
    ///
    /// No text, by the owner's decision of 2026-10-27 (ADR 0007's #868
    /// amendment): a committed trail is a file a later run's model can read
    /// through `read_file` or `bash`, and it would see what the person took
    /// back.
    Withdraw {
        turn: u32,
        kind: SteerKind,
        /// The message's length in characters, as `Steer::len` counts it.
        len: u64,
    },
    /// A control written by a newer build. Kept so its trail still parses
    /// rather than being refused whole; nothing reads anything from it.
    #[serde(other)]
    Unrecognised,
}

/// One file a run was given, as `Factual::inputs` records it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Input {
    /// `context_file`, `system_file`, `forced_skill` or `image`.
    pub kind: String,
    /// As the run's `repo:` line names it: `CLAUDE.md`,
    /// `<user>/AGENTS.md`, `.trellis/system.md`, or a skill's name; for an
    /// image, its repository-relative path, or `<outside>/<file name>` for
    /// one outside the repository. Never an absolute path, which would put
    /// the user's home layout in the trail.
    pub name: String,
    /// Bytes of content sent, the unit the caps are measured in; for an
    /// image, the file's size before encoding.
    pub bytes: u64,
    /// SHA-256 of the content, lowercase hex, for an input whose contents
    /// the trail may not hold but whose identity it must: an image
    /// (ADR 0041). Omitted when absent, so the other kinds keep their shape
    /// and every trail already committed keeps its digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

impl Input {
    /// An input recorded by name, size and hash, never by content.
    pub fn hashed(kind: &str, name: &str, content: &[u8]) -> Self {
        Self {
            kind: kind.to_string(),
            name: name.to_string(),
            bytes: content.len() as u64,
            sha256: Some(sha256_hex(content)),
        }
    }
}

/// One thing the agent did, and what came back.
///
/// **Not a verdict, and shaped so it cannot be read as one** (ADR 0004). There
/// is no `passed` here: an edit that applied is not an edit that was correct,
/// and the only field that carries a correctness claim is `check_outcomes`,
/// which comes from a `Verdict` that only an executed process can make.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Action {
    /// 1-based, matching the turn numbering a run prints.
    pub turn: u32,
    /// The tool, as the model named it: `blast_radius`, `read_file`, `edit`,
    /// `run_checks`, `read_skill`.
    ///
    /// Or `approval`: a person's answer in approve mode (ADR 0018), recorded
    /// just before the call it decided, under the same turn, with `outcome`
    /// `approved` or `declined`. A human decision is a fact about the run
    /// like any other, and a declined edit must be findable afterwards.
    pub tool: String,

    /// What it was aimed at — a path, a symbol, the checks requested. Empty
    /// when the call named nothing, which `run_checks` with no paths does.
    pub target: String,
    /// What came back, in one line.
    ///
    /// A rendering for a reader, not the observation the model saw: that runs
    /// to thousands of lines for a check and is truncated before it ever
    /// reaches the model. A trail that stored it would be mostly type-checker
    /// output.
    pub outcome: String,
    /// Whether the working tree changed.
    ///
    /// The one bit a reader most wants and cannot infer: `proposed` says which
    /// files a run ended up touching, not which of its actions did the
    /// touching. A run whose every edit was refused has an empty `proposed`
    /// and a full list of actions, and those are different stories.
    ///
    /// Set by `edit` and `write_file` only. A `bash` or custom-tool call
    /// leaves it false; what one of those changed is in `changed`.
    pub wrote: bool,
    /// For a `bash` or custom-tool call: the repository paths trellis
    /// carried back from the command's throwaway clone and wrote (ADR 0023
    /// §7, ADR 0032), at most [`CHANGED_CAP`] of them, in the order they
    /// were written.
    ///
    /// Best-effort, and the ADR names what it cannot see: a command that
    /// writes into the repository by its absolute path, a hook's writes
    /// around the call, and anything not carried back (new files,
    /// deletions, refused paths) because the repository did not change.
    ///
    /// Omitted when empty, so a trail written before the field, and every
    /// action that changed nothing, keeps its bytes and its digest. Read
    /// back, these are paths a repository chose, and data only (ADR 0007).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub changed: Vec<String>,
    /// How many paths the call changed, which is more than `changed` holds
    /// when the list was cut at [`CHANGED_CAP`]. Omitted when zero, for the
    /// same reason as `changed`.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub changed_count: u32,
    /// How long the call took, in milliseconds, from the loop's monotonic
    /// clock (`Instant`, never the wall clock, ADR 0046); a wait on a person
    /// in approve mode is excluded, so it is the tool's time (#622). For a
    /// `run_checks` action the same figure is on its `CheckOutcome`.
    ///
    /// Absent in a trail written before the field, and absent, never 0,
    /// for an action the writer did not time (a partition's record, a
    /// compaction, an approval): a 0 would read as a measurement. Omitted
    /// when absent, so an old-shaped run keeps its bytes and its digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
    /// For an `edit` that applied: how many lines its one hunk added, the
    /// figure its rpc `turn` event carries, from the same `EditDiff`, so
    /// the trail and the event cannot disagree (ADR 0007, amended #811).
    ///
    /// Absent on every other action, `write_file` included, and in a trail
    /// written before the field: absent is not recorded, never 0. A
    /// recorded 0 is written, since a pure deletion adding no lines is a
    /// measurement, which is why the skip is `is_none` and not `is_zero`.
    /// Read back, it is the trail's claim and nothing more (rule 10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines_added: Option<u64>,
    /// How many lines the same hunk removed, under the same rules as
    /// `lines_added`. The writer sets both or neither; a reader infers
    /// neither from the other.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lines_removed: Option<u64>,
}

/// The most paths one action's `changed` lists. A command that rewrites a
/// whole tree would otherwise put thousands of paths into a committed
/// file; the true count is kept beside the cut list.
pub const CHANGED_CAP: usize = 50;

impl Action {
    /// Records `paths` as what this action changed: the first
    /// [`CHANGED_CAP`] of them, and how many there were.
    #[must_use]
    pub fn with_changed(mut self, paths: &[String]) -> Self {
        self.changed = paths.iter().take(CHANGED_CAP).cloned().collect();
        self.changed_count = u32::try_from(paths.len()).unwrap_or(u32::MAX);
        self
    }
}

/// How much prompt a run consumed, and how fast it grew.
///
/// The growth is the interesting half. A loop that re-sends its whole
/// transcript every turn grows superlinearly in a wide change, and S5 asks
/// what that costs before a user finds the wall.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextFill {
    pub first_prompt_bytes: usize,
    pub peak_prompt_bytes: usize,
    pub turns_measured: u32,
}

/// What the run's blast-radius queries found, and what they could not see.
///
/// The three-field `resolved / candidates / unresolved` shape this replaces
/// invited exactly one arithmetic operation — divide the first by the sum —
/// and that division is meaningless here, because the reached counts and the
/// unbound count come from different populations. See `RadiusCoverage` and
/// `docs/adr/0014-radius-coverage.md`.
///
/// It matters more in a trail than anywhere else. Later agents read trails as
/// context, so a figure that reads as "this repository is 3.7% analysable"
/// gains authority with every run that quotes it — the S11 propagation
/// problem, applied to a number rather than an opinion.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RadiusCounts {
    /// Summed over every radius the run queried. Additive: each query asks
    /// about a different symbol, and "how many call sites did this run
    /// examine" is the sum of them.
    #[serde(default, alias = "resolved")]
    pub reached_resolved: u32,
    #[serde(default, alias = "candidates")]
    pub reached_ambiguous: u32,
    /// The **largest single** radius's unbound count, not a sum.
    ///
    /// Two radii whose touched files overlap count the same unbound call sites
    /// twice, and most radii in a repository touch overlapping files, so a sum
    /// grows without bound as a run asks more questions. The widest single
    /// query is a real figure about a real radius; a sum is not a figure about
    /// anything.
    ///
    /// Not aliased from the old `unresolved` field: that value *was* a sum, so
    /// migrating it would carry the over-count forward under a name promising
    /// it had been fixed.
    #[serde(default)]
    pub widest_unbound_in_touched_files: u32,
    /// How many radius queries the above summarises, so a reader can tell the
    /// difference between one query and twenty.
    #[serde(default)]
    pub radius_queries: u32,
    /// Call sites the radius observations summarised by file instead of
    /// listing one per line, summed over the run's queries (ADR 0033).
    ///
    /// The model saw a count for these, not a location, so a reader weighing
    /// what the run knew needs the figure. Omitted when zero, so a trail with
    /// nothing summarised keeps the shape and the factual digest it had
    /// before the field existed.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub summarised: u32,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Counts of the two audit modes, never collapsed into one number.
///
/// S3: "the callers still work" means two different things. Covered by a
/// test that ran and passed, or read by a model that did not object. The
/// first time something breaks after a clean run, this distinction decides
/// whether the user feels informed or lied to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModeCounts {
    pub verified: u32,
    pub reviewed: u32,
}

impl ModeCounts {
    pub fn record(&mut self, mode: AuditMode) {
        match mode {
            AuditMode::Verified => self.verified += 1,
            AuditMode::Reviewed => self.reviewed += 1,
        }
    }

    /// Deliberately returns both numbers and a label, never a single
    /// percentage. There is no `total()` here on purpose — ADR 0003.
    pub fn summary(&self) -> String {
        format!(
            "{} verified by an executed check; {} reviewed only \
             (no test covers them — a weak signal)",
            self.verified, self.reviewed
        )
    }
}

/// One executed check, as the trail records it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CheckOutcome {
    pub kind: String,
    pub passed: bool,
    /// How long the check call took, in milliseconds, timed by the loop
    /// around the call and never inside `Verdict` (rule 1; #622). Absent
    /// in a trail written before the field, and omitted when absent, so an
    /// old-shaped run keeps its bytes and its digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Approval {
    Human,
    /// The run did not reach approval. Rollback reason recorded alongside.
    None,
}

/// The model's opinion. Speculation, labelled as such, structurally
/// separate from anything that executed.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Judgment {
    pub tech_debt_projection: Option<String>,
    pub rationale: Option<String>,
}

/// One run.
#[derive(Debug, Clone, Serialize)]
pub struct Trail {
    /// Format version. Present from the first trail ever written.
    ///
    /// This format will change — v1.x will want fields nobody has thought of
    /// yet — and a reader that cannot tell which version it is holding has to
    /// guess from shape. Every trail written carries it so no trail is ever
    /// orphaned by a change.
    pub schema: u32,
    /// Stable identity: `<timestamp>-<slug>`, matching the filename.
    ///
    /// `trellis resume <run_id>` needs something to name, and a filename is
    /// not it — a path is where a thing lives, not what it is.
    pub run_id: String,
    pub task: String,
    /// The commit the run started from.
    ///
    /// Without it the factual section says what changed and not what it
    /// changed *from*, which makes it uncheckable against the repository a
    /// year later.
    pub base_commit: String,
    /// ISO-8601, so a trail file sorts by name and by content.
    pub started_at: String,
    /// When the trail was written, ISO-8601 from the wall clock as
    /// `started_at` is (#622). Both are recorded as read: after a clock
    /// step (ADR 0046) this can precede `started_at`, and no duration is
    /// derived from the pair; the durations in `factual` come from the
    /// monotonic clock. Omitted when absent, which only a trail written
    /// before the field is. Outside `factual`, so it is not in the digest.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<String>,
    pub provenance: Provenance,
    pub factual: Factual,
    /// SHA-256 over the canonical JSON of `factual`.
    ///
    /// The v0.3 gate asks for the factual section to be tamper-evident. It
    /// lives in a repository that takes contributions, and an edited figure
    /// there propagates into every future session that reads it (S10). This
    /// does not make editing impossible — anyone who changes the section can
    /// recompute the digest — it makes *silent* editing impossible, which is
    /// the difference between a record and a rumour.
    pub factual_sha256: String,
    pub judgment: Judgment,
}

/// Who produced this trail.
///
/// The gate asks for provenance per entry: which run, which model, which
/// version of TRELLIS. A trail read back a year later is only as useful as
/// what a reader can tell about where it came from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provenance {
    pub trellis_version: String,
    /// The `--model` spec, or `None` where nothing drove the loop.
    pub model: Option<String>,
    /// Adapter tier in force, when one was known. Tier 4 output is materially
    /// less reliable and a reader judging an old run should know which it was.
    pub tier: Option<String>,
}

/// The current trail format. Still 1 after `ended_at` and `duration_ms`
/// (#622): an additive field read with a serde default is not a schema
/// bump, which the fixture tests in `untrusted.rs` pin.
pub const SCHEMA: u32 = 1;

impl Trail {
    /// SHA-256 over `factual`, serialized canonically.
    ///
    /// Written by `cmd_run` when a trail is recorded. The *check* against it
    /// lives in [`UntrustedTrail::integrity`], which reads the raw JSON as it
    /// came off disk — a trail is read back as untrusted data (S10), so the
    /// thing that verifies it takes bytes rather than a parsed `Trail`. Both
    /// sides hash through `factual_digest`, so they share one canonical
    /// form by construction.
    pub fn compute_factual_sha256(factual: &Factual) -> String {
        // A `Factual` that will not become a `Value` hashes the empty input,
        // as it always has; the reader can never reproduce that digest from
        // a parsed file, so such a trail reads Altered rather than Intact.
        match serde_json::to_value(factual) {
            Ok(v) => factual_digest(v),
            Err(_) => sha256_hex(&[]),
        }
    }
}

/// SHA-256 over `factual` as compact JSON with every object's keys sorted.
///
/// **The sort is explicit because the key order of `serde_json::Map` is a
/// build setting, not a property of this crate.** Without
/// `serde_json/preserve_order` it is a `BTreeMap` and keys come out sorted;
/// with it, an `IndexMap` that keeps insertion order — declaration order for
/// a struct, file order for parsed JSON. Cargo unifies that feature across
/// whatever is being built: the `trellis-cli` binary's normal dependency
/// graph does not enable it, but `llguidance`, pulled in by
/// `trellis-model/grammar` through the dev-dependencies of `trellis-loop` and
/// `trellis-model`, does, so a build with those dev-dependencies in its graph
/// (`cargo test --workspace`) hashed different bytes from a
/// `cargo build -p trellis-cli` one.
/// The same trail then read Intact under one build and Altered under the
/// other, which makes the digest a statement about the compiler invocation.
///
/// Sorted is the form kept because it is what the shipped binary writes
/// (`cargo build -p trellis-cli`), so trails already committed keep their
/// digest. C-016.
/// `sort_all_objects` is a no-op on a `BTreeMap` and sorts an `IndexMap` by
/// the same `String` ordering, so both builds now emit identical bytes.
pub(crate) fn factual_digest(mut factual: serde_json::Value) -> String {
    factual.sort_all_objects();
    sha256_hex(&serde_json::to_vec(&factual).unwrap_or_default())
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

impl Trail {
    /// Path the trail is committed at, relative to the repository root.
    pub fn path(timestamp: &str, slug: &str) -> String {
        format!(".trellis/runs/{timestamp}-{slug}.json")
    }

    pub fn to_json(&self) -> serde_json::Result<String> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_counts_never_collapse_into_one_number() {
        let mut m = ModeCounts::default();
        m.record(AuditMode::Verified);
        m.record(AuditMode::Verified);
        m.record(AuditMode::Reviewed);
        assert_eq!(m.verified, 2);
        assert_eq!(m.reviewed, 1);

        let s = m.summary();
        assert!(s.contains("2 verified"));
        assert!(s.contains("1 reviewed"));
        assert!(
            s.contains("weak signal"),
            "the reviewed count must be labelled wherever it appears"
        );
    }

    #[test]
    fn trail_serializes_with_factual_and_judgment_separated() {
        let t = Trail {
            task: "rename Installer.run".into(),
            started_at: "2026-09-18T05:12:00Z".into(),
            ended_at: Some("2026-09-18T05:14:00Z".into()),
            schema: SCHEMA,
            run_id: "20260920-000000-000-test".into(),
            base_commit: "0000000".into(),
            provenance: Provenance {
                trellis_version: "0.0.1".into(),
                model: None,
                tier: None,
            },
            factual: Factual {
                proposed: vec!["src/poetry/installation/installer.py".into()],
                radius: RadiusCounts {
                    reached_resolved: 47,
                    reached_ambiguous: 3,
                    widest_unbound_in_touched_files: 2,
                    radius_queries: 1,
                    summarised: 0,
                },
                checks: vec![],
                check_outcomes: vec![CheckOutcome {
                    kind: "Tests".into(),
                    passed: true,
                    duration_ms: Some(1_200),
                }],
                approved_by: Approval::Human,
                mode_counts: ModeCounts {
                    verified: 45,
                    reviewed: 7,
                },
                context: Some(ContextFill {
                    first_prompt_bytes: 1_200,
                    peak_prompt_bytes: 9_400,
                    turns_measured: 6,
                }),
                actions: vec![
                    Action {
                        turn: 1,
                        tool: "blast_radius".into(),
                        target: "Installer.run".into(),
                        outcome: "47 resolved, 3 candidates, 2 unresolved".into(),
                        wrote: false,
                        changed: Vec::new(),
                        changed_count: 0,
                        duration_ms: Some(10),
                        lines_added: None,
                        lines_removed: None,
                    },
                    Action {
                        turn: 2,
                        tool: "edit".into(),
                        target: "src/poetry/installation/installer.py".into(),
                        outcome: "applied".into(),
                        wrote: true,
                        changed: Vec::new(),
                        changed_count: 0,
                        duration_ms: Some(10),
                        lines_added: Some(1),
                        lines_removed: Some(1),
                    },
                    Action {
                        turn: 3,
                        tool: "run_checks".into(),
                        target: "Tests".into(),
                        outcome: "Tests: passed".into(),
                        wrote: false,
                        changed: Vec::new(),
                        changed_count: 0,
                        duration_ms: Some(10),
                        lines_added: None,
                        lines_removed: None,
                    },
                ],
                control: None,
                tokens: None,
                prompt_tokens: None,
                agent: None,
                label: None,
                inputs: Vec::new(),
                steering: Vec::new(),
                controls: Vec::new(),
                suspected_recall_gaps: Vec::new(),
            },

            factual_sha256: String::new(),
            judgment: Judgment {
                tech_debt_projection: Some("the adapter shim will need removing".into()),
                rationale: Some("kept the old name as an alias".into()),
            },
        };

        let v: serde_json::Value = serde_json::from_str(&t.to_json().unwrap()).unwrap();
        assert!(v["factual"]["radius"]["widest_unbound_in_touched_files"] == 2);
        // The field name is load-bearing. `unresolved` beside `resolved` reads
        // as two parts of one whole and invites a division that is meaningless
        // here (ADR 0014), and a trail is the document later agents quote.
        assert!(
            v["factual"]["radius"]["unresolved"].is_null(),
            "the old name must not reappear, under any serde alias"
        );
        assert!(
            v["judgment"]["tech_debt_projection"].is_string(),
            "speculation lives under judgment, never under factual"
        );
        assert!(
            v["factual"]["tech_debt_projection"].is_null(),
            "nothing speculative may appear in the factual section"
        );
    }
}
