//! The desktop's run panes, as a pure reducer over child frames.
//!
//! The TUI's `AppState::update` is synchronous and does no I/O, which is why
//! its tests run without a terminal; `DesktopState::update` is the same for
//! the desktop: a [`Frame`] in, the panes it changed out, and nothing read
//! or written anywhere else. It follows what `crates/trellis-cli/src/rpc.rs`
//! serves as a turn, and takes a turn's tool, target and whether it wrote
//! from the event (#800), so the panes and the trail agree on what a turn
//! touched.
//!
//! What it shows is what trellis reported (ADR 0048 §8): radius is three
//! counts and never a ratio (CLAUDE.md rule 3); the budget is iterations,
//! the last turn's prompt in bytes (no per-turn prompt token count reaches
//! the stream) and its reply in output tokens, with the TUI's rate and queue
//! wait (#312); and a figure trellis did not report is `null` in the view,
//! never 0. The action graph is laid out from the actions (`graph.rs`), so
//! the two never disagree. Every string in the view is trellis- or
//! model-supplied text, which the page writes through `textContent` only
//! (ADR 0007, S10).
//!
//! It also keeps the vault notes view (`state/vault.rs`, #309), typed from
//! `vault_search` and `vault_graph` answers, and the board's queue view
//! (`queue.rs`, #311), typed from `get_queue` answers, and the Atlas
//! (`atlas.rs`, the owner's request of 2026-10-11), typed from its
//! `get_history`, `get_codebase`, `get_symbols` and `get_radius` answers:
//! views of their own, which a run's start leaves alone and a new child
//! clears.

mod vault;

use crate::atlas;
use crate::capped::{Capped, Item, cut};
use crate::feed::{Feed, Row};
use crate::frames::{ApprovalRequest, Frame, Response, RunBudget, RunEnd, RuntimeIdentified, Turn};
use crate::graph;
use crate::queue;
use serde_json::{Map, Value, json};

/// The transcript keeps this many lines, as the TUI's does: a run streams
/// for as long as it lasts, and an uncapped log grows with it.
pub const TRANSCRIPT_CAP: usize = 2000;

/// And this many bytes, since one line can be of any length (#814): room
/// for every line at about 500 bytes each. The whole transcript goes out in
/// every view that changes it, so this also bounds those views.
pub const TRANSCRIPT_BYTES: usize = 1024 * 1024;

/// The trellis messages pane keeps this many stderr lines (#302): the cap
/// ADR 0052's amendment to ADR 0048 section 2 puts on showing stderr.
pub const STDERR_CAP: usize = 500;

/// And this many bytes (#814), the same room per line as the transcript.
pub const STDERR_BYTES: usize = 256 * 1024;

/// The alert is one line in a banner the page announces (`role="alert"`),
/// so it is held short (#814); what failed is also in the transcript or the
/// run's outcome, which keep far more of it.
pub const ALERT_BYTES: usize = 4 * 1024;

/// One pane of the view, named as the view's key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pane {
    Transcript,
    Actions,
    Edits,
    Checks,
    Radius,
    Budget,
    Approvals,
    Running,
    Outcome,
    /// The adapter tier the last `get_state` reply reported.
    Tier,
    /// The prompt templates the child's `get_commands` listed (#301).
    Starters,
    /// What the last `abort` answer said was sent and never delivered.
    Undelivered,
    /// What last failed, as one line of text (#302).
    Alert,
    /// trellis's stderr, its human text, kept apart from the transcript.
    Stderr,
    /// The model this child's session runs prompts on, as its last
    /// successful `set_model` answer said; `null` until one (#564).
    Model,
    /// What the child's last `list_models` answer offered to choose from.
    Models,
    /// The prompt and completion tokens of this child's runs, per model,
    /// as its last `get_session_stats` answer gave them (#313).
    SessionTokens,
    /// The child's configured specs and the model catalog, from its answer
    /// to the core's own [`MODELS_ID`] question: what the picker offers
    /// (#306), from the same answer the model guard reads.
    Available,
    /// The action graph's layout (`graph.rs`), drawn from the actions.
    Graph,
    /// The vault notes view (`state/vault.rs`).
    Vault,
    /// The board's queue view (`queue.rs`).
    Queue,
    /// The Atlas: its history, snapshots, symbols, radius and layout
    /// (`atlas.rs`).
    Atlas,
    /// What the run is on now: a model call, or the tool or checks its
    /// last turn ran (#615).
    Phase,
    /// colibri's self-report for this run, as `runtime identified` carried
    /// it (#628): three separate counts, each null when unreported.
    Runtime,
    /// The Now strip (#828): the run's state word, the engine's elapsed
    /// time, the turn against its ceiling, what the run is on, the model
    /// and its tier, and whether Stop applies. Derived from the panes it
    /// reads ([`NOW_INPUTS`]), never from a clock of the desktop's own.
    Now,
    /// Where it is (#828): phase, tool and target, turn, last call, model,
    /// tier, runtime and root, derived as the Now strip is.
    Where,
    /// The composer's queue (#830): each steer and follow-up `get_state`'s
    /// `pending` listed, and the prompt a Stop and send holds, with where
    /// each stands.
    Pending,
    /// The Code level (#831): each file the run wrote or asks to write,
    /// its line counts, its state word and its diff, derived from the
    /// panes [`CODE_INPUTS`] names.
    Code,
    /// The summary level (#832): the run so far while it is in flight, and
    /// as it ended once `run_end` has come, from the panes
    /// [`SUMMARY_INPUTS`] names, the trail `run_end` named and the closing
    /// words trellis answers for a converged run.
    Summary,
    /// The model's plan (#838): the latest `plan` event of the run, its
    /// steps as the model wrote them, or why they could not be read.
    Plan,
    /// The Code level's whole-file view (#838): the text of the file a
    /// `read_file` asked for, as trellis answered it, or why it did not.
    /// A pane of its own, not part of `code`, which is drawn again on every
    /// edit turn and would carry up to 256 KiB each time.
    WholeFile,
}

/// Every pane the view carries. Each is a key the page must draw:
/// `view_renders_every_pane_key` in `lockdown.rs` drives a whole run through
/// [`DesktopState::update`] and fails naming a key here that no module the
/// page loads reads from the `view` event (ADR 0052 section 1, #513).
const PANES: [Pane; 31] = [
    Pane::Transcript,
    Pane::Actions,
    Pane::Edits,
    Pane::Checks,
    Pane::Radius,
    Pane::Budget,
    Pane::Approvals,
    Pane::Running,
    Pane::Outcome,
    Pane::Tier,
    Pane::Starters,
    Pane::Undelivered,
    Pane::Alert,
    Pane::Stderr,
    Pane::Model,
    Pane::Models,
    Pane::SessionTokens,
    Pane::Available,
    Pane::Graph,
    Pane::Vault,
    Pane::Queue,
    Pane::Atlas,
    Pane::Phase,
    Pane::Runtime,
    Pane::Now,
    Pane::Where,
    Pane::Pending,
    Pane::Code,
    Pane::Summary,
    Pane::Plan,
    Pane::WholeFile,
];

/// The panes a run starts afresh: everything but the transcript, which,
/// as the TUI's, carries on across runs within its cap; the tier, which
/// only a `get_state` reply sets: the view shows the tier trellis last
/// reported, and a run's own start reports none; and the starters, which
/// are the repository's templates, not the run's. The undelivered list is
/// the last stopped run's, so a new run drops it, and so is the alert: what
/// failed before this run started is not this run's failure. The stderr
/// pane, as the transcript, carries on. The plan is the run's model's, so a
/// new run starts with none; the whole-file view is the worktree's, read
/// when asked, and stays.
const RUN_PANES: [Pane; 17] = [
    Pane::Actions,
    Pane::Edits,
    Pane::Checks,
    Pane::Radius,
    Pane::Budget,
    Pane::Approvals,
    Pane::Running,
    Pane::Outcome,
    Pane::Undelivered,
    Pane::Alert,
    Pane::Graph,
    Pane::Phase,
    Pane::Runtime,
    Pane::Now,
    Pane::Where,
    Pane::Code,
    Pane::Plan,
];

/// The panes the Now strip and Where it is are drawn from: a change to any
/// of them changes both, so the two can never show a state the panes
/// beside them have left (#828). One list, applied in one place
/// ([`with_now`]), rather than a `Pane::Now` added by hand at each branch
/// that touches one of these.
const NOW_INPUTS: [Pane; 8] = [
    Pane::Running,
    Pane::Approvals,
    Pane::Outcome,
    Pane::Phase,
    Pane::Budget,
    Pane::Actions,
    Pane::Tier,
    Pane::Runtime,
];

/// A budget meter's cells, fixed: █ and ░ have different East Asian width
/// classes, so a meter whose length followed its figures would not line up
/// with the next. 16 as the TUI's Meter where it leads a row (#911).
const METER_CELLS: u64 = 16;

/// How many of [`METER_CELLS`] a meter fills for `used` of `ceiling`, by
/// the TUI's rule (`meterFilled`): the share rounded down, except that a
/// figure above 0 fills at least one cell, so an empty meter is exactly 0,
/// and only a figure at its ceiling or past it fills every cell. In u128:
/// trellis sends `u64::MAX` for a ceiling too long for milliseconds.
fn meter_filled(used: u64, ceiling: u64) -> u64 {
    if used >= ceiling {
        return METER_CELLS;
    }
    // used < ceiling, so the share rounded down is under METER_CELLS.
    let n = u64::try_from(u128::from(used) * u128::from(METER_CELLS) / u128::from(ceiling))
        .unwrap_or(METER_CELLS);
    if used > 0 && n == 0 { 1 } else { n }
}

/// A meter's tone, the Budget card's and the TUI's (`meterRole`): cool,
/// signal past 70% of its ceiling, down past 90%.
fn meter_tone(used: u64, ceiling: u64) -> &'static str {
    let (used, ceiling) = (u128::from(used) * 10, u128::from(ceiling));
    if used > ceiling * 9 {
        "down"
    } else if used > ceiling * 7 {
        "signal"
    } else {
        "cool"
    }
}

/// One budget row: a limit's used figure against its ceiling, each as
/// trellis sent it and as text, and the meter's cells and tone. Either
/// figure unreported is null with why, and then no cell is filled and no
/// tone chosen: cells with no share to show would read as 0 (#829). Keys
/// name no ratio, percentage or total, and no token (CLAUDE.md rule 3's
/// and #312's tests below), which the limit's own name says instead.
fn limit_row(
    limit: &str,
    unit: Option<&str>,
    (used, why_used): (Option<u64>, &str),
    (ceiling, why_ceiling): (Option<u64>, &str),
    text: fn(u64) -> String,
) -> Value {
    let meter = used.zip(ceiling);
    json!({
        "limit": limit,
        "unit": unit,
        "used": used,
        "ceiling": ceiling,
        "used_text": used.map(text),
        "ceiling_text": ceiling.map(text),
        "why_used": used.is_none().then_some(why_used),
        "why_ceiling": ceiling.is_none().then_some(why_ceiling),
        "cells": METER_CELLS,
        "filled": meter.map(|(u, c)| meter_filled(u, c)),
        "tone": meter.map(|(u, c)| meter_tone(u, c)),
    })
}

/// The panes the Code level is drawn from (#831): the writes the run
/// applied, the approvals waiting on one, and whether the run is in flight
/// and how it ended, which decide each file's state word.
const CODE_INPUTS: [Pane; 4] = [Pane::Edits, Pane::Approvals, Pane::Running, Pane::Outcome];

/// The panes the summary level is drawn from (#832), the Code level's
/// files among them. Applied where a view is built
/// ([`DesktopState::view`]), after [`with_now`] has added the Code level.
/// Its steers are the transcript's steer rows, but not the transcript,
/// which changes with every turn and note: a steer row's change says
/// [`Pane::Summary`] itself ([`DesktopState::steer_sent`] and `deliver`).
const SUMMARY_INPUTS: [Pane; 8] = [
    Pane::Running,
    Pane::Outcome,
    Pane::Checks,
    Pane::Radius,
    Pane::Budget,
    Pane::Actions,
    Pane::Code,
    Pane::Summary,
];

/// How many of the newest trails the core reads to find the one a
/// `run_end` named (#832). trellis writes that trail before it sends
/// `run_end`, so it is the newest unless another run in this repository
/// ended in between; a few more cover that, and a run not among them is
/// said to be so rather than looked for further.
pub const SUMMARY_TRAILS: u32 = 20;

/// The id of the `get_last_assistant_text` the core asks after a converged
/// run (#832); only the answer to it is read as that run's closing words.
pub const SUMMARY_ID: &str = "desktop-summary";

/// The most of the model's closing words the summary keeps: model output,
/// cut with the desktop's marker past this, as a diff is.
const CLOSING_BYTES: usize = 16 * 1024;

/// `c`, and the Now strip and Where it is when `c` changed what they read,
/// and the Code level when it changed what that reads.
fn with_now(c: Changed) -> Changed {
    let c = if NOW_INPUTS.iter().any(|&p| c.has(p)) {
        c.with(Pane::Now).with(Pane::Where)
    } else {
        c
    };
    if CODE_INPUTS.iter().any(|&p| c.has(p)) {
        c.with(Pane::Code)
    } else {
        c
    }
}

/// Milliseconds as the Now strip's clock: `mm:ss`, or `h:mm:ss` from an
/// hour, as the design system's NowStrip formats its seconds (`fmtClock`).
/// Whole seconds, cut down, so the clock never shows a second trellis has
/// not yet spent.
fn clock(ms: u64) -> String {
    let secs = ms / 1000;
    let (h, m, s) = (secs / 3600, secs / 60 % 60, secs % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// The brand book's empty-state face (docs/design/system/README.md,
/// Content and Iconography), as it writes it.
const FACE_EMPTY: &str = "(•_•)";

/// The face for a run that has ended, whatever its outcome: the brand book's
/// `(•‿•)`, not its `\(^o^)/`, whose example is a run that passed. A face
/// that differed by outcome would be a status, which play never is
/// (DESIGN.md §3.1); the outcome is the strip's word beside it.
const FACE_ENDED: &str = "(•‿•)";

/// The register dial's copy and face for the Run screen (#836), in both
/// positions; the page draws the one its dial is set to. Serious adds
/// nothing. Playful adds one face, with a line of copy, only where the Run
/// screen otherwise has nothing to say: before any run, and once one has
/// ended. It is chosen from the strip's state alone and names no figure,
/// state word or outcome, so it says nothing the strip does not.
fn register(state: &str) -> Value {
    let playful = match state {
        "idle" => Some((FACE_EMPTY, "nothing here yet")),
        "ended" => Some((FACE_ENDED, "that one is over")),
        _ => None,
    };
    json!({
        "serious": null,
        "playful": playful.map(|(face, copy)| json!({"face": face, "copy": copy})),
    })
}

impl Pane {
    fn key(self) -> &'static str {
        match self {
            Pane::Transcript => "transcript",
            Pane::Actions => "actions",
            Pane::Edits => "edits",
            Pane::Checks => "checks",
            Pane::Radius => "radius",
            Pane::Budget => "budget",
            Pane::Approvals => "approvals",
            Pane::Running => "running",
            Pane::Outcome => "outcome",
            Pane::Tier => "tier",
            Pane::Starters => "starters",
            Pane::Undelivered => "undelivered",
            Pane::Alert => "alert",
            Pane::Stderr => "stderr",
            Pane::Model => "model",
            Pane::Models => "models",
            Pane::SessionTokens => "session_tokens",
            Pane::Available => "available",
            Pane::Graph => "graph",
            Pane::Vault => "vault",
            Pane::Queue => "queue",
            Pane::Atlas => "atlas",
            Pane::Phase => "phase",
            Pane::Runtime => "runtime",
            Pane::Now => "now",
            Pane::Where => "where",
            Pane::Pending => "pending",
            Pane::Code => "code",
            // Not `summary`: that word holds `sum`, which no view key may
            // (turn_usage_is_per_turn_and_never_summed).
            Pane::Summary => "recap",
            Pane::Plan => "plan",
            Pane::WholeFile => "whole_file",
        }
    }

    /// The pane's bit in [`Changed`], at its place in this enum.
    fn bit(self) -> u64 {
        Changed::bit_at(self as u32)
    }
}

/// Which panes an update changed: what the core sends the page as one
/// `view` event, so a frame that changes nothing sends nothing.
///
/// One bit per pane, in a `u64` (#815). The `u32` it replaced had room for
/// 32 panes, and a 33rd would have shifted past it: a panic in a debug
/// build, and in a release one the transcript's bit, so that pane's changes
/// went out as the transcript's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changed(u64);

// A 65th pane fails the build here, before any shift can wrap.
const _: () = assert!(
    PANES.len() <= u64::BITS as usize,
    "Changed holds one bit per pane, 64 in all"
);

impl Changed {
    /// The bit for the pane at `place`.
    fn bit_at(place: u32) -> u64 {
        1 << place
    }

    fn with(self, p: Pane) -> Self {
        Self(self.0 | p.bit())
    }

    /// The panes either changed: the core's, for a line that changed two
    /// things at once (a Stop and send's abort and its held prompt).
    pub fn and(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    fn of(panes: &[Pane]) -> Self {
        panes.iter().fold(Self::default(), |c, &p| c.with(p))
    }

    pub fn has(self, p: Pane) -> bool {
        self.0 & p.bit() != 0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Every pane: what a page that has seen nothing yet needs.
    pub fn everything() -> Self {
        Self::of(&PANES)
    }

    /// Whether these are every pane.
    pub fn is_everything(self) -> bool {
        self == Self::everything()
    }
}

/// What the run is on, from the last loop event that said (#615). Replies
/// are not streamed, so a `model call` is the only sign of life between
/// two turns of a slow model; a `turn` says what its call ran. Each carries
/// the turn number trellis sent with it, and nothing the desktop measured:
/// elapsed time would be the desktop's clock, not a trellis figure (ADR
/// 0048 section 8), so it is left out.
#[derive(Debug, Clone, PartialEq)]
enum Phase {
    /// The model is being called for this turn.
    Model { turn: u64 },
    /// The turn's call ran the named tool.
    Tool { turn: u64, tool: String },
    /// The turn's call ran checks.
    Checks { turn: u64 },
}

/// One turn, whatever it was and however it went.
#[derive(Debug, Clone, PartialEq)]
struct Action {
    /// The turn it was, for the summary's path by turn (#832).
    turn: u64,
    tool: String,
    target: String,
    /// Whether the turn changed the tree: an applied `edit`, or a
    /// `write_file` that created its file.
    wrote: bool,
    /// The turn's own figures, `None` when it reported none.
    prompt_bytes: Option<u64>,
    completion_tokens: Option<u64>,
}

/// A check that ran: its kind and whether it passed, the turn that ran it
/// and that turn's `duration_ms`, `None` when the event carried none.
#[derive(Debug, Clone, PartialEq)]
struct Check {
    kind: String,
    passed: bool,
    turn: u64,
    took_ms: Option<u64>,
}

/// The trail a `run_end` named, as the summary reads it (#832): none
/// named, being read through `trellis log`, read, or not found with why.
#[derive(Debug, Clone, Default, PartialEq)]
enum TrailRead {
    #[default]
    Unnamed,
    Reading(String),
    Read {
        run_id: String,
        /// trellis's own word, from the digest: `intact` or `altered`.
        integrity: String,
        /// The trail's two mode counts, each kept apart and `None` when the
        /// trail records none (CLAUDE.md rule 4).
        verified: Option<u64>,
        reviewed: Option<u64>,
    },
    Failed {
        run_id: String,
        why: String,
    },
}

/// The model's closing words for the summary (#832): not asked (with the
/// run's own reason), asked of trellis, said, or not said with why.
#[derive(Debug, Clone, Default, PartialEq)]
enum Closing {
    #[default]
    Unasked,
    Asked,
    Said(String),
    Unsaid(String),
}

/// What a transcript row came from (#827), as the design system's
/// Transcript types its entries: the person's task and steers, the tools
/// the run called, the checks trellis-checks ran, what trellis noted, and
/// what it refused.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Task,
    Steer,
    Tool,
    Check,
    Note,
    Refusal,
}

impl Kind {
    fn word(self) -> &'static str {
        match self {
            Kind::Task => "task",
            Kind::Steer => "steer",
            Kind::Tool => "tool",
            Kind::Check => "check",
            Kind::Note => "note",
            Kind::Refusal => "refusal",
        }
    }
}

/// Where a steer the page sent stands (#827): sent and not yet carried by
/// a model request; carried by the request after `after_step`, as rpc's
/// answer to it says; or never carried, with trellis's reason.
#[derive(Debug, Clone, PartialEq)]
enum Delivery {
    Pending,
    After(u64),
    Not(String),
}

/// The longest a steer's id or a reason it was not delivered is kept: a
/// steer row's size is its text's, so these two are held short rather than
/// counted (`Capped::change` refuses a change that moves a row's size).
const STEER_FIELD_BYTES: usize = 256;

/// How many lines of a tool's observation a row keeps to show on demand.
const OBSERVATION_LINES: usize = 40;

/// One row of the run's transcript (#827): its kind, the turn it belongs
/// to, its text and what it shows on demand, sent to the page as the design
/// system's Transcript draws an entry.
#[derive(Debug, Clone, PartialEq)]
struct Entry {
    kind: Kind,
    /// The turn it belongs to; `None` for a task, a steer, a refusal or a
    /// note that came between turns.
    turn: Option<u64>,
    text: String,
    /// A tool's observation, its first [`OBSERVATION_LINES`] lines.
    detail: Option<String>,
    /// A check row's verdict, from trellis-checks' `Verdict` and nothing
    /// else (`check_row_comes_only_from_a_some_verdict`).
    passed: Option<bool>,
    /// A steer row's line id and where it stands.
    steer: Option<(String, Delivery)>,
}

impl Entry {
    fn new(kind: Kind, turn: Option<u64>, text: String) -> Self {
        Self {
            kind,
            turn,
            text,
            detail: None,
            passed: None,
            steer: None,
        }
    }
}

impl Item for Entry {
    fn bytes(&self) -> usize {
        self.text.len() + self.detail.as_ref().map_or(0, String::len)
    }

    /// The text keeps half the room and the detail the rest, each cut with
    /// the store's marker, so a long observation never pushes out the call
    /// it belongs to.
    fn fit(self, max: usize) -> Self {
        if self.bytes() <= max {
            return self;
        }
        let half = max / 2;
        let text_room = match &self.detail {
            Some(d) if d.len() < half => max - d.len(),
            Some(_) => half,
            None => max,
        };
        let text = self.text.fit(text_room);
        let detail = self.detail.map(|d| d.fit(max - text.len()));
        Self {
            text,
            detail,
            ..self
        }
    }
}

impl Row for Entry {
    fn to_json(&self) -> Map<String, Value> {
        let mut m = Map::new();
        m.insert("kind".into(), json!(self.kind.word()));
        m.insert("turn".into(), json!(self.turn));
        m.insert("text".into(), json!(self.text));
        m.insert("detail".into(), json!(self.detail));
        m.insert("passed".into(), json!(self.passed));
        let delivery = self.steer.as_ref().map(|(_, d)| match d {
            Delivery::Pending => json!({"state": "pending"}),
            Delivery::After(n) => json!({"state": "delivered", "after_step": n}),
            Delivery::Not(why) => json!({"state": "not_delivered", "why": why}),
        });
        m.insert("delivery".into(), delivery.unwrap_or(Value::Null));
        m
    }
}

/// The three counts a `blast_radius` turn reports. No ratio is kept or
/// derived (CLAUDE.md rule 3).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Radius {
    resolved: u64,
    candidates: u64,
    unresolved: u64,
}

/// Iterations and prompt bytes. Each is `None` until trellis reports it.
/// Named `turn` and `max_turns` in the view, not `iteration`: that word
/// contains `ratio`, which no view key may (CLAUDE.md rule 3's test below).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Budget {
    turn: Option<u64>,
    /// The latest `run budget`'s ceilings, replaced whole by each one, so a
    /// `set_budget` raise (#807) is drawn from the event that says it took
    /// effect.
    ceilings: RunBudget,
    /// The run's completion tokens so far, as the latest turn reported the
    /// counter `max_tokens` is checked against (#799). trellis's figure:
    /// the desktop sums no turns into it.
    completion_tokens_used: Option<u64>,
    /// The latest turn's prompt size; `None` for a turn that reported none
    /// (a scripted run), not 0.
    prompt_bytes: Option<u64>,
    /// The latest turn's figures, as it reported them: never summed across
    /// turns, and `None` when unreported.
    completion_tokens: Option<u64>,
    model_ms: Option<u64>,
    queue_wait_ms: Option<u64>,
}

impl Budget {
    /// The last turn's output rate and queue wait as text, by COL-11's rule
    /// (`crates/tui-demo/src/tui/view.rs`'s `turn_rate`), so the desktop and
    /// the TUI never disagree: output tokens over the model call's wall time
    /// less the queue wait (0 when unreported), and the wait, each in
    /// seconds to one decimal place. Nothing is estimated: without output
    /// tokens there is neither, and with no model time left after the wait
    /// there is no rate.
    fn rate_and_queue(&self) -> (Option<String>, Option<String>) {
        let Some(tokens) = self.completion_tokens else {
            return (None, None);
        };
        let rate = self.model_ms.and_then(|model_ms| {
            let decoding_ms = model_ms.saturating_sub(self.queue_wait_ms.unwrap_or(0));
            (decoding_ms > 0).then(|| format!("{:.1}", tokens as f64 * 1000.0 / decoding_ms as f64))
        });
        let queued = self
            .queue_wait_ms
            .map(|ms| format!("{:.1}", ms as f64 / 1000.0));
        (rate, queued)
    }
}

/// A prompt template `get_commands` listed, offered as a starter: sending
/// it is a `prompt` whose `prompt` is `name` (#301). Both are repository
/// text, shown as text.
#[derive(Debug, Clone, PartialEq)]
struct Starter {
    name: String,
    description: Option<String>,
}

impl Starter {
    /// `get_commands`'s `prompts`, or `None` when the answer does not carry
    /// them as a list. An entry with no string `name` cannot be sent, so it
    /// is not offered.
    fn list_from(data: Option<&Value>) -> Option<Vec<Self>> {
        let prompts = data?.get("prompts")?.as_array()?;
        Some(
            prompts
                .iter()
                .filter_map(|p| {
                    Some(Self {
                        name: p.get("name")?.as_str()?.to_string(),
                        description: p
                            .get("description")
                            .and_then(Value::as_str)
                            .map(str::to_string),
                    })
                })
                .collect(),
        )
    }
}

/// One line an `abort` answer handed back as sent and never delivered: a
/// `steer` or a `follow_up`, with the text the user typed, kept so it is not
/// lost with the run (`rpc.rs`'s `Unsent`).
#[derive(Debug, Clone, PartialEq)]
struct Undelivered {
    kind: String,
    message: String,
}

impl Undelivered {
    /// `abort`'s `undelivered`, in the order trellis gave it, or `None`
    /// when the answer carries no such list. An entry without a string
    /// `type` and `message` holds no text to give back, so it is skipped.
    fn list_from(data: Option<&Value>) -> Option<Vec<Self>> {
        let list = data?.get("undelivered")?.as_array()?;
        Some(
            list.iter()
                .filter_map(|u| {
                    Some(Self {
                        kind: u.get("type")?.as_str()?.to_string(),
                        message: u.get("message")?.as_str()?.to_string(),
                    })
                })
                .collect(),
        )
    }
}

/// What a queued item is (#830): a steer for the run in flight, a
/// follow-up that runs after it, or the prompt a Stop and send holds until
/// the run it stops has ended.
#[derive(Debug, Clone, Copy, PartialEq)]
enum QueuedKind {
    Steer,
    FollowUp,
    Prompt,
}

impl QueuedKind {
    fn word(self) -> &'static str {
        match self {
            QueuedKind::Steer => "steer",
            QueuedKind::FollowUp => "follow_up",
            QueuedKind::Prompt => "prompt",
        }
    }

    /// A `pending` entry's `type`; the contract lists these two, and an
    /// entry of any other is not drawn as either.
    fn listed(ty: &str) -> Option<Self> {
        match ty {
            "steer" => Some(QueuedKind::Steer),
            "follow_up" => Some(QueuedKind::FollowUp),
            _ => None,
        }
    }
}

/// Where a queued item stands. An event settles it, never its absence from
/// a later `pending`: a steer leaves that list before its `steer delivered`
/// event is written (#808), and a withdrawn one leaves it delivered nowhere.
#[derive(Debug, Clone, PartialEq)]
enum Fate {
    Queued,
    /// A steer carried by the model request after this turn.
    After(u64),
    /// A follow-up whose own run has started, or a held prompt the core
    /// has sent: either is a new run, after no turn of the one before.
    Started,
    /// Never delivered, with why.
    Not(String),
    /// Taken back: trellis answered its `withdraw` (#838), so it is in no
    /// model request and starts no run.
    Withdrawn,
}

/// One item of the composer's queue (#830), keyed by the line id it was
/// sent with.
#[derive(Debug, Clone, PartialEq)]
struct Queued {
    id: String,
    kind: QueuedKind,
    /// The text the queue shows, cut at [`QUEUED_TEXT_BYTES`].
    text: String,
    /// The text as trellis listed it, cut only at [`TAKEN_BACK_BYTES`]: what
    /// a take-back hands to the composer (#838), so a long steer comes back
    /// whole rather than as the queue's shortened copy.
    sent: String,
    fate: Fate,
    /// A `withdraw` for it was forwarded and not yet answered (#838). Its
    /// fate is unchanged until trellis answers: a withdraw asked is not a
    /// message taken back, and trellis refuses one already delivered.
    withdrawing: bool,
}

/// The longest text a take-back returns: `rpc_send`'s line cap, which no
/// steer or follow-up this desktop sent can pass. trellis's list is
/// untrusted, so it is held to that here too.
const TAKEN_BACK_BYTES: usize = 64 * 1024;

/// The longest `steps` string of a `plan` event this desktop parses (#838).
/// trellis keeps at most eight steps of at most 200 characters, about 7 KiB
/// of JSON at four bytes a character; a longer string is not a plan it
/// kept, and is said to be unreadable rather than parsed.
const PLAN_STEPS_BYTES: usize = 16 * 1024;

/// The most steps trellis keeps in a plan (ADR 0058 section 5).
const PLAN_STEPS_MAX: usize = 8;

/// The most characters trellis keeps in one step's text.
const PLAN_TEXT_CHARS: usize = 200;

/// The most text the whole-file view keeps: `read_file`'s own cap (ADR 0058
/// section 4), applied again because trellis's answer is untrusted.
const WHOLE_FILE_BYTES: usize = 256 * 1024;

/// One step of the model's plan, as it wrote it: its text and its state,
/// one of `todo`, `current` and `done`. The model's words (CLAUDE.md rule
/// 10): shown, never counted.
#[derive(Debug, Clone, PartialEq)]
struct PlanStep {
    text: String,
    state: &'static str,
}

/// The run's latest `plan` event (#838). A later one replaces it whole.
#[derive(Debug, Clone, PartialEq)]
struct Plan {
    turn: Option<u64>,
    /// The steps, or why they could not be read.
    steps: Result<Vec<PlanStep>, String>,
    steps_cut: Option<u64>,
    texts_cut: Option<u64>,
}

/// A `plan` event's `steps` string read as the contract says it is: a
/// JSON array of at most eight `{text, state}` objects, each text a string
/// of at most 200 characters and each state `todo`, `current` or `done`.
/// It is model output kept by trellis; anything else is said to be
/// unreadable, with why, and nothing of it is drawn as a step.
fn plan_steps(steps: Option<&str>) -> Result<Vec<PlanStep>, String> {
    let Some(steps) = steps else {
        return Err("the plan event carried no steps string".into());
    };
    if steps.len() > PLAN_STEPS_BYTES {
        return Err(format!(
            "its steps are {} bytes, past the {PLAN_STEPS_BYTES} this desktop reads",
            steps.len()
        ));
    }
    let list = match serde_json::from_str::<Value>(steps) {
        Ok(Value::Array(list)) => list,
        Ok(_) => return Err("its steps are not a JSON array".into()),
        Err(e) => return Err(format!("its steps are not JSON: {e}")),
    };
    if list.len() > PLAN_STEPS_MAX {
        return Err(format!(
            "it carries {} steps, and trellis keeps at most {PLAN_STEPS_MAX}",
            list.len()
        ));
    }
    list.iter()
        .enumerate()
        .map(|(i, step)| {
            let n = i + 1;
            let Some(text) = step.get("text").and_then(Value::as_str) else {
                return Err(format!("step {n} has no text"));
            };
            if text.chars().count() > PLAN_TEXT_CHARS {
                return Err(format!(
                    "step {n}'s text is longer than the {PLAN_TEXT_CHARS} characters trellis keeps"
                ));
            }
            let state = match step.get("state").and_then(Value::as_str) {
                Some("todo") => "todo",
                Some("current") => "current",
                Some("done") => "done",
                _ => return Err(format!("step {n}'s state is not todo, current or done")),
            };
            Ok(PlanStep {
                text: text.to_string(),
                state,
            })
        })
        .collect()
}

/// The whole-file view (#838): nothing asked yet, a `read_file` waiting on
/// its answer, the text trellis answered, or why there is none.
#[derive(Debug, Clone, Default, PartialEq)]
enum WholeFile {
    #[default]
    Unasked,
    /// Only the answer to this id is read, so a file asked for earlier
    /// and answered late never replaces the one asked for last.
    Reading {
        id: String,
        path: String,
    },
    Read {
        path: String,
        text: String,
        bytes: u64,
        cut: bool,
    },
    Refused {
        path: String,
        why: String,
    },
}

