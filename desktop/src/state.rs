//! The desktop's run panes, as a pure reducer over child frames.
//!
//! The TUI's `AppState::update` is synchronous and does no I/O, which is why
//! its tests run without a terminal; `DesktopState::update` is the same for
//! the desktop: a [`Frame`] in, the panes it changed out, and nothing read
//! or written anywhere else. It follows `crates/trellis-cli/src/tui_bridge.rs`
//! for what a turn means, so the desktop and the TUI draw one run the same
//! way.
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
use crate::frames::{ApprovalRequest, Frame, RunEnd, RuntimeIdentified, Turn};
use crate::graph;
use crate::queue;
use serde_json::{Map, Value, json};
use std::collections::VecDeque;

/// The transcript keeps this many lines, as the TUI's does: a run streams
/// for as long as it lasts, and an uncapped log grows with it.
pub const TRANSCRIPT_CAP: usize = 2000;

/// The trellis messages pane keeps this many stderr lines (#302): the cap
/// ADR 0052's amendment to ADR 0048 section 2 puts on showing stderr.
pub const STDERR_CAP: usize = 500;

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
}

/// Every pane the view carries. Each is a key the page must draw:
/// `view_renders_every_pane_key` in `lockdown.rs` drives a whole run through
/// [`DesktopState::update`] and fails naming a key here that no module the
/// page loads reads from the `view` event (ADR 0052 section 1, #513).
const PANES: [Pane; 24] = [
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
];

/// The panes a run starts afresh: everything but the transcript, which,
/// as the TUI's, carries on across runs within its cap; the tier, which
/// only a `get_state` reply sets: the view shows the tier trellis last
/// reported, and a run's own start reports none; and the starters, which
/// are the repository's templates, not the run's. The undelivered list is
/// the last stopped run's, so a new run drops it, and so is the alert: what
/// failed before this run started is not this run's failure. The stderr
/// pane, as the transcript, carries on.
const RUN_PANES: [Pane; 13] = [
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
];

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
        }
    }

    /// One bit per pane; `u32`, since seventeen panes are past a `u16`.
    fn bit(self) -> u32 {
        1 << (self as u32)
    }
}

/// Which panes an update changed: what the core sends the page as one
/// `view` event, so a frame that changes nothing sends nothing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Changed(u32);

impl Changed {
    fn with(self, p: Pane) -> Self {
        Self(self.0 | p.bit())
    }