impl WholeFile {
    /// `read_file`'s answer `r` for the file asked as `path`: its text, cut
    /// again at [`WHOLE_FILE_BYTES`], or why there is none. An answer in a
    /// shape the contract does not give, or for another path, is refused
    /// here rather than drawn.
    fn answered(path: String, r: &Response) -> Self {
        if r.success != Some(true) {
            let why = r.error.clone().unwrap_or_else(|| "no reason given".into());
            return WholeFile::Refused {
                path,
                why: why.fit(QUEUED_TEXT_BYTES),
            };
        }
        let d = r.data.as_ref();
        let field = |k: &str| d.and_then(|d| d.get(k));
        let shape = (
            field("path").and_then(Value::as_str),
            field("text").and_then(Value::as_str),
            field("bytes").and_then(Value::as_u64),
            field("cut").and_then(Value::as_bool),
        );
        let (Some(answered), Some(text), Some(bytes), Some(was_cut)) = shape else {
            return WholeFile::Refused {
                path,
                why: "trellis answered read_file in a shape this desktop does not read".into(),
            };
        };
        if answered != path {
            return WholeFile::Refused {
                path,
                why: "trellis answered read_file for another path than the one asked".into(),
            };
        }
        let mut text = text.to_string();
        let over = text.len() > WHOLE_FILE_BYTES;
        if over {
            // At a character boundary and with no marker: the view says it
            // was cut beside the text, never inside the file's lines.
            text.truncate(text.floor_char_boundary(WHOLE_FILE_BYTES));
        }
        WholeFile::Read {
            path,
            text,
            bytes,
            cut: was_cut || over,
        }
    }

    fn to_json(&self) -> Value {
        match self {
            WholeFile::Unasked => Value::Null,
            WholeFile::Reading { path, .. } => json!({"path": path, "state": "reading",
                "text": null, "bytes": null, "cut": null, "why": null}),
            WholeFile::Read {
                path,
                text,
                bytes,
                cut,
            } => json!({"path": path, "state": "read", "text": text, "bytes": bytes,
                "cut": cut, "why": null}),
            WholeFile::Refused { path, why } => json!({"path": path, "state": "refused",
                "text": null, "bytes": null, "cut": null, "why": why}),
        }
    }
}

/// The prompt a Stop and send holds (#830, DESIGN.md §2.1): sent once both
/// the stopped run's `run_end` and a successful answer to the abort `abort_id`
/// have been read. Both, because rpc writes `run_end` before it answers the
/// abort, so at `run_end` a refused abort (the run had ended on its own) is
/// not yet known.
#[derive(Debug, Clone, PartialEq)]
struct Held {
    abort_id: String,
    /// The prompt line as the page sent it, checked by the core.
    line: Value,
    ended: bool,
    stopped: bool,
}

/// The most items the queue keeps: past it the oldest settled one goes,
/// and with none settled a new one is not added, so a `pending` of any
/// length cannot grow the view without bound.
const QUEUED_CAP: usize = 100;

/// The longest a queued item's text is kept; the rest is cut with the
/// store's marker.
const QUEUED_TEXT_BYTES: usize = 4 * 1024;

/// The longest diff the Code level keeps per write or approval (#831):
/// twice the 8192 bytes trellis cuts a turn's diff at, so a trellis that
/// sends more cannot grow the view without bound, and the rest is cut with
/// the store's marker.
const DIFF_BYTES: usize = 16 * 1024;

/// One write a turn applied (#831): an applied `edit`, with the figures
/// and the diff #802 put on its turn, or a file `write_file` created, whose
/// turn carries none of them.
#[derive(Debug, Clone, PartialEq)]
struct Write {
    turn: u64,
    path: String,
    added: Option<u64>,
    removed: Option<u64>,
    start_line: Option<u64>,
    diff: Option<String>,
}

/// What a line of a diff is, as its first character says.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Sign {
    Context,
    Added,
    Removed,
    /// A line no sign starts: trellis's own line saying the diff was cut,
    /// or anything else that is not a line of the file.
    Note,
}

impl Sign {
    fn word(self) -> &'static str {
        match self {
            Sign::Context => "context",
            Sign::Added => "added",
            Sign::Removed => "removed",
            Sign::Note => "note",
        }
    }
}

/// One line of a hunk the Code level draws: its sign, its numbers in the
/// file before (`old`) and after (`new`) the edit where the `@@` header
/// gave them, and its text without the sign.
#[derive(Debug, Clone, PartialEq)]
struct DiffLine {
    sign: Sign,
    old: Option<u64>,
    new: Option<u64>,
    text: String,
}

/// `@@ -a[,b] +c[,d] @@` as `(a, c)`, the first line of each side.
fn hunk_start(line: &str) -> Option<(u64, u64)> {
    let rest = line.strip_prefix("@@ -")?;
    let (old, rest) = rest.split_once(" +")?;
    let (new, _) = rest.split_once(" @@")?;
    let first = |side: &str| side.split(',').next()?.parse::<u64>().ok();
    Some((first(old)?, first(new)?))
}

/// An applied edit's diff, as #802 writes it: `--- path` and `+++ path`,
/// one `@@` header, then each line behind its sign. Read by position: the
/// lines before the header are the file names, never matched by prefix,
/// since a removed line `-- x` reads `--- x` too. Each line is numbered
/// from the header, so a line comment names a line trellis numbered and
/// not one the desktop guessed. Without a header no line is numbered, and
/// every line is shown as it came.
fn applied_lines(diff: &str) -> (Option<String>, Vec<DiffLine>) {
    let lines: Vec<&str> = diff.lines().collect();
    let Some((at, (old, new))) = lines
        .iter()
        .enumerate()
        .find_map(|(i, l)| hunk_start(l).map(|n| (i, n)))
    else {
        let shown = lines
            .iter()
            .map(|l| DiffLine {
                sign: Sign::Note,
                old: None,
                new: None,
                text: (*l).to_string(),
            })
            .collect();
        return (None, shown);
    };
    let (mut old, mut new) = (old, new);
    let mut out = Vec::new();
    for l in &lines[at + 1..] {
        let mut chars = l.chars();
        let sign = match chars.next() {
            Some(' ') => Sign::Context,
            Some('+') => Sign::Added,
            Some('-') => Sign::Removed,
            _ => Sign::Note,
        };
        let (o, n) = match sign {
            Sign::Context => (Some(old), Some(new)),
            Sign::Removed => (Some(old), None),
            Sign::Added => (None, Some(new)),
            Sign::Note => (None, None),
        };
        old += u64::from(o.is_some());
        new += u64::from(n.is_some());
        out.push(DiffLine {
            sign,
            old: o,
            new: n,
            text: if sign == Sign::Note {
                (*l).to_string()
            } else {
                chars.as_str().to_string()
            },
        });
    }
    (Some(lines[at].to_string()), out)
}

/// The file an approval question asks to write, and how many of its
/// detail's lines name it: the approve gate's `edit <path> (+a -r)` with
/// `--- <path>` and `+++ <path>` heading its diff, or `write_file <path>
/// (+n new)` with `+++ <path> (new file)`. The shape is the gate's
/// (`trellis_loop::gate`), not the contract's, so a question in any other
/// shape ties to no file and is answered in the approval dialog alone.
fn approval_file(a: &ApprovalRequest) -> Option<(&str, usize)> {
    let question = a.question.as_deref()?;
    let mut lines = a.detail.as_deref()?.lines();
    let first = lines.next()?;
    if let Some(path) = first
        .strip_prefix("+++ ")
        .and_then(|r| r.strip_suffix(" (new file)"))
    {
        return question
            .starts_with(&format!("write_file {path} ("))
            .then_some((path, 1));
    }
    let path = first.strip_prefix("--- ")?;
    (lines.next()? == format!("+++ {path}") && question.starts_with(&format!("edit {path} (")))
        .then_some((path, 2))
}

/// A pending approval's diff, after the lines that name its file: each
/// line behind its sign and a space (`- `, `+ `, two spaces for context),
/// with no `@@` header, so none is numbered until the turn that applies it.
fn pending_lines(detail: &str, skip: usize) -> Vec<DiffLine> {
    detail
        .lines()
        .skip(skip)
        .map(|l| {
            let sign = match l.chars().next() {
                Some(' ') => Sign::Context,
                Some('+') => Sign::Added,
                Some('-') => Sign::Removed,
                _ => Sign::Note,
            };
            let text = match sign {
                Sign::Note => l.to_string(),
                _ => l.get(2..).unwrap_or("").to_string(),
            };
            DiffLine {
                sign,
                old: None,
                new: None,
                text,
            }
        })
        .collect()
}

/// Why a line takes no comment, by the first rule it meets: the hunk's own
/// reason, then the line's, then the run's; `None` when it takes one. A
/// comment is a steer naming `file:line`, the line's number in the file as
/// the hunk's edit left it, so it is offered only for a line the file still
/// has, at a number trellis gave, while a run can take a steer.
fn comment_refusal(
    line: &DiffLine,
    hunk: Option<&'static str>,
    running: bool,
) -> Option<&'static str> {
    if hunk.is_some() {
        return hunk;
    }
    match (line.sign, line.new) {
        (Sign::Note, _) => Some("not a line of the file"),
        (Sign::Removed, _) => Some(
            "a removed line is not in the file now: comment on the line that replaced it or one \
             beside it",
        ),
        (_, None) => Some("trellis numbered no line here"),
        _ if !running => {
            Some("a comment is sent as a steer, and a steer reaches only a run in flight")
        }
        _ => None,
    }
}

/// The read-only requests a view of its own sends and answers in place: its
/// refusal is that view's to say (the `codebase` and `radius` events, the
/// vault, queue, models and session tokens panes), not a line in the run's
/// transcript or the page's one alert. `get_trails` and `get_overview` are
/// the page's own commands, not rpc lines, so their answers never come here.
/// `get_symbols` and `get_history` are the Atlas's; `read_file` is the Code
/// level's whole-file view (#838).
const VIEW_READS: [&str; 11] = [
    "get_codebase",
    "get_radius",
    "get_symbols",
    "get_history",
    "vault_search",
    "vault_graph",
    "vault_check",
    "get_queue",
    "list_models",
    "get_session_stats",
    "read_file",
];

/// The id of the `get_available_models` the core writes first to every
/// child it starts (#305); only the answer to it is recorded.
pub const MODELS_ID: &str = "desktop-models";

/// `get_available_models`'s `configured` specs, by source, as the child
/// sent them. `None` is a source with no spec (`null`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Configured {
    pub repository: Option<String>,
    /// The user's own config's `[model] spec`: the only hosted spec the
    /// core forwards as the user's choice (ADR 0048 section 4).
    pub user: Option<String>,
    pub session: Option<String>,
}

impl Configured {
    /// The answer's `data`, or `None` when it is not the shape `rpc.rs`
    /// sends: `configured` an object whose three sources are each a string
    /// or `null`. A malformed answer records nothing, so the guard stays
    /// shut rather than reading a source it cannot trust.
    fn from_data(data: Option<&Value>) -> Option<Self> {
        let c = data?.get("configured")?.as_object()?;
        let source = |k: &str| match c.get(k) {
            None | Some(Value::Null) => Some(None),
            Some(Value::String(s)) => Some(Some(s.clone())),
            Some(_) => None,
        };
        Some(Self {
            repository: source("repository")?,
            user: source("user")?,
            session: source("session")?,
        })
    }
}

/// One entry of `get_available_models`' `catalog`: weights `trellis model
/// pull` fetches. Shown, never sent: a catalogue name is not a spec, and
/// runs under a runtime the user starts (#306).
#[derive(Debug, Clone, PartialEq)]
struct CatalogEntry {
    name: String,
    license: Option<String>,
    /// As trellis rounded it; `None` when it sent no number.
    download_gib: Option<f64>,
    default: bool,
}

impl CatalogEntry {
    /// The answer's `catalog`, or an empty list when it carries none. An
    /// entry with no string `name` cannot be shown by name, so it is skipped.
    fn list_from(data: Option<&Value>) -> Vec<Self> {
        let Some(list) = data
            .and_then(|d| d.get("catalog"))
            .and_then(Value::as_array)
        else {
            return Vec::new();
        };
        list.iter()
            .filter_map(|e| {
                Some(Self {
                    name: e.get("name")?.as_str()?.to_string(),
                    license: e.get("license").and_then(Value::as_str).map(str::to_string),
                    download_gib: e.get("download_gib").and_then(Value::as_f64),
                    default: e.get("default").and_then(Value::as_bool) == Some(true),
                })
            })
            .collect()
    }
}

/// The session's model, as the child's last successful `set_model` answer
/// gave it (#564). Until one has come, `rpc.rs` refuses every `prompt`, and
/// so does `rpc_send`; the page keeps prompt input disabled.
#[derive(Debug, Clone, PartialEq)]
struct Chosen {
    spec: String,
    /// Whether it was the configured spec confirmed, not one named.
    configured: bool,
    /// The user config file it was written to, when the user asked.
    saved_to: Option<String>,
}

impl Chosen {
    /// The answer's `data`, or `None` when it does not carry the spec as a
    /// string: an answer the core cannot name a model from opens nothing.
    fn from_data(data: Option<&Value>) -> Option<Self> {
        let d = data?;
        Some(Self {
            spec: d.get("model")?.as_str()?.to_string(),
            configured: d.get("configured").and_then(Value::as_bool) == Some(true),
            saved_to: d
                .get("saved_to")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }
}

/// One address `list_models` probed, and why it gave no model if it did not.
#[derive(Debug, Clone, PartialEq)]
struct Probe {
    runtime: String,
    address: String,
    error: Option<String>,
}

/// A user-config provider `list_models` named: its name and the variable a
/// key would be read from, never a value (#564).
#[derive(Debug, Clone, PartialEq)]
struct Provider {
    name: String,
    variable: Option<String>,
}

/// What the child's `list_models` answer offered to choose from.
#[derive(Debug, Clone, PartialEq)]
struct ModelList {
    configured: Option<String>,
    local: Vec<String>,
    probed: Vec<Probe>,
    providers: Vec<Provider>,
}

impl ModelList {
    /// The answer's `data`, or `None` when it is not the shape `rpc.rs`
    /// sends. An entry missing a string the pane would show is skipped, as
    /// a starter with no name is, rather than drawn with a gap.
    fn from_data(data: Option<&Value>) -> Option<Self> {
        let d = data?.as_object()?;
        let text = |v: &Value, k: &str| v.get(k).and_then(Value::as_str).map(str::to_string);
        Some(Self {
            configured: match d.get("configured")? {
                Value::Null => None,
                Value::String(s) => Some(s.clone()),
                _ => return None,
            },
            local: d
                .get("local")?
                .as_array()?
                .iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect(),
            probed: d
                .get("probed")?
                .as_array()?
                .iter()
                .filter_map(|p| {
                    Some(Probe {
                        runtime: text(p, "runtime")?,
                        address: text(p, "address")?,
                        error: text(p, "error"),
                    })
                })
                .collect(),
            providers: d
                .get("providers")?
                .as_array()?
                .iter()
                .filter_map(|p| {
                    Some(Provider {
                        name: text(p, "name")?,
                        variable: text(p, "key_variable"),
                    })
                })
                .collect(),
        })
    }
}

/// One model's row of a `get_session_stats` answer: its prompt and
/// completion tokens over this child's runs, two figures never summed, each
/// `None` when a run of that model did not report it (#313, PI-10). The
/// model name is trail text (ADR 0007), shown as text.
#[derive(Debug, Clone, PartialEq)]
struct ModelTokens {
    model: String,
    prompt: Option<u64>,
    completion: Option<u64>,
}

/// The child's last `get_session_stats` answer.
#[derive(Debug, Clone, PartialEq)]
struct SessionTokens {
    models: Vec<ModelTokens>,
    /// Runs whose trail did not read back, so the rows are not taken for
    /// the whole session; `None` when the answer did not say.
    runs_not_counted: Option<u64>,
}

/// How a run ended, in one vocabulary for every pane (#653).
///
/// trellis words the same run two ways: rpc's `run_end` says `converged`,
/// `not_converged`, `interrupted` or `error` (`output.rs`), while a trail,
/// read by `log` and `overview`, says `converged`, `not converged` or `no
/// check ran` (`trellis_trail::query::Entry::outcome`, which counts a run
/// that never ran a check as not converged). Drawn side by side those read
/// as two runs, so the Run, Trails and Overview panes all pass the word
/// through here. A word this does not know is trellis's and stays as is:
/// nothing here guesses a new one, and the trail's word is still untrusted
/// text after it is mapped (`trails.rs` wraps it).
pub(crate) fn outcome_words(word: &str) -> &str {
    match word {
        "not_converged" | "no check ran" => "not converged",
        other => other,
    }
}

/// An overview as trellis printed it, with its newest trail's `outcome`
/// put through [`outcome_words`]. Every other field is untouched: a figure
/// the trail did not measure (`turns: null`) stays null, never a number.
pub(crate) fn word_overview(mut overview: Value) -> Value {
    if let Some(Value::String(word)) = overview
        .get_mut("last_run")
        .and_then(|r| r.get_mut("outcome"))
    {
        *word = outcome_words(word).to_string();
    }
    overview
}

impl SessionTokens {
    /// The answer's `data`, or `None` when it carries no `models` list. A
    /// row with no string `model` cannot be named, so it is not drawn; a
    /// figure that is not a count is unknown, as a null one is, and never 0.
    fn from_data(data: Option<&Value>) -> Option<Self> {
        let d = data?;
        Some(Self {
            models: d
                .get("models")?
                .as_array()?
                .iter()
                .filter_map(|m| {
                    Some(ModelTokens {
                        model: m.get("model")?.as_str()?.to_string(),
                        prompt: m.get("prompt_tokens").and_then(Value::as_u64),
                        completion: m.get("completion_tokens").and_then(Value::as_u64),
                    })
                })
                .collect(),
            runs_not_counted: d.get("runs_not_counted").and_then(Value::as_u64),
        })
    }
}

/// The desktop's run panes.
#[derive(Debug, Default)]
pub struct DesktopState {
    /// The run's typed rows, oldest first, each sent to the page once and
    /// again only when it changes (#827).
    transcript: Feed<TRANSCRIPT_CAP, TRANSCRIPT_BYTES, Entry>,
    actions: Vec<Action>,
    edits: Vec<String>,
    /// The run's writes, oldest first, with what each turn said of its
    /// lines (#831): the Code level's files and diffs.
    writes: Vec<Write>,
    checks: Vec<Check>,
    radius: Option<Radius>,
    budget: Budget,
    approvals: Vec<ApprovalRequest>,
    /// Whether a run is in flight: from `run_start` to `run_end`.
    running: bool,
    /// Prompts the core forwarded that the child has neither started (the
    /// run's `run_start`) nor refused (#815). From the moment one is
    /// written the child may be starting its run, before `running` says so.
    prompts_in_flight: u32,
    /// The last run's `run_end`, as trellis sent it.
    outcome: Option<RunEnd>,
    /// `get_state`'s `tier`, as trellis labels it; `None` while it is null
    /// (no run has decided one) or not yet asked for. Never guessed.
    tier: Option<String>,
    /// The child's answer to [`MODELS_ID`]; `None` until it arrives, after
    /// a malformed one, and from each child's start. `rpc_send`'s model
    /// guard reads it, and the picker shows it (`available`, #306), so the
    /// page offers the user's configured spec exactly as the guard has it.
    configured: Option<Configured>,
    /// The same answer's catalogue; empty while `configured` is `None`.
    catalog: Vec<CatalogEntry>,
    /// The prompt templates the last `get_commands` answer listed; `None`
    /// until one has come, or after a refused one.
    starters: Option<Vec<Starter>>,
    /// What the last `abort` answer said was never delivered.
    undelivered: Vec<Undelivered>,
    /// The last failure, as text: a refused command, a run that ended in
    /// error, a stdout line that is not a JSON object, or a close held
    /// while the child shuts down. Empty until one, and from each run's
    /// start; a new one replaces it.
    alert: Capped<1, ALERT_BYTES>,
    /// trellis's stderr lines, shown as human text and never read as data
    /// (ADR 0048 section 2, amended by ADR 0052), capped at [`STDERR_CAP`]
    /// lines and [`STDERR_BYTES`] bytes, and sent as the transcript is.
    stderr: Feed<STDERR_CAP, STDERR_BYTES, String>,
    /// The session's model; `None` until this child has taken one, which
    /// is what keeps prompt input shut (#564).
    model: Option<Chosen>,
    /// The last `list_models` answer: what it offered, or why it was
    /// refused. `None` until one has come.
    models: Option<Result<ModelList, String>>,
    /// The last `get_session_stats` answer, or why it gave no rows. Only
    /// that answer changes it: the desktop adds up no turn itself, so
    /// trellis keeps one source for the totals (#313). `None` until one,
    /// and once the child has exited.
    session_tokens: Option<Result<SessionTokens, String>>,
    /// The vault notes view: not a run pane, so a run's start keeps it.
    vault: vault::Vault,
    /// The board's queue view, kept across runs as the vault view is.
    queue: queue::Queue,
    /// The Atlas, kept across runs as the vault view is.
    atlas: atlas::Atlas,
    /// What the run is on now; `None` before a run's first `model call`
    /// and from its `run_end`.
    phase: Option<Phase>,
    /// What colibri said of itself for this run; `None` until the run's
    /// `runtime identified`, which only an endpoint identified as colibri
    /// sends, and from each run's start. The event stays out of the trail
    /// (ADR 0047 decision 13), so a run has no other source for it.
    runtime: Option<RuntimeIdentified>,
    /// An `abort` was forwarded and the run it stops has not ended (#828):
    /// the Now strip's `stopping`. Only the core's own forwarding sets it
    /// ([`DesktopState::abort_sent`]), since no event says a stop began.
    stopping: bool,
    /// The run is held (#837): from the loop's `paused` event to its
    /// `resumed` one or the run's end. Only the engine's event sets it,
    /// never the person's click, since a pause asked is not yet a hold.
    paused: bool,
    /// A `pause` was forwarded and neither the `paused` event nor the
    /// pause's answer has come (#837). The strip still says `running`, and
    /// offers Resume, which withdraws a pause not yet reached (ADR 0058 §1),
    /// rather than a second Pause trellis would refuse as `already paused`.
    pause_asked: bool,
    /// The run's wall-clock spend as trellis last reported it (#799): the
    /// latest `model call` or `turn` figure, then `run_end`'s, which is
    /// absent for a run its loop did not finish. Never a clock of the
    /// desktop's (ADR 0048 section 8): between events it stays at the last
    /// figure trellis sent.
    elapsed_ms: Option<u64>,
    /// The model the run in flight or the last one ran on, from its
    /// `run_start`, or the session's from the last `get_state` answer.
    run_model: Option<String>,
    /// The repository trellis serves, from `run_start` or `get_state`.
    root: Option<String>,
    /// The composer's queue (#830), in the order items were first listed;
    /// a run's start drops the settled ones.
    queued: Vec<Queued>,
    /// The prompt a Stop and send holds, until it is sent or dropped.
    held: Option<Held>,
    /// The trail the last `run_end` named, for the summary (#832).
    trail: TrailRead,
    /// Whether the core has yet to read that trail.
    trail_unread: bool,
    /// The last run's closing words, for the summary (#832).
    closing: Closing,
    /// Whether the core has yet to ask for them.
    closing_unasked: bool,
    /// The run's latest `plan` event (#838); `None` until one, and from
    /// each run's start.
    plan: Option<Plan>,
    /// The Code level's whole-file view (#838).
    whole_file: WholeFile,
    /// The text of a message trellis has just confirmed taken back, for
    /// the core to hand to the composer once (#838).
    taken_back: Option<Value>,
}

impl DesktopState {
    /// Applies one child frame and returns the panes it changed.
    pub fn update(&mut self, frame: Frame) -> Changed {
        with_now(self.apply(frame))
    }

    /// [`DesktopState::update`]'s frame, without the panes derived from
    /// the ones it changed.
    fn apply(&mut self, frame: Frame) -> Changed {
        match frame {
            Frame::Turn(t) => self.turn(*t),
            Frame::ModelCall { turn, elapsed_ms } => {
                self.phase = Some(Phase::Model { turn });
                // A call without the figure leaves the last one standing:
                // it is still the latest trellis reported.
                if elapsed_ms.is_some() {
                    self.elapsed_ms = elapsed_ms;
                }
                // The budget's turns and wall-clock rows read both.
                Changed::of(&[Pane::Phase, Pane::Budget])
            }
            Frame::RunBudget(b) => {
                // Whole, as the contract sends it again after a raise: a
                // ceiling the latest event lacks is not the last one's.
                self.budget.ceilings = b;
                Changed::of(&[Pane::Budget])
            }
            // The hold is the engine's (#837): the strip says `paused` from
            // this event, never from the click that asked for it.
            Frame::Paused { .. } => {
                self.paused = true;
                self.pause_asked = false;
                Changed::of(&[Pane::Now, Pane::Where])
            }
            Frame::Resumed { .. } => {
                self.paused = false;
                Changed::of(&[Pane::Now, Pane::Where])
            }
            // The loop's word that a model request carried a steer: the
            // oldest steer row still pending is the one it carried. rpc's
            // answer to that steer's id says the same, and settles it.
            Frame::SteerDelivered { after_step } => {
                let Some(after) = after_step else {
                    return Changed::default();
                };
                let pending = |e: &Entry| matches!(&e.steer, Some((_, Delivery::Pending)));
                let oldest = self
                    .transcript
                    .rows()
                    .find(|e| pending(e))
                    .map(|e| e.steer.clone());
                match oldest.flatten() {
                    Some((id, _)) => self.deliver(&id, Delivery::After(after)),
                    None => Changed::default(),
                }
            }
            // The model's latest plan replaces the last whole (#838): its
            // steps as the model wrote them, or why they cannot be read.
            Frame::Plan {
                turn,
                steps,
                steps_cut,
                texts_cut,
            } => {
                self.plan = Some(Plan {
                    turn,
                    steps: plan_steps(steps.as_deref()),
                    steps_cut,
                    texts_cut,
                });
                Changed::of(&[Pane::Plan])
            }
            Frame::RuntimeIdentified(r) => {
                self.runtime = Some(r);
                Changed::of(&[Pane::Runtime])
            }
            Frame::RunStart(s) => {
                self.reset_run();
                self.running = true;
                self.run_model = s.model;
                self.root = s.root;
                // A forwarded prompt's run has started, and `running` says
                // so from here on.
                self.prompts_in_flight = self.prompts_in_flight.saturating_sub(1);
                let mut c = Changed::of(&RUN_PANES).with(Pane::Pending);
                if let Some(task) = s.task {
                    self.transcript.push(Entry::new(Kind::Task, None, task));
                    c = c.with(Pane::Transcript);
                }
                // The queue is this run's: what the last one settled goes,
                // and a follow-up whose run this is has been delivered.
                self.queued.retain(|q| q.fate == Fate::Queued);
                if let Some(id) = s.id.as_deref() {
                    self.settle(QueuedKind::FollowUp, id, Fate::Started);
                }
                c
            }
            Frame::RunEnd(e) => {
                self.running = false;
                self.stopping = false;
                // An abort ends a held run with no `resumed` (#806).
                self.paused = false;
                self.pause_asked = false;
                // The run a Stop and send stops has ended; its prompt waits
                // on the abort's answer too.
                if let Some(h) = &mut self.held {
                    h.ended = true;
                }
                // The run's own figure, or none: run_end leaves it out for a
                // run its loop did not finish, and the last turn's figure
                // shown beside `ended` would be a partial one passed off as
                // the run's (client_contract.json).
                self.elapsed_ms = e.elapsed_ms;
                // Nothing is on once the run has ended.
                self.phase = None;
                // Nobody is left to answer a question the run asked.
                self.approvals.clear();
                // The budget's wall-clock row reads run_end's figure, and
                // the queue's Take back and the plan's Edit go with the run
                // (#838).
                let mut c = Changed::of(&[
                    Pane::Running,
                    Pane::Approvals,
                    Pane::Outcome,
                    Pane::Phase,
                    Pane::Budget,
                    Pane::Pending,
                    Pane::Plan,
                ]);
                if let Some(error) = &e.error {
                    self.alert.push(format!("The run ended in error: {error}"));
                    c = c.with(Pane::Alert);
                }
                // The summary's trail and closing words (#832), which the
                // core reads once this event is fed: the trail run_end
                // named, and the model's words only for a converged run,
                // the one outcome that ended on `done`. trellis keeps the
                // words of the last run that did, so after any other an
                // answer could be an earlier run's, shown as this one's.
                self.trail = match &e.run_id {
                    Some(id) => TrailRead::Reading(id.clone()),
                    None => TrailRead::Unnamed,
                };
                self.trail_unread = e.run_id.is_some();
                let converged = e.outcome.as_deref() == Some("converged");
                self.closing = if converged {
                    Closing::Asked
                } else {
                    Closing::Unasked
                };
                self.closing_unasked = converged;
                self.outcome = Some(e);
                c
            }
            Frame::Response(r) => {
                // A vault read's or get_queue's answer is its view's,
                // whatever else the branches below make of it.
                let mut views = Changed::default();
                if self.vault.answer(&r) {
                    views = views.with(Pane::Vault);
                }
                if self.queue.answer(&r) {
                    views = views.with(Pane::Queue);
                }
                if self.atlas.answer(&r) {
                    views = views.with(Pane::Atlas);
                }
                // The core's own models question: its answer is recorded,
                // and anything but a well-formed success records nothing.
                // It changes the picker's `available` pane (#306), and a
                // refusal still reaches the transcript below. It is a
                // snapshot from the child's start: a config edited later
                // takes effect on the next child.
                let mut c = Changed::default();
                if r.command.as_deref() == Some("get_available_models")
                    && r.id.as_deref() == Some(MODELS_ID)
                {
                    self.configured = if r.success == Some(true) {
                        Configured::from_data(r.data.as_ref())
                    } else {
                        None
                    };
                    self.catalog = match self.configured {
                        Some(_) => CatalogEntry::list_from(r.data.as_ref()),
                        None => Vec::new(),
                    };
                    c = c.with(Pane::Available);
                }
                // The composer's two answers (#301): the starters a
                // `get_commands` lists, and the text an `abort` hands back.
                // A refused `get_commands` leaves no starters to offer; its
                // reason reaches the transcript below.
                match (r.command.as_deref(), r.success) {
                    (Some("get_commands"), Some(true)) => {
                        self.starters = Starter::list_from(r.data.as_ref());
                        c = c.with(Pane::Starters);
                    }
                    (Some("get_commands"), _) => {
                        self.starters = None;
                        c = c.with(Pane::Starters);
                    }
                    // The model pane's two answers (#564). A refused
                    // `set_model` leaves the model the child already had, as
                    // `rpc.rs` does; its reason reaches the alert below.
                    (Some("set_model"), Some(true)) => {
                        if let Some(m) = Chosen::from_data(r.data.as_ref()) {
                            self.model = Some(m);
                            c = c.with(Pane::Model);
                        }
                    }
                    (Some("list_models"), Some(true)) => {
                        self.models =
                            Some(ModelList::from_data(r.data.as_ref()).ok_or_else(|| {
                                "trellis answered list_models in a shape this desktop does not \
                                read"
                                    .to_string()
                            }));
                        c = c.with(Pane::Models);
                    }
                    (Some("list_models"), _) => {
                        let why = r.error.clone().unwrap_or_else(|| "no reason given".into());
                        self.models = Some(Err(why));
                        c = c.with(Pane::Models);
                    }
                    (Some("get_session_stats"), Some(true)) => {
                        self.session_tokens =
                            Some(SessionTokens::from_data(r.data.as_ref()).ok_or_else(|| {
                                "trellis answered get_session_stats in a shape this desktop \
                                 does not read"
                                    .to_string()
                            }));
                        c = c.with(Pane::SessionTokens);
                    }
                    (Some("get_session_stats"), _) => {
                        let why = r.error.clone().unwrap_or_else(|| "no reason given".into());
                        self.session_tokens = Some(Err(why));
                        c = c.with(Pane::SessionTokens);
                    }
                    (Some("abort"), Some(true)) => {
                        if let Some(u) = Undelivered::list_from(r.data.as_ref()) {
                            self.undelivered = u;
                            c = c.with(Pane::Undelivered);
                        }
                        if self.undelivered_settled(r.data.as_ref()) {
                            c = c.with(Pane::Pending);
                        }
                        if let Some(h) = &mut self.held
                            && r.id.as_deref() == Some(h.abort_id.as_str())
                        {
                            h.stopped = true;
                        }
                    }
                    // A refused abort stops nothing: the run goes on, and a
                    // Stop and send's prompt is not sent after it.
                    (Some("abort"), Some(false)) => {
                        self.stopping = false;
                        c = c
                            .with(Pane::Now)
                            .with(Pane::Where)
                            .with(Pane::Budget)
                            .with(Pane::Plan);
                        if self
                            .held
                            .as_ref()
                            .is_some_and(|h| r.id.as_deref() == Some(h.abort_id.as_str()))
                        {
                            let why = r.error.as_deref().unwrap_or("no reason given");
                            c = c.and(self.held_dropped(&format!("the abort was refused: {why}")));
                        }
                    }
                    // A pause's answer, either way, ends the wait for it: held
                    // (the `paused` event came first and said so), or refused
                    // (no hold, and the reason reaches the transcript below).
                    (Some("pause"), Some(_)) => {
                        self.pause_asked = false;
                        c = c.with(Pane::Now).with(Pane::Where);
                    }
                    // A withdraw is answered with the message's own id
                    // (#838): taken back, its text goes to the composer; refused
                    // (already delivered, unknown id), the item stands where it
                    // did, and the reason reaches the transcript below.
                    (Some("withdraw"), Some(answer)) => {
                        if let Some(id) = r.id.as_deref()
                            && self.withdraw_answered(id, answer, r.data.as_ref())
                        {
                            c = c.with(Pane::Pending);
                        }
                    }
                    // The whole-file view reads only the answer to the
                    // read_file it last asked (#838).
                    (Some("read_file"), Some(_)) => {
                        if let WholeFile::Reading { id, path } = &self.whole_file
                            && r.id.as_deref() == Some(id.as_str())
                        {
                            self.whole_file = WholeFile::answered(path.clone(), &r);
                            c = c.with(Pane::WholeFile);
                        }
                    }
                    // An `approve` is sent with the question's own id, and
                    // either answer means trellis holds it open no more: an
                    // answer taken, or a question it no longer has (#302).
                    (Some("approve"), Some(_)) => {
                        let before = self.approvals.len();
                        self.approvals
                            .retain(|a| Some(a.id.as_str()) != r.id.as_deref());
                        if self.approvals.len() != before {
                            c = c.with(Pane::Approvals);
                        }
                    }
                    // A refused prompt starts no run (#815); its reason
                    // reaches the transcript and the alert below.
                    (Some("prompt"), Some(false)) => {
                        self.prompts_in_flight = self.prompts_in_flight.saturating_sub(1);
                        // A Stop and send's prompt the core sent and trellis
                        // refused starts no run: it was never delivered.
                        let why = r.error.as_deref().unwrap_or("no reason given");
                        let why = cut(why.to_string(), why.len(), QUEUED_TEXT_BYTES);
                        if let Some(q) = self.queued.iter_mut().find(|q| {
                            q.kind == QueuedKind::Prompt
                                && Some(q.id.as_str()) == r.id.as_deref()
                                && q.fate == Fate::Started
                        }) {
                            q.fate = Fate::Not(why);
                            c = c.with(Pane::Pending);
                        }
                    }
                    _ => {}
                }
                // The summary's closing words (#832): only the answer to the
                // core's own question, and only while this run's is open, so
                // an answer that comes after another run has started is not
                // read as that run's.
                if r.command.as_deref() == Some("get_last_assistant_text")
                    && r.id.as_deref() == Some(SUMMARY_ID)
                    && self.closing == Closing::Asked
                {
                    let text = r
                        .data
                        .as_ref()
                        .and_then(|d| d.get("text"))
                        .and_then(Value::as_str);
                    self.closing = match (r.success, text) {
                        (Some(true), Some(t)) => Closing::Said(t.to_string().fit(CLOSING_BYTES)),
                        (Some(true), None) => Closing::Unsaid(
                            "trellis holds no closing words: no run of this trellis rpc has \
                             ended on done"
                                .into(),
                        ),
                        _ => Closing::Unsaid(format!(
                            "get_last_assistant_text refused: {}",
                            r.error.as_deref().unwrap_or("no reason given")
                        )),
                    };
                    c = c.with(Pane::Summary);
                }
                // A steer's answer says where its row stands: the step whose
                // request carried it, or why none did, which the row says.
                let mut steer_row = false;
                if r.command.as_deref() == Some("steer")
                    && let Some(id) = r.id.as_deref()
                {
                    let delivery =
                        match (r.success, r.data.as_ref().and_then(|d| d.get("after_step"))) {
                            (Some(true), Some(n)) if n.as_u64().is_some() => {
                                n.as_u64().map(Delivery::After)
                            }
                            (Some(false), _) => Some(Delivery::Not(
                                r.error.clone().unwrap_or_else(|| "no reason given".into()),
                            )),
                            _ => None,
                        };
                    if let Some(d) = delivery {
                        let changed = self.deliver(id, d);
                        steer_row = !changed.is_empty();
                        c = c.and(changed);
                    }
                }
                // A refused command is said in the transcript, unless a view
                // asked and shows the reason itself, or it was a steer whose
                // row says it; a successful answer changes no run pane but
                // the tier a `get_state` reports.
                let view_read = steer_row
                    || r.command
                        .as_deref()
                        .is_some_and(|c| VIEW_READS.contains(&c));
                let c = if r.success == Some(true) && r.command.as_deref() == Some("get_state") {
                    let text = |k: &str| {
                        r.data
                            .as_ref()
                            .and_then(|d| d.get(k))
                            .and_then(Value::as_str)
                            .map(str::to_string)
                    };
                    self.tier = text("tier");
                    // The session's model and root, for Where it is before
                    // a run has started; a run's own start says them again.
                    self.run_model = text("model");
                    if let Some(root) = text("root") {
                        self.root = Some(root);
                    }
                    let c = c.with(Pane::Tier);
                    if self.pending_listed(r.data.as_ref()) {
                        c.with(Pane::Pending)
                    } else {
                        c
                    }
                } else if r.success == Some(false) && !view_read {
                    let command = r.command.as_deref().unwrap_or("command");
                    let said = match r.error {
                        Some(e) => format!("{command} refused: {e}"),
                        None => format!("{command} refused"),
                    };
                    self.transcript
                        .push(Entry::new(Kind::Refusal, None, said.clone()));
                    self.alert.push(said);
                    c.with(Pane::Transcript).with(Pane::Alert)
                } else {
                    c
                };
                c.and(views)
            }
            Frame::ApprovalRequest(a) => {
                if self.approvals.iter().any(|o| o.id == a.id) {
                    return Changed::default();
                }
                self.approvals.push(a);
                Changed::of(&[Pane::Approvals])
            }
            Frame::Ignored => Changed::default(),
            // Text on stdout, which carries only JSON objects: shown as it
            // came in the transcript, and said in the alert, since a reader
            // that cannot parse the stream may be missing a frame.
            Frame::Unparsed(line) => {
                self.alert.push(format!(
                    "trellis rpc printed a line that is not a JSON object: {line}"
                ));
                self.transcript.push(Entry::new(Kind::Note, None, line));
                Changed::of(&[Pane::Transcript, Pane::Alert])
            }
            // trellis's human text: its own capped pane, apart from the
            // run's transcript, and never read for anything (#302).
            Frame::Human(line) => {
                self.stderr.push(line);
                Changed::of(&[Pane::Stderr])
            }
        }
    }

    /// The child has exited: no run is in flight any more, or about to
    /// start, nothing is on, and nobody is left to answer a question it
    /// asked.
    pub fn child_exited(&mut self) -> Changed {
        self.running = false;
        self.stopping = false;
        self.paused = false;
        self.pause_asked = false;
        // The root and model were that child's; the next may serve another
        // repository, and Where it is would name the last one's until it
        // said its own.
        self.run_model = None;
        self.root = None;
        self.prompts_in_flight = 0;
        // Left set, the page read `calling the model, turn N` beside
        // `exited` (#815), a hang that is not one.
        self.phase = None;
        self.approvals.clear();
        self.configured = None;
        self.catalog.clear();
        // The model was that child's session's; the next child has none.
        self.model = None;
        self.models = None;
        // So were the runs the totals counted (#313): a new child's start
        // empty.
        self.session_tokens = None;
        // Nothing that child held will be delivered now, and a Stop and
        // send's prompt has no session to run in.
        self.held_dropped("trellis rpc exited before the run ended");
        for q in &mut self.queued {
            if q.fate == Fate::Queued {
                q.fate = Fate::Not("trellis rpc exited".into());
            }
        }
        with_now(Changed::of(&[
            Pane::Running,
            Pane::Phase,
            Pane::Approvals,
            // Its Raise goes with the run, and so does the plan's Edit.
            Pane::Budget,
            Pane::Plan,
            Pane::Model,
            Pane::Models,
            Pane::SessionTokens,
            Pane::Available,
            Pane::Pending,
        ]))
    }

    /// The core forwarded an `abort` (#828): the run is stopping until its
    /// `run_end`. Called before the line is written, as a steer's row is,
    /// so a `run_end` read as soon as it is written finds it set.
    pub fn abort_sent(&mut self) -> Changed {
        self.stopping = true;
        // A stopping run takes no raise and no steer: the budget's Raise
        // and the plan's Edit go with it.
        Changed::of(&[Pane::Now, Pane::Where, Pane::Budget, Pane::Plan])
    }

    /// The `abort` could not be written to the child: nothing is stopping.
    pub fn abort_unsent(&mut self) -> Changed {
        self.stopping = false;
        Changed::of(&[Pane::Now, Pane::Where, Pane::Budget, Pane::Plan])
    }

    /// The core forwarded a `pause` (#837): Resume replaces Pause until the
    /// engine's `paused` event or the pause's answer. Called before the line
    /// is written, as `abort_sent` is. The state word stays `running`.
    pub fn pause_sent(&mut self) -> Changed {
        self.pause_asked = true;
        Changed::of(&[Pane::Now])
    }

    /// The `pause` could not be written to the child: nothing was asked.
    pub fn pause_unsent(&mut self) -> Changed {
        self.pause_asked = false;
        Changed::of(&[Pane::Now])
    }

    /// Whether the queue holds a steer or follow-up sent with `id` that a
    /// `withdraw` may take back now (#838): still queued, no withdraw for
    /// it waiting, and its run in flight, since rpc takes back nothing
    /// with none. A Stop and send's prompt is the core's to hold, not
    /// trellis's, so no withdraw names it.
    pub fn withdrawable(&self, id: &str) -> bool {
        self.running
            && self.queued.iter().any(|q| {
                q.id == id
                    && q.kind != QueuedKind::Prompt
                    && q.fate == Fate::Queued
                    && !q.withdrawing
            })
    }

    /// The core forwards a `withdraw` for `id` (#838): its item waits on the
    /// answer, offering no second Take back. Called before the line is
    /// written, so the answer finds it set. Its fate is not changed: only
    /// trellis's answer says it was taken back.
    pub fn withdraw_sent(&mut self, id: &str) -> Changed {
        match self.queued.iter_mut().find(|q| {
            q.id == id && q.kind != QueuedKind::Prompt && q.fate == Fate::Queued && !q.withdrawing
        }) {
            Some(q) => {
                q.withdrawing = true;
                Changed::of(&[Pane::Pending])
            }
            None => Changed::default(),
        }
    }

    /// The `withdraw` for `id` could not be written to the child: nothing
    /// was asked, and Take back is offered again.
    pub fn withdraw_unsent(&mut self, id: &str) -> Changed {
        self.withdraw_settled(id)
    }

    /// The item waiting on a withdraw for `id` waits no more; whether there
    /// was one.
    fn withdraw_settled(&mut self, id: &str) -> Changed {
        match self.queued.iter_mut().find(|q| q.id == id && q.withdrawing) {
            Some(q) => {
                q.withdrawing = false;
                Changed::of(&[Pane::Pending])
            }
            None => Changed::default(),
        }
    }

    /// trellis's answer to the withdraw for `id` (#838). Taken back, the
    /// item is withdrawn and its text is held for the composer, once. The
    /// answer is trellis's word that it withdrew the message, so it settles
    /// the item whatever the view last made of it: the withdrawn steer's own
    /// `steer` answer, `success: false`, may be read first and mark it not
    /// delivered. A message already withdrawn or delivered is not one
    /// trellis can withdraw, so it is left alone. Refused, the item stands
    /// where it did. Whether the queue changed.
    fn withdraw_answered(&mut self, id: &str, taken: bool, data: Option<&Value>) -> bool {
        if !taken {
            return !self.withdraw_settled(id).is_empty();
        }
        let kind = data
            .and_then(|d| d.get("type"))
            .and_then(Value::as_str)
            .and_then(QueuedKind::listed);
        let Some(q) = self.queued.iter_mut().find(|q| {
            q.id == id
                && q.kind != QueuedKind::Prompt
                && kind.is_none_or(|k| k == q.kind)
                && matches!(q.fate, Fate::Queued | Fate::Not(_))
        }) else {
            return false;
        };
        q.fate = Fate::Withdrawn;
        q.withdrawing = false;
        self.taken_back = Some(json!({"id": q.id, "kind": q.kind.word(), "text": q.sent}));
        true
    }

    /// The text of a message trellis has confirmed taken back (#838),
    /// `{id, kind, text}`, once: the core hands it to the composer, so the
    /// text returns on trellis's answer and never on the click.
    pub fn taken_back(&mut self) -> Option<Value> {
        self.taken_back.take()
    }

    /// The core forwards a `read_file` for `path` with line id `id` (#838):
    /// the whole-file view says it is reading, and reads only this answer.
    pub fn read_file_asked(&mut self, id: &str, path: &str) -> Changed {
        self.whole_file = WholeFile::Reading {
            id: id.to_string(),
            path: path.to_string(),
        };
        Changed::of(&[Pane::WholeFile])
    }

    /// The `read_file` with id `id` could not be written to the child, for
    /// `why`: no answer will come, so the view says why rather than reading.
    pub fn read_file_unsent(&mut self, id: &str, why: &str) -> Changed {
        match &self.whole_file {
            WholeFile::Reading { id: asked, path } if asked == id => {
                self.whole_file = WholeFile::Refused {
                    path: path.clone(),
                    why: why.to_string().fit(QUEUED_TEXT_BYTES),
                };
                Changed::of(&[Pane::WholeFile])
            }
            _ => Changed::default(),
        }
    }

    /// Whether a run is in flight, from `run_start` to `run_end` or the
    /// child's exit.
    pub fn running(&self) -> bool {
        self.running
    }

    /// The core forwarded a `prompt`: until its run starts or the child
    /// refuses it, a run may be starting that `running` does not show yet.
    pub fn prompt_forwarded(&mut self) {
        self.prompts_in_flight = self.prompts_in_flight.saturating_add(1);
    }

    /// Whether a forwarded prompt has neither started its run nor been
    /// refused (#815): `open_repository` waits it out as it waits out a
    /// run, since the child it quits would take that run with it.
    pub fn prompt_in_flight(&self) -> bool {
        self.prompts_in_flight > 0
    }

    /// A close came while an earlier one waits for the child to exit
    /// (#815): it is held, and the page is told why the window stays, in
    /// the alert it announces.
    pub fn close_held(&mut self) -> Changed {
        self.alert.push(
            "The window closes once trellis rpc has exited. It was sent quit, so a run in \
             flight rolls back and writes its trail first."
                .to_string(),
        );
        Changed::of(&[Pane::Alert])
    }

    /// Whether `id` is an approval question the child asked and nobody has
    /// answered yet: the only kind `rpc_send` lets an `approve` answer.
    pub fn approval_open(&self, id: &str) -> bool {
        self.approvals.iter().any(|a| a.id == id)
    }

    /// The configured specs the current child reported, `None` until it
    /// has answered [`MODELS_ID`].
    pub fn configured(&self) -> Option<&Configured> {
        self.configured.as_ref()
    }

    /// Whether this child's session has a model, from its own answer to a
    /// `set_model`: what `rpc_send` lets a `prompt` through on (#564).
    pub fn model_chosen(&self) -> bool {
        self.model.is_some()
    }

    /// A new child is starting: what the last one reported is not its
    /// answer, so the guard is shut until it gives its own, and the model
    /// its session had is not the new session's.
    pub fn forget_models(&mut self) {
        self.configured = None;
        self.catalog.clear();
        self.model = None;
        self.models = None;
    }

    /// The page sent a `vault_search` or `vault_graph` with `id`: the
    /// vault view takes only that question's answer from now on, so an
    /// earlier question answered late cannot replace it.
    pub fn vault_asked(&mut self, command: &str, id: Option<&str>) {
        self.vault.asked(command, id);
    }

    /// The page sent a `get_queue` with `id`: the queue view takes only
    /// that answer from now on.
    pub fn queue_asked(&mut self, id: Option<&str>) {
        self.queue.asked(id);
    }

    /// The page sent `line`, a `command` the Atlas reads through: the
    /// Atlas takes only its answer from now on, for its kind or its
    /// snapshot. A line without an Atlas id changes nothing.
    pub fn atlas_asked(&mut self, command: &str, line: &Value) {
        self.atlas.asked(command, line);
    }

    /// Clears the whole view, transcript included: colibri's New chat and
    /// Clear. Whether a run is in flight, stopping, or a prompt is, the
    /// model and root the child reported, which specs the
    /// child reported, and the model its session has and offered, are the
    /// child's facts, not the view's, so they are kept. Returns every pane,
    /// so the view event that follows carries all of them.
    pub fn clear(&mut self) -> Changed {
        let running = self.running;
        let stopping = self.stopping;
        let (paused, pause_asked) = (self.paused, self.pause_asked);
        let run_model = self.run_model.take();
        let root = self.root.take();
        let prompts_in_flight = self.prompts_in_flight;
        let configured = self.configured.take();
        let catalog = std::mem::take(&mut self.catalog);
        let model = self.model.take();
        let models = self.models.take();
        // The two feeds are emptied but go on numbering their rows, so a
        // page never takes a new row for one it drew before the clear.
        let mut transcript = std::mem::take(&mut self.transcript);
        transcript.clear();
        let mut stderr = std::mem::take(&mut self.stderr);
        stderr.clear();
        *self = Self {
            running,
            stopping,
            paused,
            pause_asked,
            run_model,
            root,
            prompts_in_flight,
            configured,
            catalog,
            model,
            models,
            transcript,
            stderr,
            ..Self::default()
        };
        Changed::of(&PANES)
    }

    /// The panes in `changed`, as the page receives them. A view of every
    /// pane is for a page that has drawn nothing, or asked for the run again
    /// (`get_state`), so it carries the whole transcript and stderr; any
    /// other carries only their rows not sent yet, and marks them sent.
    pub fn view(&mut self, changed: Changed) -> Value {
        let whole = changed.is_everything();
        // The summary is derived here, after `whole` is read, from every
        // path a view comes by (see [`SUMMARY_INPUTS`]).
        let changed = if SUMMARY_INPUTS.iter().any(|&p| changed.has(p)) {
            changed.with(Pane::Summary)
        } else {
            changed
        };
        let mut v = Map::new();
        for p in PANES.into_iter().filter(|&p| changed.has(p)) {
            let value = match p {
                Pane::Transcript => self.transcript.view(whole),
                Pane::Stderr => self.stderr.view(whole),
                _ => self.pane(p),
            };
            v.insert(p.key().to_string(), value);
        }
        Value::Object(v)
    }

    fn pane(&self, p: Pane) -> Value {
        match p {
            Pane::Transcript | Pane::Stderr => unreachable!("a feed is drawn by view"),
            Pane::Actions => Value::Array(
                self.actions
                    .iter()
                    .map(|a| {
                        json!({"tool": a.tool, "target": a.target, "wrote": a.wrote,
                               "prompt_bytes": a.prompt_bytes,
                               "completion_tokens": a.completion_tokens})
                    })
                    .collect(),
            ),
            Pane::Edits => json!(self.edits),
            Pane::Checks => Value::Array(
                self.checks
                    .iter()
                    .map(|c| {
                        json!({"kind": c.kind, "result": if c.passed { "passed" } else { "failed" }})
                    })
                    .collect(),
            ),
            Pane::Radius => match self.radius {
                Some(r) => json!({
                    "resolved": r.resolved,
                    "candidates": r.candidates,
                    "unresolved": r.unresolved,
                }),
                None => Value::Null,
            },
            Pane::Budget => {
                let (rate, queued) = self.budget.rate_and_queue();
                json!({
                    "turn": self.budget.turn,
                    "max_turns": self.budget.ceilings.max_iterations,
                    "limits": self.limits(),
                    "raise": self.raisable(),
                    "prompt_bytes": self.budget.prompt_bytes,
                    "completion_tokens": self.budget.completion_tokens,
                    "model_ms": self.budget.model_ms,
                    "queue_wait_ms": self.budget.queue_wait_ms,
                    "tokens_per_second": rate,
                    "queued_seconds": queued,
                })
            }
            Pane::Approvals => Value::Array(
                self.approvals
                    .iter()
                    .map(|a| json!({"id": a.id, "question": a.question, "detail": a.detail}))
                    .collect(),
            ),
            Pane::Running => json!(self.running),
            Pane::Phase => match &self.phase {
                Some(Phase::Model { turn }) => json!({"phase": "model", "turn": turn}),
                Some(Phase::Tool { turn, tool }) => {
                    json!({"phase": "tool", "turn": turn, "tool": tool})
                }
                Some(Phase::Checks { turn }) => json!({"phase": "checks", "turn": turn}),
                None => Value::Null,
            },
            // Each count as sent, or null: never 0 for one colibri did not
            // report, and no sum or share of the three tiers (ADR 0048 §8).
            Pane::Runtime => match &self.runtime {
                Some(r) => json!({
                    "runtime": r.runtime,
                    "served": r.served,
                    "kv_slots": r.kv_slots,
                    "tiers_vram": r.tiers_vram,
                    "tiers_ram": r.tiers_ram,
                    "tiers_disk": r.tiers_disk,
                }),
                None => Value::Null,
            },
            Pane::Tier => json!(self.tier),
            Pane::Starters => match &self.starters {
                Some(list) => Value::Array(
                    list.iter()
                        .map(|t| json!({"name": t.name, "description": t.description}))
                        .collect(),
                ),
                None => Value::Null,
            },
            Pane::Undelivered => Value::Array(
                self.undelivered
                    .iter()
                    .map(|u| json!({"type": u.kind, "message": u.message}))
                    .collect(),
            ),
            Pane::Alert => json!(self.alert.back()),
            Pane::Model => match &self.model {
                Some(m) => json!({"spec": m.spec, "configured": m.configured,
                                  "saved_to": m.saved_to, "source": self.source(m)}),
                None => Value::Null,
            },
            Pane::Available => match &self.configured {
                None => Value::Null,
                Some(c) => json!({
                    "configured": {"repository": c.repository, "user": c.user,
                                   "session": c.session},
                    "catalog": self.catalog.iter().map(|e| json!({
                        "name": e.name, "license": e.license,
                        "download_gib": e.download_gib, "default": e.default,
                    })).collect::<Vec<_>>(),
                }),
            },
            Pane::SessionTokens => match &self.session_tokens {
                None => Value::Null,
                Some(Err(e)) => json!({"models": [], "runs_not_counted": null, "error": e}),
                Some(Ok(t)) => json!({
                    "models": t.models.iter().map(|m| json!({
                        "model": m.model,
                        "prompt_tokens": m.prompt,
                        "completion_tokens": m.completion,
                    })).collect::<Vec<_>>(),
                    "runs_not_counted": t.runs_not_counted,
                    "error": null,
                }),
            },
            Pane::Models => match &self.models {
                None => Value::Null,
                Some(Err(e)) => json!({"error": e}),
                Some(Ok(l)) => json!({
                    "configured": l.configured,
                    "local": l.local,
                    "probed": l.probed.iter().map(|p| json!({
                        "runtime": p.runtime, "address": p.address, "error": p.error,
                    })).collect::<Vec<_>>(),
                    "providers": l.providers.iter().map(|p| json!({
                        "name": p.name, "key_variable": p.variable,
                    })).collect::<Vec<_>>(),
                }),
            },
            // Laid out from the actions each time it is sent, so it cannot
            // fall out of step with the actions pane beside it.
            Pane::Graph => graph::layout(self.actions.iter().map(|a| graph::Called {
                tool: &a.tool,
                target: &a.target,
                wrote: a.wrote,
            }))
            .to_json(),
            Pane::Vault => self.vault.to_json(),
            Pane::Queue => self.queue.to_json(),
            Pane::Atlas => self.atlas.to_json(),
            Pane::Now => self.now(),
            Pane::Where => self.where_it_is(),
            Pane::Code => self.code(),
            Pane::Summary => self.summary(),
            Pane::Plan => match &self.plan {
                None => Value::Null,
                Some(p) => {
                    let (steps, why) = match &p.steps {
                        Ok(list) => (
                            Value::Array(
                                list.iter()
                                    .map(|s| json!({"text": s.text, "state": s.state}))
                                    .collect(),
                            ),
                            None,
                        ),
                        Err(why) => (Value::Null, Some(why.as_str())),
                    };
                    // A person's edit goes back as a steer, which reaches
                    // only a run in flight and not stopping.
                    json!({"turn": p.turn, "steps": steps, "why": why,
                           "steps_cut": p.steps_cut, "texts_cut": p.texts_cut,
                           "can_steer": self.running && !self.stopping})
                }
            },
            Pane::WholeFile => self.whole_file.to_json(),
            Pane::Pending => Value::Array(
                self.queued
                    .iter()
                    .map(|q| {
                        let (state, after, why) = match &q.fate {
                            Fate::Queued => ("queued", None, None),
                            Fate::After(n) => ("delivered", Some(*n), None),
                            Fate::Started => ("delivered", None, None),
                            Fate::Not(why) => ("not_delivered", None, Some(why.as_str())),
                            Fate::Withdrawn => ("withdrawn", None, None),
                        };
                        // Take back (#838): offered on a steer or follow-up
                        // still queued while its run is in flight, since rpc
                        // takes back nothing with none; `asked` while the
                        // withdraw waits on its answer.
                        let take_back = if q.withdrawing {
                            Some("asked")
                        } else if self.withdrawable(&q.id) {
                            Some("offer")
                        } else {
                            None
                        };
                        json!({"id": q.id, "kind": q.kind.word(), "text": q.text,
                               "state": state, "after_turn": after, "why": why,
                               "take_back": take_back})
                    })
                    .collect(),
            ),
            Pane::Outcome => match &self.outcome {
                Some(e) => json!({
                    "outcome": e.outcome.as_deref().map(outcome_words),
                    "exit": e.exit,
                    "run_id": e.run_id,
                    "restored": e.restored,
                    "error": e.error,
                    "id": e.id,
                }),
                None => Value::Null,
            },
        }
    }

    /// Where the session's model came from, for the status bar (#306), in
    /// `get_available_models`' words: for a spec `set_model` named, `user`
    /// when it is the user's configured spec byte for byte and `session`
    /// otherwise; for the configured spec confirmed, `repository` or `user` by
    /// which of the child's own `configured` specs it is byte for byte, the
    /// repository's first since its file wins (ADR 0025 §7). `None` when it
    /// is neither, or the child gave no `configured` answer: never guessed.
    fn source(&self, m: &Chosen) -> Option<&'static str> {
        let c = self.configured.as_ref();
        if !m.configured {
            // Named by the picker's "your configured spec": the user's own,
            // byte for byte, as the model guard compares it.
            let users = c.and_then(|c| c.user.as_deref()) == Some(m.spec.as_str());
            return Some(if users { "user" } else { "session" });
        }
        let c = c?;
        if c.repository.as_deref() == Some(m.spec.as_str()) {
            Some("repository")
        } else if c.user.as_deref() == Some(m.spec.as_str()) {
            Some("user")
        } else {
            None
        }
    }

    /// The run's state as the Now strip words it (#828, DESIGN.md §2.1):
    /// `stopping` once an abort is sent, `needs you` while an approval
    /// waits, `paused` from the engine's `paused` event to its `resumed`
    /// (#837), `running`, and once a run has ended its `run_end` outcome in
    /// that event's own words, never `done` or `failed`.
    fn state_word(&self) -> (&'static str, String) {
        if self.running && self.stopping {
            ("stopping", "stopping".into())
        } else if self.running && !self.approvals.is_empty() {
            ("needs_you", "needs you".into())
        } else if self.running && self.paused {
            ("paused", "paused".into())
        } else if self.running {
            ("running", "running".into())
        } else if let Some(e) = &self.outcome {
            let word = e
                .outcome
                .as_deref()
                .map_or("outcome not reported", outcome_words);
            ("ended", format!("ended · {word}"))
        } else {
            ("idle", "idle".into())
        }
    }

    /// The turn counted against the run's ceiling: the one the model is
    /// being called for, else the last one reported, as the TUI counts it.
    fn turn_now(&self) -> Option<u64> {
        match self.phase {
            Some(Phase::Model { turn }) => Some(turn),
            _ => self.budget.turn,
        }
    }

    /// The tool and target the run is on: the last turn's, while the phase
    /// is that turn's tool or checks; none while the model is called.
    fn tool_now(&self) -> (Option<&str>, Option<&str>) {
        match (&self.phase, self.actions.last()) {
            (Some(Phase::Tool { .. } | Phase::Checks { .. }), Some(a)) => (
                Some(a.tool.as_str()).filter(|t| !t.is_empty()),
                Some(a.target.as_str()).filter(|t| !t.is_empty()),
            ),
            _ => (None, None),
        }
    }

    /// The budget's rows (#829), one per ceiling the run enforces: the
    /// turn against max_iterations, trellis's elapsed time against
    /// max_wall_clock_ms, its completion-token counter against max_tokens
    /// (never prompt tokens, never a sum of the two), and the last prompt's
    /// bytes against context_bytes. Each figure is trellis's; the desktop
    /// keeps no clock and sums no turns.
    fn limits(&self) -> Value {
        let c = &self.budget.ceilings;
        let not_sent = "trellis has sent no run budget carrying it for this run";
        let elapsed_why = if self.state_word().0 == "ended" {
            "run_end carried no elapsed time: trellis sends none for a run its loop did not finish"
        } else {
            "trellis has sent no elapsed time for this run yet"
        };
        let count = |n: u64| n.to_string();
        json!([
            limit_row(
                "turns",
                None,
                (self.turn_now(), "no turn has started"),
                (c.max_iterations, not_sent),
                count,
            ),
            limit_row(
                "wall clock",
                None,
                (self.elapsed_ms, elapsed_why),
                (c.max_wall_clock_ms, not_sent),
                clock,
            ),
            limit_row(
                "completion tokens",
                Some("tok"),
                (
                    self.budget.completion_tokens_used,
                    "no turn has reported completion_tokens_used",
                ),
                (c.max_tokens, not_sent),
                count,
            ),
            limit_row(
                "context",
                Some("B"),
                (
                    self.budget.prompt_bytes,
                    "the last turn sent no prompt size"
                ),
                (
                    c.context_bytes,
                    if *c == RunBudget::default() {
                        not_sent
                    } else {
                        "the run budget carried no context_bytes: a driver that elides \
                         nothing, the scripted one, has no context ceiling"
                    },
                ),
                count,
            ),
        ])
    }

    /// The budget rows a person may raise now (#837), by the row's `limit`:
    /// the `set_budget` field each sends, the unit its input is typed in,
    /// and how many of the field's units one typed unit is. Only the three
    /// ceilings `set_budget` takes, never context, which the driver fixes
    /// for the run; and only while a run is in flight and not stopping,
    /// since rpc refuses a raise with none. The wall clock is typed in
    /// seconds and sent in milliseconds, the unit `run budget` carries.
    /// Null when nothing may be raised. What the rows draw is still the
    /// latest `run budget`'s ceiling, never the number typed or the answer.
    fn raisable(&self) -> Value {
        if !self.running || self.stopping {
            return Value::Null;
        }
        json!({
            "turns": {"field": "max_iterations", "unit": "turns", "per_unit": 1},
            "wall clock": {"field": "max_wall_clock_ms", "unit": "s", "per_unit": 1000},
            "completion tokens": {"field": "max_tokens", "unit": "tok", "per_unit": 1},
        })
    }

    /// The Now strip (#828). The clock is trellis's elapsed figure, shown
    /// for a run in flight or ended, and with none reported it is null with
    /// the reason: the desktop draws no clock of its own (ADR 0048 §8).
    fn now(&self) -> Value {
        let (state, word) = self.state_word();
        let shows_clock = state != "idle";
        let elapsed_why = match (shows_clock, self.elapsed_ms, state) {
            (false, _, _) | (true, Some(_), _) => None,
            (true, None, "ended") => Some(
                "run_end carried no elapsed time: trellis sends none for a run its loop did not \
                 finish",
            ),
            (true, None, _) => Some("trellis has sent no elapsed time for this run yet"),
        };
        let reason = match state {
            "needs_you" => self.approvals.first().map(|a| {
                a.question
                    .clone()
                    .unwrap_or_else(|| "trellis asks for an approval".into())
            }),
            _ => None,
        };
        let (outcome, exit) = match (state, &self.outcome) {
            ("ended", Some(e)) => (e.outcome.as_deref().map(outcome_words), e.exit),
            _ => (None, None),
        };
        let phase = match (&self.phase, self.running) {
            (Some(Phase::Model { .. }), true) => Some("model"),
            (Some(Phase::Tool { .. }), true) => Some("tool"),
            (Some(Phase::Checks { .. }), true) => Some("checks"),
            _ => None,
        };
        let (tool, target) = self.tool_now();
        json!({
            "state": state,
            "word": word,
            "reason": reason,
            "outcome": outcome,
            "exit": exit,
            "elapsed": self.elapsed_ms.filter(|_| shows_clock).map(clock),
            "elapsed_why": elapsed_why,
            "turn": self.turn_now().filter(|_| self.running),
            "max_turns": self.budget.ceilings.max_iterations.filter(|_| self.running),
            "phase": phase,
            "tool": tool,
            "target": target,
            "model": self.run_model,
            "tier": self.tier,
            "can_stop": self.running && !self.stopping,
            // Pause or Resume (#837), or neither: one of the two while a
            // run is in flight and not stopping, Resume from the pause
            // asked until the run goes on, as the engine counts a pause
            // asked as one.
            "pause_control": match (self.running && !self.stopping, self.paused || self.pause_asked) {
                (false, _) => None,
                (true, false) => Some("pause"),
                (true, true) => Some("resume"),
            },
            "register": register(state),
        })
    }

    /// Where it is (#828): each fact as trellis reported it, null where it
    /// did not. The phase is a sentence the TUI's Where it is also uses
    /// (`view.Activity`), so the two clients say the same thing.
    fn where_it_is(&self) -> Value {
        let (state, _) = self.state_word();
        let phase = match (state, &self.phase) {
            ("stopping", _) => "rolling back".to_string(),
            ("needs_you", _) => "waiting on you".to_string(),
            ("paused", _) => "paused before the next model call".to_string(),
            ("running", Some(Phase::Model { turn })) => format!("calling the model, turn {turn}"),
            ("running", Some(Phase::Tool { turn, tool })) => format!("ran {tool}, turn {turn}"),
            ("running", Some(Phase::Checks { turn })) => format!("ran checks, turn {turn}"),
            // Before its first model call, as the TUI's Where it is says it.
            ("running", None) => "none".to_string(),
            ("ended", _) => "ended".to_string(),
            _ => "no run in flight".to_string(),
        };
        let (tool, target) = self.tool_now();
        json!({
            "phase": phase,
            "tool": tool,
            "target": target,
            "turn": self.turn_now(),
            "max_turns": self.budget.ceilings.max_iterations,
            "model_ms": self.budget.model_ms,
            "completion_tokens": self.budget.completion_tokens,
            "model": self.run_model,
            "tier": self.tier,
            "runtime": self.runtime.as_ref().map(|r| json!({
                "runtime": r.runtime,
                "served": r.served,
            })),
            "root": self.root,
        })
    }

    /// A file's state word (#831, DESIGN.md §2.1) as `(state, word, tone,
    /// why)`, from what trellis reported and nothing else: an approval
    /// waiting on an edit to it; a run in flight, whose writes are in the
    /// worktree until run_end; then run_end's outcome and `restored`. A run
    /// whose end did not say whether the tree was put back is `not
    /// reported`, with why, never guessed either way.
    fn file_state(
        &self,
        awaiting: bool,
    ) -> (
        &'static str,
        &'static str,
        &'static str,
        Option<&'static str>,
    ) {
        if self.running && awaiting {
            return ("awaiting", "awaiting your approval", "warn", None);
        }
        if self.running {
            return ("worktree", "in the worktree", "signal", None);
        }
        match &self.outcome {
            Some(e) if e.outcome.as_deref() == Some("converged") => {
                ("left", "left in the tree", "ok", None)
            }
            Some(e) => match e.restored {
                Some(true) => ("rolled_back", "rolled back", "down", None),
                Some(false) => ("left", "left in the tree", "warn", None),
                None => (
                    "not_reported",
                    "not reported",
                    "idle",
                    Some("run_end did not say whether the tree was restored"),
                ),
            },
            None => (
                "not_reported",
                "not reported",
                "idle",
                Some("trellis rpc exited before a run_end said what became of the run's writes"),
            ),
        }
    }

    /// The Code level (#831): one row per file the run wrote or an open
    /// approval asks to write, in the order each was first touched, with
    /// its two line counts, its state word, the open approval's id while
    /// one waits on it, and its hunks, each line numbered from trellis's
    /// `@@` header and, where it takes a comment, the `file:line` a steer
    /// names it by. The diff is repository text and model output, sent as
    /// text for the page to show and never interpret (CLAUDE.md rule 10).
    fn code(&self) -> Value {
        let pending: Vec<(&str, &ApprovalRequest, usize)> = self
            .approvals
            .iter()
            .filter_map(|a| approval_file(a).map(|(path, skip)| (path, a, skip)))
            .collect();
        let mut paths: Vec<&str> = Vec::new();
        for p in self
            .writes
            .iter()
            .map(|w| w.path.as_str())
            .chain(pending.iter().map(|(p, _, _)| *p))
        {
            if !paths.contains(&p) {
                paths.push(p);
            }
        }
        let line = |l: &DiffLine, path: &str, why: Option<&'static str>| {
            let why = comment_refusal(l, why, self.running);
            let at = match (why, l.new) {
                (None, Some(n)) => Some(format!("{path}:{n}")),
                _ => None,
            };
            json!({"sign": l.sign.word(), "old": l.old, "new": l.new, "text": l.text,
                   "at": at, "why": why})
        };
        let files: Vec<Value> = paths
            .into_iter()
            .map(|path| {
                let writes: Vec<&Write> = self.writes.iter().filter(|w| w.path == path).collect();
                let mut hunks = Vec::new();
                // Each side counted only while every hunk reported it: a
                // write_file turn carries no counts, and its file's then
                // say so rather than count part of the change.
                let mut added = Some(0u64);
                let mut removed = Some(0u64);
                for (i, w) in writes.iter().enumerate() {
                    added = added.zip(w.added).map(|(a, b)| a.saturating_add(b));
                    removed = removed.zip(w.removed).map(|(a, b)| a.saturating_add(b));
                    let (header, lines) = match &w.diff {
                        Some(d) => applied_lines(d),
                        None => (None, Vec::new()),
                    };
                    let why = match writes.get(i + 1) {
                        _ if header.is_none() => {
                            Some("trellis's diff carried no @@ header to number its lines from")
                        }
                        Some(_) => Some(
                            "a later edit to this file may have moved this line: comment on the \
                             latest edit's lines",
                        ),
                        None => None,
                    };
                    hunks.push(json!({
                        "turn": w.turn,
                        "approval": null,
                        "header": header,
                        "start_line": w.start_line,
                        "added": w.added,
                        "removed": w.removed,
                        "why_no_diff": w.diff.is_none().then_some(
                            "trellis sends a diff only with an applied edit, not with a file \
                             write_file created"
                        ),
                        "lines": lines.iter().map(|l| line(l, path, why)).collect::<Vec<_>>(),
                    }));
                }
                let mut approval = None;
                for (_, a, skip) in pending.iter().filter(|(p, _, _)| *p == path) {
                    let detail = a.detail.as_deref().unwrap_or_default();
                    let detail = detail.to_string().fit(DIFF_BYTES);
                    let lines = pending_lines(&detail, *skip);
                    let count = |s: Sign| lines.iter().filter(|l| l.sign == s).count() as u64;
                    let (a_n, r_n) = (count(Sign::Added), count(Sign::Removed));
                    added = added.map(|n| n.saturating_add(a_n));
                    removed = removed.map(|n| n.saturating_add(r_n));
                    let why = Some("trellis numbers an edit's lines with the turn that applies it");
                    approval.get_or_insert(a.id.as_str());
                    hunks.push(json!({
                        "turn": null,
                        "approval": a.id,
                        "header": null,
                        "start_line": null,
                        "added": a_n,
                        "removed": r_n,
                        "why_no_diff": null,
                        "lines": lines.iter().map(|l| line(l, path, why)).collect::<Vec<_>>(),
                    }));
                }
                let (state, word, tone, why_state) = self.file_state(approval.is_some());
                json!({
                    "path": path,
                    "added": added,
                    "removed": removed,
                    "why_counts": added.is_none().then_some(
                        "trellis sends line counts only with an applied edit, not with a file \
                         write_file created"
                    ),
                    "writes": writes.len(),
                    "state": state,
                    "word": word,
                    "tone": tone,
                    "why_state": why_state,
                    "approval": approval.filter(|_| state == "awaiting"),
                    "hunks": hunks,
                })
            })
            .collect();
        json!({"files": files})
    }