    fn and(self, other: Self) -> Self {
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

/// One turn, whatever it was and however it went (tui_bridge.rs).
#[derive(Debug, Clone, PartialEq)]
struct Action {
    tool: String,
    target: String,
    /// Whether the turn changed the tree: an applied `edit`, or a
    /// `write_file` that created its file.
    wrote: bool,
    /// The turn's own figures, `None` when it reported none.
    prompt_bytes: Option<u64>,
    completion_tokens: Option<u64>,
}

/// A check that ran: its kind and whether it passed.
#[derive(Debug, Clone, PartialEq)]
struct Check {
    kind: String,
    passed: bool,
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
    max_turns: Option<u64>,
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

/// The read-only requests a view of its own sends and answers in place: its
/// refusal is that view's to say (the `codebase` and `radius` events, the
/// vault, queue, models and session tokens panes), not a line in the run's
/// transcript or the page's one alert. `get_trails` and `get_overview` are
/// the page's own commands, not rpc lines, so their answers never come here.
/// `get_symbols` and `get_history` are the Atlas's.
const VIEW_READS: [&str; 10] = [
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
    transcript: VecDeque<String>,
    actions: Vec<Action>,
    edits: Vec<String>,
    checks: Vec<Check>,
    radius: Option<Radius>,
    budget: Budget,
    approvals: Vec<ApprovalRequest>,
    /// Whether a run is in flight: from `run_start` to `run_end`.
    running: bool,
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
    /// error, or a stdout line that is not a JSON object. `None` until one,
    /// and from each run's start.
    alert: Option<String>,
    /// trellis's stderr lines, shown as human text and never read as data
    /// (ADR 0048 section 2, amended by ADR 0052), capped at [`STDERR_CAP`].
    stderr: VecDeque<String>,
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
}

impl DesktopState {
    /// Applies one child frame and returns the panes it changed.
    pub fn update(&mut self, frame: Frame) -> Changed {
        match frame {
            Frame::Turn(t) => self.turn(t),
            Frame::ModelCall { turn } => {
                self.phase = Some(Phase::Model { turn });
                Changed::of(&[Pane::Phase])
            }
            Frame::RunBudget { max_iterations } => {
                self.budget.max_turns = max_iterations;
                Changed::of(&[Pane::Budget])
            }
            Frame::SteerDelivered { after_step } => {
                self.push_line(match after_step {
                    Some(n) => format!("· steer delivered after step {n}"),
                    None => "· steer delivered".to_string(),
                });
                Changed::of(&[Pane::Transcript])
            }
            Frame::RuntimeIdentified(r) => {
                self.runtime = Some(r);
                Changed::of(&[Pane::Runtime])
            }
            Frame::RunStart(s) => {
                self.reset_run();
                self.running = true;
                let mut c = Changed::of(&RUN_PANES);
                if let Some(task) = s.task {
                    self.push_line(format!("▸ {task}"));
                    c = c.with(Pane::Transcript);
                }
                c
            }
            Frame::RunEnd(e) => {
                self.running = false;
                // Nothing is on once the run has ended.
                self.phase = None;
                // Nobody is left to answer a question the run asked.
                self.approvals.clear();
                let mut c =
                    Changed::of(&[Pane::Running, Pane::Approvals, Pane::Outcome, Pane::Phase]);
                if let Some(error) = &e.error {
                    self.alert = Some(format!("The run ended in error: {error}"));
                    c = c.with(Pane::Alert);
                }
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
                    _ => {}
                }
                // A refused command is said in the transcript, unless a view
                // asked and shows the reason itself; a successful answer
                // changes no run pane but the tier a `get_state` reports.
                let view_read = r
                    .command
                    .as_deref()
                    .is_some_and(|c| VIEW_READS.contains(&c));
                let c = if r.success == Some(true) && r.command.as_deref() == Some("get_state") {
                    self.tier = r
                        .data
                        .as_ref()
                        .and_then(|d| d.get("tier"))
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    c.with(Pane::Tier)
                } else if r.success == Some(false) && !view_read {
                    let command = r.command.as_deref().unwrap_or("command");
                    let said = match r.error {
                        Some(e) => format!("{command} refused: {e}"),
                        None => format!("{command} refused"),
                    };
                    self.push_line(format!("· {said}"));
                    self.alert = Some(said);
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
                self.alert = Some(format!(
                    "trellis rpc printed a line that is not a JSON object: {line}"
                ));
                self.push_line(line);
                Changed::of(&[Pane::Transcript, Pane::Alert])
            }
            // trellis's human text: its own capped pane, apart from the
            // run's transcript, and never read for anything (#302).
            Frame::Human(line) => {
                self.stderr.push_back(line);
                while self.stderr.len() > STDERR_CAP {
                    self.stderr.pop_front();
                }
                Changed::of(&[Pane::Stderr])
            }
        }
    }

    /// The child has exited: no run is in flight any more, and nobody is
    /// left to answer a question it asked.
    pub fn child_exited(&mut self) -> Changed {
        self.running = false;
        self.approvals.clear();
        self.configured = None;
        self.catalog.clear();
        // The model was that child's session's; the next child has none.
        self.model = None;
        self.models = None;
        // So were the runs the totals counted (#313): a new child's start
        // empty.
        self.session_tokens = None;
        Changed::of(&[
            Pane::Running,
            Pane::Approvals,
            Pane::Model,
            Pane::Models,
            Pane::SessionTokens,
            Pane::Available,
        ])
    }

    /// Whether a run is in flight, from `run_start` to `run_end` or the
    /// child's exit.
    pub fn running(&self) -> bool {
        self.running
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
    /// Clear. Whether a run is in flight, which specs the child reported,
    /// and the model its session has and offered, are the child's facts,
    /// not the view's, so they are kept. Returns every pane, so the view
    /// event that follows carries all of them.
    pub fn clear(&mut self) -> Changed {
        let running = self.running;
        let configured = self.configured.take();
        let catalog = std::mem::take(&mut self.catalog);
        let model = self.model.take();
        let models = self.models.take();
        *self = Self {
            running,
            configured,
            catalog,
            model,
            models,
            ..Self::default()
        };
        Changed::of(&PANES)
    }

    /// The panes in `changed`, as the page receives them.
    pub fn view(&self, changed: Changed) -> Value {
        let mut v = Map::new();
        for p in PANES.into_iter().filter(|&p| changed.has(p)) {
            v.insert(p.key().to_string(), self.pane(p));
        }
        Value::Object(v)
    }

    fn pane(&self, p: Pane) -> Value {
        match p {
            Pane::Transcript => json!(self.transcript),
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
                    "max_turns": self.budget.max_turns,
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
            Pane::Alert => json!(self.alert),
            Pane::Stderr => json!(self.stderr),
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

    /// A turn, as `tui_bridge.rs` maps it.
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

        let mut line = format!("· {}", t.call);
        match passed {
            Some(true) => line.push_str("  ✓ check passed"),
            Some(false) => line.push_str("  ✗ check FAILED"),
            None => {}
        }
        self.push_line(line);
        if let Some(obs) = t.observation.as_deref() {
            for l in obs.lines().take(6) {
                self.push_line(format!("    {l}"));
            }
        }

        // Only an applied edit, or a file `write_file` created, changed the
        // tree; the observation is the only place that says so. A refused
        // edit is still an action, but not an edit.
        let obs = t.observation.as_deref().unwrap_or_default();
        let wrote = (t.call.starts_with("edit ") && obs.contains("applied: true"))
            || (t.call.starts_with("write_file ") && obs.starts_with("created "));
        let mut parts = t.call.splitn(2, ' ');
        let tool = parts.next().unwrap_or_default().to_string();
        // The call's tool is what the turn ran; `run_checks` is the checks
        // phase, by the call and not the verdict, so the governance audit
        // (a `run_checks` the loop ran itself) reads as checks too.
        self.phase = Some(if tool == "run_checks" {
            Phase::Checks { turn: t.turn }
        } else {
            Phase::Tool {
                turn: t.turn,
                tool: tool.clone(),
            }
        });
        self.actions.push(Action {
            tool,
            target: parts.next().unwrap_or_default().to_string(),
            wrote,
            prompt_bytes: t.prompt_bytes,
            completion_tokens: t.completion_tokens,
        });
        if wrote && let Some(path) = t.call.split_whitespace().nth(1) {
            self.edits.push(path.to_string());
            c = c.with(Pane::Edits);
        }

        if let Some(passed) = passed {
            let kind = t
                .check_kind
                .filter(|k| !k.is_empty())
                .unwrap_or_else(|| "check".to_string());
            self.checks.push(Check { kind, passed });
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
        c
    }

    /// Empties the panes a run owns; the transcript is left to its cap.
    fn reset_run(&mut self) {
        self.actions.clear();
        self.edits.clear();
        self.checks.clear();
        self.radius = None;
        self.budget = Budget::default();
        self.approvals.clear();
        self.outcome = None;
        self.undelivered.clear();
        self.alert = None;
        self.phase = None;
        self.runtime = None;
    }

    fn push_line(&mut self, line: String) {
        self.transcript.push_back(line);
        while self.transcript.len() > TRANSCRIPT_CAP {
            self.transcript.pop_front();
        }
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

    fn all(s: &DesktopState) -> Value {
        s.view(Changed::of(&PANES))
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
                         "check_kind": "", "observation": "def f():\n    return 1"}),
            ),
        );
        assert!(c.has(Pane::Actions) && c.has(Pane::Transcript), "{c:?}");
        let v = all(&s);
        assert_eq!(
            v["actions"],
            json!([{"tool": "read_file", "target": "app/core.py", "wrote": false,
                    "prompt_bytes": null, "completion_tokens": null}])
        );
        assert_eq!(
            v["transcript"],
            json!([
                "· read_file app/core.py",
                "    def f():",
                "        return 1"
            ])
        );
        assert_eq!(v["checks"], json!([]), "verdict None ran no check");
    }

    #[test]
    fn refused_edit_is_an_action_but_not_an_edit() {
        // The risk (tui_bridge.rs): a refused edit listed as pending makes
        // the edits pane contradict the action graph a user checks before
        // accepting.
        let mut s = DesktopState::default();
        feed(
            &mut s,
            &turn(
                json!({"turn": 1, "call": "edit a.py", "verdict": "None", "check_kind": "",
                         "observation": "error: old text not found"}),
            ),
        );
        feed(
            &mut s,
            &turn(
                json!({"turn": 2, "call": "edit a.py", "verdict": "None", "check_kind": "",
                         "observation": "applied: false"}),
            ),
        );
        let v = all(&s);
        assert_eq!(v["actions"].as_array().unwrap().len(), 2, "{v}");
        assert_eq!(v["actions"][0]["wrote"], false);
        assert_eq!(v["edits"], json!([]), "{v}");

        feed(
            &mut s,
            &turn(
                json!({"turn": 3, "call": "edit a.py", "verdict": "None", "check_kind": "",
                         "observation": "applied: true (+1 -1)"}),
            ),
        );
        let v = all(&s);
        assert_eq!(v["actions"].as_array().unwrap().len(), 3);
        assert_eq!(v["actions"][2]["wrote"], true);
        assert_eq!(v["edits"], json!(["a.py"]));
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
        let v = all(&s);
        assert_eq!(
            v["checks"],
            json!([{"kind": "Typecheck", "result": "failed"}])
        );
        assert!(
            v["transcript"][0]
                .as_str()
                .unwrap()
                .ends_with("check FAILED"),
            "{v}"
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
            assert_eq!(all(&s)["checks"], json!([]), "verdict {verdict}");
        }
        let mut s = DesktopState::default();
        feed(
            &mut s,
            &turn(json!({"turn": 1, "call": "run_checks a.py", "check_kind": "Tests"})),
        );
        assert_eq!(all(&s)["checks"], json!([]), "a missing verdict");

        let mut s = DesktopState::default();
        feed(
            &mut s,
            &turn(
                json!({"turn": 1, "call": "run_checks a.py", "verdict": "Some(true)",
                         "check_kind": "Tests"}),
            ),
        );
        assert_eq!(
            all(&s)["checks"],
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
    fn phase_is_model_with_its_turn_from_a_model_call_frame() {
        // The risk (#615): replies are not streamed, so a `model call` is
        // the only sign of life between two turns of a slow local model.
        // Dropped, the page shows nothing changing for the whole call and
        // the run reads as hung. The turn is trellis's number, carried as
        // sent; `prompt_bytes` is never on it (client_contract.json).
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p1"));
        assert_eq!(all(&s)["phase"], Value::Null, "nothing is on before a call");
        let c = feed(&mut s, &event("model call", json!({"turn": 3})));
        assert!(c.has(Pane::Phase), "{c:?}");
        assert_eq!(all(&s)["phase"], json!({"phase": "model", "turn": 3}));
        // Without a turn the event names nothing, and changes nothing.
        let c = feed(&mut s, &event("model call", json!({})));
        assert!(c.is_empty(), "{c:?}");
        assert_eq!(all(&s)["phase"], json!({"phase": "model", "turn": 3}));
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
                         "check_kind": "", "observation": "applied: true (+1 -1)"}),
            ),
        );
        assert!(c.has(Pane::Phase), "{c:?}");
        assert_eq!(
            all(&s)["phase"],
            json!({"phase": "tool", "turn": 1, "tool": "edit"})
        );
        feed(&mut s, &event("model call", json!({"turn": 2})));
        feed(
            &mut s,
            &turn(
                json!({"turn": 2, "call": "run_checks app/core.py", "verdict": "Some(true)",
                         "check_kind": "Tests", "observation": "3 passed"}),
            ),
        );
        assert_eq!(all(&s)["phase"], json!({"phase": "checks", "turn": 2}));
        // The governance audit is a run_checks the loop ran itself, under
        // the last call's number: checks too.
        feed(
            &mut s,
            &turn(
                json!({"turn": 2, "call": "run_checks", "verdict": "Some(false)",
                         "check_kind": "Governance", "observation": "VIOLATION"}),
            ),
        );
        assert_eq!(all(&s)["phase"], json!({"phase": "checks", "turn": 2}));
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
        assert_eq!(all(&s)["phase"], Value::Null);
        feed(&mut s, &event("model call", json!({"turn": 1})));
        let c = feed(&mut s, &run_start("p2"));
        assert!(c.has(Pane::Phase), "{c:?}");
        assert_eq!(all(&s)["phase"], Value::Null);
    }

    #[test]
    fn runtime_pane_carries_every_field_colibri_reported() {
        // The risk (#628): the self-report trellis sends for a colibri run
        // (client_contract.json's `runtime identified`) dropped on the way
        // to the page, or a count carried under another number.
        let mut s = DesktopState::default();
        feed(&mut s, &run_start("p1"));
        assert_eq!(all(&s)["runtime"], Value::Null, "nothing before the event");
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
            all(&s)["runtime"],
            json!({"runtime": "colibri", "served": "glm-5.2", "kv_slots": 16,
                   "tiers_vram": 120, "tiers_ram": 2400, "tiers_disk": 17000})
        );
        // Three counts and nothing derived from them (ADR 0048 §8).
        let mut ks = Vec::new();
        keys(&all(&s)["runtime"], &mut ks);
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
        assert_eq!(all(&s)["runtime"]["served"], "glm-5.2");
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
            all(&s)["runtime"],
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
        assert_eq!(all(&s)["runtime"]["kv_slots"], Value::Null);
        assert_eq!(all(&s)["runtime"]["tiers_vram"], Value::Null);
        let c = feed(&mut s, &run_start("p2"));
        assert!(c.has(Pane::Runtime), "{c:?}");
        assert_eq!(all(&s)["runtime"], Value::Null);
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
        let v = all(&s);
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
        assert_eq!(all(&s)["radius"], Value::Null);
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
        let v = all(&s);
        assert_eq!(
            v["budget"],
            json!({"turn": 1, "max_turns": 40, "prompt_bytes": 9800,
                   "completion_tokens": null, "model_ms": null, "queue_wait_ms": null,
                   "tokens_per_second": null, "queued_seconds": null})
        );

        // A scripted turn: no prompt was sent, so no size is reported.
        feed(
            &mut s,
            &turn(
                json!({"turn": 2, "call": "read_file a.py", "verdict": "None",
                         "check_kind": "", "observation": ""}),
            ),
        );
        let v = all(&s);
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
        all(&s)["budget"].clone()
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
        let v = all(&s);
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
        let before = all(&s)["transcript"].as_array().unwrap().len();

        let c = feed(&mut s, &run_start("p2"));
        for p in RUN_PANES {
            assert!(c.has(p), "{p:?} not sent");
        }
        let v = all(&s);
        assert_eq!(v["actions"], json!([]));
        assert_eq!(v["edits"], json!([]));
        assert_eq!(v["checks"], json!([]));
        assert_eq!(v["radius"], Value::Null);
        assert_eq!(
            v["budget"],
            json!({"turn": null, "max_turns": null, "prompt_bytes": null,
                   "completion_tokens": null, "model_ms": null, "queue_wait_ms": null,
                   "tokens_per_second": null, "queued_seconds": null})
        );
        assert_eq!(v["approvals"], json!([]));
        assert_eq!(v["outcome"], Value::Null);
        assert_eq!(v["running"], true);
        // The transcript runs on across runs, as the TUI's does.
        assert_eq!(v["transcript"].as_array().unwrap().len(), before + 1);
        assert_eq!(v["transcript"][before], "▸ change f");

        let c = s.clear();
        for p in PANES {
            assert!(c.has(p), "{p:?} not sent");
        }
        let v = all(&s);
        assert_eq!(v["transcript"], json!([]));
        assert_eq!(
            v["running"], true,
            "clearing the view does not end the run in flight"
        );
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
        let v = all(&s);
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
        let v = all(&s);
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
        let v = all(&s);
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
        assert_eq!(all(&s)["approvals"], json!([]));
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
        assert_eq!(c, Changed::of(&[Pane::Tier]));
        assert_eq!(all(&s)["tier"], "2, native tool calls");
        // A run's start does not clear it: only trellis's next answer does.
        feed(&mut s, &run_start("p"));
        assert_eq!(all(&s)["tier"], "2, native tool calls");
        // null is reported as null, not as the last tier or a guess.
        assert_eq!(
            feed(&mut s, &reply(Value::Null)),
            Changed::of(&[Pane::Tier])
        );
        assert_eq!(all(&s)["tier"], Value::Null);
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
        assert_eq!(all(&s)["tier"], "scripted");
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
        assert_eq!(all(&s)["atlas"]["history"]["error"], "unknown command");
        assert_eq!(all(&s)["alert"], Value::Null);
        // What `start` does for a new child.
        assert!(s.clear().has(Pane::Atlas));
        assert_eq!(all(&s)["atlas"], atlas::Atlas::default().to_json());
        assert_eq!(all(&s)["atlas"]["history"], Value::Null);
        // The old child's question is not the new one's.
        assert!(!feed(&mut s, &refusal).has(Pane::Atlas));
    }

    fn response(command: &str, success: bool, data: Value) -> String {
        let mut r = json!({"type": "response", "command": command, "success": success,
                           "id": "desktop-x"});
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
            all(&s)["undelivered"],
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
        assert_eq!(all(&s)["undelivered"].as_array().unwrap().len(), 2);
        // A refused abort, or an answer with no list, keeps it as it was.
        assert!(
            !feed(
                &mut s,
                &response("abort", false, json!("no run in progress"))
            )
            .has(Pane::Undelivered)
        );
        assert!(!feed(&mut s, &response("abort", true, json!({}))).has(Pane::Undelivered));
        assert_eq!(all(&s)["undelivered"].as_array().unwrap().len(), 2);
        // The next run starts with none.
        let c = feed(&mut s, &run_start("p2"));
        assert!(c.has(Pane::Undelivered));
        assert_eq!(all(&s)["undelivered"], json!([]));
    }

    #[test]
    fn get_commands_response_fills_the_starters() {
        // The risk (#301): a starter sends a template name trellis never
        // listed, or a repository's template text is drawn as markup; or a
        // refused get_commands leaves the last repository's starters up.
        let mut s = DesktopState::default();
        assert_eq!(all(&s)["starters"], Value::Null, "none before an answer");
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
            all(&s)["starters"],
            json!([
                {"name": "fix", "description": "Fix <i>it</i>"},
                {"name": "review", "description": null}
            ]),
            "the prompt templates only, by name, description as text"
        );
        // A run does not touch them: they are the repository's.
        feed(&mut s, &run_start("p"));
        assert_eq!(all(&s)["starters"].as_array().unwrap().len(), 2);
        // An answer with no prompts list offers none; a refused one too.
        feed(
            &mut s,
            &response("get_commands", true, json!({"commands": []})),
        );
        assert_eq!(all(&s)["starters"], Value::Null);
        feed(
            &mut s,
            &response("get_commands", true, json!({"prompts": []})),
        );
        assert_eq!(all(&s)["starters"], json!([]));
        let c = feed(
            &mut s,
            &response("get_commands", false, json!("manifest unreadable")),
        );
        assert!(c.has(Pane::Starters) && c.has(Pane::Transcript), "{c:?}");
        assert_eq!(all(&s)["starters"], Value::Null);
    }

    #[test]
    fn failed_responses_and_unparsed_lines_set_the_alert() {
        // The risk (#302): a refused command, a run that ended in error, or
        // a stdout line nothing could parse goes by with no sign on the
        // page, so the user waits on something that already failed.
        let mut s = DesktopState::default();
        assert_eq!(all(&s)["alert"], Value::Null, "nothing failed yet");
        let c = feed(
            &mut s,
            &response("set_model", false, json!("<b>bad</b> spec")),
        );
        assert!(c.has(Pane::Alert), "{c:?}");
        assert_eq!(all(&s)["alert"], "set_model refused: <b>bad</b> spec");
        // A success says nothing, and leaves the last failure up.
        assert!(
            !feed(&mut s, &response("get_state", true, json!({"tier": null}))).has(Pane::Alert)
        );
        assert_eq!(all(&s)["alert"], "set_model refused: <b>bad</b> spec");

        // A run's start is a fresh attempt: its failures are its own.
        let c = feed(&mut s, &run_start("p"));
        assert!(c.has(Pane::Alert));
        assert_eq!(all(&s)["alert"], Value::Null);
        let c = feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "error", "exit": 1,
                    "error": "a clean git worktree is required", "id": "p"})
            .to_string(),
        );
        assert!(c.has(Pane::Alert), "{c:?}");
        assert_eq!(
            all(&s)["alert"],
            "The run ended in error: a clean git worktree is required"
        );
        // A run that ended without an error raises none.
        feed(&mut s, &run_start("p2"));
        let c = feed(
            &mut s,
            &json!({"type": "run_end", "outcome": "converged", "exit": 0, "id": "p2"}).to_string(),
        );
        assert!(!c.has(Pane::Alert));
        assert_eq!(all(&s)["alert"], Value::Null);

        let c = feed(&mut s, "<script>not json</script>");
        assert!(c.has(Pane::Alert) && c.has(Pane::Transcript), "{c:?}");
        assert_eq!(
            all(&s)["alert"],
            "trellis rpc printed a line that is not a JSON object: <script>not json</script>"
        );
        // A frame of a type the desktop does not read is not a failure.
        feed(&mut s, &run_start("p3"));
        assert!(feed(&mut s, r#"{"type":"something_new"}"#).is_empty());
        assert_eq!(all(&s)["alert"], Value::Null);
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
                let v = all(&s);
                assert_eq!(v["transcript"], json!([]), "{command}");
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
        let v = all(&s);
        assert_eq!(v["stderr"], json!(["warning: <i>human</i>"]));
        assert_eq!(v["transcript"], json!([]), "not in the transcript");
        assert_eq!(v["alert"], Value::Null, "human text is not a failure");
        for i in 0..STDERR_CAP + 5 {
            s.update(Frame::Human(format!("line {i}")));
        }
        let lines = all(&s)["stderr"].as_array().unwrap().clone();
        assert_eq!(lines.len(), STDERR_CAP);
        assert_eq!(STDERR_CAP, 500);
        assert_eq!(lines[0], "line 5", "the oldest go first");
        // A run's start leaves it: it is the process's text, not the run's.
        feed(&mut s, &run_start("p"));
        assert_eq!(all(&s)["stderr"].as_array().unwrap().len(), STDERR_CAP);
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
        assert_eq!(all(&s)["model"], Value::Null, "shut at launch");
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
            all(&s)["models"],
            json!({"configured": "vllm:http://127.0.0.1:8000#<b>q</b>",
                   "local": ["vllm:http://127.0.0.1:11434#qwen3:8b"],
                   "probed": [{"runtime": "Ollama",
                               "address": "http://127.0.0.1:11434/api/tags",
                               "error": null}],
                   "providers": [{"name": "mine", "key_variable": "TRELLIS_TEST_VAR"}]}),
            "text as text, and nothing it did not send whole"
        );
        assert_eq!(all(&s)["model"], Value::Null, "a list is not a choice");
        // A refused choice, or an answer naming no spec, opens nothing.
        feed(
            &mut s,
            &response("set_model", false, json!("choose a model first")),
        );
        feed(
            &mut s,
            &response("set_model", true, json!({"configured": true})),
        );
        assert_eq!(all(&s)["model"], Value::Null);
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
            all(&s)["model"],
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
        assert_eq!(all(&s)["model"], Value::Null);
        assert_eq!(all(&s)["models"], Value::Null);
        feed(
            &mut s,
            &response(
                "set_model",
                true,
                json!({"model": "scripted:x", "configured": true, "saved_to": null}),
            ),
        );
        assert_eq!(
            all(&s)["model"],
            json!({"spec": "scripted:x", "configured": true, "saved_to": null,
                   "source": null}),
            "no configured answer to name its source by"
        );
        s.forget_models();
        assert!(!s.model_chosen());
        // A refused list says why, in place of the list.
        feed(&mut s, &response("list_models", false, json!("bad config")));
        assert_eq!(all(&s)["models"], json!({"error": "bad config"}));
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
        let source = |s: &DesktopState| all(s)["model"]["source"].clone();
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
            assert_eq!(source(&s), want, "{spec:?}");
        }
        // A spec set_model named is the session's, even the repository's,
        // which rpc then counts as the user's choice; the user's own spec
        // named is the user's.
        let mut s = DesktopState::default();
        feed(&mut s, &models(json!("r:x"), json!("u:y")));
        feed(&mut s, &chose("r:x", false));
        assert_eq!(source(&s), json!("session"));
        feed(&mut s, &chose("u:y", false));
        assert_eq!(source(&s), json!("user"));
        feed(&mut s, &chose("u:y ", false));
        assert_eq!(source(&s), json!("session"), "byte for byte");
    }

    #[test]
    fn picker_available_comes_only_from_the_core_s_own_models_answer() {
        // The risk (#306): the picker offers a spec the child never
        // reported as the user's, or a catalogue figure trellis did not
        // send, or keeps the last child's list for a new repository.
        let mut s = DesktopState::default();
        assert_eq!(all(&s)["available"], Value::Null);
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
        assert_eq!(all(&s)["available"], Value::Null);
        let c = feed(&mut s, &answer(MODELS_ID));
        assert!(c.has(Pane::Available), "{c:?}");
        assert_eq!(
            all(&s)["available"],
            json!({"configured": {"repository": "r:x", "user": "u:y", "session": null},
                   "catalog": [
                       {"name": "bonsai-2-27b", "license": "Apache-2.0", "download_gib": 5.3,
                        "default": true},
                       {"name": "<b>x</b>", "license": null, "download_gib": null,
                        "default": false}]})
        );
        // The child's exit, or a new child, empties it.
        s.child_exited();
        assert_eq!(all(&s)["available"], Value::Null);
        feed(&mut s, &answer(MODELS_ID));
        s.forget_models();
        assert_eq!(all(&s)["available"], Value::Null);
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
        assert_eq!(all(&s)["session_tokens"], Value::Null, "none asked yet");
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
            all(&s)["session_tokens"],
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
        let rows = all(&s)["session_tokens"]["models"].clone();
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
        let before = all(&s)["session_tokens"].clone();
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
        assert_eq!(all(&s)["session_tokens"], before);
        feed(
            &mut s,
            &session_stats(json!([{"model": "m", "prompt_tokens": 200,
                                    "completion_tokens": 6}])),
        );
        assert_eq!(all(&s)["session_tokens"]["models"][0]["prompt_tokens"], 200);
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
        assert_eq!(all(&s)["session_tokens"], Value::Null, "no rows");
        // A new child's view starts with none too.
        feed(
            &mut s,
            &session_stats(json!([{"model": "m", "prompt_tokens": 1,
                                    "completion_tokens": 1}])),
        );
        s.clear();
        assert_eq!(all(&s)["session_tokens"], Value::Null);
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
            all(&s)["session_tokens"],
            json!({"models": [], "runs_not_counted": null,
                   "error": "<i>trail</i> unreadable"})
        );
        // An answer this desktop cannot read says so, with no rows.
        feed(
            &mut s,
            &response("get_session_stats", true, json!({"x": 1})),
        );
        let v = all(&s)["session_tokens"].clone();
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
        assert_eq!(all(&s)["approvals"].as_array().unwrap().len(), 1);
        // Refused (trellis no longer has it): closed too.
        let gone = json!({"type": "response", "command": "approve", "success": false,
                          "id": "approval-2", "error": "no open approval request"})
        .to_string();
        assert!(feed(&mut s, &gone).has(Pane::Approvals));
        assert_eq!(all(&s)["approvals"], json!([]));
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
        assert_eq!(all(&s)["transcript"], json!(["<b>not json</b>"]));
        assert_eq!(feed(&mut s, "[1,2]"), text);
    }

    #[test]
    fn transcript_is_capped_at_the_tuis_bound() {
        // The risk: a long run grows the transcript without bound.
        let mut s = DesktopState::default();
        for i in 0..TRANSCRIPT_CAP + 5 {
            feed(&mut s, &format!("line {i}"));
        }
        let t = all(&s)["transcript"].as_array().unwrap().clone();
        assert_eq!(t.len(), TRANSCRIPT_CAP);
        assert_eq!(t[0], "line 5", "the oldest go first");
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
            "turn" | "resolved" | "candidates" | "unresolved" | "prompt_bytes" | "model_ms"
            | "completion_tokens" | "queue_wait_ms" | "max_iterations" | "after_step"
            | "version" | "exit" | "kv_slots" | "tiers_vram" | "tiers_ram" | "tiers_disk" => {
                *n += 1;
                json!(*n)
            }
            "data" => json!({"answer": format!("tok-{record}-{field}-end")}),
            _ => json!(format!("tok-{record}-{field}-end")),
        }
    }

    /// Fields whose value is a boolean on the wire.
    const BOOLEANS: [&str; 2] = ["success", "restored"];

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
        for (name, spec) in rpc["events"].as_object().expect("rpc.events") {
            let mut m = Map::new();
            for f in fields_of(spec) {
                let v = if BOOLEANS.contains(&f.as_str()) {
                    json!(true)
                } else {
                    sample(&f, name, spec, &mut n)
                };
                m.insert(f, v);
            }
            build.push((name.clone(), m, true));
        }
        for (ty, spec) in rpc["framing"].as_object().expect("rpc.framing") {
            let mut m = Map::new();
            for f in fields_of(spec) {
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
}