    /// The summary level (#832): what the run has done so far while it is
    /// in flight, and how it ended once `run_end` has come. Every part is
    /// another pane's data as the reducer already holds it, never
    /// recomputed: the checks as trellis-checks reported them, by kind with
    /// each turn's own time; the radius as its three counts (CLAUDE.md rule
    /// 3); the Code level's files with their counts and state words; the
    /// budget's rows; this run's steer rows with their delivery; and the
    /// path, one step per turn. From the trail `run_end` named, its
    /// integrity and its Verified and Reviewed counts, two figures and never
    /// one (CLAUDE.md rule 4). The model's closing words are fenced as
    /// `{untrusted}` text under a label saying whose they are (rule 10).
    /// No outcome is given before `run_end`: a run in flight has none.
    fn summary(&self) -> Value {
        let (state, _) = self.state_word();
        let (stage, word) = match state {
            "ended" => ("final", "how the run ended"),
            "idle" if self.actions.is_empty() => ("none", "no run yet"),
            "idle" => (
                "unended",
                "no run_end: trellis rpc exited before the run ended",
            ),
            _ => (
                "so_far",
                "so far: the run is in flight, and this is not its end",
            ),
        };
        let outcome = match (stage, &self.outcome) {
            ("final", Some(e)) => json!({
                "outcome": e.outcome.as_deref().map(outcome_words),
                "exit": e.exit,
                "restored": e.restored,
                "error": e.error,
            }),
            _ => Value::Null,
        };
        let why_outcome = match stage {
            "final" => None,
            "so_far" => Some("the run is in flight: its outcome and exit code come with run_end"),
            "unended" => Some("trellis rpc exited before a run_end said how the run ended"),
            _ => Some("no run has started"),
        };

        // By kind, in the order each kind first ran; each run its verdict
        // and its turn's time. `took`, not `duration`: that word holds
        // `ratio`, which no view key may (CLAUDE.md rule 3's test).
        let mut kinds: Vec<(&str, Vec<Value>)> = Vec::new();
        for c in &self.checks {
            let run = json!({
                "turn": c.turn,
                "result": if c.passed { "passed" } else { "failed" },
                "took_ms": c.took_ms,
                "why_took": c.took_ms.is_none().then_some("the turn's event carried no duration_ms"),
            });
            match kinds.iter_mut().find(|(k, _)| *k == c.kind) {
                Some((_, runs)) => runs.push(run),
                None => kinds.push((c.kind.as_str(), vec![run])),
            }
        }
        let checks: Vec<Value> = kinds
            .into_iter()
            .map(|(kind, runs)| json!({"kind": kind, "runs": runs}))
            .collect();

        let radius = self.radius.map(|r| {
            json!({"resolved": r.resolved, "candidates": r.candidates, "unresolved": r.unresolved})
        });

        let code = self.code();
        let files: Vec<Value> = code["files"]
            .as_array()
            .map(|fs| {
                fs.iter()
                    .map(|f| {
                        json!({
                            "path": f["path"], "added": f["added"], "removed": f["removed"],
                            "why_counts": f["why_counts"], "word": f["word"], "tone": f["tone"],
                            "why_state": f["why_state"],
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        // This run's steers: the rows after its task's, which run_start
        // writes first. Each delivery as the transcript row says it.
        let mut steers: Vec<Value> = self
            .transcript
            .rows()
            .rev()
            .take_while(|e| e.kind != Kind::Task)
            .filter(|e| e.kind == Kind::Steer)
            .map(|e| {
                let row = e.to_json();
                json!({"text": row["text"], "delivery": row["delivery"]})
            })
            .collect();
        steers.reverse();

        let path: Vec<Value> = self
            .actions
            .iter()
            .map(|a| json!({"turn": a.turn, "tool": a.tool, "target": a.target, "wrote": a.wrote}))
            .collect();

        let (trail, modes) = match &self.trail {
            TrailRead::Read {
                run_id,
                integrity,
                verified,
                reviewed,
            } => (
                json!({"run_id": run_id, "integrity": integrity, "why": null}),
                json!({
                    "verified": verified,
                    "reviewed": reviewed,
                    "why_verified": verified.is_none().then_some("the trail records no verified count"),
                    "why_reviewed": reviewed.is_none().then_some("the trail records no reviewed count"),
                }),
            ),
            other => {
                let (run_id, why) = match other {
                    TrailRead::Reading(id) => {
                        (Some(id.as_str()), "reading the trail run_end named")
                    }
                    TrailRead::Failed { run_id, why } => (Some(run_id.as_str()), why.as_str()),
                    _ => match stage {
                        "final" => (
                            None,
                            "run_end named no trail: trellis wrote none for this run",
                        ),
                        "so_far" => (
                            None,
                            "trellis writes the run's trail, and counts its modes, at run_end",
                        ),
                        "unended" => (None, "trellis rpc exited before run_end named a trail"),
                        _ => (None, "no run has started"),
                    },
                };
                (
                    json!({"run_id": run_id, "integrity": null, "why": why}),
                    json!({"verified": null, "reviewed": null, "why_verified": why, "why_reviewed": why}),
                )
            }
        };

        let closing_why = match (&self.closing, stage) {
            (Closing::Said(_), _) => None,
            (Closing::Asked, _) => Some("asked trellis for the model's closing words".to_string()),
            (Closing::Unsaid(why), _) => Some(why.clone()),
            (Closing::Unasked, "final") => Some(
                "this run did not converge, and trellis keeps only the closing words of the \
                 last run that ended on done, which may be an earlier run's"
                    .to_string(),
            ),
            (Closing::Unasked, "so_far") => {
                Some("the model's closing words come once the run has converged".to_string())
            }
            (Closing::Unasked, _) => Some("no run has converged".to_string()),
        };
        let words = match &self.closing {
            Closing::Said(t) => json!({"untrusted": t}),
            _ => Value::Null,
        };

        json!({
            "stage": stage,
            "word": word,
            "outcome": outcome,
            "why_outcome": why_outcome,
            "checks": checks,
            "modes": modes,
            "radius": radius,
            "why_radius": radius.is_none().then_some("no blast_radius turn has reported one this run"),
            "files": files,
            "budget": self.limits(),
            "steers": steers,
            "path": path,
            "trail": trail,
            "closing": {
                "label": "the model's closing words: its own, not a verdict",
                "words": words,
                "why": closing_why,
            },
        })
    }

    /// A turn, as the `turn` event reports it.
    fn turn(&mut self, t: Turn) -> Changed {
        let mut c = Changed::of(&[
            Pane::Transcript,
            Pane::Actions,
            Pane::Budget,
            Pane::Graph,
            Pane::Phase,
        ]);
        // The verdict as the loop renders it; only these two mean a check
        // ran (DESK-07's contract). "None", or any other string, is no
        // check.
        let passed = match t.verdict.as_deref() {
            Some("Some(true)") => Some(true),
            Some("Some(false)") => Some(false),
            _ => None,
        };

        // The call as a tool row, its observation shown on demand; then,
        // only for a verdict trellis-checks produced, a check row.
        let mut tool = Entry::new(Kind::Tool, Some(t.turn), t.call.clone());
        tool.detail = t.observation.as_deref().map(|obs| {
            let mut lines = obs.lines();
            let kept: Vec<&str> = lines.by_ref().take(OBSERVATION_LINES).collect();
            let more = lines.count();
            let mut d = kept.join("\n");
            if more > 0 {
                d.push_str(&format!("\n… {more} more line(s) not kept"));
            }
            d
        });
        self.transcript.push(tool);

        // Only an applied edit, or a file `write_file` created, changed the
        // tree, and the event's `wrote` says which, as the trail records it
        // (#800). Read from the observation instead, the live view and the
        // trail could disagree about what a turn touched. A refused edit is
        // still an action, but not an edit; a turn whose event carried no
        // `wrote` is not counted as one.
        let wrote = t.wrote == Some(true);
        let tool = t.tool.unwrap_or_default();
        let target = t.target.unwrap_or_default();
        // The event's tool is what the turn ran; `run_checks` is the checks
        // phase, by the tool and not the verdict, so the governance audit
        // (a `run_checks` the loop ran itself) reads as checks too.
        self.phase = Some(if tool == "run_checks" {
            Phase::Checks { turn: t.turn }
        } else {
            Phase::Tool {
                turn: t.turn,
                tool: tool.clone(),
            }
        });
        // Only an edit or a write_file sets `wrote`, and both target the
        // path they wrote.
        if wrote {
            self.edits.push(target.clone());
            self.writes.push(Write {
                turn: t.turn,
                path: target.clone(),
                added: t.lines_added,
                removed: t.lines_removed,
                start_line: t.start_line,
                diff: t.diff.map(|d| d.fit(DIFF_BYTES)),
            });
            c = c.with(Pane::Edits);
        }
        self.actions.push(Action {
            turn: t.turn,
            tool,
            target,
            wrote,
            prompt_bytes: t.prompt_bytes,
            completion_tokens: t.completion_tokens,
        });

        if let Some(passed) = passed {
            let kind = t
                .check_kind
                .filter(|k| !k.is_empty())
                .unwrap_or_else(|| "check".to_string());
            let word = if passed { "passed" } else { "failed" };
            let mut row = Entry::new(Kind::Check, Some(t.turn), format!("{kind} {word}"));
            row.passed = Some(passed);
            self.transcript.push(row);
            self.checks.push(Check {
                kind,
                passed,
                turn: t.turn,
                took_ms: t.duration_ms,
            });
            c = c.with(Pane::Checks);
        }

        if let (Some(resolved), Some(candidates), Some(unresolved)) =
            (t.resolved, t.candidates, t.unresolved)
        {
            self.radius = Some(Radius {
                resolved,
                candidates,
                unresolved,
            });
            c = c.with(Pane::Radius);
        }

        self.budget.turn = Some(t.turn);
        self.budget.prompt_bytes = t.prompt_bytes;
        self.budget.completion_tokens = t.completion_tokens;
        self.budget.model_ms = t.model_ms;
        self.budget.queue_wait_ms = t.queue_wait_ms;
        if t.elapsed_ms.is_some() {
            self.elapsed_ms = t.elapsed_ms;
        }
        // A running counter: a turn without it leaves trellis's last
        // figure standing, as elapsed_ms does.
        if t.completion_tokens_used.is_some() {
            self.budget.completion_tokens_used = t.completion_tokens_used;
        }
        c
    }

    /// Empties the panes a run owns; the transcript is left to its caps.
    fn reset_run(&mut self) {
        self.actions.clear();
        self.edits.clear();
        self.writes.clear();
        self.checks.clear();
        self.radius = None;
        self.budget = Budget::default();
        self.approvals.clear();
        self.outcome = None;
        self.undelivered.clear();
        self.alert.clear();
        self.phase = None;
        self.runtime = None;
        self.stopping = false;
        self.paused = false;
        self.pause_asked = false;
        self.elapsed_ms = None;
        self.trail = TrailRead::Unnamed;
        self.trail_unread = false;
        self.closing = Closing::Unasked;
        self.closing_unasked = false;
        self.plan = None;
    }

    /// The page sent a steer, `text`, with line id `id`: its row, pending
    /// until rpc's answer to that id says which request carried it. Called
    /// before the line is written, so the answer, which a reader thread may
    /// feed as soon as it is written, finds the row.
    pub fn steer_sent(&mut self, id: &str, text: &str) -> Changed {
        let mut row = Entry::new(Kind::Steer, None, text.to_string());
        let id = cut(id.to_string(), id.len(), STEER_FIELD_BYTES);
        row.steer = Some((id, Delivery::Pending));
        self.transcript.push(row);
        Changed::of(&[Pane::Transcript, Pane::Summary])
    }

    /// The steer sent with `id` could not be written to the child, for
    /// `why`: it will never be answered, so its row says so.
    pub fn steer_unsent(&mut self, id: &str, why: &str) -> Changed {
        self.deliver(id, Delivery::Not(why.to_string()))
    }

    /// The newest steer row sent with `id` now stands at `delivery`; a
    /// steer already settled keeps where it stood.
    fn deliver(&mut self, id: &str, delivery: Delivery) -> Changed {
        let delivery = match delivery {
            Delivery::Not(why) => Delivery::Not(cut(why.clone(), why.len(), STEER_FIELD_BYTES)),
            d => d,
        };
        let fate = match &delivery {
            Delivery::Pending => None,
            Delivery::After(n) => Some(Fate::After(*n)),
            Delivery::Not(why) => Some(Fate::Not(why.clone())),
        };
        let changed = self.transcript.change(
            |e| matches!(&e.steer, Some((i, Delivery::Pending)) if i == id),
            |e| {
                if let Some((_, d)) = &mut e.steer {
                    *d = delivery;
                }
            },
        );
        let queued = fate.is_some_and(|f| self.settle(QueuedKind::Steer, id, f));
        let mut c = Changed::default();
        if changed {
            c = c.with(Pane::Transcript).with(Pane::Summary);
        }
        if queued {
            c = c.with(Pane::Pending);
        }
        c
    }

    /// The first queued item of `kind` sent with `id` and still queued now
    /// stands at `fate`; whether there was one.
    fn settle(&mut self, kind: QueuedKind, id: &str, fate: Fate) -> bool {
        match self
            .queued
            .iter_mut()
            .find(|q| q.kind == kind && q.id == id && q.fate == Fate::Queued)
        {
            Some(q) => {
                q.fate = fate;
                true
            }
            None => false,
        }
    }

    /// A successful `get_state`'s `pending` (#830): each entry not yet in
    /// the queue joins it as `queued`, unless it is a steer whose row
    /// already says where it stands (its answer read before this one), in
    /// which case it takes that. An entry already in the queue keeps its
    /// state, and one missing from the list keeps its own: only an event
    /// settles an item. An entry without a string id, a listed type and a
    /// string message is not drawn: this client sends each with an id, so
    /// an item without one is not its own. Whether the queue changed.
    fn pending_listed(&mut self, data: Option<&Value>) -> bool {
        let Some(list) = data
            .and_then(|d| d.get("pending"))
            .and_then(Value::as_array)
        else {
            return false;
        };
        let mut changed = false;
        for p in list {
            let id = p
                .get("id")
                .and_then(Value::as_str)
                .filter(|i| !i.is_empty());
            let kind = p
                .get("type")
                .and_then(Value::as_str)
                .and_then(QueuedKind::listed);
            let message = p.get("message").and_then(Value::as_str);
            let (Some(id), Some(kind), Some(message)) = (id, kind, message) else {
                continue;
            };
            // Cut as a steer row's id is, so the two compare.
            let id = cut(id.to_string(), id.len(), STEER_FIELD_BYTES);
            if self.queued.iter().any(|q| q.kind == kind && q.id == id) {
                continue;
            }
            if self.queued.len() >= QUEUED_CAP {
                match self.queued.iter().position(|q| q.fate != Fate::Queued) {
                    Some(i) => {
                        self.queued.remove(i);
                    }
                    None => break,
                }
            }
            let fate = match kind {
                QueuedKind::Steer => self.steer_fate(&id),
                _ => Fate::Queued,
            };
            let text = cut(message.to_string(), message.len(), QUEUED_TEXT_BYTES);
            let sent = cut(message.to_string(), message.len(), TAKEN_BACK_BYTES);
            self.queued.push(Queued {
                id,
                kind,
                text,
                sent,
                fate,
                withdrawing: false,
            });
            changed = true;
        }
        changed
    }

    /// Where the newest steer row sent with `id` stands, as a queue item's
    /// fate; queued when no row has it.
    fn steer_fate(&self, id: &str) -> Fate {
        let row = self
            .transcript
            .rows()
            .rev()
            .find_map(|e| e.steer.as_ref().filter(|(i, _)| i == id));
        match row {
            Some((_, Delivery::After(n))) => Fate::After(*n),
            Some((_, Delivery::Not(why))) => Fate::Not(why.clone()),
            Some((_, Delivery::Pending)) | None => Fate::Queued,
        }
    }

    /// A successful `abort` answer's `undelivered`: each item it names by
    /// id was never delivered, since the run was stopped first. Whether
    /// the queue changed.
    fn undelivered_settled(&mut self, data: Option<&Value>) -> bool {
        let Some(list) = data
            .and_then(|d| d.get("undelivered"))
            .and_then(Value::as_array)
        else {
            return false;
        };
        let mut changed = false;
        for u in list {
            let id = u.get("id").and_then(Value::as_str);
            let kind = u
                .get("type")
                .and_then(Value::as_str)
                .and_then(QueuedKind::listed);
            if let (Some(id), Some(kind)) = (id, kind) {
                let id = cut(id.to_string(), id.len(), STEER_FIELD_BYTES);
                let why = Fate::Not("the run was stopped first".into());
                changed |= self.settle(kind, &id, why);
            }
        }
        changed
    }

    /// The core forwards an `abort` with id `abort_id` for a Stop and send
    /// (#830), holding `line`, the prompt to run once the stopped run has
    /// ended. Called before the abort is written, as [`abort_sent`] is, so
    /// the answers it waits on find it set. The page sees the prompt in the
    /// queue at once.
    ///
    /// [`abort_sent`]: DesktopState::abort_sent
    pub fn stop_and_send(&mut self, abort_id: &str, line: Value) -> Changed {
        let id = line.get("id").and_then(Value::as_str).unwrap_or_default();
        let id = cut(id.to_string(), id.len(), STEER_FIELD_BYTES);
        let task = line.get("task").and_then(Value::as_str);
        let template = line.get("prompt").and_then(Value::as_str);
        let text = match (template, task) {
            (Some(t), Some(task)) => format!("template {t}: {task}"),
            (Some(t), None) => format!("template {t}"),
            (None, task) => task.unwrap_or_default().to_string(),
        };
        let len = text.len();
        let text = cut(text, len, QUEUED_TEXT_BYTES);
        self.queued.push(Queued {
            id,
            kind: QueuedKind::Prompt,
            sent: text.clone(),
            text,
            fate: Fate::Queued,
            withdrawing: false,
        });
        self.held = Some(Held {
            abort_id: abort_id.to_string(),
            line,
            ended: false,
            stopped: false,
        });
        Changed::of(&[Pane::Pending])
    }

    /// Whether a Stop and send's prompt is held: a second one is refused
    /// while it is, since only one prompt can follow the stopped run.
    pub fn holding(&self) -> bool {
        self.held.is_some()
    }

    /// The held prompt, taken, once the stopped run's `run_end` and the
    /// abort's successful answer have both been read; `None` until then,
    /// and ever after it is taken or dropped. Its queue item stays queued
    /// until [`held_sent`] says what became of it.
    ///
    /// [`held_sent`]: DesktopState::held_sent
    pub fn held_ready(&mut self) -> Option<Value> {
        if !self.held.as_ref().is_some_and(|h| h.ended && h.stopped) {
            return None;
        }
        self.held.take().map(|h| h.line)
    }

    /// What the core's forwarding of a taken held prompt, sent with `id`,
    /// came to: sent, a new run on its way; or refused, with why, and so
    /// never delivered.
    pub fn held_sent(&mut self, id: &str, sent: Result<(), &str>) -> Changed {
        let id = cut(id.to_string(), id.len(), STEER_FIELD_BYTES);
        let fate = match sent {
            Ok(()) => Fate::Started,
            Err(why) => Fate::Not(cut(why.to_string(), why.len(), QUEUED_TEXT_BYTES)),
        };
        if self.settle(QueuedKind::Prompt, &id, fate) {
            Changed::of(&[Pane::Pending])
        } else {
            Changed::default()
        }
    }

    /// The held prompt will not be sent, for `why`: its abort was refused or
    /// never written, or the child exited. Nothing when none is held.
    pub fn held_dropped(&mut self, why: &str) -> Changed {
        let Some(h) = self.held.take() else {
            return Changed::default();
        };
        let id = h.line.get("id").and_then(Value::as_str).unwrap_or_default();
        self.held_sent(id, Err(why))
    }
    /// The run id of a trail the last `run_end` named and the core has not
    /// yet read for the summary (#832), once: the core reads it with
    /// `trellis log` and hands the rows to [`DesktopState::trail_read`].
    pub fn trail_to_read(&mut self) -> Option<String> {
        if !std::mem::take(&mut self.trail_unread) {
            return None;
        }
        match &self.trail {
            TrailRead::Reading(id) => Some(id.clone()),
            _ => None,
        }
    }

    /// `trellis log`'s rows, or why it gave none, for the trail `run_id`.
    /// Read only while the summary still waits on that run's trail: rows
    /// that come after another run has started are not this run's. The
    /// row's integrity is trellis's word, and its two mode counts are kept
    /// as two (CLAUDE.md rule 4).
    pub fn trail_read(&mut self, run_id: &str, rows: Result<Vec<Value>, String>) -> Changed {
        if self.trail != TrailRead::Reading(run_id.to_string()) {
            return Changed::default();
        }
        let failed = |why: String| TrailRead::Failed {
            run_id: run_id.to_string(),
            why,
        };
        self.trail = match rows {
            Err(why) => failed(why),
            Ok(rows) => {
                let row = rows.iter().find(|r| {
                    r.get("run_id")
                        .and_then(|i| i.get("untrusted"))
                        .and_then(Value::as_str)
                        == Some(run_id)
                });
                match row {
                    None => failed(format!(
                        "trellis log's newest {SUMMARY_TRAILS} trails do not include run {run_id}"
                    )),
                    Some(r) => match r.get("integrity").and_then(Value::as_str) {
                        Some(integrity) => {
                            let count = |k: &str| r["mode_counts"][k].as_u64();
                            TrailRead::Read {
                                run_id: run_id.to_string(),
                                integrity: integrity.to_string(),
                                verified: count("verified"),
                                reviewed: count("reviewed"),
                            }
                        }
                        None => failed("trellis log's row for the run carries no integrity".into()),
                    },
                }
            }
        };
        Changed::of(&[Pane::Summary])
    }

    /// Whether the core is to ask trellis for the run's closing words
    /// (#832), once, with [`SUMMARY_ID`].
    pub fn closing_to_ask(&mut self) -> bool {
        std::mem::take(&mut self.closing_unasked) && self.closing == Closing::Asked
    }

    /// The core's question for the closing words could not be written.
    pub fn closing_unasked(&mut self, why: &str) -> Changed {
        if self.closing != Closing::Asked {
            return Changed::default();
        }
        self.closing =
            Closing::Unsaid(format!("the desktop could not ask trellis for them: {why}"));
        Changed::of(&[Pane::Summary])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frames::parse_stdout;

    /// The contract the binary's own tests hold trellis to (ADR 0048 §6).
    const CONTRACT: &str = include_str!("../../crates/trellis-cli/tests/client_contract.json");

    /// A loop event as `trellis rpc` writes it: a `tracing` JSON line with
    /// its fields beside `message`.
    fn event(message: &str, fields: Value) -> String {
        let mut f = fields.as_object().cloned().unwrap_or_default();
        f.insert("message".into(), json!(message));
        json!({
            "timestamp": "2026-10-06T00:00:00.000000Z",
            "level": "INFO",
            "fields": f,
            "target": "trellis_loop::agent",
        })
        .to_string()
    }

    fn turn(fields: Value) -> String {
        event("turn", fields)
    }

    fn feed(s: &mut DesktopState, line: &str) -> Changed {
        s.update(parse_stdout(line))
    }

    fn all(s: &mut DesktopState) -> Value {
        s.view(Changed::of(&PANES))
    }

    /// A feed's rows in a view (`feed.rs`), oldest first.
    fn rows(feed: &Value) -> Vec<Value> {
        feed["rows"].as_array().cloned().unwrap_or_default()
    }

    /// A feed's rows' text.
    fn texts(feed: &Value) -> Vec<String> {
        rows(feed)
            .iter()
            .map(|r| r["text"].as_str().unwrap_or_default().to_string())
            .collect()
    }

    fn run_start(id: &str) -> String {
        json!({"type": "run_start", "version": 1, "trellis": "0.0.1", "task": "change f",
               "model": "scripted:x", "root": "/r", "id": id})
        .to_string()
    }

    #[test]
    fn turn_event_becomes_an_action_and_a_transcript_line() {
        // The risk: a turn the loop reported never reaches the panes, so the
        // desktop draws a run that did nothing.
        let mut s = DesktopState::default();
        let c = feed(
            &mut s,
            &turn(
                json!({"turn": 1, "call": "read_file app/core.py", "verdict": "None",
                         "check_kind": "", "observation": "def f():\n    return 1",
                         "tool": "read_file", "target": "app/core.py", "wrote": false}),
            ),
        );
        assert!(c.has(Pane::Actions) && c.has(Pane::Transcript), "{c:?}");
        let v = all(&mut s);
        assert_eq!(
            v["actions"],
            json!([{"tool": "read_file", "target": "app/core.py", "wrote": false,
                    "prompt_bytes": null, "completion_tokens": null}])
        );
        assert_eq!(
            rows(&v["transcript"]),
            [
                json!({"seq": 0, "kind": "tool", "turn": 1, "text": "read_file app/core.py",
                    "detail": "def f():\n    return 1", "passed": null, "delivery": null})
            ]
        );
        assert_eq!(v["checks"], json!([]), "verdict None ran no check");
    }

    #[test]
    fn refused_edit_is_an_action_but_not_an_edit() {
        // The risk: a refused edit listed as pending makes
        // the edits pane contradict the action graph a user checks before
        // accepting.
        let mut s = DesktopState::default();
        feed(
            &mut s,
            &turn(
                json!({"turn": 1, "call": "edit a.py", "verdict": "None", "check_kind": "",
                         "observation": "error: old text not found",
                         "tool": "edit", "target": "a.py", "wrote": false}),
            ),
        );
        feed(
            &mut s,
            &turn(
                json!({"turn": 2, "call": "edit a.py", "verdict": "None", "check_kind": "",
                         "observation": "applied: false",
                         "tool": "edit", "target": "a.py", "wrote": false}),
            ),
        );
        let v = all(&mut s);
        assert_eq!(v["actions"].as_array().unwrap().len(), 2, "{v}");
        assert_eq!(v["actions"][0]["wrote"], false);
        assert_eq!(v["edits"], json!([]), "{v}");

        feed(
            &mut s,
            &turn(
                json!({"turn": 3, "call": "edit a.py", "verdict": "None", "check_kind": "",
                         "observation": "applied: true (+1 -1)",
                         "tool": "edit", "target": "a.py", "wrote": true}),
            ),
        );
        let v = all(&mut s);
        assert_eq!(v["actions"].as_array().unwrap().len(), 3);
        assert_eq!(v["actions"][2]["wrote"], true);
        assert_eq!(v["edits"], json!(["a.py"]));
    }

    #[test]
    fn a_turn_s_edits_come_from_wrote_not_from_its_observation() {
        // The risk (#834): the desktop reads what a turn touched from its
        // call label and observation text while the trail reads it from
        // `wrote` (#800), so the edits pane lists a file the trail says no
        // turn wrote, or misses one it did. A file the model read can hold
        // the text `applied: true`; an edit's observation tail need not.
        let mut s = DesktopState::default();
        let c = feed(
            &mut s,
            &turn(
                json!({"turn": 1, "call": "read_file notes.txt", "verdict": "None",
                         "check_kind": "", "observation": "edit notes.txt\napplied: true (+1 -1)",
                         "tool": "read_file", "target": "notes.txt", "wrote": false}),
            ),
        );
        assert!(!c.has(Pane::Edits), "{c:?}");
        let v = all(&mut s);
        assert_eq!(v["edits"], json!([]), "{v}");
        assert_eq!(v["actions"][0]["wrote"], false, "{v}");

        // An applied edit whose kept observation tail does not say so.
        let c = feed(
            &mut s,
            &turn(
                json!({"turn": 2, "call": "edit a.py", "verdict": "None", "check_kind": "",
                         "observation": "    return 2",
                         "tool": "edit", "target": "a.py", "wrote": true}),
            ),
        );
        assert!(c.has(Pane::Edits), "{c:?}");
        let v = all(&mut s);
        assert_eq!(v["edits"], json!(["a.py"]), "{v}");
        assert_eq!(
            (&v["actions"][1]["tool"], &v["actions"][1]["target"]),
            (&json!("edit"), &json!("a.py")),
            "{v}"
        );

        // A turn whose event carried no `wrote` is not counted as an edit,
        // whatever its call and observation say.
        feed(
            &mut s,
            &turn(
                json!({"turn": 3, "call": "edit b.py", "verdict": "None", "check_kind": "",
                         "observation": "applied: true (+1 -1)"}),
            ),
        );
        let v = all(&mut s);
        assert_eq!(v["edits"], json!(["a.py"]), "{v}");
        assert_eq!(v["actions"][2]["wrote"], false, "{v}");
    }

    #[test]
    fn check_row_comes_only_from_a_some_verdict() {
        // The risk: a check row drawn for a turn that ran no check, or a
        // verdict string read loosely, shows a verdict trellis never gave.
        let mut s = DesktopState::default();
        feed(
            &mut s,
            &turn(
                json!({"turn": 1, "call": "run_checks a.py", "verdict": "Some(false)",
                         "check_kind": "Typecheck", "observation": "1 error"}),
            ),
        );
        let v = all(&mut s);
        assert_eq!(
            v["checks"],
            json!([{"kind": "Typecheck", "result": "failed"}])
        );
        // The tool row, then a check row carrying the verdict.
        let t = rows(&v["transcript"]);
        assert_eq!(t.len(), 2, "{v}");
        assert_eq!(
            (&t[1]["kind"], &t[1]["text"], &t[1]["passed"], &t[1]["turn"]),
            (
                &json!("check"),
                &json!("Typecheck failed"),
                &json!(false),
                &json!(1)
            )
        );

        for verdict in [
            json!("None"),
            json!("Some(maybe)"),
            json!("true"),
            json!(true),
        ] {
            let mut s = DesktopState::default();
            feed(
                &mut s,
                &turn(
                    json!({"turn": 1, "call": "run_checks a.py", "verdict": verdict,
                             "check_kind": "Tests", "observation": ""}),
                ),
            );
            let v = all(&mut s);
            assert_eq!(v["checks"], json!([]), "verdict {verdict}");
            assert!(
                rows(&v["transcript"]).iter().all(|r| r["kind"] != "check"),
                "a check row with no verdict: {verdict}"
            );
        }
        let mut s = DesktopState::default();
        feed(
            &mut s,
            &turn(json!({"turn": 1, "call": "run_checks a.py", "check_kind": "Tests"})),
        );
        assert_eq!(all(&mut s)["checks"], json!([]), "a missing verdict");

        let mut s = DesktopState::default();
        feed(
            &mut s,
            &turn(
                json!({"turn": 1, "call": "run_checks a.py", "verdict": "Some(true)",
                         "check_kind": "Tests"}),
            ),
        );
        assert_eq!(
            all(&mut s)["checks"],
            json!([{"kind": "Tests", "result": "passed"}])
        );
    }

    /// Every key in `v`, at any depth.
    fn keys(v: &Value, out: &mut Vec<String>) {
        match v {
            Value::Object(o) => {
                for (k, x) in o {
                    out.push(k.clone());
                    keys(x, out);
                }
            }
            Value::Array(a) => a.iter().for_each(|x| keys(x, out)),
            _ => {}
        }
    }

    #[test]
    fn pause_and_resume_follow_the_engines_events() {
        // The risk (#837, ADR 0058 section 1): the strip says `paused` on
        // the click, while the loop is still finishing its turn and may end
        // before it holds, so the person reads a hold that never happened;
        // or it keeps saying `paused` after the engine's `resumed`, an
        // abort's run_end or the child's exit. The word follows the engine's
        // `paused` and `resumed` events alone. The click only swaps Pause
        // for Resume, which withdraws a pause not yet reached, and the
        // pause's answer, held or refused, ends that.
        let mut s = DesktopState::default();
        let now = |s: &mut DesktopState| all(s)["now"].clone();
        let controls = |v: &Value| v["pause_control"].clone();
        assert_eq!(controls(&now(&mut s)), Value::Null);
        feed(&mut s, &run_start("p1"));
        assert_eq!(controls(&now(&mut s)), "pause");

        // Asked, not held: still running, Resume offered in Pause's place.
        let c = s.pause_sent();
        assert!(c.has(Pane::Now), "{c:?}");
        let v = now(&mut s);
        assert_eq!(v["word"], "running", "paused on the click: {v}");
        assert_eq!(controls(&v), "resume");

        // The engine's event is what says it holds.
        let c = feed(&mut s, &event("paused", json!({"turn": 3})));
        assert!(c.has(Pane::Now) && c.has(Pane::Where), "{c:?}");
        let v = now(&mut s);
        assert_eq!(
            (v["state"].clone(), v["word"].clone()),
            (json!("paused"), json!("paused"))
        );
        assert_eq!(controls(&v), "resume");
        assert_eq!(v["can_stop"], true, "a held run can still be stopped");
        assert_eq!(
            all(&mut s)["where"]["phase"],
            "paused before the next model call"
        );
        // Its answer, after the event, changes nothing the event said.
        feed(&mut s, &response("pause", true, json!({"turn": 3})));
        assert_eq!(now(&mut s)["word"], "paused");
        // A resume sent is not the run going on: the `resumed` event is.
        feed(&mut s, &response("resume", true, Value::Null));
        assert_eq!(now(&mut s)["word"], "paused");
        let c = feed(
            &mut s,
            &event("resumed", json!({"turn": 3, "paused_ms": 4200})),
        );
        assert!(c.has(Pane::Now), "{c:?}");
        let v = now(&mut s);
        assert_eq!(v["word"], "running");
        assert_eq!(controls(&v), "pause");

        // A pause the run never reached: refused, it was no hold, and the
        // reason is said.
        s.pause_sent();
        let c = feed(
            &mut s,
            &response(
                "pause",
                false,
                json!("the run ended before it reached the hold; it did not pause"),
            ),
        );
        assert!(c.has(Pane::Now) && c.has(Pane::Alert), "{c:?}");
        let v = now(&mut s);
        assert_eq!(v["word"], "running");
        assert_eq!(controls(&v), "pause");
        // A pause that could not be written was never asked.
        s.pause_sent();
        s.pause_unsent();
        assert_eq!(controls(&now(&mut s)), "pause");

        // An abort ends a held run with no `resumed`.
        feed(&mut s, &event("paused", json!({"turn": 4})));
        s.abort_sent();
        let v = now(&mut s);
        assert_eq!(v["word"], "stopping");
        assert_eq!(controls(&v), Value::Null);
        feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "interrupted", "exit": 130, "id": "p1"})
                .to_string(),
        );
        let v = now(&mut s);
        assert_eq!(v["word"], "ended · interrupted");
        assert_eq!(controls(&v), Value::Null);
        // The next run starts unheld, and so does one after the child's exit.
        feed(&mut s, &run_start("p2"));
        assert_eq!(now(&mut s)["word"], "running");
        feed(&mut s, &event("paused", json!({"turn": 1})));
        s.child_exited();
        let v = now(&mut s);
        assert_ne!(v["state"], "paused", "{v}");
        assert_eq!(controls(&v), Value::Null);
    }

    #[test]
    fn a_raised_ceiling_is_drawn_from_run_budget_not_from_the_input() {
        // The risk (#837, ADR 0058 section 2): the budget draws the ceiling
        // the person typed, or the one set_budget's answer names, while the
        // loop still enforces the old one until its next budget check, or
        // never (a raise in the run's last turn); the row then claims a
        // limit the run does not have. The ceiling comes from `run budget`
        // alone, sent again when the raise takes effect. Raise is offered
        // only on the three ceilings set_budget takes, never context, and
        // only while a run is in flight and not stopping.
        let mut s = DesktopState::default();
        assert_eq!(
            all(&mut s)["budget"]["raise"],
            Value::Null,
            "nothing to raise"
        );
        feed(&mut s, &run_start("p1"));
        feed(
            &mut s,
            &event(
                "run budget",
                json!({"max_iterations": 8, "max_wall_clock_ms": 60_000,
                       "max_tokens": 1000, "context_bytes": 10_000}),
            ),
        );
        let v = all(&mut s);
        let raise = v["budget"]["raise"]
            .as_object()
            .expect("raisable rows")
            .clone();
        let mut rows: Vec<&String> = raise.keys().collect();
        rows.sort();
        assert_eq!(rows, ["completion tokens", "turns", "wall clock"]);
        assert_eq!(raise["turns"]["field"], "max_iterations");
        assert_eq!(raise["completion tokens"]["field"], "max_tokens");
        // Typed in seconds, sent in the milliseconds run budget carries.
        assert_eq!(
            raise["wall clock"],
            json!({"field": "max_wall_clock_ms", "unit": "s", "per_unit": 1000})
        );
        for row in v["budget"]["limits"].as_array().unwrap() {
            let name = row["limit"].as_str().unwrap();
            assert_eq!(
                raise.contains_key(name),
                name != "context",
                "{name}: context is the driver's, fixed for the run"
            );
        }

        // rpc takes the raise and answers with the ceilings after it: the
        // rows still draw the ceiling in force.
        let c = feed(
            &mut s,
            &response(
                "set_budget",
                true,
                json!({"max_iterations": 20, "max_wall_clock_ms": 60_000, "max_tokens": 1000}),
            ),
        );
        assert!(!c.has(Pane::Budget), "the answer redrew the budget: {c:?}");
        let v = all(&mut s);
        assert_eq!(
            limit(&v, "turns")["ceiling_text"],
            "8",
            "drawn from the answer"
        );
        assert_eq!(v["now"]["max_turns"], 8);

        // The loop's next budget check sends run budget again: that is the
        // ceiling drawn, in the strip as in the row.
        let c = feed(
            &mut s,
            &event(
                "run budget",
                json!({"max_iterations": 20, "max_wall_clock_ms": 60_000,
                       "max_tokens": 1000, "context_bytes": 10_000}),
            ),
        );
        assert!(c.has(Pane::Budget) && c.has(Pane::Now), "{c:?}");
        let v = all(&mut s);
        assert_eq!(limit(&v, "turns")["ceiling_text"], "20");
        assert_eq!(v["now"]["max_turns"], 20);

        // A refused raise changes no ceiling, and says why.
        let c = feed(
            &mut s,
            &response(
                "set_budget",
                false,
                json!("max_iterations 5 is below the ceiling in force, 20"),
            ),
        );
        assert!(c.has(Pane::Alert), "{c:?}");
        let v = all(&mut s);
        assert_eq!(limit(&v, "turns")["ceiling_text"], "20");
        assert!(
            v["alert"].to_string().contains("set_budget refused"),
            "{}",
            v["alert"]
        );

        // Stopping, and ended: nothing to raise.
        let c = s.abort_sent();
        assert!(c.has(Pane::Budget), "{c:?}");
        assert_eq!(all(&mut s)["budget"]["raise"], Value::Null);
        s.abort_unsent();
        assert!(all(&mut s)["budget"]["raise"].is_object());
        feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "converged", "exit": 0, "id": "p1"}).to_string(),
        );
        assert_eq!(all(&mut s)["budget"]["raise"], Value::Null);
    }

    #[test]
    fn phase_is_model_with_its_turn_from_a_model_call_frame() {
        // The risk (#615): replies are not streamed, so a `model call` is
        // the only sign of life between two turns of a slow local model.
        // Dropped, the page shows nothing changing for the whole call and
        // the run reads as hung. The turn is trellis's number, carried as
        // sent; `prompt_bytes` is never on it (client_contract.json).
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p1"));
        assert_eq!(
            all(&mut s)["phase"],
            Value::Null,
            "nothing is on before a call"
        );
        let c = feed(&mut s, &event("model call", json!({"turn": 3})));
        assert!(c.has(Pane::Phase), "{c:?}");
        assert_eq!(all(&mut s)["phase"], json!({"phase": "model", "turn": 3}));
        // Without a turn the event names nothing, and changes nothing.
        let c = feed(&mut s, &event("model call", json!({})));
        assert!(c.is_empty(), "{c:?}");
        assert_eq!(all(&mut s)["phase"], json!({"phase": "model", "turn": 3}));
    }

    #[test]
    fn now_state_word_follows_run_start_approval_abort_and_run_end() {
        // The risk (#828): the Now strip says `running` while the run waits
        // on the person, or after Stop while it rolls back, so the person
        // reads the one row meant to answer "what is happening now" wrong;
        // or an ended run reads `done` or `failed` rather than run_end's own
        // words and exit code (DESIGN.md §2.1). Every change to the panes
        // the strip reads sends the strip again, so it never lags them.
        let mut s = DesktopState::default();
        let now = |s: &mut DesktopState| all(s)["now"].clone();
        assert_eq!(now(&mut s)["word"], "idle");
        assert_eq!(now(&mut s)["can_stop"], false);

        let c = feed(&mut s, &run_start("p1"));
        assert!(c.has(Pane::Now) && c.has(Pane::Where), "{c:?}");
        assert_eq!(now(&mut s)["state"], "running");
        assert_eq!(now(&mut s)["word"], "running");
        assert_eq!(now(&mut s)["model"], "scripted:x");
        assert_eq!(now(&mut s)["can_stop"], true);
        assert_eq!(all(&mut s)["where"]["root"], "/r");
        assert_eq!(all(&mut s)["where"]["phase"], "none");

        feed(&mut s, &event("run budget", json!({"max_iterations": 8})));
        let c = feed(&mut s, &event("model call", json!({"turn": 2})));
        assert!(
            c.has(Pane::Now),
            "a model call leaves the strip behind: {c:?}"
        );
        let v = now(&mut s);
        assert_eq!(
            (v["turn"].clone(), v["max_turns"].clone()),
            (json!(2), json!(8))
        );
        assert_eq!(v["phase"], "model");
        assert_eq!(v["tool"], Value::Null, "no tool while the model is called");
        assert_eq!(all(&mut s)["where"]["phase"], "calling the model, turn 2");
        feed(
            &mut s,
            &turn(json!({"turn": 2, "call": "edit a.py", "verdict": "None",
                         "check_kind": "", "observation": "applied: true",
                         "tool": "edit", "target": "a.py", "wrote": true,
                         "model_ms": 900, "completion_tokens": 12})),
        );
        let v = now(&mut s);
        assert_eq!(
            (v["tool"].clone(), v["target"].clone()),
            (json!("edit"), json!("a.py"))
        );
        let w = all(&mut s)["where"].clone();
        assert_eq!(w["phase"], "ran edit, turn 2");
        assert_eq!(
            (w["model_ms"].clone(), w["completion_tokens"].clone()),
            (json!(900), json!(12))
        );

        let c = s.update(Frame::ApprovalRequest(ApprovalRequest {
            id: "q1".into(),
            question: Some("Run bash?".into()),
            detail: None,
        }));
        assert!(c.has(Pane::Now), "{c:?}");
        let v = now(&mut s);
        assert_eq!(
            (v["state"].clone(), v["word"].clone()),
            (json!("needs_you"), json!("needs you"))
        );
        assert_eq!(v["reason"], "Run bash?", "needs you says what it needs");
        assert_eq!(all(&mut s)["where"]["phase"], "waiting on you");
        // The answer taken, the run goes on.
        feed(
            &mut s,
            &json!({"type": "response", "command": "approve", "success": true, "id": "q1",
                    "data": {"approved": true}})
            .to_string(),
        );
        assert_eq!(now(&mut s)["word"], "running");
        assert_eq!(now(&mut s)["reason"], Value::Null);

        // Stop: an abort forwarded is `stopping`, over a question waiting
        // too, and Stop no longer applies; a refused one stops nothing.
        s.update(Frame::ApprovalRequest(ApprovalRequest {
            id: "q2".into(),
            question: None,
            detail: None,
        }));
        let c = s.abort_sent();
        assert!(c.has(Pane::Now), "{c:?}");
        assert_eq!(now(&mut s)["word"], "stopping");
        assert_eq!(now(&mut s)["can_stop"], false);
        assert_eq!(all(&mut s)["where"]["phase"], "rolling back");
        let c = feed(
            &mut s,
            &json!({"type": "response", "command": "abort", "success": false, "id": "ab",
                    "error": "no run in flight"})
            .to_string(),
        );
        assert!(c.has(Pane::Now), "{c:?}");
        assert_eq!(now(&mut s)["word"], "needs you");
        s.abort_sent();
        let c = feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "interrupted", "exit": 130,
                    "restored": true, "id": "p1"})
            .to_string(),
        );
        assert!(c.has(Pane::Now) && c.has(Pane::Where), "{c:?}");
        let v = now(&mut s);
        assert_eq!(v["state"], "ended");
        assert_eq!(v["word"], "ended · interrupted");
        assert_eq!(v["exit"], 130, "an ended run carries its exit code");
        assert_eq!(v["can_stop"], false);
        assert_eq!(
            (v["turn"].clone(), v["phase"].clone()),
            (Value::Null, Value::Null)
        );
        assert_eq!(all(&mut s)["where"]["phase"], "ended");
        // run_end's word as the outcome pane spells it, never done/failed.
        feed(&mut s, &run_start("p2"));
        assert_eq!(now(&mut s)["word"], "running", "a new run is not stopping");
        feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "not_converged", "exit": 1, "id": "p2"})
                .to_string(),
        );
        assert_eq!(now(&mut s)["word"], "ended · not converged");
        // A child that exits mid-run leaves no run, and nothing stopping.
        feed(&mut s, &run_start("p3"));
        s.abort_sent();
        let c = s.child_exited();
        assert!(c.has(Pane::Now), "{c:?}");
        assert_eq!(now(&mut s)["word"], "idle");
        assert_eq!(now(&mut s)["model"], Value::Null, "the last child's model");
        assert_eq!(
            all(&mut s)["where"]["root"],
            Value::Null,
            "the last child's root"
        );
        assert_eq!(
            s.abort_unsent(),
            Changed::of(&[Pane::Now, Pane::Where, Pane::Budget, Pane::Plan])
        );
    }

    /// The view as the page draws it with the register dial at `dial`:
    /// `now.register` resolved to that position's slot, everything else as
    /// the reducer sent it. now.js reads the dial nowhere else, which
    /// `register_dial_changes_copy_and_faces_only` also holds.
    fn drawn_at(view: &Value, dial: &str) -> (Value, Value) {
        let mut rest = view.clone();
        let slot = rest["now"]
            .as_object_mut()
            .and_then(|n| n.remove("register"))
            .map(|r| r[dial].clone())
            .unwrap_or(Value::Null);
        (rest, slot)
    }

    #[test]
    fn register_dial_changes_copy_and_faces_only() {
        // The risk (#836, DESIGN.md §3.1): the playful register moving a
        // figure, a state word, a check row, an action or an outcome, or a
        // face that differs by outcome and so reads as a status; or more
        // than one face on a screen. The dial lives in the page (a Tauri
        // command for it would be a capability ADR 0048 does not allow),
        // so the reducer sends both positions under `now.register` and the
        // page picks one: with that slot set aside, the view each position
        // draws is the same, and the slot holds a face and its copy only.
        const FACES: [&str; 5] = ["(•_•)", "(•‿•)", "(ಠ_ಠ)", "\\(^o^)/", "¯\\_(ツ)_/¯"];
        let mut s = DesktopState::default();
        let mut views = vec![("idle", all(&mut s))];
        feed(&mut s, &run_start("p1"));
        feed(&mut s, &event("run budget", json!({"max_iterations": 8})));
        views.push(("running", all(&mut s)));
        feed(&mut s, &event("model call", json!({"turn": 1})));
        views.push(("model call", all(&mut s)));
        feed(
            &mut s,
            &turn(
                json!({"turn": 1, "call": "run_checks a.py", "verdict": "Some(true)",
                         "check_kind": "Typecheck", "observation": "ok",
                         "tool": "run_checks", "target": "a.py", "wrote": false,
                         "elapsed_ms": 4000}),
            ),
        );
        views.push(("check row", all(&mut s)));
        s.update(Frame::ApprovalRequest(ApprovalRequest {
            id: "q1".into(),
            question: Some("Run bash?".into()),
            detail: None,
        }));
        views.push(("needs you", all(&mut s)));
        s.abort_sent();
        views.push(("stopping", all(&mut s)));
        feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "converged", "exit": 0, "id": "p1",
                    "elapsed_ms": 5000})
            .to_string(),
        );
        let converged = all(&mut s);
        views.push(("converged", converged.clone()));
        feed(&mut s, &run_start("p2"));
        feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "error", "exit": 2, "id": "p2"}).to_string(),
        );
        let errored = all(&mut s);
        views.push(("error", errored.clone()));
        s.child_exited();
        views.push(("child gone", all(&mut s)));

        // Every word the strip, the outcome and the checks can say, none of
        // which the copy may borrow.
        let mut words: Vec<String> = [
            "idle",
            "running",
            "needs you",
            "stopping",
            "ended",
            "converged",
            "passed",
            "failed",
            "error",
            "interrupted",
            "not converged",
            "done",
        ]
        .map(String::from)
        .to_vec();
        for (_, v) in &views {
            for key in ["word", "outcome", "phase"] {
                if let Some(w) = v["now"][key].as_str() {
                    words.push(w.to_string());
                }
            }
        }

        for (at, v) in &views {
            let (serious_rest, serious) = drawn_at(v, "serious");
            let (playful_rest, playful) = drawn_at(v, "playful");
            assert_eq!(
                serious_rest, playful_rest,
                "{at}: the dial moved something other than the face and its copy"
            );
            assert_eq!(serious, Value::Null, "{at}: serious adds a face or copy");
            let mut faces = 0;
            if !playful.is_null() {
                let slot = playful.as_object().expect("a slot is an object or null");
                let keys: Vec<&str> = slot.keys().map(String::as_str).collect();
                assert_eq!(keys, ["copy", "face"], "{at}: the slot holds {keys:?}");
                let face = slot["face"].as_str().expect("a face is one string");
                assert!(
                    FACES.contains(&face),
                    "{at}: {face} is not the brand book's"
                );
                let copy = slot["copy"].as_str().expect("copy is text");
                assert!(
                    !copy.chars().any(|c| c.is_ascii_digit()),
                    "{at}: the copy carries a figure: {copy}"
                );
                for w in &words {
                    assert!(
                        !copy.contains(w.as_str()),
                        "{at}: the copy says the state word {w:?}: {copy}"
                    );
                }
                faces += 1;
            }
            // No face anywhere else in the view: one per screen at most.
            let rest = serde_json::to_string(&playful_rest).unwrap();
            faces += FACES.iter().filter(|f| rest.contains(*f)).count();
            assert!(faces <= 1, "{at}: {faces} faces on one screen");
        }

        // Where a person would otherwise see nothing: before a run, and
        // once one has ended; never while the strip has a run to show.
        let has_face = |at: &str| {
            let v = &views.iter().find(|(a, _)| *a == at).unwrap().1;
            !drawn_at(v, "playful").1.is_null()
        };
        for at in ["idle", "converged", "error", "child gone"] {
            assert!(has_face(at), "{at} shows no face in the playful register");
        }
        for at in [
            "running",
            "model call",
            "check row",
            "needs you",
            "stopping",
        ] {
            assert!(!has_face(at), "{at} shows a face over a run in flight");
        }
        // A face is never a status: a converged and an errored run's are one.
        assert_eq!(
            drawn_at(&converged, "playful").1,
            drawn_at(&errored, "playful").1,
            "the face differs by outcome"
        );

        // The page reads the dial in one place, the face's, and draws the
        // face hidden from a screen reader.
        let ui = |name: &str| {
            std::fs::read_to_string(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("ui")
                    .join(name),
            )
            .unwrap()
        };
        let now_js = ui("now.js");
        assert_eq!(
            now_js.matches("currentRegister(").count(),
            1,
            "now.js reads the dial outside the face"
        );
        let face_fn = now_js
            .split("function registerFace(")
            .nth(1)
            .and_then(|r| r.split("\n}\n").next())
            .expect("now.js draws the face in registerFace");
        assert!(face_fn.contains("currentRegister()"), "{face_fn}");
        assert!(face_fn.contains("asciiFace(doc, "), "{face_fn}");
        let ds_js = ui("ds.js");
        let ascii = ds_js
            .split("export function asciiFace(")
            .nth(1)
            .and_then(|r| r.split("\n}\n").next())
            .expect("ds.js draws the Ascii face");
        assert!(
            ascii.contains("glyphs.setAttribute(\"aria-hidden\", \"true\")"),
            "the face is read aloud: {ascii}"
        );
    }

    #[test]
    fn now_clock_is_the_engines_elapsed_or_absent() {
        // The risk (#828, ADR 0048 §8): the strip draws a clock the desktop
        // keeps, which counts the time a question waited on the person
        // that trellis's own budget leaves out, and so disagrees with the
        // budget it is shown beside; or a run its loop did not finish shows
        // the last turn's figure as the run's (run_end leaves elapsed_ms
        // out, "never a partial figure"). The clock is only ever the last
        // elapsed_ms trellis sent, and null with the reason otherwise.
        let mut s = DesktopState::default();
        let now = |s: &mut DesktopState| all(s)["now"].clone();
        let v = now(&mut s);
        assert_eq!(
            (v["elapsed"].clone(), v["elapsed_why"].clone()),
            (Value::Null, Value::Null),
            "idle shows no clock at all"
        );

        feed(&mut s, &run_start("p1"));
        let v = now(&mut s);
        assert_eq!(
            v["elapsed"],
            Value::Null,
            "no figure yet is none, never 00:00"
        );
        assert_eq!(
            v["elapsed_why"],
            "trellis has sent no elapsed time for this run yet"
        );

        let c = feed(
            &mut s,
            &event("model call", json!({"turn": 1, "elapsed_ms": 1999})),
        );
        assert!(c.has(Pane::Now), "{c:?}");
        assert_eq!(
            now(&mut s)["elapsed"],
            "00:01",
            "cut down, never rounded up"
        );
        assert_eq!(now(&mut s)["elapsed_why"], Value::Null);
        feed(
            &mut s,
            &turn(
                json!({"turn": 1, "call": "read_file a.py", "verdict": "None",
                         "check_kind": "", "observation": "x", "tool": "read_file",
                         "target": "a.py", "wrote": false, "elapsed_ms": 125_000}),
            ),
        );
        assert_eq!(now(&mut s)["elapsed"], "02:05");
        // An event without the figure leaves the last one: between events
        // the strip shows what trellis last said, and nothing ticks.
        feed(&mut s, &event("model call", json!({"turn": 2})));
        assert_eq!(now(&mut s)["elapsed"], "02:05");
        assert_eq!(s.elapsed_ms, Some(125_000));
        feed(
            &mut s,
            &event("model call", json!({"turn": 3, "elapsed_ms": 3_725_000})),
        );
        assert_eq!(now(&mut s)["elapsed"], "1:02:05");

        // run_end with its figure: the run's whole spend.
        feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "converged", "exit": 0, "id": "p1",
                    "elapsed_ms": 3_726_000})
            .to_string(),
        );
        assert_eq!(now(&mut s)["elapsed"], "1:02:06");

        // run_end without it: absent, with why, not the last turn's figure.
        feed(&mut s, &run_start("p2"));
        assert_eq!(
            now(&mut s)["elapsed"],
            Value::Null,
            "a new run starts with none"
        );
        feed(
            &mut s,
            &event("model call", json!({"turn": 1, "elapsed_ms": 4000})),
        );
        assert_eq!(now(&mut s)["elapsed"], "00:04");
        feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "interrupted", "exit": 130, "id": "p2"})
                .to_string(),
        );
        let v = now(&mut s);
        assert_eq!(
            v["elapsed"],
            Value::Null,
            "a partial figure passed off as the run's"
        );
        assert_eq!(
            v["elapsed_why"],
            "run_end carried no elapsed time: trellis sends none for a run its loop did not finish"
        );
        assert_eq!(s.elapsed_ms, None);
    }

    #[test]
    fn phase_is_tool_or_checks_from_a_turn_frame_s_call() {
        // The risk: a turn says what its call ran, and the page keeps
        // saying "model" after the reply came; or `run_checks` reads as a
        // tool like any other, when it is the one the user waits on.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p1"));
        feed(&mut s, &event("model call", json!({"turn": 1})));
        let c = feed(
            &mut s,
            &turn(
                json!({"turn": 1, "call": "edit app/core.py", "verdict": "None",
                         "check_kind": "", "observation": "applied: true (+1 -1)",
                         "tool": "edit", "target": "app/core.py", "wrote": true}),
            ),
        );
        assert!(c.has(Pane::Phase), "{c:?}");
        assert_eq!(
            all(&mut s)["phase"],
            json!({"phase": "tool", "turn": 1, "tool": "edit"})
        );
        feed(&mut s, &event("model call", json!({"turn": 2})));
        feed(
            &mut s,
            &turn(
                json!({"turn": 2, "call": "run_checks app/core.py", "verdict": "Some(true)",
                         "check_kind": "Tests", "observation": "3 passed",
                         "tool": "run_checks", "target": "Tests on app/core.py",
                         "wrote": false}),
            ),
        );
        assert_eq!(all(&mut s)["phase"], json!({"phase": "checks", "turn": 2}));
        // The governance audit is a run_checks the loop ran itself, under
        // the last call's number: checks too.
        feed(
            &mut s,
            &turn(
                json!({"turn": 2, "call": "run_checks", "verdict": "Some(false)",
                         "check_kind": "Governance", "observation": "VIOLATION",
                         "tool": "run_checks", "target": "Governance on whole repository",
                         "wrote": false}),
            ),
        );
        assert_eq!(all(&mut s)["phase"], json!({"phase": "checks", "turn": 2}));
    }

    #[test]
    fn phase_is_cleared_by_run_end_and_by_a_new_run_s_start() {
        // The risk: a run ends and the bar still says the model is being
        // called, which is the hang it was meant to tell apart; or the next
        // run starts under the last run's phase.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p1"));
        feed(&mut s, &event("model call", json!({"turn": 1})));
        let c = feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "not_converged", "exit": 1,
                    "restored": true, "id": "p1"})
            .to_string(),
        );
        assert!(c.has(Pane::Phase), "{c:?}");
        assert_eq!(all(&mut s)["phase"], Value::Null);
        feed(&mut s, &event("model call", json!({"turn": 1})));
        let c = feed(&mut s, &run_start("p2"));
        assert!(c.has(Pane::Phase), "{c:?}");
        assert_eq!(all(&mut s)["phase"], Value::Null);
    }

    #[test]
    fn child_exit_clears_the_phase() {
        // The risk (#815): the child exited mid-call and the page went on
        // reading `calling the model, turn 3` beside `exited`, a hang that
        // is not one; or the phase cleared and not sent, so the page kept
        // drawing it. The exit clears it, as run_end does, and sends it.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p1"));
        feed(&mut s, &event("model call", json!({"turn": 3})));
        assert_eq!(all(&mut s)["phase"], json!({"phase": "model", "turn": 3}));
        let c = s.child_exited();
        assert!(c.has(Pane::Phase), "{c:?}");
        assert_eq!(s.view(c)["phase"], Value::Null);
        assert_eq!(s.view(c)["running"], false);
    }

    #[test]
    fn a_prompt_is_in_flight_until_its_run_starts_or_it_is_refused() {
        // The risk (#815): `running` comes only with the run's `run_start`,
        // so a prompt the child had and not yet answered counted as nothing
        // in flight, and `open_repository` quit the run it was starting.
        // Each forwarded prompt counts until its run starts, the child
        // refuses it, or the child exits; a second prompt's refusal does
        // not end the first's count; and Clear keeps it, as it keeps
        // `running`.
        let run_end = |id: &str| {
            json!({"type": "run_end", "outcome": "converged", "exit": 0, "id": id}).to_string()
        };
        let mut s = DesktopState::default();
        assert!(!s.prompt_in_flight());
        s.prompt_forwarded();
        assert!(s.prompt_in_flight());
        s.clear();
        assert!(s.prompt_in_flight(), "Clear ended a prompt in flight");
        feed(&mut s, &run_start("p1"));
        assert!(!s.prompt_in_flight() && s.running(), "the run says it now");
        feed(&mut s, &run_end("p1"));
        // Two sent before either is answered: the second is refused, and
        // the first is still starting.
        s.prompt_forwarded();
        s.prompt_forwarded();
        feed(&mut s, &response("prompt", false, json!("one at a time")));
        assert!(s.prompt_in_flight(), "the refusal was the second prompt's");
        feed(&mut s, &run_start("p2"));
        assert!(!s.prompt_in_flight());
        feed(&mut s, &run_end("p2"));
        // A successful answer is no run yet; the child's exit takes it.
        s.prompt_forwarded();
        feed(&mut s, &response("prompt", true, Value::Null));
        assert!(s.prompt_in_flight());
        s.child_exited();
        assert!(!s.prompt_in_flight());
    }

    #[test]
    fn runtime_pane_carries_every_field_colibri_reported() {
        // The risk (#628): the self-report trellis sends for a colibri run
        // (client_contract.json's `runtime identified`) dropped on the way
        // to the page, or a count carried under another number.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p1"));
        assert_eq!(
            all(&mut s)["runtime"],
            Value::Null,
            "nothing before the event"
        );
        let c = feed(
            &mut s,
            &event(
                "runtime identified",
                json!({"runtime": "colibri", "served": "glm-5.2", "kv_slots": 16,
                       "tiers_vram": 120, "tiers_ram": 2400, "tiers_disk": 17000}),
            ),
        );
        assert!(c.has(Pane::Runtime), "{c:?}");
        assert_eq!(
            all(&mut s)["runtime"],
            json!({"runtime": "colibri", "served": "glm-5.2", "kv_slots": 16,
                   "tiers_vram": 120, "tiers_ram": 2400, "tiers_disk": 17000})
        );
        // Three counts and nothing derived from them (ADR 0048 §8).
        let mut ks = Vec::new();
        keys(&all(&mut s)["runtime"], &mut ks);
        assert!(
            !ks.iter().any(|k| {
                let k = k.to_lowercase();
                ["ratio", "percent", "fraction", "total", "share"]
                    .iter()
                    .any(|w| k.contains(w))
            }),
            "{ks:?}"
        );
        // Without `served` the event names no runtime, and changes nothing.
        let c = feed(&mut s, &event("runtime identified", json!({"kv_slots": 1})));
        assert!(c.is_empty(), "{c:?}");
        assert_eq!(all(&mut s)["runtime"]["served"], "glm-5.2");
    }

    #[test]
    fn runtime_pane_unreported_counts_are_null_and_a_new_run_starts_without_one() {
        // The risk: a liveness-only /health, which reports no slot and no
        // tier, drawn as 0 slots and 0 experts (ADR 0048 §8); or the next
        // run, against another endpoint, still showing this run's report.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p1"));
        let c = feed(
            &mut s,
            &event(
                "runtime identified",
                json!({"runtime": "colibri", "served": "glm-5.2"}),
            ),
        );
        assert!(c.has(Pane::Runtime), "{c:?}");
        assert_eq!(
            all(&mut s)["runtime"],
            json!({"runtime": "colibri", "served": "glm-5.2", "kv_slots": null,
                   "tiers_vram": null, "tiers_ram": null, "tiers_disk": null})
        );
        // A count of the wrong JSON type is unreported too, never read as 0.
        feed(
            &mut s,
            &event(
                "runtime identified",
                json!({"runtime": "colibri", "served": "glm-5.2", "kv_slots": "16",
                       "tiers_vram": -1}),
            ),
        );
        assert_eq!(all(&mut s)["runtime"]["kv_slots"], Value::Null);
        assert_eq!(all(&mut s)["runtime"]["tiers_vram"], Value::Null);
        let c = feed(&mut s, &run_start("p2"));
        assert!(c.has(Pane::Runtime), "{c:?}");
        assert_eq!(all(&mut s)["runtime"], Value::Null);
    }

    #[test]
    fn radius_holds_three_counts_and_no_ratio() {
        // The risk (CLAUDE.md rule 3): a ratio or percentage turns three
        // counts into a confidence figure trellis never computed.
        let mut s = DesktopState::default();
        let c = feed(
            &mut s,
            &turn(
                json!({"turn": 1, "call": "blast_radius f", "verdict": "None",
                         "check_kind": "", "observation": "", "resolved": 4,
                         "candidates": 2, "unresolved": 1, "prompt_bytes": 900}),
            ),
        );
        assert!(c.has(Pane::Radius));
        let v = all(&mut s);
        assert_eq!(
            v["radius"],
            json!({"resolved": 4, "candidates": 2, "unresolved": 1})
        );
        let mut ks = Vec::new();
        keys(&v, &mut ks);
        let bad: Vec<&String> = ks
            .iter()
            .filter(|k| {
                let k = k.to_lowercase();
                ["ratio", "percent", "fraction", "total"]
                    .iter()
                    .any(|w| k.contains(w))
            })
            .collect();
        assert!(bad.is_empty(), "the view derives a figure: {bad:?} in {v}");

        // Two of three counts is not a radius.
        let mut s = DesktopState::default();
        feed(
            &mut s,
            &turn(json!({"turn": 1, "call": "blast_radius f", "resolved": 4, "candidates": 2})),
        );
        assert_eq!(all(&mut s)["radius"], Value::Null);
    }

    #[test]
    fn budget_reports_prompt_bytes_not_tokens() {
        // The risk (#243): an unreported prompt size drawn as 0, or labelled
        // tokens, reads as a measurement trellis did not make.
        let mut s = DesktopState::default();
        feed(&mut s, &event("run budget", json!({"max_iterations": 40})));
        feed(
            &mut s,
            &turn(
                json!({"turn": 1, "call": "read_file a.py", "verdict": "None",
                         "check_kind": "", "observation": "", "prompt_bytes": 9800}),
            ),
        );
        let v = all(&mut s);
        let not_sent = "trellis has sent no run budget carrying it for this run";
        assert_eq!(
            v["budget"],
            json!({"turn": 1, "max_turns": 40, "prompt_bytes": 9800,
            "completion_tokens": null, "model_ms": null, "queue_wait_ms": null,
            "tokens_per_second": null, "queued_seconds": null, "raise": null,
            "limits": [
                {"limit": "turns", "unit": null, "used": 1, "ceiling": 40,
                 "used_text": "1", "ceiling_text": "40", "why_used": null,
                 "why_ceiling": null, "cells": 16, "filled": 1, "tone": "cool"},
                {"limit": "wall clock", "unit": null, "used": null, "ceiling": null,
                 "used_text": null, "ceiling_text": null,
                 "why_used": "trellis has sent no elapsed time for this run yet",
                 "why_ceiling": not_sent, "cells": 16, "filled": null, "tone": null},
                {"limit": "completion tokens", "unit": "tok", "used": null,
                 "ceiling": null, "used_text": null, "ceiling_text": null,
                 "why_used": "no turn has reported completion_tokens_used",
                 "why_ceiling": not_sent, "cells": 16, "filled": null, "tone": null},
                {"limit": "context", "unit": "B", "used": 9800, "ceiling": null,
                 "used_text": "9800", "ceiling_text": null, "why_used": null,
                 "why_ceiling": "the run budget carried no context_bytes: a driver \
                                 that elides nothing, the scripted one, has no \
                                 context ceiling",
                 "cells": 16, "filled": null, "tone": null},
            ]})
        );

        // A scripted turn: no prompt was sent, so no size is reported.
        feed(
            &mut s,
            &turn(
                json!({"turn": 2, "call": "read_file a.py", "verdict": "None",
                         "check_kind": "", "observation": ""}),
            ),
        );
        let v = all(&mut s);
        assert_eq!(v["budget"]["turn"], 2);
        assert!(
            v["budget"]["prompt_bytes"].is_null(),
            "unreported, not 0: {v}"
        );
        // The prompt is in bytes: no per-turn prompt token count reaches
        // the stream, so the only token figures are the reply's (#312).
        let mut ks = Vec::new();
        keys(&v["budget"], &mut ks);
        let tokens: Vec<&String> = ks.iter().filter(|k| k.contains("token")).collect();
        assert_eq!(
            tokens,
            ["completion_tokens", "tokens_per_second"],
            "a prompt figure labelled tokens: {ks:?}"
        );
    }

    /// The budget view after one provider turn reporting `usage`'s fields.
    fn budget_after(usage: Value) -> Value {
        let mut s = DesktopState::default();
        let mut f = json!({"turn": 1, "call": "read_file a.py", "verdict": "None",
                           "check_kind": "", "observation": "", "prompt_bytes": 900});
        f.as_object_mut()
            .unwrap()
            .extend(usage.as_object().unwrap().clone());
        feed(&mut s, &turn(f));
        all(&mut s)["budget"].clone()
    }

    #[test]
    fn turn_usage_rate_excludes_queue_wait() {
        // The risk (#257, COL-11): a rate over the whole call reads a turn
        // that sat in colibri's queue as a slow model. 30 tokens in 25 s
        // with 10 s queued is 2.0 tok/s, not 1.2, the figures the TUI's
        // `budget_line_rate_excludes_queue_wait` shows.
        let b = budget_after(
            json!({"completion_tokens": 30, "model_ms": 25000, "queue_wait_ms": 10000}),
        );
        assert_eq!(b["completion_tokens"], 30, "{b}");
        assert_eq!(b["model_ms"], 25000, "{b}");
        assert_eq!(b["queue_wait_ms"], 10000, "{b}");
        assert_eq!(b["tokens_per_second"], "2.0", "{b}");
        assert_eq!(b["queued_seconds"], "10.0", "{b}");
    }

    #[test]
    fn turn_usage_unknown_queue_wait_counts_as_zero_and_shows_nothing() {
        // The risk: an unreported wait shown as "queued 0.0 s" claims the
        // runtime said the turn never queued.
        let b = budget_after(json!({"completion_tokens": 30, "model_ms": 15000}));
        assert_eq!(b["tokens_per_second"], "2.0", "{b}");
        assert!(b["queue_wait_ms"].is_null(), "{b}");
        assert!(b["queued_seconds"].is_null(), "{b}");
    }

    #[test]
    fn turn_usage_rate_needs_model_time_beyond_the_queue_wait() {
        // The risk: a call that was all queue divides by zero, or draws an
        // infinite rate, where there is no rate to draw.
        let b = budget_after(
            json!({"completion_tokens": 30, "model_ms": 10000, "queue_wait_ms": 10000}),
        );
        assert!(b["tokens_per_second"].is_null(), "{b}");
        assert_eq!(b["queued_seconds"], "10.0", "{b}");
    }

    #[test]
    fn turn_usage_without_output_tokens_has_no_rate() {
        // The risk: a provider that reported no output tokens drawn as
        // "0 tok, 0.0 tok/s", which reads as a stuck turn. Absent is null.
        let b = budget_after(json!({"model_ms": 25000, "queue_wait_ms": 10000}));
        assert!(b["completion_tokens"].is_null(), "{b}");
        assert!(b["tokens_per_second"].is_null(), "{b}");
        assert!(b["queued_seconds"].is_null(), "{b}");
        assert_eq!(b["model_ms"], 25000, "the call's time was reported: {b}");
    }

    #[test]
    fn turn_usage_is_per_turn_and_never_summed() {
        // The risk: the budget adds turns' output tokens into a run total
        // nobody reported, or an action shows another turn's figure.
        let mut s = DesktopState::default();
        feed(
            &mut s,
            &turn(
                json!({"turn": 1, "call": "read_file a.py", "verdict": "None",
                         "check_kind": "", "observation": "", "prompt_bytes": 900,
                         "completion_tokens": 3, "model_ms": 1000}),
            ),
        );
        feed(
            &mut s,
            &turn(json!({"turn": 2, "call": "done ok", "verdict": "None",
                         "check_kind": "", "observation": "", "prompt_bytes": 1200,
                         "completion_tokens": 5, "model_ms": 1000})),
        );
        let v = all(&mut s);
        assert_eq!(v["budget"]["completion_tokens"], 5, "{v}");
        assert_eq!(v["budget"]["prompt_bytes"], 1200, "{v}");
        let per_action: Vec<(Value, Value)> = v["actions"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| (a["prompt_bytes"].clone(), a["completion_tokens"].clone()))
            .collect();
        assert_eq!(
            per_action,
            [(json!(900), json!(3)), (json!(1200), json!(5))],
            "{v}"
        );
        let mut ks = Vec::new();
        keys(&v, &mut ks);
        let bad: Vec<&String> = ks
            .iter()
            .filter(|k| k.contains("total") || k.contains("sum"))
            .collect();
        assert!(bad.is_empty(), "the view sums turns: {bad:?} in {v}");
    }

    /// The budget row for `limit` in a view.
    fn limit<'a>(v: &'a Value, limit: &str) -> &'a Value {
        v["budget"]["limits"]
            .as_array()
            .and_then(|rows| rows.iter().find(|r| r["limit"] == limit))
            .unwrap_or_else(|| panic!("no {limit} row: {v}"))
    }

    /// A row's meter: (used text, ceiling text, cells filled, tone).
    fn meter(row: &Value) -> (Value, Value, Value, Value) {
        assert_eq!(row["cells"], 16, "the cell count moved: {row}");
        (
            row["used_text"].clone(),
            row["ceiling_text"].clone(),
            row["filled"].clone(),
            row["tone"].clone(),
        )
    }

    #[test]
    fn budget_meter_is_text_and_names_unreported_rows() {
        // The risk (#829): a budget drawn as a percentage, which reads as
        // percent-complete with no denominator (DESIGN.md rule 4); a limit
        // trellis did not report drawn as an empty meter or 0, a claim it
        // never made; a raise (#807) drawn against the old ceiling; or a
        // meter whose tone the page decides. Each row is two figures as
        // text and a fixed count of cells, its tone the TUI's (cool, signal
        // past 70%, down past 90%), and an unreported side is null with why.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p1"));
        let c = feed(
            &mut s,
            &event(
                "run budget",
                json!({"max_iterations": 10, "max_wall_clock_ms": 100_000,
                       "max_tokens": 1000, "context_bytes": 10_000}),
            ),
        );
        assert!(c.has(Pane::Budget), "{c:?}");
        let c = feed(
            &mut s,
            &event("model call", json!({"turn": 7, "elapsed_ms": 70_000})),
        );
        assert!(c.has(Pane::Budget), "the clock row left stale: {c:?}");
        let v = all(&mut s);
        // Exactly 70% is not past it.
        assert_eq!(
            meter(limit(&v, "wall clock")),
            (json!("01:10"), json!("01:40"), json!(11), json!("cool"))
        );
        assert_eq!(
            meter(limit(&v, "turns")),
            (json!("7"), json!("10"), json!(11), json!("cool"))
        );

        feed(
            &mut s,
            &turn(
                json!({"turn": 7, "call": "read_file a.py", "verdict": "None",
                         "check_kind": "", "observation": "", "prompt_bytes": 9100,
                         "completion_tokens": 30, "completion_tokens_used": 710,
                         "elapsed_ms": 71_000}),
            ),
        );
        let v = all(&mut s);
        assert_eq!(
            meter(limit(&v, "wall clock")),
            (json!("01:11"), json!("01:40"), json!(11), json!("signal"))
        );
        // trellis's counter, not the turn's own figure nor a sum of turns.
        assert_eq!(
            meter(limit(&v, "completion tokens")),
            (json!("710"), json!("1000"), json!(11), json!("signal"))
        );
        assert_eq!(limit(&v, "completion tokens")["unit"], "tok");
        assert_eq!(
            meter(limit(&v, "context")),
            (json!("9100"), json!("10000"), json!(14), json!("down"))
        );
        assert_eq!(limit(&v, "context")["unit"], "B");
        assert_eq!(v["budget"]["completion_tokens"], 30, "{v}");

        // A raise is drawn from the run budget sent again, whole.
        feed(
            &mut s,
            &event(
                "run budget",
                json!({"max_iterations": 10, "max_wall_clock_ms": 100_000,
                       "max_tokens": 2000, "context_bytes": 10_000}),
            ),
        );
        let v = all(&mut s);
        assert_eq!(
            meter(limit(&v, "completion tokens")),
            (json!("710"), json!("2000"), json!(5), json!("cool"))
        );

        // A scripted run: no context ceiling, no prompt, and a counter of 0
        // that trellis did report. At the ceiling every cell is filled.
        feed(&mut s, &run_start("p2"));
        feed(
            &mut s,
            &event(
                "run budget",
                json!({"max_iterations": 3, "max_wall_clock_ms": 1000, "max_tokens": 5}),
            ),
        );
        feed(
            &mut s,
            &turn(
                json!({"turn": 3, "call": "blast_radius f", "verdict": "None",
                         "check_kind": "", "observation": "", "resolved": 4,
                         "candidates": 2, "unresolved": 1, "completion_tokens_used": 0,
                         "elapsed_ms": 10}),
            ),
        );
        let v = all(&mut s);
        assert_eq!(
            meter(limit(&v, "turns")),
            (json!("3"), json!("3"), json!(16), json!("down"))
        );
        assert_eq!(
            meter(limit(&v, "completion tokens")),
            (json!("0"), json!("5"), json!(0), json!("cool")),
            "a reported 0 is 0"
        );
        let context = limit(&v, "context");
        assert_eq!(
            meter(context),
            (Value::Null, Value::Null, Value::Null, Value::Null),
            "an unreported row drawn as a meter: {context}"
        );
        assert_eq!(
            (&context["used"], &context["ceiling"]),
            (&Value::Null, &Value::Null)
        );
        assert!(
            context["why_ceiling"]
                .as_str()
                .is_some_and(|w| w.contains("context_bytes")),
            "{context}"
        );
        assert!(context["why_used"].as_str().is_some_and(|w| !w.is_empty()));
        // A reported figure carries no reason; every reason is for a null.
        for row in v["budget"]["limits"].as_array().unwrap() {
            for (fig, why) in [("used", "why_used"), ("ceiling", "why_ceiling")] {
                assert_eq!(row[fig].is_null(), row[why].is_string(), "{row}");
            }
        }

        // Text, never a share: no figure the rows carry is a percentage,
        // and the radius beside them carries its three counts and no meter
        // (CLAUDE.md rule 3).
        let mut texts = Vec::new();
        let mut stack = vec![&v["budget"]["limits"]];
        while let Some(x) = stack.pop() {
            match x {
                Value::String(t) => texts.push(t.clone()),
                Value::Array(a) => stack.extend(a),
                Value::Object(o) => stack.extend(o.values()),
                _ => {}
            }
        }
        assert!(!texts.iter().any(|t| t.contains('%')), "{texts:?}");
        assert_eq!(
            v["radius"],
            json!({"resolved": 4, "candidates": 2, "unresolved": 1})
        );
    }

    #[test]
    fn run_start_resets_the_run_panes_and_clear_resets_the_view() {
        // The risk: a second run drawn on top of the first's actions, edits
        // and outcome, so the panes describe neither.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p1"));
        feed(&mut s, &event("run budget", json!({"max_iterations": 40})));
        feed(
            &mut s,
            &turn(
                json!({"turn": 1, "call": "edit a.py", "verdict": "Some(true)",
                         "check_kind": "Tests", "observation": "applied: true",
                         "tool": "edit", "target": "a.py", "wrote": true,
                         "resolved": 1, "candidates": 0, "unresolved": 0,
                         "prompt_bytes": 100}),
            ),
        );
        feed(
            &mut s,
            &json!({"type": "approval_request", "id": "approval-1", "question": "q",
                    "detail": "d"})
            .to_string(),
        );
        feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "converged", "exit": 0, "id": "p1"}).to_string(),
        );
        let before = rows(&all(&mut s)["transcript"]).len();
        assert_eq!(all(&mut s)["edits"], json!(["a.py"]), "an edit to reset");

        let c = feed(&mut s, &run_start("p2"));
        for p in RUN_PANES {
            assert!(c.has(p), "{p:?} not sent");
        }
        let v = all(&mut s);
        assert_eq!(v["actions"], json!([]));
        assert_eq!(v["edits"], json!([]));
        assert_eq!(v["checks"], json!([]));
        assert_eq!(v["radius"], Value::Null);
        let limits = v["budget"]["limits"].as_array().expect("budget rows");
        assert_eq!(limits.len(), 4, "{v}");
        for row in limits {
            for k in ["used", "ceiling", "filled", "tone"] {
                assert!(row[k].is_null(), "the last run's {k} kept: {row}");
            }
        }
        assert_eq!(
            v["budget"],
            json!({"turn": null, "max_turns": null, "prompt_bytes": null,
                   "completion_tokens": null, "model_ms": null, "queue_wait_ms": null,
                   "tokens_per_second": null, "queued_seconds": null,
                   "raise": v["budget"]["raise"], "limits": limits})
        );
        assert_eq!(v["approvals"], json!([]));
        assert_eq!(v["outcome"], Value::Null);
        assert_eq!(v["running"], true);
        // The transcript runs on across runs, as the TUI's does.
        let t = rows(&v["transcript"]);
        assert_eq!(t.len(), before + 1);
        assert_eq!(
            (&t[before]["kind"], &t[before]["text"]),
            (&json!("task"), &json!("change f"))
        );

        let c = s.clear();
        for p in PANES {
            assert!(c.has(p), "{p:?} not sent");
        }
        let v = all(&mut s);
        assert_eq!(rows(&v["transcript"]), Vec::<Value>::new());
        assert_eq!(
            v["transcript"]["whole"], true,
            "the page drops what it drew"
        );
        assert_eq!(
            v["running"], true,
            "clearing the view does not end the run in flight"
        );
    }

    #[test]
    fn pane_bits_hold_more_than_thirty_two_panes() {
        // The risk (#815): `Changed` was a u32, so a 33rd pane shifted past
        // it: a panic in a debug build, and in a release one the
        // transcript's bit, so that pane's changes went out as the
        // transcript's and its own never did. Every place up to 64 is a bit
        // of its own (a debug build, as this one, panics on a shift past
        // the width), so a pane at the 33rd place is carried as itself.
        let mut taken = 0u64;
        for place in 0..64 {
            let bit = Changed::bit_at(place);
            assert_eq!(bit.count_ones(), 1, "place {place}");
            assert_eq!(taken & bit, 0, "place {place} is an earlier place's bit");
            taken |= bit;
        }
        let past = Changed(Changed::bit_at(32));
        assert!(
            PANES.iter().all(|&p| !past.has(p)),
            "a 33rd pane reads as one of these: {past:?}"
        );
        // And the panes there are now are each their own.
        for (i, &p) in PANES.iter().enumerate() {
            let one = Changed::of(&[p]);
            assert!(PANES[..i].iter().all(|&q| !one.has(q)), "{p:?}");
        }
    }

    #[test]
    fn run_end_records_outcome_exit_and_run_id() {
        // The risk: the outcome pane shows a run_id, restored flag or error
        // trellis did not send, or loses the one it did.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p1"));
        let c = feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "not_converged", "exit": 2,
                    "run_id": "20261006-abc", "restored": true, "id": "p1"})
            .to_string(),
        );
        assert!(c.has(Pane::Outcome) && c.has(Pane::Running));
        let v = all(&mut s);
        assert_eq!(v["running"], false);
        assert_eq!(
            v["outcome"],
            json!({"outcome": "not converged", "exit": 2, "run_id": "20261006-abc",
                   "restored": true, "error": null, "id": "p1"})
        );

        // No trail written: no run_id is made up.
        feed(&mut s, &run_start("p2"));
        feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "error", "exit": 1, "error": "boom",
                    "id": "p2"})
            .to_string(),
        );
        let v = all(&mut s);
        assert_eq!(v["outcome"]["run_id"], Value::Null);
        assert_eq!(v["outcome"]["restored"], Value::Null);
        assert_eq!(v["outcome"]["error"], "boom");
        assert_eq!(v["outcome"]["exit"], 1);
    }

    #[test]
    fn approval_request_opens_one_question_and_run_end_drops_it() {
        // The risk: a question shown twice, or left open after its run
        // ended, invites an answer nobody is waiting for.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p"));
        let ask = json!({"type": "approval_request", "id": "approval-1",
                         "request_id": "p", "question": "edit a.py (+1 -1)",
                         "detail": "<script>alert(1)</script>"})
        .to_string();
        let c = feed(&mut s, &ask);
        assert!(c.has(Pane::Approvals));
        assert!(feed(&mut s, &ask).is_empty(), "a repeat opens nothing");
        let v = all(&mut s);
        assert_eq!(
            v["approvals"],
            json!([{"id": "approval-1", "question": "edit a.py (+1 -1)",
                    "detail": "<script>alert(1)</script>"}]),
            "one question, its text carried as text"
        );
        // Unanswerable without an id: not opened.
        assert!(
            feed(
                &mut s,
                &json!({"type": "approval_request", "question": "q"}).to_string()
            )
            .is_empty()
        );

        let c = feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "interrupted", "exit": 130,
                    "restored": true, "id": "p"})
            .to_string(),
        );
        assert!(c.has(Pane::Approvals));
        assert_eq!(all(&mut s)["approvals"], json!([]));
    }

    #[test]
    fn get_state_tier_reaches_the_view() {
        // The risk: the run view shows no tier, or one the core guessed
        // from the model spec, when trellis reported its own (#269).
        let mut s = DesktopState::default();
        let reply = |tier: Value| {
            json!({"type": "response", "command": "get_state", "success": true,
                   "id": "desktop-1", "data": {"running": false, "tier": tier}})
            .to_string()
        };
        let c = feed(&mut s, &reply(json!("2, native tool calls")));
        // The tier is also the Now strip's and Where it is's (#828).
        assert_eq!(c, Changed::of(&[Pane::Tier, Pane::Now, Pane::Where]));
        assert_eq!(all(&mut s)["tier"], "2, native tool calls");
        // A run's start does not clear it: only trellis's next answer does.
        feed(&mut s, &run_start("p"));
        assert_eq!(all(&mut s)["tier"], "2, native tool calls");
        // null is reported as null, not as the last tier or a guess.
        assert_eq!(
            feed(&mut s, &reply(Value::Null)),
            Changed::of(&[Pane::Tier, Pane::Now, Pane::Where])
        );
        assert_eq!(all(&mut s)["tier"], Value::Null);
        // Another command's answer, or a refused get_state, is no tier.
        feed(&mut s, &reply(json!("scripted")));
        let other = json!({"type": "response", "command": "get_messages", "success": true,
                           "id": "x", "data": {"tier": "4"}})
        .to_string();
        assert!(feed(&mut s, &other).is_empty());
        let refused = json!({"type": "response", "command": "get_state", "success": false,
                             "id": "x", "error": "busy"})
        .to_string();
        assert!(!feed(&mut s, &refused).has(Pane::Tier));
        assert_eq!(all(&mut s)["tier"], "scripted");
    }

    #[test]
    fn atlas_answers_are_its_own_and_a_new_child_clears_them() {
        // The risk (the owner's request of 2026-10-11): an Atlas read trellis
        // refused (an older trellis with no get_history) raised the page's
        // alert as if a run had failed; or another repository's map stayed
        // drawn, and answerable, after a new child started.
        let mut s = DesktopState::default();
        s.atlas_asked(
            "get_history",
            &json!({"id": "desktop-atlas-history-1", "type": "get_history"}),
        );
        let refusal = json!({"type": "response", "command": "get_history", "success": false,
                             "id": "desktop-atlas-history-1", "error": "unknown command"})
        .to_string();
        let c = feed(&mut s, &refusal);
        assert_eq!(c, Changed::of(&[Pane::Atlas]), "{c:?}");
        assert_eq!(all(&mut s)["atlas"]["history"]["error"], "unknown command");
        assert_eq!(all(&mut s)["alert"], Value::Null);
        // What `start` does for a new child.
        assert!(s.clear().has(Pane::Atlas));
        assert_eq!(all(&mut s)["atlas"], atlas::Atlas::default().to_json());
        assert_eq!(all(&mut s)["atlas"]["history"], Value::Null);
        // The old child's question is not the new one's.
        assert!(!feed(&mut s, &refusal).has(Pane::Atlas));
    }

    fn response(command: &str, success: bool, data: Value) -> String {
        response_to(command, "desktop-x", success, data)
    }

    /// `response` to the line sent with `id`.
    fn response_to(command: &str, id: &str, success: bool, data: Value) -> String {
        let mut r = json!({"type": "response", "command": command, "success": success,
                           "id": id});
        if success {
            r["data"] = data;
        } else {
            r["error"] = data;
        }
        r.to_string()
    }

    #[test]
    fn abort_response_lists_the_undelivered_steers() {
        // The risk (#301): text the user typed into a run they then
        // stopped is lost with the run, because the page drew Stop's answer
        // nowhere; or one run's leftovers are shown under the next.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p"));
        let c = feed(
            &mut s,
            &response(
                "abort",
                true,
                json!({"undelivered": [
                    {"id": "u1", "type": "follow_up", "message": "first"},
                    {"id": "s1", "type": "steer", "message": "<b>second</b>"},
                    {"id": null, "type": "steer"},
                    {"type": "steer", "message": 3}
                ]}),
            ),
        );
        assert!(c.has(Pane::Undelivered), "{c:?}");
        assert_eq!(
            all(&mut s)["undelivered"],
            json!([
                {"type": "follow_up", "message": "first"},
                {"type": "steer", "message": "<b>second</b>"}
            ]),
            "in trellis's order, text as text, and nothing it did not send"
        );
        // The run ends after its abort answer is written; the list stays.
        feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "interrupted", "exit": 130, "id": "p"})
                .to_string(),
        );
        assert_eq!(all(&mut s)["undelivered"].as_array().unwrap().len(), 2);
        // A refused abort, or an answer with no list, keeps it as it was.
        assert!(
            !feed(
                &mut s,
                &response("abort", false, json!("no run in progress"))
            )
            .has(Pane::Undelivered)
        );
        assert!(!feed(&mut s, &response("abort", true, json!({}))).has(Pane::Undelivered));
        assert_eq!(all(&mut s)["undelivered"].as_array().unwrap().len(), 2);
        // The next run starts with none.
        let c = feed(&mut s, &run_start("p2"));
        assert!(c.has(Pane::Undelivered));
        assert_eq!(all(&mut s)["undelivered"], json!([]));
    }

    #[test]
    fn get_commands_response_fills_the_starters() {
        // The risk (#301): a starter sends a template name trellis never
        // listed, or a repository's template text is drawn as markup; or a
        // refused get_commands leaves the last repository's starters up.
        let mut s = DesktopState::default();
        assert_eq!(
            all(&mut s)["starters"],
            Value::Null,
            "none before an answer"
        );
        let c = feed(
            &mut s,
            &response(
                "get_commands",
                true,
                json!({"commands": [{"name": "prompt", "description": "run a task"}],
                       "prompts": [
                           {"name": "fix", "description": "Fix <i>it</i>",
                            "argument_hint": "<file>"},
                           {"name": "review", "description": null},
                           {"description": "no name"},
                           {"name": 7}
                       ],
                       "skills": [{"name": "s", "description": "d"}]}),
            ),
        );
        assert!(c.has(Pane::Starters), "{c:?}");
        assert_eq!(
            all(&mut s)["starters"],
            json!([
                {"name": "fix", "description": "Fix <i>it</i>"},
                {"name": "review", "description": null}
            ]),
            "the prompt templates only, by name, description as text"
        );
        // A run does not touch them: they are the repository's.
        feed(&mut s, &run_start("p"));
        assert_eq!(all(&mut s)["starters"].as_array().unwrap().len(), 2);
        // An answer with no prompts list offers none; a refused one too.
        feed(
            &mut s,
            &response("get_commands", true, json!({"commands": []})),
        );
        assert_eq!(all(&mut s)["starters"], Value::Null);
        feed(
            &mut s,
            &response("get_commands", true, json!({"prompts": []})),
        );
        assert_eq!(all(&mut s)["starters"], json!([]));
        let c = feed(
            &mut s,
            &response("get_commands", false, json!("manifest unreadable")),
        );
        assert!(c.has(Pane::Starters) && c.has(Pane::Transcript), "{c:?}");
        assert_eq!(all(&mut s)["starters"], Value::Null);
    }

    #[test]
    fn failed_responses_and_unparsed_lines_set_the_alert() {
        // The risk (#302): a refused command, a run that ended in error, or
        // a stdout line nothing could parse goes by with no sign on the
        // page, so the user waits on something that already failed.
        let mut s = DesktopState::default();
        assert_eq!(all(&mut s)["alert"], Value::Null, "nothing failed yet");
        let c = feed(
            &mut s,
            &response("set_model", false, json!("<b>bad</b> spec")),
        );
        assert!(c.has(Pane::Alert), "{c:?}");
        assert_eq!(all(&mut s)["alert"], "set_model refused: <b>bad</b> spec");
        // A success says nothing, and leaves the last failure up.
        assert!(
            !feed(&mut s, &response("get_state", true, json!({"tier": null}))).has(Pane::Alert)
        );
        assert_eq!(all(&mut s)["alert"], "set_model refused: <b>bad</b> spec");

        // A run's start is a fresh attempt: its failures are its own.
        let c = feed(&mut s, &run_start("p"));
        assert!(c.has(Pane::Alert));
        assert_eq!(all(&mut s)["alert"], Value::Null);
        let c = feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "error", "exit": 1,
                    "error": "a clean git worktree is required", "id": "p"})
            .to_string(),
        );
        assert!(c.has(Pane::Alert), "{c:?}");
        assert_eq!(
            all(&mut s)["alert"],
            "The run ended in error: a clean git worktree is required"
        );
        // A run that ended without an error raises none.
        feed(&mut s, &run_start("p2"));
        let c = feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "converged", "exit": 0, "id": "p2"}).to_string(),
        );
        assert!(!c.has(Pane::Alert));
        assert_eq!(all(&mut s)["alert"], Value::Null);

        let c = feed(&mut s, "<script>not json</script>");
        assert!(c.has(Pane::Alert) && c.has(Pane::Transcript), "{c:?}");
        assert_eq!(
            all(&mut s)["alert"],
            "trellis rpc printed a line that is not a JSON object: <script>not json</script>"
        );
        // A frame of a type the desktop does not read is not a failure.
        feed(&mut s, &run_start("p3"));
        assert!(feed(&mut s, r#"{"type":"something_new"}"#).is_empty());
        assert_eq!(all(&mut s)["alert"], Value::Null);
    }

    #[test]
    fn a_refused_view_read_stays_in_its_own_view() {
        // The risk (#307, #308 review): a refused read-only view request
        // (a radius for an unknown symbol, a codebase of a folder with no
        // .git, a vault with none configured) said in the run's transcript
        // as if the run had failed, and raised as the page's one alert,
        // when the view that asked shows trellis's reason itself.
        for (command, own) in [
            ("get_codebase", None),
            ("get_radius", None),
            ("vault_search", Some(Pane::Vault)),
            ("vault_graph", Some(Pane::Vault)),
            ("vault_check", Some(Pane::Vault)),
            ("get_queue", Some(Pane::Queue)),
            ("list_models", Some(Pane::Models)),
            ("get_session_stats", Some(Pane::SessionTokens)),
        ] {
            for error in [json!("<b>no</b> such symbol"), Value::Null] {
                let mut s = DesktopState::default();
                let mut r = json!({"type": "response", "command": command,
                                   "success": false, "id": "desktop-x"});
                if !error.is_null() {
                    r["error"] = error.clone();
                }
                let c = feed(&mut s, &r.to_string());
                assert!(!c.has(Pane::Transcript), "{command} {error}: {c:?}");
                assert!(!c.has(Pane::Alert), "{command} {error}: {c:?}");
                if let Some(p) = own {
                    assert!(c.has(p), "{command}: its own view is told: {c:?}");
                }
                let v = all(&mut s);
                assert_eq!(rows(&v["transcript"]), Vec::<Value>::new(), "{command}");
                assert_eq!(v["alert"], Value::Null, "{command}");
            }
        }
        // A refused command that is not a view's read is still said.
        let mut s = DesktopState::default();
        let c = feed(&mut s, &response("set_model", false, json!("bad spec")));
        assert!(c.has(Pane::Transcript) && c.has(Pane::Alert), "{c:?}");
    }

    #[test]
    fn stderr_lines_are_kept_apart_and_capped() {
        // The risk (#302, ADR 0048 section 2): trellis's human text mixed
        // into the run's transcript, where it reads as the run's own
        // output, or kept without bound.
        let mut s = DesktopState::default();
        let c = s.update(Frame::Human("warning: <i>human</i>".into()));
        assert_eq!(c, Changed::of(&[Pane::Stderr]));
        let v = all(&mut s);
        assert_eq!(texts(&v["stderr"]), ["warning: <i>human</i>"]);
        assert_eq!(
            rows(&v["transcript"]),
            Vec::<Value>::new(),
            "not in the transcript"
        );
        assert_eq!(v["alert"], Value::Null, "human text is not a failure");
        for i in 0..STDERR_CAP + 5 {
            s.update(Frame::Human(format!("line {i}")));
        }
        let lines = texts(&all(&mut s)["stderr"]);
        assert_eq!(lines.len(), STDERR_CAP);
        assert_eq!(STDERR_CAP, 500);
        assert_eq!(lines[0], "line 5", "the oldest go first");
        // A run's start leaves it: it is the process's text, not the run's.
        feed(&mut s, &run_start("p"));
        assert_eq!(rows(&all(&mut s)["stderr"]).len(), STDERR_CAP);
    }

    #[test]
    fn model_gate_composer_enables_only_after_a_model_is_set() {
        // The risk (#564): prompt input opens before the user has chosen a
        // model, on a list answer, a refused choice or the last child's
        // choice, so a prompt runs on whatever the repository's config
        // names with the user's keys in the child's environment. The page
        // enables the composer on the view's `model` alone, so it must stay
        // null until this child has taken a `set_model`.
        let mut s = DesktopState::default();
        assert_eq!(all(&mut s)["model"], Value::Null, "shut at launch");
        assert!(!s.model_chosen());
        // What there is to choose from opens nothing.
        let c = feed(
            &mut s,
            &response(
                "list_models",
                true,
                json!({"configured": "vllm:http://127.0.0.1:8000#<b>q</b>",
                       "local": ["vllm:http://127.0.0.1:11434#qwen3:8b", 7],
                       "probed": [{"runtime": "Ollama",
                                   "address": "http://127.0.0.1:11434/api/tags",
                                   "error": null},
                                  {"runtime": "vLLM"}],
                       "providers": [{"name": "mine", "key_variable": "TRELLIS_TEST_VAR"},
                                     {"key_variable": "X"}]}),
            ),
        );
        assert_eq!(c, Changed::of(&[Pane::Models]));
        assert_eq!(
            all(&mut s)["models"],
            json!({"configured": "vllm:http://127.0.0.1:8000#<b>q</b>",
                   "local": ["vllm:http://127.0.0.1:11434#qwen3:8b"],
                   "probed": [{"runtime": "Ollama",
                               "address": "http://127.0.0.1:11434/api/tags",
                               "error": null}],
                   "providers": [{"name": "mine", "key_variable": "TRELLIS_TEST_VAR"}]}),
            "text as text, and nothing it did not send whole"
        );
        assert_eq!(all(&mut s)["model"], Value::Null, "a list is not a choice");
        // A refused choice, or an answer naming no spec, opens nothing.
        feed(
            &mut s,
            &response("set_model", false, json!("choose a model first")),
        );
        feed(
            &mut s,
            &response("set_model", true, json!({"configured": true})),
        );
        assert_eq!(all(&mut s)["model"], Value::Null);
        assert!(!s.model_chosen());
        // The child's own yes opens it.
        let c = feed(
            &mut s,
            &response(
                "set_model",
                true,
                json!({"model": "vllm:http://127.0.0.1:11434#qwen3:8b", "configured": false,
                       "saved_to": "/home/u/.config/trellis/config.toml"}),
            ),
        );
        assert!(c.has(Pane::Model), "{c:?}");
        assert!(s.model_chosen());
        assert_eq!(
            all(&mut s)["model"],
            json!({"spec": "vllm:http://127.0.0.1:11434#qwen3:8b", "configured": false,
                   "saved_to": "/home/u/.config/trellis/config.toml", "source": "session"})
        );
        // A run, a clear and a refused change leave it: it is the session's.
        feed(&mut s, &run_start("p"));
        s.clear();
        feed(&mut s, &response("set_model", false, json!("not a spec")));
        assert!(s.model_chosen());
        // The child's exit shuts it, and so does a new child's start.
        let c = s.child_exited();
        assert!(c.has(Pane::Model) && c.has(Pane::Models), "{c:?}");
        assert_eq!(all(&mut s)["model"], Value::Null);
        assert_eq!(all(&mut s)["models"], Value::Null);
        feed(
            &mut s,
            &response(
                "set_model",
                true,
                json!({"model": "scripted:x", "configured": true, "saved_to": null}),
            ),
        );
        assert_eq!(
            all(&mut s)["model"],
            json!({"spec": "scripted:x", "configured": true, "saved_to": null,
                   "source": null}),
            "no configured answer to name its source by"
        );
        s.forget_models();
        assert!(!s.model_chosen());
        // A refused list says why, in place of the list.
        feed(&mut s, &response("list_models", false, json!("bad config")));
        assert_eq!(all(&mut s)["models"], json!({"error": "bad config"}));
    }

    #[test]
    fn model_source_is_the_child_s_own_word_never_a_guess() {
        // The risk (#306): the status bar names a model's source the child
        // never reported: a confirmed spec called the user's when it is the
        // repository's, whose endpoint then reads as one the user chose.
        let models = |repository: Value, user: Value| {
            json!({"type": "response", "command": "get_available_models", "id": MODELS_ID,
                   "success": true,
                   "data": {"catalog": [],
                            "configured": {"repository": repository, "user": user,
                                           "session": null}}})
            .to_string()
        };
        let chose = |spec: &str, configured: bool| {
            response(
                "set_model",
                true,
                json!({"model": spec, "configured": configured, "saved_to": null}),
            )
        };
        let source = |s: &mut DesktopState| all(s)["model"]["source"].clone();
        for (repository, user, spec, want) in [
            (json!("r:x"), json!("u:y"), "r:x", json!("repository")),
            (Value::Null, json!("u:y"), "u:y", json!("user")),
            // Both files name it: the repository's wins the layering.
            (
                json!("same:z"),
                json!("same:z"),
                "same:z",
                json!("repository"),
            ),
            // Reformatted, or from a config edited since: not known.
            (json!("r:x"), json!("u:y"), "r:x ", Value::Null),
        ] {
            let mut s = DesktopState::default();
            feed(&mut s, &models(repository, user));
            feed(&mut s, &chose(spec, true));
            assert_eq!(source(&mut s), want, "{spec:?}");
        }
        // A spec set_model named is the session's, even the repository's,
        // which rpc then counts as the user's choice; the user's own spec
        // named is the user's.
        let mut s = DesktopState::default();
        feed(&mut s, &models(json!("r:x"), json!("u:y")));
        feed(&mut s, &chose("r:x", false));
        assert_eq!(source(&mut s), json!("session"));
        feed(&mut s, &chose("u:y", false));
        assert_eq!(source(&mut s), json!("user"));
        feed(&mut s, &chose("u:y ", false));
        assert_eq!(source(&mut s), json!("session"), "byte for byte");
    }

    #[test]
    fn picker_available_comes_only_from_the_core_s_own_models_answer() {
        // The risk (#306): the picker offers a spec the child never
        // reported as the user's, or a catalogue figure trellis did not
        // send, or keeps the last child's list for a new repository.
        let mut s = DesktopState::default();
        assert_eq!(all(&mut s)["available"], Value::Null);
        let answer = |id: &str| {
            json!({"type": "response", "command": "get_available_models", "id": id,
                   "success": true,
                   "data": {"catalog": [
                                {"name": "bonsai-2-27b", "license": "Apache-2.0",
                                 "download_gib": 5.3, "default": true},
                                {"name": "<b>x</b>", "license": null},
                                {"license": "MIT"}],
                            "configured": {"repository": "r:x", "user": "u:y",
                                           "session": null}}})
            .to_string()
        };
        // The page's own ask is not the core's: it changes nothing.
        assert!(!feed(&mut s, &answer("page")).has(Pane::Available));
        assert_eq!(all(&mut s)["available"], Value::Null);
        let c = feed(&mut s, &answer(MODELS_ID));
        assert!(c.has(Pane::Available), "{c:?}");
        assert_eq!(
            all(&mut s)["available"],
            json!({"configured": {"repository": "r:x", "user": "u:y", "session": null},
                   "catalog": [
                       {"name": "bonsai-2-27b", "license": "Apache-2.0", "download_gib": 5.3,
                        "default": true},
                       {"name": "<b>x</b>", "license": null, "download_gib": null,
                        "default": false}]})
        );
        // The child's exit, or a new child, empties it.
        s.child_exited();
        assert_eq!(all(&mut s)["available"], Value::Null);
        feed(&mut s, &answer(MODELS_ID));
        s.forget_models();
        assert_eq!(all(&mut s)["available"], Value::Null);
    }

    /// A `get_session_stats` answer as `rpc.rs` writes it, in the shape
    /// `rpc_get_session_stats_counts_each_model_s_tokens_from_the_trails_apart`
    /// asserts.
    fn session_stats(models: Value) -> String {
        response(
            "get_session_stats",
            true,
            json!({"models": models, "runs_not_counted": 0}),
        )
    }

    #[test]
    fn session_tokens_one_row_per_model_with_two_figures() {
        // The risk (#313): two models' figures merged into one row, a
        // model's prompt and completion joined into one figure, or a row
        // the answer did not send.
        let mut s = DesktopState::default();
        assert_eq!(all(&mut s)["session_tokens"], Value::Null, "none asked yet");
        let c = feed(
            &mut s,
            &session_stats(json!([
                {"model": "vllm:http://127.0.0.1:8000#qwen", "prompt_tokens": 2750,
                 "completion_tokens": 77},
                {"model": "<b>scripted</b>", "prompt_tokens": 10, "completion_tokens": 2},
                {"prompt_tokens": 1, "completion_tokens": 1},
            ])),
        );
        assert_eq!(c, Changed::of(&[Pane::SessionTokens]));
        assert_eq!(
            all(&mut s)["session_tokens"],
            json!({"models": [
                       {"model": "vllm:http://127.0.0.1:8000#qwen", "prompt_tokens": 2750,
                        "completion_tokens": 77},
                       {"model": "<b>scripted</b>", "prompt_tokens": 10,
                        "completion_tokens": 2}],
                   "runs_not_counted": 0, "error": null}),
            "one row per named model, two figures each, the name as text"
        );
    }

    #[test]
    fn session_tokens_unknown_is_null_never_zero() {
        // The risk (#313): colibri counts a missing usage figure as 0, which
        // here would read as an exact total trellis did not record.
        let mut s = DesktopState::default();
        feed(
            &mut s,
            &session_stats(json!([
                {"model": "scripted:x", "prompt_tokens": null, "completion_tokens": null},
                {"model": "m", "prompt_tokens": "12", "completion_tokens": -3},
                {"model": "n"},
            ])),
        );
        let rows = all(&mut s)["session_tokens"]["models"].clone();
        for r in rows.as_array().unwrap() {
            assert_eq!(r["prompt_tokens"], Value::Null, "{rows}");
            assert_eq!(r["completion_tokens"], Value::Null, "{rows}");
        }
        assert_eq!(rows.as_array().unwrap().len(), 3);
    }

    #[test]
    fn session_tokens_have_no_sum() {
        // The risk (#313, PI-10): the desktop adds prompt and completion, or
        // the models, into a figure no trail records.
        let mut s = DesktopState::default();
        feed(
            &mut s,
            &session_stats(json!([
                {"model": "a", "prompt_tokens": 5, "completion_tokens": 6},
                {"model": "b", "prompt_tokens": 7, "completion_tokens": 8},
            ])),
        );
        let v = s.view(Changed::of(&[Pane::SessionTokens]));
        let mut ks = Vec::new();
        keys(&v, &mut ks);
        let bad: Vec<&String> = ks
            .iter()
            .filter(|k| {
                let k = k.to_lowercase();
                k.contains("total") || k.contains("sum")
            })
            .collect();
        assert!(bad.is_empty(), "the view sums: {bad:?} in {v}");
        let numbers: Vec<u64> = v["session_tokens"]["models"]
            .as_array()
            .unwrap()
            .iter()
            .flat_map(|m| [&m["prompt_tokens"], &m["completion_tokens"]])
            .filter_map(Value::as_u64)
            .collect();
        assert_eq!(numbers, [5, 6, 7, 8], "only the figures sent");
    }

    #[test]
    fn session_tokens_change_only_on_a_get_session_stats_answer() {
        // The risk (#313): a run's start clears the session's totals, or
        // turns and run ends are added into them, so the desktop holds a
        // count of its own beside trellis's.
        let mut s = DesktopState::default();
        feed(
            &mut s,
            &session_stats(json!([{"model": "m", "prompt_tokens": 100,
                                    "completion_tokens": 3}])),
        );
        let before = all(&mut s)["session_tokens"].clone();
        for line in [
            run_start("p"),
            turn(
                json!({"turn": 1, "call": "read_file a.py", "verdict": "None",
                        "check_kind": "", "observation": "", "prompt_bytes": 900,
                        "completion_tokens": 40, "model_ms": 1000}),
            ),
            json!({"type": "run_end", "outcome": "converged", "exit": 0, "id": "p"}).to_string(),
            response("get_state", true, json!({"tier": null})),
        ] {
            assert!(!feed(&mut s, &line).has(Pane::SessionTokens), "{line}");
        }
        assert_eq!(all(&mut s)["session_tokens"], before);
        feed(
            &mut s,
            &session_stats(json!([{"model": "m", "prompt_tokens": 200,
                                    "completion_tokens": 6}])),
        );
        assert_eq!(
            all(&mut s)["session_tokens"]["models"][0]["prompt_tokens"],
            200
        );
    }

    #[test]
    fn session_tokens_drop_when_the_child_exits() {
        // The risk (#313): a child that exited leaves its totals up, and
        // the next child's runs read as continuing them.
        let mut s = DesktopState::default();
        feed(
            &mut s,
            &session_stats(json!([{"model": "m", "prompt_tokens": 1,
                                    "completion_tokens": 1}])),
        );
        let c = s.child_exited();
        assert!(c.has(Pane::SessionTokens), "{c:?}");
        assert_eq!(all(&mut s)["session_tokens"], Value::Null, "no rows");
        // A new child's view starts with none too.
        feed(
            &mut s,
            &session_stats(json!([{"model": "m", "prompt_tokens": 1,
                                    "completion_tokens": 1}])),
        );
        s.clear();
        assert_eq!(all(&mut s)["session_tokens"], Value::Null);
    }

    #[test]
    fn session_tokens_refused_answer_shows_the_error_text() {
        // The risk (#313): a refused get_session_stats leaves the last rows
        // up as if current, or shows no reason.
        let mut s = DesktopState::default();
        feed(
            &mut s,
            &session_stats(json!([{"model": "m", "prompt_tokens": 1,
                                    "completion_tokens": 1}])),
        );
        let c = feed(
            &mut s,
            &response("get_session_stats", false, json!("<i>trail</i> unreadable")),
        );
        // Its pane says why; the run's alert does not (#308 review).
        assert!(c.has(Pane::SessionTokens) && !c.has(Pane::Alert), "{c:?}");
        assert_eq!(
            all(&mut s)["session_tokens"],
            json!({"models": [], "runs_not_counted": null,
                   "error": "<i>trail</i> unreadable"})
        );
        // An answer this desktop cannot read says so, with no rows.
        feed(
            &mut s,
            &response("get_session_stats", true, json!({"x": 1})),
        );
        let v = all(&mut s)["session_tokens"].clone();
        assert_eq!(v["models"], json!([]), "{v}");
        assert!(
            v["error"].as_str().unwrap().contains("does not read"),
            "{v}"
        );
    }

    #[test]
    fn approve_answer_closes_its_question() {
        // The risk (#302): the dialog shows a question trellis has already
        // had its answer to, so the user is asked twice.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p"));
        for id in ["approval-1", "approval-2"] {
            feed(
                &mut s,
                &json!({"type": "approval_request", "id": id, "question": "q", "detail": "d"})
                    .to_string(),
            );
        }
        assert!(s.approval_open("approval-1") && s.approval_open("approval-2"));
        let answer = json!({"type": "response", "command": "approve", "success": true,
                            "id": "approval-1", "data": {"approved": false}})
        .to_string();
        assert!(feed(&mut s, &answer).has(Pane::Approvals));
        assert!(!s.approval_open("approval-1"));
        assert_eq!(all(&mut s)["approvals"].as_array().unwrap().len(), 1);
        // Refused (trellis no longer has it): closed too.
        let gone = json!({"type": "response", "command": "approve", "success": false,
                          "id": "approval-2", "error": "no open approval request"})
        .to_string();
        assert!(feed(&mut s, &gone).has(Pane::Approvals));
        assert_eq!(all(&mut s)["approvals"], json!([]));
    }

    #[test]
    fn unknown_lines_change_nothing_and_unparsed_lines_reach_transcript_and_alert() {
        // The risk: an unknown frame type, a non-object line or a field of
        // the wrong type panics the core or reaches a pane as data; or a
        // line that is not an object goes by unsaid (#302).
        let mut s = DesktopState::default();
        for line in [
            r#"{"type":"something_new","x":1}"#,
            r#"{"type":7}"#,
            r#"{"fields":{"message":"index rebuilt","files":3}}"#,
            r#"{"fields":{"message":"turn","call":"edit a.py"}}"#,
            r#"{"fields":{"message":"turn","turn":"one","call":"edit a.py"}}"#,
            r#"{"fields":"turn"}"#,
            r#"{}"#,
        ] {
            assert!(feed(&mut s, line).is_empty(), "{line}");
        }
        // A line that is not an object is text, and a failure (#302).
        let text = Changed::of(&[Pane::Transcript, Pane::Alert]);
        let c = feed(&mut s, "<b>not json</b>\r\n");
        assert_eq!(c, text);
        let t = rows(&all(&mut s)["transcript"]);
        assert_eq!(
            (&t[0]["kind"], &t[0]["text"]),
            (&json!("note"), &json!("<b>not json</b>"))
        );
        assert_eq!(feed(&mut s, "[1,2]"), text);
    }

    #[test]
    fn transcript_is_capped_at_the_tuis_bound() {
        // The risk: a long run grows the transcript without bound.
        let mut s = DesktopState::default();
        for i in 0..TRANSCRIPT_CAP + 5 {
            feed(&mut s, &format!("line {i}"));
        }
        let t = texts(&all(&mut s)["transcript"]);
        assert_eq!(t.len(), TRANSCRIPT_CAP);
        assert_eq!(t[0], "line 5", "the oldest go first");
    }

    #[test]
    fn transcript_view_carries_only_new_entries_past_the_cap() {
        // The risk (#827): every view carried the whole transcript, so once
        // the cap was full each new line redrew two thousand rows on the
        // page, and every view was a megabyte. Past the cap, a view that is
        // not a full resend carries only the rows it adds, numbered on.
        let mut s = DesktopState::default();
        for i in 0..TRANSCRIPT_CAP + 5 {
            feed(&mut s, &format!("line {i}"));
        }
        let full = all(&mut s);
        assert_eq!(full["transcript"]["whole"], true);
        assert_eq!(rows(&full["transcript"]).len(), TRANSCRIPT_CAP);
        assert_eq!(full["transcript"]["keep"], TRANSCRIPT_CAP);
        let c = feed(
            &mut s,
            &turn(
                json!({"turn": 7, "call": "run_checks a.py", "verdict": "Some(true)",
                         "check_kind": "Tests", "observation": "ok"}),
            ),
        );
        let v = s.view(c);
        let new = rows(&v["transcript"]);
        assert_eq!(v["transcript"]["whole"], false);
        assert_eq!(
            new.len(),
            2,
            "the tool row and its check row, nothing older: {v}"
        );
        let first = TRANSCRIPT_CAP as u64 + 5;
        assert_eq!(
            (&new[0]["seq"], &new[1]["seq"]),
            (&json!(first), &json!(first + 1))
        );
        assert_eq!(
            (&new[0]["kind"], &new[1]["kind"]),
            (&json!("tool"), &json!("check"))
        );
        // A view with nothing new carries no row.
        let c = s.update(Frame::Human("aside".into()));
        assert!(!c.has(Pane::Transcript));
    }

    #[test]
    fn a_steer_row_carries_its_text_and_delivery_turn() {
        // The risk (#827): a steer the person sent shown nowhere, or as
        // "steer delivered" with neither its words nor which turn it
        // followed; or one the run ended before delivering left pending, or
        // said as a failure in the alert.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p"));
        let c = s.steer_sent("composer-steer-1", "<b>keep</b> tokens.py");
        // The row, and the summary level's steers, which list it (#832).
        assert_eq!(c, Changed::of(&[Pane::Transcript, Pane::Summary]));
        let v = s.view(c);
        let row = rows(&v["transcript"]).pop().unwrap();
        assert_eq!(
            (&row["kind"], &row["text"], &row["delivery"]),
            (
                &json!("steer"),
                &json!("<b>keep</b> tokens.py"),
                &json!({"state": "pending"})
            )
        );
        let seq = row["seq"].clone();
        // rpc's answer to its id: the step whose request carried it. The
        // row is sent again, changed, under its own number.
        let c = feed(
            &mut s,
            &response_to("steer", "composer-steer-1", true, json!({"after_step": 3})),
        );
        let v = s.view(c);
        let row = rows(&v["transcript"]).pop().unwrap();
        assert_eq!(row["seq"], seq, "the same row, changed");
        assert_eq!(
            row["delivery"],
            json!({"state": "delivered", "after_step": 3})
        );
        // A steer the run ended before delivering: its row says so, with
        // trellis's reason, and nothing else does.
        s.steer_sent("composer-steer-2", "also run the benchmark");
        let c = feed(
            &mut s,
            &response_to("steer", "composer-steer-2", false, json!("the run ended")),
        );
        assert!(!c.has(Pane::Alert), "{c:?}");
        let v = all(&mut s);
        let t = rows(&v["transcript"]);
        assert_eq!(
            t.iter().filter(|r| r["kind"] == "refusal").count(),
            0,
            "{v}"
        );
        let last = t.last().unwrap();
        assert_eq!(
            last["delivery"],
            json!({"state": "not_delivered", "why": "the run ended"})
        );
        // The loop's own event marks the oldest steer still pending, and a
        // settled one keeps where it stood.
        s.steer_sent("composer-steer-3", "third");
        s.steer_sent("composer-steer-4", "fourth");
        feed(&mut s, &event("steer delivered", json!({"after_step": 5})));
        let t = rows(&all(&mut s)["transcript"]);
        let by = |text: &str| t.iter().find(|r| r["text"] == text).unwrap()["delivery"].clone();
        assert_eq!(by("third"), json!({"state": "delivered", "after_step": 5}));
        assert_eq!(by("fourth"), json!({"state": "pending"}));
        assert_eq!(
            by("<b>keep</b> tokens.py"),
            json!({"state": "delivered", "after_step": 3})
        );
        // A steer the child never received says why, and is never answered.
        s.steer_sent("composer-steer-5", "fifth");
        s.steer_unsent(
            "composer-steer-5",
            "rpc_send: cannot write to trellis rpc: broken pipe",
        );
        let t = rows(&all(&mut s)["transcript"]);
        assert_eq!(t.last().unwrap()["delivery"]["state"], "not_delivered");
    }

    /// A `get_state` answer whose `pending` is `pending`.
    fn state_with(pending: Value) -> String {
        response_to(
            "get_state",
            "desktop-pending",
            true,
            json!({"running": true, "tier": null, "model": "scripted:x", "root": "/r",
                   "pending": pending}),
        )
    }

    /// The queue as the page receives it: each item's id, state and turn.
    fn queue_of(s: &mut DesktopState) -> Vec<(String, String, Value)> {
        all(s)["pending"]
            .as_array()
            .unwrap()
            .iter()
            .map(|q| {
                (
                    q["id"].as_str().unwrap().to_string(),
                    q["state"].as_str().unwrap().to_string(),
                    q["after_turn"].clone(),
                )
            })
            .collect()
    }

    #[test]
    fn queued_items_show_their_state_from_pending() {
        // The risk (#830): the queue drawn from what the page sent rather
        // than what trellis holds, so an item trellis refused reads as
        // waiting; or an item marked delivered because a later `pending`
        // left it out, which a steer does before its `steer delivered` is
        // written (#808), and a withdrawn one does without being delivered
        // at all; or a late `get_state` answer putting back as queued a
        // steer whose answer was already read. Items come from `pending`;
        // only an event settles one.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p"));
        s.steer_sent("s1", "keep tokens.py");
        s.steer_sent("s2", "<b>and</b> the lexer");
        let c = feed(
            &mut s,
            &state_with(json!([
                {"id": "f1", "type": "follow_up", "message": "then rename it"},
                {"id": "s1", "type": "steer", "message": "keep tokens.py"},
                {"id": "s2", "type": "steer", "message": "<b>and</b> the lexer"},
                {"id": null, "type": "steer", "message": "sent by no id"},
                {"id": "x", "type": "abort", "message": "not a queued type"},
            ])),
        );
        assert!(c.has(Pane::Pending), "{c:?}");
        let q = all(&mut s)["pending"].clone();
        assert_eq!(
            q[2],
            json!({"id": "s2", "kind": "steer", "text": "<b>and</b> the lexer",
                   "state": "queued", "after_turn": null, "why": null,
                   "take_back": "offer"}),
            "the text is kept as sent, for the page to write as text"
        );
        assert_eq!(q[0]["kind"], "follow_up", "{q}");
        assert_eq!(q.as_array().unwrap().len(), 3, "{q}");

        // Gone from a later `pending`, s1 is not delivered on that alone.
        feed(
            &mut s,
            &state_with(json!([{"id": "s2", "type": "steer", "message": "<b>and</b> the lexer"}])),
        );
        assert_eq!(
            queue_of(&mut s)[1],
            ("s1".into(), "queued".into(), Value::Null)
        );
        // The loop's event settles the oldest pending steer, s1.
        let c = feed(&mut s, &event("steer delivered", json!({"after_step": 4})));
        assert!(c.has(Pane::Pending) && c.has(Pane::Transcript), "{c:?}");
        assert_eq!(
            queue_of(&mut s)[1],
            ("s1".into(), "delivered".into(), json!(4))
        );

        // A steer answered before any `pending` listed it takes its row's
        // state when one does, not `queued`.
        s.steer_sent("s3", "late");
        feed(
            &mut s,
            &response_to("steer", "s3", false, json!("the run ended")),
        );
        feed(
            &mut s,
            &state_with(json!([{"id": "s3", "type": "steer", "message": "late"}])),
        );
        let late = all(&mut s)["pending"][3].clone();
        assert_eq!(
            (&late["state"], &late["why"]),
            (&json!("not_delivered"), &json!("the run ended")),
            "{late}"
        );

        // An abort's answer names the follow-up as never delivered, and the
        // steer still waiting with it.
        let c = feed(
            &mut s,
            &response_to(
                "abort",
                "ab",
                true,
                json!({"undelivered": [
                    {"id": "f1", "type": "follow_up", "message": "then rename it"},
                    {"id": "s2", "type": "steer", "message": "<b>and</b> the lexer"},
                ]}),
            ),
        );
        assert!(c.has(Pane::Pending), "{c:?}");
        let states: Vec<String> = queue_of(&mut s).into_iter().map(|q| q.1).collect();
        assert_eq!(
            states,
            [
                "not_delivered",
                "delivered",
                "not_delivered",
                "not_delivered"
            ]
        );

        // A follow-up is delivered when its own run starts, carrying its id;
        // that run drops what the last one settled.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p"));
        feed(
            &mut s,
            &state_with(json!([{"id": "f2", "type": "follow_up", "message": "next"}])),
        );
        feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "converged", "exit": 0, "id": "p"}).to_string(),
        );
        assert_eq!(queue_of(&mut s)[0].1, "queued", "run_end delivers nothing");
        let c = feed(&mut s, &run_start("f2"));
        assert!(c.has(Pane::Pending), "{c:?}");
        assert_eq!(
            queue_of(&mut s),
            [("f2".into(), "delivered".into(), Value::Null)]
        );
        feed(&mut s, &run_start("p3"));
        assert!(
            queue_of(&mut s).is_empty(),
            "a settled item outlived the next run"
        );

        // Queued when the child exits: never delivered, and said so.
        feed(
            &mut s,
            &state_with(json!([{"id": "s9", "type": "steer", "message": "m"}])),
        );
        let c = s.child_exited();
        assert!(c.has(Pane::Pending), "{c:?}");
        assert_eq!(queue_of(&mut s)[0].1, "not_delivered");

        // A `pending` of any length keeps the queue to its cap.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p"));
        let many: Vec<Value> = (0..QUEUED_CAP + 5)
            .map(|i| json!({"id": format!("s{i}"), "type": "steer", "message": "m"}))
            .collect();
        feed(&mut s, &state_with(json!(many)));
        assert_eq!(queue_of(&mut s).len(), QUEUED_CAP);
    }

    #[test]
    fn stop_and_send_prompt_waits_for_run_end_and_the_abort_s_answer() {
        // The risk (#830, DESIGN.md §2.1): the held prompt sent at run_end
        // when the abort is then refused (the run had ended on its own, so
        // the person's stop did nothing they were told), sent before the
        // stopped run has ended, which rpc refuses ("one at a time"), sent
        // twice, or sent into a session whose child has exited. It is ready
        // once both the run's end and the abort's success are read, in
        // either order, and only once.
        let prompt = json!({"id": "p2", "type": "prompt", "task": "run this instead"});
        let ended =
            json!({"type": "run_end", "outcome": "interrupted", "exit": 1, "id": "p"}).to_string();
        let stopped = response_to("abort", "ab", true, json!({"undelivered": []}));
        for (first, second) in [(&ended, &stopped), (&stopped, &ended)] {
            let mut s = DesktopState::default();
            feed(&mut s, &run_start("p"));
            let c = s.stop_and_send("ab", prompt.clone());
            assert!(c.has(Pane::Pending), "{c:?}");
            assert_eq!(
                queue_of(&mut s),
                [("p2".into(), "queued".into(), Value::Null)]
            );
            assert!(s.holding());
            feed(&mut s, first);
            assert_eq!(s.held_ready(), None, "sent on one of the two");
            feed(&mut s, second);
            assert_eq!(s.held_ready(), Some(prompt.clone()));
            assert_eq!(s.held_ready(), None, "sent twice");
            assert!(!s.holding());
            assert!(s.held_sent("p2", Ok(())).has(Pane::Pending));
            assert_eq!(queue_of(&mut s)[0].1, "delivered");
        }
        // Sent, and refused by trellis: no run comes, so it was never
        // delivered, whatever the core's forwarding said.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p"));
        s.stop_and_send("ab", prompt.clone());
        feed(&mut s, &ended);
        feed(&mut s, &stopped);
        s.held_ready().unwrap();
        s.held_sent("p2", Ok(()));
        let c = feed(
            &mut s,
            &response_to("prompt", "p2", false, json!("one at a time")),
        );
        assert!(c.has(Pane::Pending), "{c:?}");
        let q = all(&mut s)["pending"][0].clone();
        assert_eq!(
            (&q["state"], &q["why"]),
            (&json!("not_delivered"), &json!("one at a time"))
        );
        // Another abort's answer is not this one's.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p"));
        s.stop_and_send("ab", prompt.clone());
        feed(&mut s, &ended);
        feed(
            &mut s,
            &response_to("abort", "other", true, json!({"undelivered": []})),
        );
        assert_eq!(s.held_ready(), None);
        // A refused abort: never sent, and the queue says why.
        let c = feed(
            &mut s,
            &response_to("abort", "ab", false, json!("no run in progress")),
        );
        assert!(c.has(Pane::Pending), "{c:?}");
        assert_eq!(s.held_ready(), None);
        assert!(!s.holding());
        let q = all(&mut s)["pending"][0].clone();
        assert_eq!(q["state"], "not_delivered", "{q}");
        assert_eq!(q["why"], "the abort was refused: no run in progress", "{q}");
        // The child exits before the run ends: never sent.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p"));
        s.stop_and_send("ab", prompt.clone());
        s.child_exited();
        feed(&mut s, &ended);
        feed(&mut s, &stopped);
        assert_eq!(s.held_ready(), None);
        assert_eq!(queue_of(&mut s)[0].1, "not_delivered");
        // Sent, then refused by the core's own rules: not delivered, why.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p"));
        s.stop_and_send("ab", prompt.clone());
        feed(&mut s, &ended);
        feed(&mut s, &stopped);
        assert!(s.held_ready().is_some());
        s.held_sent("p2", Err("rpc_send: no trellis rpc child is running"));
        assert_eq!(
            all(&mut s)["pending"][0]["why"],
            "rpc_send: no trellis rpc child is running"
        );
    }

    #[test]
    fn a_check_row_comes_only_from_a_verdict() {
        // The risk (#827; ADR 0004): the typed transcript drawing a check
        // row for a turn trellis-checks gave no verdict on, so the model's
        // own claim, or a tool that merely ran, reads as a check result.
        for (verdict, want) in [
            (json!("Some(true)"), Some(true)),
            (json!("Some(false)"), Some(false)),
            (json!("None"), None),
            (json!("Some(maybe)"), None),
            (json!(true), None),
            (Value::Null, None),
        ] {
            let mut s = DesktopState::default();
            let mut fields = json!({"turn": 2, "call": "run_checks a.py", "check_kind": "Tests"});
            if !verdict.is_null() {
                fields["verdict"] = verdict.clone();
            }
            feed(&mut s, &turn(fields));
            let checks: Vec<Value> = rows(&all(&mut s)["transcript"])
                .into_iter()
                .filter(|r| r["kind"] == "check")
                .collect();
            match want {
                Some(passed) => {
                    assert_eq!(checks.len(), 1, "{verdict}");
                    assert_eq!(checks[0]["passed"], passed, "{verdict}");
                }
                None => assert!(checks.is_empty(), "a check row from {verdict}"),
            }
        }
    }

    #[test]
    fn a_stored_line_past_the_cap_is_cut_and_said_so() {
        // The risk (#814): a stdout line that is not JSON, or a stderr line,
        // of any length was stored whole in the transcript, the trellis
        // messages pane and the alert, and sent whole with every view that
        // changed them; or a cut one, with nothing saying so, read as the
        // whole line. Each copy is held to its own store's byte cap, keeps
        // the line's start on a character boundary, and ends saying how
        // many of its bytes were cut.
        let line = "é".repeat(TRANSCRIPT_BYTES);
        let alert = format!("trellis rpc printed a line that is not a JSON object: {line}");
        let mut s = DesktopState::default();
        feed(&mut s, &line);
        s.update(Frame::Human(line.clone()));
        let v = all(&mut s);
        for (pane, kept, whole, cap) in [
            (
                "transcript",
                &v["transcript"]["rows"][0]["text"],
                &line,
                TRANSCRIPT_BYTES,
            ),
            (
                "stderr",
                &v["stderr"]["rows"][0]["text"],
                &line,
                STDERR_BYTES,
            ),
            ("alert", &v["alert"], &alert, ALERT_BYTES),
        ] {
            let kept = kept.as_str().unwrap_or_else(|| panic!("{pane}: {kept}"));
            assert!(kept.len() <= cap, "{pane}: {} bytes kept", kept.len());
            assert!(kept.len() >= cap - 3, "{pane}: cut short of its cap");
            let at = kept
                .rfind(" [trellis-desktop:")
                .unwrap_or_else(|| panic!("{pane}: the cut is not said"));
            assert!(
                whole.starts_with(&kept[..at]),
                "{pane}: the start, as it came"
            );
            assert_eq!(
                &kept[at..],
                format!(
                    " [trellis-desktop: the last {} of {} bytes were cut]",
                    whole.len() - at,
                    whole.len()
                ),
                "{pane}"
            );
        }
        // The cut line fills the transcript's bytes, so the next line,
        // however short, pushes it out; a line within the caps is whole.
        feed(&mut s, "<b>short</b>");
        let v = all(&mut s);
        assert_eq!(texts(&v["transcript"]), ["<b>short</b>"]);
        assert_eq!(
            v["alert"],
            "trellis rpc printed a line that is not a JSON object: <b>short</b>"
        );
    }

    /// An approve-mode question for an edit to `path`, in the approve
    /// gate's shape: `edit <path> (+a -r)` with the diff as its detail.
    fn edit_question(id: &str, path: &str, diff: &str) -> String {
        json!({"type": "approval_request", "id": id, "request_id": "p",
               "question": format!("edit {path} (+1 -1)"),
               "detail": format!("--- {path}\n+++ {path}\n{diff}")})
        .to_string()
    }

    /// The turn of an applied edit to `path`, with #802's fields.
    fn applied_edit(n: u64, path: &str, added: u64, removed: u64, diff: &str) -> String {
        turn(
            json!({"turn": n, "call": format!("edit {path}"), "verdict": "None",
                    "check_kind": "", "observation": "applied: true",
                    "tool": "edit", "target": path, "wrote": true,
                    "lines_added": added, "lines_removed": removed, "start_line": 12,
                    "diff": diff}),
        )
    }

    /// The Code level's row for `path`.
    fn code_file(s: &mut DesktopState, path: &str) -> Option<Value> {
        all(s)["code"]["files"]
            .as_array()
            .expect("the code pane lists files")
            .iter()
            .find(|f| f["path"] == path)
            .cloned()
    }

    /// A file row's state, word and tone.
    fn words(f: &Value) -> (String, String, String) {
        let s = |k: &str| {
            f[k].as_str()
                .unwrap_or_else(|| panic!("{k}: {f}"))
                .to_string()
        };
        (s("state"), s("word"), s("tone"))
    }

    const ONE_HUNK: &str = "--- app/core.py\n+++ app/core.py\n@@ -11,3 +11,3 @@\n def f():\n-    return 1\n+    return 2\n # end\n";

    #[test]
    fn file_state_words_follow_approval_and_outcome() {
        // The risk (#831): a file said to be in the tree after its run
        // rolled back, or awaiting an approval nobody waits on, or a state
        // the desktop guessed where trellis said nothing. Each word follows
        // an event: an open approval_request for an edit to the file, a run
        // in flight, then run_end's outcome and restored.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p"));
        let c = feed(
            &mut s,
            &edit_question(
                "approval-1",
                "app/core.py",
                "  def f():\n-     return 1\n+     return 2\n",
            ),
        );
        assert!(c.has(Pane::Code), "an approval changes the Code level");
        let f = code_file(&mut s, "app/core.py").expect("the asked-for file is listed");
        assert_eq!(
            words(&f),
            (
                "awaiting".into(),
                "awaiting your approval".into(),
                "warn".into()
            )
        );
        assert_eq!(
            f["approval"], "approval-1",
            "Approve and Reject answer this id"
        );
        assert_eq!(
            (f["added"].clone(), f["removed"].clone()),
            (json!(1), json!(1))
        );
        // Its lines are shown, numbered by nothing: the detail has no @@.
        let lines = f["hunks"][0]["lines"].as_array().unwrap();
        assert_eq!(
            lines
                .iter()
                .map(|l| l["sign"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["context", "removed", "added"]
        );
        assert!(
            lines
                .iter()
                .all(|l| l["new"].is_null() && l["at"].is_null())
        );

        // A question that is not an edit's, or whose detail names another
        // file than its question, ties to no file.
        for (id, question, detail) in [
            ("approval-2", "bash", "pytest -q"),
            ("approval-3", "bash", "--- x.py\n+++ x.py\n+ y"),
            ("approval-4", "edit y.py (+1 -0)", "--- x.py\n+++ x.py\n+ y"),
            ("approval-5", "edit x.py (+1 -0)", "--- x.py\n+++ y.py\n+ y"),
        ] {
            feed(
                &mut s,
                &json!({"type": "approval_request", "id": id, "question": question,
                        "detail": detail})
                .to_string(),
            );
            let files = all(&mut s)["code"]["files"].clone();
            assert_eq!(files.as_array().unwrap().len(), 1, "{id}: {files}");
        }

        // Answered and applied: in the worktree, with the turn's counts.
        let answer = json!({"type": "response", "command": "approve", "success": true,
                            "id": "approval-1", "data": {"approved": true}});
        let c = feed(&mut s, &answer.to_string());
        assert!(c.has(Pane::Code));
        assert!(
            code_file(&mut s, "app/core.py").is_none(),
            "an answered question that wrote nothing yet leaves no file"
        );
        feed(&mut s, &applied_edit(2, "app/core.py", 1, 1, ONE_HUNK));
        let f = code_file(&mut s, "app/core.py").unwrap();
        assert_eq!(
            words(&f),
            ("worktree".into(), "in the worktree".into(), "signal".into())
        );
        assert_eq!(f["approval"], Value::Null, "no Approve or Reject now");
        assert_eq!(
            (f["added"].clone(), f["removed"].clone()),
            (json!(1), json!(1))
        );

        // Converged: left in the tree, and the approvals still open are
        // dropped with the run, so none is awaited.
        feed(
            &mut s,
            &edit_question("approval-6", "app/core.py", "- a\n+ b\n"),
        );
        assert_eq!(
            words(&code_file(&mut s, "app/core.py").unwrap()).0,
            "awaiting"
        );
        let c = feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "converged", "exit": 0, "id": "p"}).to_string(),
        );
        assert!(c.has(Pane::Code));
        let f = code_file(&mut s, "app/core.py").unwrap();
        assert_eq!(
            words(&f),
            ("left".into(), "left in the tree".into(), "ok".into())
        );
        assert_eq!(
            f["hunks"].as_array().unwrap().len(),
            1,
            "the dropped question's hunk is gone"
        );

        // Each ending, as run_end said it.
        for (end, want) in [
            (
                json!({"outcome": "not_converged", "exit": 2, "restored": true}),
                ("rolled_back", "rolled back", "down"),
            ),
            (
                json!({"outcome": "interrupted", "exit": 130, "restored": true}),
                ("rolled_back", "rolled back", "down"),
            ),
            (
                json!({"outcome": "not_converged", "exit": 2, "restored": false}),
                ("left", "left in the tree", "warn"),
            ),
            (
                json!({"outcome": "error", "exit": 1, "error": "boom"}),
                ("not_reported", "not reported", "idle"),
            ),
        ] {
            feed(&mut s, &run_start("q"));
            assert!(
                code_file(&mut s, "app/core.py").is_none(),
                "a new run starts with no files"
            );
            feed(&mut s, &applied_edit(1, "app/core.py", 1, 1, ONE_HUNK));
            let mut e = end.as_object().unwrap().clone();
            e.insert("type".into(), json!("run_end"));
            e.insert("id".into(), json!("q"));
            feed(&mut s, &Value::Object(e).to_string());
            let f = code_file(&mut s, "app/core.py").unwrap();
            let (a, b, c) = want;
            assert_eq!(words(&f), (a.into(), b.into(), c.into()), "{end}");
            assert_eq!(f["why_state"].is_null(), a != "not_reported", "{end}: {f}");
        }

        // The child gone before its run_end: nothing said what became of
        // the write, so nothing is claimed.
        feed(&mut s, &run_start("r"));
        feed(&mut s, &applied_edit(1, "app/core.py", 1, 1, ONE_HUNK));
        let c = s.child_exited();
        assert!(c.has(Pane::Code));
        let f = code_file(&mut s, "app/core.py").unwrap();
        assert_eq!(words(&f).0, "not_reported");
        assert!(f["why_state"].as_str().unwrap().contains("exited"), "{f}");

        // A file write_file created: listed, its counts unreported, not 0.
        feed(&mut s, &run_start("w"));
        feed(
            &mut s,
            &turn(
                json!({"turn": 1, "call": "write_file app/new.py", "verdict": "None",
                         "check_kind": "", "observation": "created",
                         "tool": "write_file", "target": "app/new.py", "wrote": true}),
            ),
        );
        let f = code_file(&mut s, "app/new.py").unwrap();
        assert_eq!(
            (f["added"].clone(), f["removed"].clone()),
            (Value::Null, Value::Null)
        );
        assert!(f["why_counts"].is_string(), "{f}");
        assert!(f["hunks"][0]["why_no_diff"].is_string(), "{f}");
    }

    #[test]
    fn a_line_comment_becomes_a_steer_naming_file_and_line() {
        // The risk (#831): a comment that names a line the desktop guessed,
        // or one the file no longer has, so the steer points the model at
        // the wrong code. A line is named `file:line` only from trellis's
        // @@ header, in the file as its edit left it; a removed line, a
        // pending edit's line, an earlier edit's line and any line once the
        // run has ended take no comment, each saying why; and the steer
        // sent carries that name as its first words.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p"));
        feed(&mut s, &applied_edit(2, "app/core.py", 1, 1, ONE_HUNK));
        let f = code_file(&mut s, "app/core.py").unwrap();
        let hunk = &f["hunks"][0];
        assert_eq!(hunk["header"], "@@ -11,3 +11,3 @@");
        let lines = hunk["lines"].as_array().unwrap();
        let shape: Vec<(Value, Value, Value, Value)> = lines
            .iter()
            .map(|l| {
                (
                    l["sign"].clone(),
                    l["old"].clone(),
                    l["new"].clone(),
                    l["at"].clone(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            [
                (
                    json!("context"),
                    json!(11),
                    json!(11),
                    json!("app/core.py:11")
                ),
                (json!("removed"), json!(12), Value::Null, Value::Null),
                (
                    json!("added"),
                    Value::Null,
                    json!(12),
                    json!("app/core.py:12")
                ),
                (
                    json!("context"),
                    json!(13),
                    json!(13),
                    json!("app/core.py:13")
                ),
            ]
        );
        assert_eq!(
            lines[2]["text"], "    return 2",
            "the text without its sign"
        );
        assert!(
            lines[1]["why"]
                .as_str()
                .unwrap()
                .starts_with("a removed line is not in the file now"),
            "{}",
            lines[1]
        );

        // What code.js sends for a comment on the added line: a steer whose
        // message starts with the line's name. Its row says so.
        let at = lines[2]["at"].as_str().unwrap();
        let c = s.steer_sent("desktop-code-steer-1", &format!("{at}: make it 3"));
        assert!(c.has(Pane::Transcript));
        let v = all(&mut s);
        let row = rows(&v["transcript"]).last().cloned().unwrap();
        assert_eq!(row["kind"], "steer");
        assert_eq!(row["text"], "app/core.py:12: make it 3");

        // A model line that reads like a header is a line of the file, and
        // trellis's cut line is not one.
        let tricky = "--- app/core.py\n+++ app/core.py\n@@ -40,2 +40,2 @@\n x = 1\n-@@ -900 +900 @@\n+--- y\n...(3 more diff lines cut at 8192 bytes)...\n";
        feed(&mut s, &applied_edit(3, "app/core.py", 1, 1, tricky));
        let f = code_file(&mut s, "app/core.py").unwrap();
        let latest = f["hunks"][1]["lines"].as_array().unwrap();
        let named: Vec<Value> = latest.iter().map(|l| l["at"].clone()).collect();
        assert_eq!(
            named,
            [
                json!("app/core.py:40"),
                Value::Null,
                json!("app/core.py:41"),
                Value::Null
            ]
        );
        assert_eq!(latest[1]["text"], "@@ -900 +900 @@");
        assert_eq!(latest[3]["sign"], "note");
        // The earlier edit's lines may have moved: no comment there now.
        let earlier = f["hunks"][0]["lines"].as_array().unwrap();
        assert!(earlier.iter().all(|l| l["at"].is_null()), "{f}");
        assert!(
            earlier[0]["why"]
                .as_str()
                .unwrap()
                .starts_with("a later edit to this file"),
            "{}",
            earlier[0]
        );
        // Without a header, no line is numbered at all.
        feed(
            &mut s,
            &applied_edit(
                4,
                "app/other.py",
                1,
                0,
                "--- app/other.py\n+++ app/other.py\n+x\n",
            ),
        );
        let f = code_file(&mut s, "app/other.py").unwrap();
        let only = &f["hunks"][0]["lines"][2];
        assert_eq!(
            (only["new"].clone(), only["at"].clone()),
            (Value::Null, Value::Null)
        );

        // A pending edit's lines carry no number until the turn applies it.
        feed(
            &mut s,
            &edit_question("approval-1", "app/core.py", "- x = 1\n+ x = 2\n"),
        );
        let f = code_file(&mut s, "app/core.py").unwrap();
        let pending = f["hunks"][2]["lines"].as_array().unwrap();
        assert!(pending.iter().all(|l| l["at"].is_null()), "{f}");
        assert!(
            pending[1]["why"]
                .as_str()
                .unwrap()
                .contains("with the turn that applies it"),
            "{f}"
        );

        // Once the run has ended no steer can reach it.
        feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "converged", "exit": 0, "id": "p"}).to_string(),
        );
        let f = code_file(&mut s, "app/core.py").unwrap();
        let latest = f["hunks"][1]["lines"].as_array().unwrap();
        assert!(latest.iter().all(|l| l["at"].is_null()), "{f}");
        assert!(
            latest[0]["why"]
                .as_str()
                .unwrap()
                .contains("a steer reaches only a run in flight"),
            "{f}"
        );
    }

    /// A value of the shape `field` has on the wire, distinct from every
    /// other value built: numbers are four digits from 7001 up, strings embed
    /// the record and field name. A field this does not know gets a string,
    /// so a contract field added with another shape fails below by name
    /// rather than being skipped.
    fn sample(field: &str, record: &str, spec: &Value, n: &mut u64) -> Value {
        if let Some(vals) = spec
            .get(format!("{field}_values"))
            .and_then(Value::as_array)
        {
            return vals.last().cloned().expect("a non-empty value list");
        }
        match field {
            "turn"
            | "resolved"
            | "candidates"
            | "unresolved"
            | "prompt_bytes"
            | "model_ms"
            | "completion_tokens"
            | "queue_wait_ms"
            | "max_iterations"
            | "after_step"
            | "version"
            | "exit"
            | "kv_slots"
            | "tiers_vram"
            | "tiers_ram"
            | "tiers_disk"
            | "elapsed_ms"
            | "completion_tokens_used"
            | "max_wall_clock_ms"
            | "max_tokens"
            | "context_bytes"
            | "lines_added"
            | "lines_removed"
            | "start_line"
            | "duration_ms"
            | "paused_ms"
            | "steps_cut"
            | "texts_cut" => {
                *n += 1;
                json!(*n)
            }
            "data" => json!({"answer": format!("tok-{record}-{field}-end")}),
            _ => json!(format!("tok-{record}-{field}-end")),
        }
    }

    /// Fields whose value is a boolean on the wire.
    const BOOLEANS: [&str; 3] = ["success", "restored", "wrote"];

    /// `run_start` as the frame's Debug spells its variant, `RunStart`.
    fn camel(ty: &str) -> String {
        ty.split('_')
            .map(|w| {
                let mut c = w.chars();
                c.next()
                    .map(|f| f.to_uppercase().chain(c).collect::<String>())
                    .unwrap_or_default()
            })
            .collect()
    }

    /// The fields one record carries, from the contract: every list in an
    /// event's spec but a `*_values` one, and a framing type's `always` and
    /// `optional` keys.
    fn fields_of(spec: &Value) -> Vec<String> {
        let mut out = Vec::new();
        for (k, v) in spec.as_object().expect("a contract record") {
            match v {
                Value::Array(a) if !k.ends_with("_values") => {
                    out.extend(a.iter().map(|f| f.as_str().unwrap().to_string()))
                }
                Value::Object(o) if k == "optional" => out.extend(o.keys().cloned()),
                _ => {}
            }
        }
        out
    }

    #[test]
    fn every_field_in_the_rpc_contract_is_read() {
        // The risk (ADR 0048 §6): trellis keeps a field the contract
        // promises and the desktop's parser drops it, so a pane goes blank
        // while every trellis test passes. This measures what the parser
        // keeps, from its Debug output, for every field the contract lists.
        let contract: Value = serde_json::from_str(CONTRACT).expect("the contract is JSON");
        let rpc = &contract["rpc"];
        let mut n = 7000u64;
        let mut lost: Vec<String> = Vec::new();
        let mut records = 0;

        // (what, the record's fields with their values, how to wrap it)
        let mut build: Vec<(String, Map<String, Value>, bool)> = Vec::new();
        // The fields not every client reads yet, but whose `read_by` names
        // the desktop: a claim the contract makes for this parser, held here
        // as the rest are. They go on the same record, since without its
        // `always` fields the line is no event or framing record at all.
        let ours = |extra: &Value| -> Vec<String> {
            fields_of(extra)
                .into_iter()
                .filter(|f| {
                    extra["read_by"][f.as_str()]
                        .as_array()
                        .is_some_and(|by| by.contains(&json!("desktop")))
                })
                .collect()
        };
        let later = &rpc["not_every_client"];
        for (name, spec) in rpc["events"].as_object().expect("rpc.events") {
            let mut fields: Vec<(String, &Value)> =
                fields_of(spec).into_iter().map(|f| (f, spec)).collect();
            if let Some(extra) = later["events"].get(name) {
                fields.extend(ours(extra).into_iter().map(|f| (f, extra)));
            }
            let mut m = Map::new();
            for (f, spec) in fields {
                let v = if BOOLEANS.contains(&f.as_str()) {
                    json!(true)
                } else {
                    sample(&f, name, spec, &mut n)
                };
                m.insert(f, v);
            }
            build.push((name.clone(), m, true));
        }
        // An event every client does not read yet has no rpc.events record
        // to join (`paused` and `resumed`, #837): its desktop fields are a
        // record of their own, so a read_by naming the desktop is held too.
        for (name, extra) in later["events"]
            .as_object()
            .expect("not_every_client.events")
        {
            let fields = ours(extra);
            if rpc["events"].get(name).is_some() || fields.is_empty() {
                continue;
            }
            let mut m = Map::new();
            for f in fields {
                let v = if BOOLEANS.contains(&f.as_str()) {
                    json!(true)
                } else {
                    sample(&f, name, extra, &mut n)
                };
                m.insert(f, v);
            }
            build.push((name.clone(), m, true));
        }
        for (ty, spec) in rpc["framing"].as_object().expect("rpc.framing") {
            let mut fields = fields_of(spec);
            if let Some(extra) = later["framing"].get(ty) {
                fields.extend(ours(extra));
            }
            let mut m = Map::new();
            for f in fields {
                let v = if f == "type" {
                    json!(ty)
                } else if BOOLEANS.contains(&f.as_str()) {
                    json!(true)
                } else {
                    sample(&f, ty, spec, &mut n)
                };
                m.insert(f, v);
            }
            build.push((ty.clone(), m, false));
        }

        let line = |name: &str, m: &Map<String, Value>, is_event: bool| {
            if is_event {
                event(name, Value::Object(m.clone()))
            } else {
                Value::Object(m.clone()).to_string()
            }
        };
        for (name, m, is_event) in &build {
            records += 1;
            let debug = format!("{:?}", parse_stdout(&line(name, m, *is_event)));
            for (f, v) in m {
                let kept = if f == "type" {
                    debug.starts_with(&format!("{}(", camel(name)))
                } else if let Some(b) = v.as_bool() {
                    // Present as true; flipped, the parse must differ.
                    let mut flipped = m.clone();
                    flipped.insert(f.clone(), json!(!b));
                    let other = format!("{:?}", parse_stdout(&line(name, &flipped, *is_event)));
                    other != debug
                } else {
                    let needle = match v {
                        Value::String(s) => s.clone(),
                        Value::Object(o) => o["answer"].as_str().unwrap().to_string(),
                        other => other.to_string(),
                    };
                    debug.contains(&needle)
                };
                if !kept {
                    lost.push(format!("{name}.{f}"));
                }
            }
        }
        assert!(
            records >= 7,
            "the contract lists fewer records than expected"
        );
        assert!(
            lost.is_empty(),
            "the parser drops what client_contract.json lists: {lost:?}"
        );
    }

    /// A run to summarise (#832): its start, a radius, a check that passed
    /// with its time, an applied edit, and a steer delivered after turn 1.
    fn summary_run(s: &mut DesktopState) {
        feed(s, &run_start("p1"));
        feed(s, &event("run budget", json!({"max_iterations": 40})));
        feed(
            s,
            &turn(
                json!({"turn": 1, "call": "blast_radius f", "verdict": "None",
                         "check_kind": "", "observation": "", "tool": "blast_radius",
                         "target": "f", "wrote": false, "resolved": 4, "candidates": 2,
                         "unresolved": 1}),
            ),
        );
        s.steer_sent("composer-steer-1", "keep the name");
        feed(
            s,
            &json!({"type": "response", "command": "steer", "success": true,
                    "id": "composer-steer-1", "data": {"after_step": 1}})
            .to_string(),
        );
        feed(
            s,
            &turn(
                json!({"turn": 2, "call": "edit a.py", "verdict": "None", "check_kind": "",
                         "observation": "applied", "tool": "edit", "target": "a.py",
                         "wrote": true, "lines_added": 3, "lines_removed": 1}),
            ),
        );
        feed(
            s,
            &turn(
                json!({"turn": 3, "call": "run_checks", "verdict": "Some(true)",
                         "check_kind": "Tests", "observation": "ok", "tool": "run_checks",
                         "target": "", "wrote": false, "duration_ms": 1250}),
            ),
        );
    }

    /// A `trellis log` row as `trails.rs` makes it, for run `id`.
    fn trail_row(id: &str, verified: Value, reviewed: Value) -> Value {
        json!({"integrity": "intact", "run_id": {"untrusted": id},
               "outcome": {"untrusted": "converged"}, "task": {"untrusted": "change f"},
               "mode_counts": {"verified": verified, "reviewed": reviewed}})
    }

    #[test]
    fn summary_so_far_shows_no_outcome_before_run_end() {
        // The risk (#832): the summary reached mid-run shows an outcome, or
        // reads as the run's end, while the run is still in flight; or
        // recomputes what the panes beside it hold, so the two disagree.
        let mut s = DesktopState::default();
        summary_run(&mut s);
        let v = all(&mut s);
        let sm = &v["recap"];
        assert_eq!(sm["stage"], "so_far", "{sm}");
        assert!(sm["word"].as_str().unwrap().contains("so far"), "{sm}");
        assert_eq!(
            sm["outcome"],
            Value::Null,
            "an outcome before run_end: {sm}"
        );
        assert!(sm["why_outcome"].as_str().unwrap().contains("run_end"));
        assert_eq!(
            sm["checks"],
            json!([{"kind": "Tests", "runs": [
                {"turn": 3, "result": "passed", "took_ms": 1250, "why_took": null}]}])
        );
        assert_eq!(sm["radius"], v["radius"], "the radius pane's three counts");
        assert_eq!(
            sm["budget"], v["budget"]["limits"],
            "the budget pane's rows"
        );
        assert_eq!(sm["files"][0]["path"], "a.py");
        assert_eq!(sm["files"][0]["added"], v["code"]["files"][0]["added"]);
        assert_eq!(sm["files"][0]["word"], "in the worktree");
        assert_eq!(
            sm["steers"],
            json!([{"text": "keep the name",
                    "delivery": {"state": "delivered", "after_step": 1}}])
        );
        assert_eq!(
            sm["path"]
                .as_array()
                .unwrap()
                .iter()
                .map(|p| p["turn"].clone())
                .collect::<Vec<_>>(),
            [json!(1), json!(2), json!(3)]
        );
        assert_eq!(sm["trail"]["integrity"], Value::Null);
        assert_eq!(sm["closing"]["words"], Value::Null);
        // A steer row changed outside update still re-sends the summary.
        let c = s.steer_sent("composer-steer-2", "and the test");
        let v = s.view(c);
        assert_eq!(
            v["recap"]["steers"][1]["delivery"],
            json!({"state": "pending"})
        );

        // Once run_end has come it is the run's end, with its outcome.
        feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "not_converged", "exit": 1, "id": "p1",
                    "restored": true})
            .to_string(),
        );
        let sm = all(&mut s)["recap"].clone();
        assert_eq!(sm["stage"], "final");
        assert_eq!(sm["outcome"]["outcome"], "not converged");
        assert_eq!(sm["outcome"]["exit"], 1);
        assert_eq!(sm["files"][0]["word"], "rolled back");
        // A run that did not converge is never given trellis's last closing
        // words, which may be an earlier run's.
        assert!(!s.closing_to_ask());
        assert!(
            sm["closing"]["why"]
                .as_str()
                .unwrap()
                .contains("earlier run")
        );
        // A new run starts its summary afresh.
        feed(&mut s, &run_start("p2"));
        let sm = all(&mut s)["recap"].clone();
        assert_eq!(sm["stage"], "so_far");
        assert_eq!(sm["checks"], json!([]));
        assert_eq!(sm["steers"], json!([]));
    }

    #[test]
    fn summary_keeps_verified_and_reviewed_apart() {
        // The risk (CLAUDE.md rule 4, ADR 0003): the summary's mode counts
        // collapsed into one figure (a total, a sum, a share), which says
        // how much of a run was verified when part of it was only reviewed;
        // or a count the trail does not record drawn as 0.
        let mut s = DesktopState::default();
        summary_run(&mut s);
        feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "converged", "exit": 0, "id": "p1",
                    "run_id": "r-1"})
            .to_string(),
        );
        // Mid-read the trail is said to be read, its counts null, never 0.
        let sm = all(&mut s)["recap"].clone();
        assert_eq!(sm["modes"]["verified"], Value::Null);
        assert_eq!(sm["modes"]["reviewed"], Value::Null);
        assert_eq!(sm["trail"]["run_id"], "r-1");
        assert_eq!(s.trail_to_read().as_deref(), Some("r-1"));
        assert_eq!(s.trail_to_read(), None, "the core reads it once");

        // Another run's row is not this run's.
        let c = s.trail_read("r-0", Ok(vec![trail_row("r-0", json!(9), json!(9))]));
        assert!(c.is_empty(), "{c:?}");
        let c = s.trail_read(
            "r-1",
            Ok(vec![
                trail_row("r-2", json!(7), json!(7)),
                trail_row("r-1", json!(3), json!(1)),
            ]),
        );
        assert!(c.has(Pane::Summary), "{c:?}");
        let v = s.view(c);
        let sm = &v["recap"];
        assert_eq!(
            sm["modes"],
            json!({"verified": 3, "reviewed": 1, "why_verified": null, "why_reviewed": null})
        );
        assert_eq!(
            sm["trail"],
            json!({"run_id": "r-1", "integrity": "intact", "why": null})
        );
        // No figure derived from the two: no key names one, and their sum
        // appears nowhere in the summary.
        let mut ks = Vec::new();
        keys(sm, &mut ks);
        let derived: Vec<&String> = ks
            .iter()
            .filter(|k| {
                let k = k.to_lowercase();
                [
                    "total",
                    "sum",
                    "ratio",
                    "percent",
                    "fraction",
                    "share",
                    "mode_count",
                ]
                .iter()
                .any(|w| k.contains(w))
            })
            .collect();
        assert!(derived.is_empty(), "{derived:?}");
        fn numbers(v: &Value, out: &mut Vec<u64>) {
            match v {
                Value::Number(n) => out.extend(n.as_u64()),
                Value::Array(a) => a.iter().for_each(|x| numbers(x, out)),
                Value::Object(o) => o.values().for_each(|x| numbers(x, out)),
                _ => {}
            }
        }
        let mut ns = Vec::new();
        numbers(&sm["modes"], &mut ns);
        numbers(&sm["trail"], &mut ns);
        ns.sort_unstable();
        assert_eq!(
            ns,
            [1, 3],
            "Verified and Reviewed, two figures and no third"
        );

        // A trail that records one count and not the other keeps them
        // apart: the missing one is null with why, never 0 or the other's.
        let mut s = DesktopState::default();
        summary_run(&mut s);
        feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "converged", "exit": 0, "id": "p1",
                    "run_id": "r-1"})
            .to_string(),
        );
        let c = s.trail_read("r-1", Ok(vec![trail_row("r-1", json!(2), Value::Null)]));
        let sm = s.view(c)["recap"].clone();
        assert_eq!(sm["modes"]["verified"], 2);
        assert_eq!(sm["modes"]["reviewed"], Value::Null);
        assert!(sm["modes"]["why_reviewed"].is_string());
        // A run trellis log does not list is said to be missing, not read.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p1"));
        feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "error", "exit": 2, "id": "p1",
                    "run_id": "r-9", "error": "boom"})
            .to_string(),
        );
        let c = s.trail_read("r-9", Ok(vec![]));
        let sm = s.view(c)["recap"].clone();
        assert_eq!(sm["trail"]["integrity"], Value::Null);
        assert!(
            sm["trail"]["why"]
                .as_str()
                .unwrap()
                .contains("do not include run r-9")
        );
        assert_eq!(sm["modes"]["verified"], Value::Null);
    }

    #[test]
    fn summary_closing_words_are_fenced_text() {
        // The risk (CLAUDE.md rule 10, ADR 0007): the model's closing words
        // reach the summary as anything but its own text: unfenced, so the
        // page could draw them as the desktop's or as a verdict; altered
        // (markup or control characters dropped), so they are not what the
        // model said; read from an answer the core did not ask for, or
        // asked after a run that did not converge, so an earlier run's words
        // stand as this run's.
        let mut s = DesktopState::default();
        summary_run(&mut s);
        // Mid-run there are none, and nothing is asked.
        assert!(!s.closing_to_ask());
        let said = "Done. <script>alert(1)</script> \u{1b}[31mred\u{1b}[0m\n\tall checks ok";
        let answer = |id: &str| {
            json!({"type": "response", "command": "get_last_assistant_text",
                   "success": true, "id": id, "data": {"text": said}})
            .to_string()
        };
        // Before the core has asked, an answer is not read.
        feed(&mut s, &answer(SUMMARY_ID));
        assert_eq!(all(&mut s)["recap"]["closing"]["words"], Value::Null);

        feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "converged", "exit": 0, "id": "p1"}).to_string(),
        );
        assert!(s.closing_to_ask(), "a converged run's words are asked for");
        assert!(!s.closing_to_ask(), "once");
        // An answer to another id is the page's, not the summary's.
        feed(&mut s, &answer("desktop-last"));
        assert_eq!(all(&mut s)["recap"]["closing"]["words"], Value::Null);
        let c = feed(&mut s, &answer(SUMMARY_ID));
        let v = s.view(c);
        let closing = &v["recap"]["closing"];
        // Fenced as `{untrusted}`, the trails' one shape for text not the
        // desktop's, byte for byte, under a label saying whose it is.
        assert_eq!(closing["words"], json!({"untrusted": said}));
        assert_eq!(closing["why"], Value::Null);
        let label = closing["label"].as_str().unwrap();
        assert!(
            label.contains("the model's") && label.contains("not a verdict"),
            "{label}"
        );
        // Nowhere else in the summary: no tone, result or check carries it.
        let elsewhere = v["recap"]
            .as_object()
            .unwrap()
            .iter()
            .filter(|(k, _)| *k != "closing")
            .any(|(_, x)| x.to_string().contains("alert(1)"));
        assert!(!elsewhere, "the words leak out of their fence");
        // Nor into the transcript as a row of any kind.
        assert!(!v.to_string().contains("\"text\":\"Done."), "{v}");

        // Long words are cut with the desktop's marker, not kept whole.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p1"));
        feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "converged", "exit": 0, "id": "p1"}).to_string(),
        );
        let long = "x".repeat(CLOSING_BYTES * 2);
        let c = feed(
            &mut s,
            &json!({"type": "response", "command": "get_last_assistant_text",
                    "success": true, "id": SUMMARY_ID, "data": {"text": long}})
            .to_string(),
        );
        let words = s.view(c)["recap"]["closing"]["words"]["untrusted"]
            .as_str()
            .unwrap()
            .to_string();
        assert!(words.len() <= CLOSING_BYTES, "{}", words.len());
        assert!(words.ends_with("bytes were cut]"), "{words}");

        // No words held, or a question the core could not write: null with
        // why, never an empty string drawn as the model's.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p1"));
        feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "converged", "exit": 0, "id": "p1"}).to_string(),
        );
        let c = feed(
            &mut s,
            &json!({"type": "response", "command": "get_last_assistant_text",
                    "success": true, "id": SUMMARY_ID, "data": {"text": null}})
            .to_string(),
        );
        let closing = s.view(c)["recap"]["closing"].clone();
        assert_eq!(closing["words"], Value::Null);
        assert!(closing["why"].as_str().unwrap().contains("ended on done"));
        // A new run started before an answer came: that answer is not read
        // as the new run's.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p1"));
        feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "converged", "exit": 0, "id": "p1"}).to_string(),
        );
        feed(&mut s, &run_start("p2"));
        feed(&mut s, &answer(SUMMARY_ID));
        assert_eq!(all(&mut s)["recap"]["closing"]["words"], Value::Null);
    }
}
