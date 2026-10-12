//! State, and the one function allowed to change it.
//!
//! `update` is synchronous and does no IO — no file reads, no `.await`, no
//! subprocess. It is a pure state transition, which is what makes every test
//! in this file runnable without a terminal.

use crate::tui::message::{Message, SteerOut};
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind};

/// How many transcript lines are kept.
///
/// A run streams tokens for as long as it lasts; without a cap this grows
/// without bound on a long one.
const TRANSCRIPT_CAP: usize = 2000;

/// How many rows PgUp and PgDn move the transcript (#660). The arrows move
/// one, and the start of a capped run was hundreds of presses away.
const TRANSCRIPT_PAGE: u16 = 10;

/// How many `@` path candidates are offered at once.
///
/// The list is drawn under the input line, and every row it takes comes out
/// of the transcript. Ten is enough to pick from and small enough that a bare
/// `@` in a large repository does not push the transcript off the screen.
const AT_FILE_LIMIT: usize = 10;

#[derive(Debug, Default)]
pub struct AppState {
    /// What the status bar says about the model, set by whoever is driving
    /// runs. The view used to hardcode "mock", which was a lie in every
    /// binary except the demo.
    pub model: String,
    pub tier: String,
    /// What the user is typing. Submitted on Enter.
    pub input: String,
    /// Set by `update` when Enter is pressed, drained by the event loop,
    /// which is what keeps `update` free of IO: it records the intent, and
    /// something outside it does the work.
    pub submitted: Option<String>,
    /// Lines typed while a run is in flight, to be sent to it as steers
    /// (#115), with a [`SteerOut::TaskFollows`] wherever a task was
    /// submitted between them and a [`SteerOut::Stop`] where leaving was
    /// asked for mid-run (#634). Drained by the event loop like `submitted`.
    /// A `Vec`, in typing order, because one batch of keys can carry
    /// several Enters, and the order is what tells a driver which run a
    /// steer was typed for.
    pub steered: Vec<SteerOut>,
    /// The steers queued during the run in flight, in the order they were
    /// sent, and the turn each reached if it has. Private: the order is what
    /// `Message::SteerDelivered` names a steer by, and it holds only while
    /// nothing but `submit` and `RunFinished` changes this list.
    steers: Vec<QueuedSteer>,
    /// True between submitting a task and the run reporting finished. A
    /// second task is refused while it holds — one run at a time, because
    /// two runs sharing a worktree is the collision the whole design avoids —
    /// and a line typed then is a steer for this run instead.
    pub running: bool,
    pub transcript: Vec<String>,
    /// Whether the last transcript line was opened by a *stream* and may
    /// still be continued.
    ///
    /// False after a whole line is pushed at once — a task echo, a notice.
    /// Without it the first token of a reply lands on the end of the line
    /// that announced the task, and `> hello` plus `! cannot reach the
    /// runtime` render as `> hello! cannot reach the runtime`: two
    /// unrelated statements read as one sentence.
    line_open: bool,
    /// What the agent has done this run, in order. The action graph's data.
    pub actions: Vec<Action>,
    pub edits: Vec<Edit>,
    pub checks: Vec<Check>,
    pub radius: Radius,
    pub budget: Budget,
    pub modal: Option<Modal>,
    /// The answer to the modal, set by `update` and drained by the event
    /// loop, which sends it to whoever asked. The same shape as `submitted`,
    /// for the same reason: `update` records the decision and does no IO.
    pub answered: Option<bool>,
    /// Lines above the transcript's end that the view is scrolled back.
    /// 0 is following: new output stays in view (#244). Up moves back, Down
    /// returns, and reaching 0 follows again.
    pub scroll_back: u16,
    /// How the last run ended, until the next one starts.
    pub outcome: Option<String>,
    /// Whether the last run put the worktree back, as its outcome said;
    /// `None` until one says, and again once the next task starts (#636).
    pub restored: Option<bool>,
    pub should_quit: bool,
    /// Leaving was asked for while a run was in flight: the run has been
    /// asked to stop, and the view leaves once it reports finished (#634).
    leaving: bool,
    /// Repository paths `@` completion may offer, supplied by the driver.
    ///
    /// Filled from outside rather than read here: `update` does no IO, so
    /// the list is whatever the driver chose to hand over — for `trellis
    /// tui`, what git lists as tracked or untracked-and-not-ignored. An
    /// empty list means no completion, not an empty repository.
    pub paths: Vec<String>,
}

/// One thing the agent did.
///
/// No outcome field and no `passed`. The graph says what was done and whether
/// it wrote; whether it was *correct* is the checks pane's business, and only
/// an executed check can answer it (ADR 0004).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action {
    pub tool: String,
    pub target: String,
    /// Whether this action changed the working tree.
    pub wrote: bool,
}

#[derive(Debug, Clone)]
pub struct Edit {
    pub path: String,
    pub status: EditStatus,
}

#[derive(Debug, Clone)]
pub struct Check {
    pub name: String,
    pub passed: bool,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Radius {
    pub resolved: usize,
    pub candidates: usize,
    pub unresolved: usize,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Budget {
    /// Turns are counted from 1, so 0 is "no turn reported yet" and is
    /// drawn as that, never as "iteration 0" (#641).
    pub iteration: u32,
    /// The last turn's prompt size, `None` when the turn did not report
    /// one (#641). It was a plain integer, and an unreported size drew as
    /// "0 B prompt", which the desktop's never-0 rule forbids.
    pub prompt_bytes: Option<u64>,
    /// Seconds since the run began, from the last turn or the driver's
    /// clock (`Message::Elapsed`); `None` until a run has reported any.
    pub elapsed_secs: Option<u64>,
    /// The last turn's output tokens, model wall time and queue wait, each
    /// `None` when that turn did not report it (#257).
    pub completion_tokens: Option<u64>,
    pub model_ms: Option<u64>,
    pub queue_wait_ms: Option<u64>,
    /// The run's ceiling, 0 until a `BudgetKnown` says it.
    pub max_iterations: u32,
}

/// A steer sent to the run in flight, and the turn it reached.
#[derive(Debug, Clone)]
struct QueuedSteer {
    text: String,
    delivered: Option<usize>,
}

/// How far PgUp and PgDn move the approval modal. Ten of the twelve rows
/// the box has at 80×24, so a page keeps two lines of the last one.
const MODAL_PAGE: usize = 10;

#[derive(Debug, Clone)]
pub enum Modal {
    /// `scroll` is the first line of `diff` in view (#640). Held below the
    /// line count here; the view stops short of it once the rest fits, since
    /// how much fits is layout and the state holds none (as `scroll_back`).
    Approval {
        question: String,
        diff: String,
        scroll: usize,
    },
}

/// What the status bar says about the worktree (#636).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Worktree {
    /// Nothing this task wrote is in the tree.
    Clean,
    /// The run in flight has written this many times.
    Pending(usize),
    /// The last run put the tree back.
    Restored,
    /// The last run left this many files changed for the person to review:
    /// a converged run, or one whose ending did not say the tree was put
    /// back.
    LeftForReview(usize),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditStatus {
    Proposed,
    Applied,
    Rejected,
}

impl AppState {
    pub fn update(&mut self, msg: Message) {
        match msg {
            Message::Key(k) => self.handle_key(k),
            Message::Paste(text) => self.paste(&text),
            Message::AgentToken(t) => self.push_transcript(t),
            Message::EditProposed { path } => self.edits.push(Edit {
                path,
                status: EditStatus::Proposed,
            }),
            Message::CheckFinished { name, passed } => self.checks.push(Check { name, passed }),
            Message::TierKnown(t) => self.tier = t,
            Message::ActionTaken {
                tool,
                target,
                wrote,
            } => self.actions.push(Action {
                tool,
                target,
                wrote,
            }),
            Message::RadiusUpdated {
                resolved,
                candidates,
                unresolved,
            } => {
                self.radius = Radius {
                    resolved,
                    candidates,
                    unresolved,
                }
            }
            Message::BudgetUpdated {
                iteration,
                prompt_bytes,
                elapsed_secs,
                completion_tokens,
                model_ms,
                queue_wait_ms,
            } => {
                self.budget = Budget {
                    iteration,
                    prompt_bytes,
                    elapsed_secs: Some(elapsed_secs),
                    completion_tokens,
                    model_ms,
                    queue_wait_ms,
                    max_iterations: self.budget.max_iterations,
                }
            }
            // A new run: its ceiling, and nothing counted yet.
            Message::BudgetKnown { max_iterations } => {
                self.budget = Budget {
                    max_iterations,
                    ..Budget::default()
                }
            }
            // The driver's clock, sent every second while a run is in
            // flight so a long model call is seen to be passing (#641). One
            // that lands after the run ended is not applied: the run's time
            // stopped with it.
            Message::Elapsed(secs) => {
                if self.running {
                    self.budget.elapsed_secs = Some(secs);
                }
            }
            // Not while stopping: the driver drops a question once the run
            // has taken the stop path, so a modal opened now would ask
            // something nobody is waiting to hear.
            Message::ApprovalRequested { question, diff } if !self.leaving => {
                self.modal = Some(Modal::Approval {
                    question,
                    diff,
                    scroll: 0,
                })
            }
            Message::ApprovalRequested { .. } => {}
            Message::Quit => self.leave(),
            Message::SteerDelivered { nth, turn } => self.steer_delivered(nth, turn),
            Message::RunOutcome { line, restored } => {
                self.outcome = Some(line);
                self.restored = restored;
            }
            Message::RunFinished => self.run_finished(),
            Message::Tick | Message::Resize(..) => {}
        }
    }

    /// The worktree's state, from this task's writes and how its run ended.
    ///
    /// The edits pane holds one entry per write (`EditProposed` is sent only
    /// for a write that happened), so a run in flight counts writes and a
    /// finished one counts the files they touched.
    pub fn worktree(&self) -> Worktree {
        if self.running {
            return match self.edits.len() {
                0 => Worktree::Clean,
                n => Worktree::Pending(n),
            };
        }
        if self.restored == Some(true) {
            return Worktree::Restored;
        }
        let files: std::collections::BTreeSet<&str> =
            self.edits.iter().map(|e| e.path.as_str()).collect();
        match files.len() {
            0 => Worktree::Clean,
            n => Worktree::LeftForReview(n),
        }
    }

    /// Marks the `nth` steer of this run delivered, once.
    ///
    /// A repeat is ignored because the driver sends every delivery again
    /// before the run's end, so that one dropped under load still lands
    /// before `RunFinished` would call it not delivered. An `nth` past the
    /// end names no steer this view sent, and there is no line it could
    /// honestly print for one.
    fn steer_delivered(&mut self, nth: usize, turn: usize) {
        let Some(s) = self.steers.get_mut(nth) else {
            return;
        };
        if s.delivered.is_some() {
            return;
        }
        s.delivered = Some(turn);
        let line = format!("· delivered at turn {turn}: {}", s.text);
        self.push_line(line);
    }

    /// Hands input back, and says which steers never reached the model.
    ///
    /// A steer is only text the model was shown; whatever it says, it does
    /// not touch `checks` or `edits`, so nothing here can read as the run
    /// having converged on it (ADR 0004). What it does need is to not be
    /// lost: one the run never delivered is named, so the person knows to
    /// type it again as a task.
    fn run_finished(&mut self) {
        self.running = false;
        let undelivered: Vec<String> = self
            .steers
            .drain(..)
            .filter(|s| s.delivered.is_none())
            .map(|s| s.text)
            .collect();
        for text in undelivered {
            self.push_line(format!("· not delivered: {text}"));
        }
        // The stopped run has rolled back and written its trail; the leave
        // asked for during it happens now.
        if std::mem::take(&mut self.leaving) {
            self.should_quit = true;
        }
    }

    /// Ctrl-C, or Esc on an empty line: leave, stopping a run in flight
    /// first.
    ///
    /// Mid-run the view used to leave at once, and the run went on editing
    /// the tree with nobody watching while /help said it was stopped (#634).
    /// Now it asks the run to stop, which rolls it back, and leaves when the
    /// run reports finished. A second press asks nothing more: one stop per
    /// run.
    fn leave(&mut self) {
        if !self.running {
            self.should_quit = true;
            return;
        }
        if self.leaving {
            return;
        }
        self.leaving = true;
        // Withdrawn rather than answered: the driver drops the question once
        // the run has taken the stop path, and a dropped reply is a no (ADR
        // 0018). A key pressed on it now would answer what the run no longer
        // asks.
        self.modal = None;
        self.push_line("· stopping: trellis rolls the run back".into());
        self.steered.push(SteerOut::Stop);
    }

    /// Appends a token, keeping the transcript bounded.
    ///
    /// Tokens arrive mid-line, so a token containing a newline starts a new
    /// line and anything else extends the last one. Pushing every token as
    /// its own entry would render one word per line.
    fn push_transcript(&mut self, t: String) {
        if self.transcript.is_empty() || !self.line_open {
            self.transcript.push(String::new());
            self.line_open = true;
        }
        for (i, part) in t.split('\n').enumerate() {
            if i > 0 {
                self.transcript.push(String::new());
            }
            if let Some(last) = self.transcript.last_mut() {
                last.push_str(part);
            }
        }
        self.cap_transcript();
    }

    /// Drops the oldest lines past `TRANSCRIPT_CAP`. One place, so both
    /// ways into the transcript keep the same bound.
    fn cap_transcript(&mut self) {
        if self.transcript.len() > TRANSCRIPT_CAP {
            self.transcript
                .drain(0..self.transcript.len() - TRANSCRIPT_CAP);
        }
    }

    /// A state labelled with the model and tier actually in play.
    ///
    /// A constructor rather than `..Default::default()` at the call site,
    /// because `line_open` is an invariant of the transcript and not a
    /// setting: it is maintained by `push_line` and `push_transcript`, and
    /// a caller that set it by hand could make a task echo continuable
    /// again — which is the bug this pair exists to close.
    pub fn with_model(model: String, tier: String) -> Self {
        Self {
            model,
            tier,
            ..Default::default()
        }
    }

    /// Marks a run as in flight.
    ///
    /// A builder rather than a field assignment at the call site: once
    /// `line_open` became private, `..Default::default()` stopped working
    /// across the crate boundary, and every other field a driver sets has
    /// one of these. Setting `running` by hand still works inside this
    /// crate; this exists for the ones outside it.
    #[must_use]
    pub fn with_running(mut self, running: bool) -> Self {
        self.running = running;
        self
    }

    /// The paths `@` completion may offer.
    #[must_use]
    pub fn with_paths(mut self, paths: Vec<String>) -> Self {
        self.paths = paths;
        self
    }

    /// The `@fragment` being typed, without its `@`, if there is one.
    ///
    /// Only the word at the very end of the line: Tab replaces it, so a
    /// fragment the cursor has moved past (the line ends in a space) is
    /// finished and is not offered again.
    fn at_fragment(&self) -> Option<&str> {
        self.input
            .rsplit(char::is_whitespace)
            .next()
            .and_then(|w| w.strip_prefix('@'))
    }

    /// The paths offered for the `@fragment` at the end of the input.
    ///
    /// A path matches when it contains the fragment, ignoring case. Shortest
    /// first, because the fragment is most often the tail of a name and the
    /// shortest match is the least nested one; then lexicographic, so the
    /// order does not depend on the order the driver listed them in.
    pub fn at_file_candidates(&self) -> Vec<&str> {
        let Some(fragment) = self.at_fragment() else {
            return Vec::new();
        };
        let needle = fragment.to_lowercase();
        let mut hits: Vec<&str> = self
            .paths
            .iter()
            .map(String::as_str)
            .filter(|p| p.to_lowercase().contains(&needle))
            .collect();
        hits.sort_by(|a, b| a.len().cmp(&b.len()).then_with(|| a.cmp(b)));
        hits.truncate(AT_FILE_LIMIT);
        hits
    }

    /// Replaces the `@fragment` with the first candidate and a space.
    ///
    /// With nothing offered the line is left exactly as typed: a Tab that
    /// deleted the fragment, or inserted a literal tab, would change a task
    /// the user can no longer see the reason for.
    fn complete_at_file(&mut self) {
        let Some(first) = self.at_file_candidates().first().map(|p| p.to_string()) else {
            return;
        };
        let Some(fragment_len) = self.at_fragment().map(str::len) else {
            return;
        };
        // The fragment is a suffix of `input` and `@` is one byte, so the
        // cut lands on a char boundary.
        let keep = self.input.len() - fragment_len - 1;
        self.input.truncate(keep);
        self.input.push_str(&first);
        self.input.push(' ');
    }

    /// A complete line, which the next streamed token may not continue.
    pub fn push_line(&mut self, line: String) {
        self.transcript.push(line);
        self.line_open = false;
        self.cap_transcript();
    }

    /// The `scroll_back` that shows the transcript's first row (#660).
    ///
    /// Every row the transcript takes at the narrowest pane the view draws,
    /// where it wraps to the most, so at any size it reaches the top and the
    /// view stops there. State holds no layout (#244), so this overshoots by
    /// the pane's own height, and by the rows a wider pane saves; the first
    /// PgDn after it can land on the same page.
    fn transcript_top(&self) -> u16 {
        crate::tui::view::wrapped_rows(
            &self.transcript.join("\n"),
            crate::tui::view::MIN_TRANSCRIPT_WIDTH,
        )
    }

    /// Only `Press`.
    ///
    /// Windows also reports `Release` and `Repeat`, and without this filter
    /// every keystroke fires twice — which looks like a scroll that jumps
    /// two lines and a `q` that quits before you see it register.
    fn handle_key(&mut self, k: KeyEvent) {
        if k.kind != KeyEventKind::Press {
            return;
        }
        // The modal is answered first: while it is up, the keys underneath
        // it are not the ones on screen.
        //
        // The answer touches no edit (#635). The loop asks before a call
        // runs and reports an edit after it has written, so every row in
        // `edits` is already in the tree when the question arrives, and the
        // answer is to the *next* call. Marking rows applied or rejected
        // here painted that answer onto files that were written either way;
        // the decline itself is a transcript line the driver writes.
        if self.modal.is_some() {
            match k.code {
                KeyCode::Char('y') => {
                    self.modal = None;
                    self.answered = Some(true);
                }
                KeyCode::Char('n') => {
                    self.modal = None;
                    self.answered = Some(false);
                }
                // Scrolling, so a long diff is read whole before it is
                // answered (#640). Every other key is still ignored.
                KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown => {
                    if let Some(Modal::Approval { diff, scroll, .. }) = &mut self.modal {
                        let last = diff.lines().count().saturating_sub(1);
                        *scroll = match k.code {
                            KeyCode::Up => scroll.saturating_sub(1),
                            KeyCode::Down => scroll.saturating_add(1),
                            KeyCode::PageUp => scroll.saturating_sub(MODAL_PAGE),
                            _ => scroll.saturating_add(MODAL_PAGE),
                        }
                        .min(last);
                    }
                }
                _ => {}
            }
            return;
        }
        match k.code {
            // Esc on an empty line leaves; Esc with text clears it. `q` is a
            // letter now that there is somewhere to type, so it cannot also
            // be the way out — Ctrl-C does that too, handled in the loop.
            // Both stop a run in flight before leaving (#634).
            KeyCode::Esc if self.input.is_empty() => self.leave(),
            KeyCode::Esc => self.input.clear(),
            KeyCode::Enter => self.submit(),
            KeyCode::Tab => self.complete_at_file(),
            KeyCode::Backspace => {
                self.input.pop();
            }
            KeyCode::Char(c) => self.input.push(c),
            KeyCode::Up => self.scroll_back = self.scroll_back.saturating_add(1),
            KeyCode::Down => self.scroll_back = self.scroll_back.saturating_sub(1),
            // Held at the top, so PgDn after it moves at once rather than
            // first spending presses on rows above the first.
            KeyCode::PageUp => {
                self.scroll_back = self
                    .scroll_back
                    .saturating_add(TRANSCRIPT_PAGE)
                    .min(self.transcript_top())
            }
            KeyCode::PageDown => {
                self.scroll_back = self.scroll_back.saturating_sub(TRANSCRIPT_PAGE)
            }
            KeyCode::Home => self.scroll_back = self.transcript_top(),
            KeyCode::End => self.scroll_back = 0,
            _ => {}
        }
    }

    /// Appends a paste to the input, line breaks as spaces (#639).
    ///
    /// Never submits: a paste's line breaks are not Enters. Fed through the
    /// key path a multi-line task started a run on its first line and
    /// queued the rest as steers — a snapshot, a model call and a trail for
    /// a task the user had not finished entering. Ignored while the modal
    /// is up, as typing is.
    fn paste(&mut self, text: &str) {
        if self.modal.is_some() {
            return;
        }
        let flat = text.replace("\r\n", " ").replace(['\r', '\n'], " ");
        self.input.push_str(&flat);
    }

    /// Hands the typed line to whatever is driving runs.
    ///
    /// While one is in flight, a line is a steer for it rather than a second
    /// task (ADR 0027, #115): a queued task would start against a worktree
    /// the first is still editing, and a steer only adds text to the next
    /// request of the run already going. A `/` command is still refused
    /// then, as it was before steering existed; it is addressed to this
    /// view, and sending it to the model would be a surprise either way.
    fn submit(&mut self) {
        let task = self.input.trim().to_string();
        if task.is_empty() {
            return;
        }
        if self.running {
            if task.starts_with('/') {
                self.push_line("· a run is already in flight; wait for it to finish".into());
                return;
            }
            self.input.clear();
            self.push_line(format!("· queued: {task}"));
            self.steers.push(QueuedSteer {
                text: task.clone(),
                delivered: None,
            });
            self.steered.push(SteerOut::Steer(task));
            return;
        }
        self.input.clear();

        // Not everything typed here is a task, and treating it as one is
        // expensive: a task takes a snapshot, drives the model and writes a
        // trail. Reported from a real session, where `trellis model` was
        // typed at this prompt and became an agent run that failed against
        // a dead runtime — the user was looking for the command surface and
        // got the loop instead, twice.
        // `/clear` is the one that mutates, so it is handled here rather
        // than by `local_command`, which only reads.
        if task == "/clear" {
            self.transcript.clear();
            self.line_open = false;
            self.push_line("· transcript cleared".into());
            return;
        }
        if let Some(reply) = self.local_command(&task) {
            self.push_line(format!("> {task}"));
            for line in reply {
                self.push_line(line);
            }
            return;
        }

        self.running = true;
        self.outcome = None;
        self.restored = None;
        // The panes describe this task. Kept from earlier ones, the bar read
        // "N edit(s) pending" at the start of every later run, when a run
        // starts only from a clean tree, and listed files a rollback had
        // already put back (#636).
        self.actions.clear();
        self.edits.clear();
        self.checks.clear();
        self.radius = Radius::default();
        // A new run is followed from its start, wherever the view was.
        self.scroll_back = 0;
        self.push_line(format!("> {task}"));
        self.steered.push(SteerOut::TaskFollows);
        self.submitted = Some(task);
    }

    /// Input this interface can answer itself, without a run.
    ///
    /// Pure: every answer is built from state already held, so `update`
    /// stays free of IO. Anything needing the filesystem or a model is not
    /// here — it is a task, or it belongs to `trellis session`.
    fn local_command(&self, input: &str) -> Option<Vec<String>> {
        // A CLI invocation typed at a prompt that is not a shell. Caught by
        // name rather than by guessing: `trellis` is the one word a user
        // reaches for here, having just come from the command line.
        if let Some(rest) = input.strip_prefix("trellis ") {
            let sub = rest.split_whitespace().next().unwrap_or("");
            return Some(vec![
                format!("· `{input}` is a shell command, and this is not a shell."),
                format!(
                    "  Leave with esc and run it, or use /{sub} if this view has one — /help lists what it does."
                ),
            ]);
        }

        let mut parts = input.strip_prefix('/')?.split_whitespace();
        let name = parts.next().unwrap_or("");

        // `/codebase` reads the index off disk, and this function is pure by
        // design — that is what keeps `update` free of IO. Declining it sends
        // it on to the driver, which does the read and streams the graph back
        // as transcript lines.
        if name == "codebase" {
            return None;
        }

        Some(match name {
            "help" | "?" => vec![
                "· this view runs one task at a time against the model in the status bar.".into(),
                "  /model    what is driving runs".into(),
                "  /graph    the blast radius, and what it does not claim".into(),
                "  /codebase which modules call which, and how much is unbound".into(),
                "  /clear    empty the transcript".into(),
                "  /help     this".into(),
                "  @<text>   repository paths containing it; tab inserts the first".into(),
                "  esc       clear the line; again to leave".into(),
                "  ^c        leave; a run in flight is stopped and rolled back first".into(),
                "  enter during a run queues the line as a steer for its next turn".into(),
                "  ↑ ↓       scroll the transcript".into(),
                "  PgUp PgDn ten lines at a time; Home to the start, End back to following".into(),
                String::new(),
                "  Anything else is a task: it snapshots the worktree, drives the".into(),
                "  model, runs the repository's own checks and rolls back if it".into(),
                "  does not converge.".into(),
                String::new(),
                "  For the model catalogue, weights and config, leave and use".into(),
                "  `trellis model` or `trellis session`.".into(),
            ],
            "graph" | "radius" => {
                let r = self.radius;
                let known = r.resolved + r.candidates + r.unresolved;
                let mut out = vec![
                    format!(
                        "· reach: {} resolved · {} candidates · {} unresolved",
                        r.resolved, r.candidates, r.unresolved
                    ),
                    "  drawn in the radius pane on the right, as converging edges.".into(),
                ];
                if known == 0 {
                    out.push("  nothing yet — a task that calls blast_radius fills it.".into());
                }
                // Never a ratio between the three: they are filled from
                // different populations, and dividing them printed "3.7%
                // resolved" for a resolver audited at 72% (ADR 0014).
                out.push(
                    "  three counts, never a ratio: they come from different populations.".into(),
                );
                out.push("  resolved is a floor on reach, not a boundary (Constraint 1).".into());
                out
            }
            "model" => vec![
                format!("· model  {}", self.model),
                format!("  tier   {}", self.tier),
                "  `trellis model` outside this view lists the weights you can fetch.".into(),
            ],
            other => vec![format!(
                "· `/{other}` is not a command here. /help lists them."
            )],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEventState, KeyModifiers};

    fn key(code: KeyCode, kind: KeyEventKind) -> Message {
        Message::Key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind,
            state: KeyEventState::NONE,
        })
    }

    fn press(code: KeyCode) -> Message {
        key(code, KeyEventKind::Press)
    }

    #[test]
    fn esc_on_an_empty_line_quits() {
        // Was `q`. Task entry took the letters, so the exit moved to Esc —
        // and to Ctrl-C, which the event loop turns into `Quit` before this
        // ever sees it.
        let mut s = AppState::default();
        assert!(!s.should_quit);
        s.update(press(KeyCode::Esc));
        assert!(s.should_quit);
    }

    #[test]
    fn quit_is_still_reachable_as_a_message() {
        // Ctrl-C arrives this way rather than as a key.
        let mut s = AppState::default();
        s.update(Message::Quit);
        assert!(s.should_quit);
    }

    #[test]
    fn a_release_event_changes_nothing() {
        // Windows reports Release and Repeat alongside Press. Without the
        // filter every keystroke is handled twice.
        let mut s = AppState::default();
        s.update(key(KeyCode::Char('q'), KeyEventKind::Release));
        assert!(!s.should_quit, "a key release must not quit");

        s.update(key(KeyCode::Up, KeyEventKind::Repeat));
        assert_eq!(s.scroll_back, 0, "a repeat must not scroll");
    }

    #[test]
    fn the_transcript_is_capped() {
        let mut s = AppState::default();
        for i in 0..TRANSCRIPT_CAP + 500 {
            s.update(Message::AgentToken(format!("line {i}\n")));
        }
        assert_eq!(
            s.transcript.len(),
            TRANSCRIPT_CAP,
            "an unbounded transcript grows for as long as a run lasts"
        );
        // The oldest were dropped, not the newest. 2500 tokens make 2501
        // entries (the trailing newline opens an empty one), so 501 go.
        assert_eq!(s.transcript[0], "line 501");
        assert_eq!(s.transcript[TRANSCRIPT_CAP - 2], "line 2499");
    }

    #[test]
    fn a_shell_command_typed_here_does_not_become_an_agent_run() {
        // Reported from a real session: `trellis model` was typed at this
        // prompt, became a task, and failed against a dead runtime — the
        // user was looking for the command surface and got the loop. A task
        // snapshots the worktree, drives the model and writes a trail, so
        // starting one by mistake is not free.
        let mut s = AppState {
            input: "trellis model".into(),
            ..Default::default()
        };
        s.update(key(KeyCode::Enter, KeyEventKind::Press));

        assert!(s.submitted.is_none(), "no run may be started");
        assert!(!s.running);
        let text = s.transcript.join(
            "
",
        );
        assert!(text.contains("not a shell"), "{text}");
        assert!(
            text.contains("/help"),
            "and it must say where to look: {text}"
        );
    }

    #[test]
    fn codebase_is_handed_to_the_driver_rather_than_answered_or_refused() {
        // It needs the index off disk, and this module is pure. Two wrong
        // answers were available: refuse it as an unknown command, which is
        // what every other unrecognised slash gets, or invent a reply from
        // state that does not hold the graph.
        let mut s = AppState {
            input: "/codebase".into(),
            ..Default::default()
        };
        s.update(key(KeyCode::Enter, KeyEventKind::Press));

        assert_eq!(
            s.submitted.as_deref(),
            Some("/codebase"),
            "it has to reach the driver, which can read the index"
        );
        let text = s.transcript.join("\n");
        assert!(
            !text.contains("not a command here"),
            "and must not be refused on the way: {text}"
        );
    }

    #[test]
    fn codebase_is_listed_in_help_beside_the_command_it_is_not() {
        let mut s = AppState {
            input: "/help".into(),
            ..Default::default()
        };
        s.update(key(KeyCode::Enter, KeyEventKind::Press));
        let text = s.transcript.join("\n");
        assert!(text.contains("/codebase"), "{text}");
        assert!(
            text.contains("/graph"),
            "the two are different graphs and both are listed: {text}"
        );
    }

    #[test]
    fn graph_is_a_command_and_reports_the_reach_without_a_run() {
        // Typed at this prompt in a real session and answered "`/graph` is
        // not a command here" — in the one interface that has a call graph
        // behind it. The radius pane was three bare numbers.
        let mut s = AppState {
            radius: Radius {
                resolved: 47,
                candidates: 3,
                unresolved: 2,
            },
            input: "/graph".into(),
            ..Default::default()
        };
        s.update(key(KeyCode::Enter, KeyEventKind::Press));

        assert!(s.submitted.is_none(), "answering must not cost a run");
        let text = s.transcript.join(
            "
",
        );
        assert!(text.contains("47"), "{text}");
        assert!(
            text.contains("radius pane"),
            "it must say where to look:
{text}"
        );
        assert!(
            text.contains("never a ratio"),
            "the three counts come from different populations, and saying so              is the reason this pane exists:
{text}"
        );
        assert!(
            text.contains("floor"),
            "Constraint 1: resolved is a floor on reach, not a boundary:
{text}"
        );
    }

    #[test]
    fn graph_appears_in_help() {
        // A command nobody can discover is not a command.
        let mut s = AppState {
            input: "/help".into(),
            ..Default::default()
        };
        s.update(key(KeyCode::Enter, KeyEventKind::Press));
        assert!(
            s.transcript
                .join(
                    "
"
                )
                .contains("/graph")
        );
    }

    #[test]
    fn help_and_model_are_answered_without_a_run() {
        let mut s = AppState::with_model("qwen3:8b".into(), "2".into());
        s.input = "/model".into();
        s.update(key(KeyCode::Enter, KeyEventKind::Press));
        assert!(s.submitted.is_none());
        let text = s.transcript.join(
            "
",
        );
        assert!(text.contains("qwen3:8b"), "{text}");

        s.input = "/help".into();
        s.update(key(KeyCode::Enter, KeyEventKind::Press));
        assert!(s.submitted.is_none());
        let text = s.transcript.join(
            "
",
        );
        assert!(text.contains("/model"), "{text}");
        assert!(
            text.contains("rolls back"),
            "help must say what a task actually costs: {text}"
        );
    }

    #[test]
    fn an_unknown_slash_command_is_refused_rather_than_run() {
        let mut s = AppState {
            input: "/nosuch".into(),
            ..Default::default()
        };
        s.update(key(KeyCode::Enter, KeyEventKind::Press));
        assert!(s.submitted.is_none(), "a typo must not become a turn");
        assert!(
            s.transcript
                .join(
                    "
"
                )
                .contains("not a command here")
        );
    }

    #[test]
    fn clear_empties_the_transcript_and_says_so() {
        let mut s = AppState::default();
        s.update(Message::AgentToken(
            "noise
"
            .into(),
        ));
        s.input = "/clear".into();
        s.update(key(KeyCode::Enter, KeyEventKind::Press));
        assert!(
            !s.transcript
                .join(
                    "
"
                )
                .contains("noise")
        );
        assert!(
            s.transcript
                .join(
                    "
"
                )
                .contains("cleared")
        );
        // And the cleared transcript is not left open for a stream to
        // continue the notice line.
        s.update(Message::AgentToken(
            "next
"
            .into(),
        ));
        assert_eq!(s.transcript[0], "· transcript cleared");
    }

    #[test]
    fn an_ordinary_task_is_still_a_task() {
        // The guard must not swallow real input. Anything that is not a
        // slash command or a `trellis` invocation goes to the model.
        let mut s = AppState {
            input: "rename helper to assist".into(),
            ..Default::default()
        };
        s.update(key(KeyCode::Enter, KeyEventKind::Press));
        assert_eq!(s.submitted.as_deref(), Some("rename helper to assist"));
        assert!(s.running);
    }

    #[test]
    fn a_reply_starts_its_own_line_and_does_not_land_on_the_task_echo() {
        // Reported from a real session. A run against a dead runtime
        // rendered as:
        //
        //     > hello! cannot reach the runtime at 127.0.0.1:8793
        //
        // Two unrelated statements — what the user asked, and why it
        // failed — read as one sentence, and the `!` that marks a failure
        // looked like an exclamation mark.
        //
        // The cause is that streamed tokens continue the last transcript
        // line, and the task echo was pushed as a bare line, so the first
        // token of the reply continued *it*.
        let mut s = AppState {
            input: "hello".into(),
            ..Default::default()
        };
        s.update(key(KeyCode::Enter, KeyEventKind::Press));
        s.update(Message::AgentToken(
            "! cannot reach the runtime
"
            .into(),
        ));

        assert_eq!(s.transcript[0], "> hello");
        assert_eq!(s.transcript[1], "! cannot reach the runtime");
    }

    #[test]
    fn a_second_task_also_gets_its_own_line() {
        // The same defect repeats per turn, and the screenshot showed it
        // twice. Asserted separately because the first task is the one that
        // opens an empty transcript, and the second is the one that follows
        // a line a stream left open.
        let mut s = AppState {
            input: "first".into(),
            ..Default::default()
        };
        s.update(key(KeyCode::Enter, KeyEventKind::Press));
        s.update(Message::AgentToken(
            "reply one
"
            .into(),
        ));
        s.update(Message::RunFinished);
        s.input = "second".into();
        s.update(key(KeyCode::Enter, KeyEventKind::Press));

        assert!(
            s.transcript.iter().any(|l| l == "> second"),
            "got {:?}",
            s.transcript
        );
    }

    #[test]
    fn a_multi_line_failure_keeps_every_line_marked() {
        // The screenshot also showed the first line of the error losing its
        // `!` into the task line while the rest kept theirs — so the block
        // read as one marked paragraph with an unmarked opening.
        let mut s = AppState {
            input: "hello".into(),
            ..Default::default()
        };
        s.update(key(KeyCode::Enter, KeyEventKind::Press));
        for line in [
            "! one
", "! two
", "! three
",
        ] {
            s.update(Message::AgentToken(line.into()));
        }
        let marked = s.transcript.iter().filter(|l| l.starts_with("! ")).count();
        assert_eq!(marked, 3, "got {:?}", s.transcript);
    }

    #[test]
    fn tokens_join_into_lines_rather_than_one_line_each() {
        let mut s = AppState::default();
        s.update(Message::AgentToken("Reading ".into()));
        s.update(Message::AgentToken("blast ".into()));
        s.update(Message::AgentToken("radius".into()));
        assert_eq!(s.transcript, vec!["Reading blast radius".to_string()]);

        s.update(Message::AgentToken("\n".into()));
        s.update(Message::AgentToken("next".into()));
        assert_eq!(s.transcript.len(), 2);
        assert_eq!(s.transcript[1], "next");
    }

    #[test]
    fn y_accepts_the_modal_and_n_rejects_it() {
        let mut s = AppState::default();
        s.update(Message::EditProposed {
            path: "a.rs".into(),
        });
        s.update(Message::ApprovalRequested {
            question: "edit a.py (+1 -1)".into(),
            diff: "--- a\n+++ b".into(),
        });
        assert!(s.modal.is_some());

        s.update(press(KeyCode::Char('y')));
        assert!(s.modal.is_none(), "y must clear the modal");
        assert_eq!(
            s.answered,
            Some(true),
            "and the answer is kept for the asker"
        );

        let mut s = AppState::default();
        s.update(Message::EditProposed {
            path: "a.rs".into(),
        });
        s.update(Message::ApprovalRequested {
            question: "edit a.py (+1 -1)".into(),
            diff: String::new(),
        });
        s.update(press(KeyCode::Char('n')));
        assert!(s.modal.is_none(), "n must clear the modal");
        assert_eq!(s.answered, Some(false));
    }

    #[test]
    fn approval_answer_keeps_edits_as_reported_on_n() {
        // #635: the bridge reports an edit after it has written and the loop
        // asks before the next call, so `a.py` is in the tree whatever the
        // answer. Marking it rejected told an approve-mode user a written
        // file was not there to review.
        let mut s = AppState::default();
        s.update(Message::EditProposed {
            path: "a.py".into(),
        });
        s.update(Message::ApprovalRequested {
            question: "edit a.py (+1 -1)".into(),
            diff: "apply the edit?".into(),
        });
        s.update(press(KeyCode::Char('n')));
        assert_eq!(s.edits.len(), 1);
        assert_eq!(s.edits[0].path, "a.py");
        assert_eq!(s.edits[0].status, EditStatus::Proposed);
        assert_eq!(s.answered, Some(false));
    }

    #[test]
    fn approval_answer_keeps_edits_as_reported_on_y() {
        // The same for `y`: an approval is an answer about the next call,
        // not a claim that the rows already shown were applied by it.
        let mut s = AppState::default();
        s.update(Message::EditProposed {
            path: "a.py".into(),
        });
        s.update(Message::ApprovalRequested {
            question: "edit a.py (+1 -1)".into(),
            diff: "apply the edit?".into(),
        });
        s.update(press(KeyCode::Char('y')));
        assert_eq!(s.edits[0].status, EditStatus::Proposed);
        assert_eq!(s.answered, Some(true));
    }

    #[test]
    fn budget_unreported_elapsed_follows_the_drivers_clock_while_running() {
        // #641: between turns nothing on screen moved, and QA read a long
        // model call as a hang. The driver's tick keeps elapsed current;
        // one arriving after the run ended does not restart its clock.
        let mut s = AppState::default().with_running(true);
        assert_eq!(s.budget.elapsed_secs, None, "nothing reported yet");
        s.update(Message::Elapsed(75));
        assert_eq!(s.budget.elapsed_secs, Some(75));
        s.update(Message::RunFinished);
        s.update(Message::Elapsed(90));
        assert_eq!(s.budget.elapsed_secs, Some(75));
    }

    #[test]
    fn budget_unreported_prompt_size_stays_none_through_a_turn_without_one() {
        // A turn that did not report its prompt size is not a 0-byte prompt.
        let mut s = AppState::default();
        s.update(Message::BudgetUpdated {
            iteration: 1,
            prompt_bytes: None,
            elapsed_secs: 4,
            completion_tokens: None,
            model_ms: None,
            queue_wait_ms: None,
        });
        assert_eq!(s.budget.prompt_bytes, None);
        assert_eq!(s.budget.elapsed_secs, Some(4));
    }

    #[test]
    fn approval_modal_scroll_stays_inside_the_diff_and_other_keys_do_nothing() {
        let mut s = AppState::default();
        s.update(Message::ApprovalRequested {
            question: "edit a.py (+3 -0)".into(),
            diff: "+ a\n+ b\n+ c".into(),
        });
        let scroll = |s: &AppState| match &s.modal {
            Some(Modal::Approval { scroll, .. }) => *scroll,
            None => panic!("the modal closed"),
        };
        s.update(press(KeyCode::Up));
        s.update(press(KeyCode::PageUp));
        assert_eq!(scroll(&s), 0, "nothing above the first line");
        s.update(press(KeyCode::PageDown));
        s.update(press(KeyCode::Down));
        assert_eq!(scroll(&s), 2, "the last line is as far as it goes");
        for code in [KeyCode::Char('q'), KeyCode::Esc, KeyCode::Enter] {
            s.update(press(code));
        }
        assert!(!s.should_quit, "the keys on screen are the modal's");
        assert_eq!(s.answered, None, "and only y or n answers it");
        assert_eq!(scroll(&s), 2);
    }

    #[test]
    fn while_the_modal_is_up_q_does_not_quit() {
        // The keys on screen are the modal's. Quitting out from under a
        // pending approval would discard the decision silently.
        let mut s = AppState::default();
        s.update(Message::ApprovalRequested {
            question: "edit a.py (+1 -1)".into(),
            diff: String::new(),
        });
        s.update(press(KeyCode::Char('q')));
        assert!(!s.should_quit);
        assert!(s.modal.is_some());
    }

    /// A transcript of `n` numbered lines, following its end.
    fn numbered(n: usize) -> AppState {
        let mut s = AppState::default();
        for i in 0..n {
            s.push_line(format!("line {i:03}"));
        }
        s
    }

    /// The transcript as drawn at 80×24, one string.
    fn drawn(s: &AppState) -> String {
        let mut t = ratatui::Terminal::new(ratatui::backend::TestBackend::new(80, 24)).unwrap();
        t.draw(|f| crate::tui::view::render(s, f)).unwrap();
        t.backend()
            .buffer()
            .content()
            .iter()
            .map(ratatui::buffer::Cell::symbol)
            .collect()
    }

    #[test]
    fn page_keys_page_up_moves_ten_rows_back() {
        // #660: the arrows move one row, and the start of a long run was
        // hundreds of presses away.
        let mut s = numbered(200);
        s.update(press(KeyCode::PageUp));
        assert_eq!(s.scroll_back, 10);
        s.update(press(KeyCode::PageUp));
        assert_eq!(s.scroll_back, 20);
        assert!(drawn(&s).contains("line 179"), "twenty rows above the end");
    }

    #[test]
    fn page_keys_page_down_moves_ten_rows_forward_and_stops_at_the_end() {
        let mut s = numbered(200);
        s.scroll_back = 25;
        s.update(press(KeyCode::PageDown));
        assert_eq!(s.scroll_back, 15);
        s.update(press(KeyCode::PageDown));
        s.update(press(KeyCode::PageDown));
        assert_eq!(s.scroll_back, 0, "saturating at the end: following again");
    }

    #[test]
    fn page_keys_home_shows_the_first_line_and_page_up_stops_there() {
        // Lines long enough to wrap in the pane, so a count of lines alone
        // would stop short of the top.
        let mut s = AppState::default();
        for i in 0..120 {
            s.push_line(format!("line {i:03} {}", "wrapped words ".repeat(5)));
        }
        assert!(!drawn(&s).contains("line 000"), "it starts at the end");
        s.update(press(KeyCode::Home));
        assert!(drawn(&s).contains("line 000"), "{}", drawn(&s));
        let top = s.scroll_back;
        s.update(press(KeyCode::PageUp));
        assert_eq!(s.scroll_back, top, "nothing above the top to page to");
    }

    #[test]
    fn page_keys_end_follows_the_end_again() {
        let mut s = numbered(200);
        s.update(press(KeyCode::Home));
        assert!(s.scroll_back > 0);
        s.update(press(KeyCode::End));
        assert_eq!(s.scroll_back, 0);
        // Following: a line that arrives now is in view.
        s.push_line("THE NEWEST".into());
        assert!(drawn(&s).contains("THE NEWEST"));
    }

    #[test]
    fn page_keys_are_listed_in_help() {
        let mut s = AppState {
            input: "/help".into(),
            ..Default::default()
        };
        s.update(press(KeyCode::Enter));
        let help = s.transcript.join("\n");
        assert!(
            help.contains(
                "PgUp PgDn ten lines at a time; Home to the start, End back to following"
            ),
            "{help}"
        );
    }

    #[test]
    fn transcript_scrolling_back_stops_at_the_end_and_resumes_following() {
        // #244: 0 is "following". Down at the end stays at the end,
        // saturating rather than wrapping to u16::MAX; Up moves back; coming
        // back down to 0 follows again.
        let mut s = AppState::default();
        s.update(press(KeyCode::Down));
        assert_eq!(s.scroll_back, 0, "saturating, not wrapping to u16::MAX");
        s.update(press(KeyCode::Up));
        s.update(press(KeyCode::Up));
        assert_eq!(s.scroll_back, 2);
        s.update(press(KeyCode::Down));
        s.update(press(KeyCode::Down));
        assert_eq!(s.scroll_back, 0, "back at the end: following again");
    }

    #[test]
    fn transcript_outcome_is_held_until_the_next_run_starts() {
        let mut s = AppState::default().with_running(true);
        s.update(Message::RunOutcome {
            line: "did not converge; worktree restored: true".into(),
            restored: Some(true),
        });
        s.update(Message::RunFinished);
        assert_eq!(
            s.outcome.as_deref(),
            Some("did not converge; worktree restored: true")
        );
        s.update(press(KeyCode::Up));
        s.input = "fix the bug".into();
        s.update(press(KeyCode::Enter));
        assert_eq!(s.outcome, None, "a new run clears the last one's outcome");
        assert_eq!(s.scroll_back, 0, "and is followed from its start");
    }

    #[test]
    fn work_events_land_where_they_belong() {
        let mut s = AppState::default();
        s.update(Message::RadiusUpdated {
            resolved: 47,
            candidates: 3,
            unresolved: 2,
        });
        s.update(Message::BudgetUpdated {
            iteration: 2,
            prompt_bytes: Some(8400),
            elapsed_secs: 18,
            completion_tokens: None,
            model_ms: None,
            queue_wait_ms: None,
        });
        s.update(Message::CheckFinished {
            name: "typecheck".into(),
            passed: true,
        });
        assert_eq!(s.radius.resolved, 47);
        assert_eq!(s.budget.prompt_bytes, Some(8400));
        assert!(s.checks[0].passed);

        // Ticks, resizes and RunFinished are accepted and change nothing.
        s.update(Message::Tick);
        s.update(Message::Resize(10, 10));
        s.update(Message::RunFinished);
        assert_eq!(s.radius.resolved, 47);
        assert_eq!(s.budget.prompt_bytes, Some(8400));
    }
}

#[cfg(test)]
mod input_tests {
    use super::*;
    use ratatui::crossterm::event::{KeyEventState, KeyModifiers};

    fn press(code: KeyCode) -> Message {
        Message::Key(KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        })
    }

    fn type_in(s: &mut AppState, text: &str) {
        for c in text.chars() {
            s.update(press(KeyCode::Char(c)));
        }
    }

    fn with_paths(paths: &[&str]) -> AppState {
        AppState::default().with_paths(paths.iter().map(|p| (*p).to_string()).collect())
    }

    #[test]
    fn at_file_offers_matches_case_insensitively_shortest_first() {
        let mut s = with_paths(&[
            "src/deep/nested/Models.py",
            "README.md",
            "src/models.py",
            "app/models.py",
            "tests/test_views.py",
        ]);
        type_in(&mut s, "fix @MODEL");
        assert_eq!(
            s.at_file_candidates(),
            vec![
                "app/models.py",
                "src/models.py",
                "src/deep/nested/Models.py"
            ],
            "a match ignores case, and equal lengths fall back to byte order              so the list does not depend on the order the driver gave it"
        );
    }

    #[test]
    fn at_file_offers_nothing_unless_the_last_word_starts_with_at() {
        let mut s = with_paths(&["src/models.py"]);
        type_in(&mut s, "fix models");
        assert!(s.at_file_candidates().is_empty(), "no `@`, no list");

        s.input = "fix @models then".into();
        assert!(
            s.at_file_candidates().is_empty(),
            "an `@` word the line has moved past is finished"
        );

        s.input = "mail me@models".into();
        assert!(
            s.at_file_candidates().is_empty(),
            "`@` inside a word is not a path request"
        );
    }

    #[test]
    fn at_file_offers_at_most_ten() {
        // A bare `@` in a large repository matches every path; the list is
        // drawn under the input line and takes its rows from the transcript.
        let many: Vec<String> = (0..50).map(|i| format!("f{i:02}.py")).collect();
        let mut s = AppState::default().with_paths(many);
        type_in(&mut s, "@");
        let got = s.at_file_candidates();
        assert_eq!(got.len(), AT_FILE_LIMIT);
        assert_eq!(got[0], "f00.py");
    }

    #[test]
    fn at_file_tab_inserts_the_first_candidate_and_a_space() {
        let mut s = with_paths(&["src/models.py", "app/models.py"]);
        type_in(&mut s, "fix @models");
        s.update(press(KeyCode::Tab));
        assert_eq!(
            s.input, "fix app/models.py ",
            "the `@` goes with the fragment"
        );
        assert!(
            s.at_file_candidates().is_empty(),
            "and the inserted path is not offered again"
        );
        assert!(s.submitted.is_none(), "Tab completes; it never submits");
    }

    #[test]
    fn at_file_tab_with_no_candidate_changes_nothing() {
        // A Tab that ate the fragment, or typed a literal tab, would alter a
        // task the user can no longer see the reason for.
        let mut s = with_paths(&["src/models.py"]);
        type_in(&mut s, "fix @nosuch");
        s.update(press(KeyCode::Tab));
        assert_eq!(s.input, "fix @nosuch");

        let mut s = AppState::default();
        type_in(&mut s, "fix @models");
        s.update(press(KeyCode::Tab));
        assert_eq!(s.input, "fix @models", "no paths supplied, no completion");
    }

    #[test]
    fn at_file_enter_submits_the_line_as_typed() {
        // Completion is offered, never applied on the user's behalf: Enter
        // sends the `@fragment` exactly as it stands.
        let mut s = with_paths(&["src/models.py"]);
        type_in(&mut s, "fix @models");
        s.update(press(KeyCode::Enter));
        assert_eq!(s.submitted.as_deref(), Some("fix @models"));
    }

    #[test]
    fn paste_lands_in_the_input_and_submits_nothing() {
        // #639: one paste made N submissions. The paste is one input and
        // the run starts only on Enter.
        let mut s = AppState::default();
        s.update(Message::Paste("a\nb".into()));
        assert_eq!(s.input, "a b");
        assert_eq!(s.submitted, None);
        assert!(!s.running);
        assert!(s.steered.is_empty());
        s.update(press(KeyCode::Enter));
        assert_eq!(s.submitted.as_deref(), Some("a b"));
    }

    #[test]
    fn paste_with_crlf_breaks_is_one_space_per_break() {
        // Terminals send a pasted break as CR, LF or both; each is one word
        // boundary, not two.
        let mut s = AppState::default();
        type_in(&mut s, "x ");
        s.update(Message::Paste("a\r\nb\rc\n".into()));
        assert_eq!(s.input, "x a b c ");
    }

    #[test]
    fn typing_then_enter_submits_the_line() {
        let mut s = AppState::default();
        type_in(&mut s, "fix the type error");
        assert_eq!(s.input, "fix the type error");
        assert!(s.submitted.is_none(), "nothing leaves before Enter");

        s.update(press(KeyCode::Enter));
        assert_eq!(s.submitted.as_deref(), Some("fix the type error"));
        assert!(s.input.is_empty(), "the line is cleared for the next one");
        assert!(s.running, "and the interface knows a run is in flight");
        assert_eq!(s.transcript.last().unwrap(), "> fix the type error");
    }

    #[test]
    fn q_types_a_q_now_that_there_is_somewhere_to_type() {
        // The spec's original binding. Once the interface takes text, a
        // letter cannot also be the exit, and silently quitting mid-sentence
        // would be the worst version of that.
        let mut s = AppState::default();
        type_in(&mut s, "quit the queue");
        assert_eq!(s.input, "quit the queue");
        assert!(!s.should_quit);
    }

    #[test]
    fn esc_clears_a_line_and_leaves_an_empty_one() {
        let mut s = AppState::default();
        type_in(&mut s, "half a thought");
        s.update(press(KeyCode::Esc));
        assert!(s.input.is_empty(), "first Esc clears");
        assert!(!s.should_quit, "and does not also quit");

        s.update(press(KeyCode::Esc));
        assert!(s.should_quit, "Esc on an empty line leaves");
    }

    /// One run's worth of pane traffic: two writes to a.py, one to b.py.
    fn worked(s: &mut AppState) {
        for path in ["a.py", "a.py", "b.py"] {
            s.update(Message::ActionTaken {
                tool: "edit".into(),
                target: path.into(),
                wrote: true,
            });
            s.update(Message::EditProposed { path: path.into() });
        }
        s.update(Message::CheckFinished {
            name: "pytest".into(),
            passed: false,
        });
        s.update(Message::RadiusUpdated {
            resolved: 4,
            candidates: 1,
            unresolved: 2,
        });
    }

    #[test]
    fn worktree_state_a_second_task_clears_the_four_panes() {
        // #636: nothing cleared them, so the second run's panes listed the
        // first run's edits and checks beside its own.
        let mut s = AppState::default();
        type_in(&mut s, "first");
        s.update(press(KeyCode::Enter));
        worked(&mut s);
        s.update(Message::RunOutcome {
            line: "did not converge; worktree restored: true".into(),
            restored: Some(true),
        });
        s.update(Message::RunFinished);

        type_in(&mut s, "second");
        s.update(press(KeyCode::Enter));
        assert!(s.actions.is_empty(), "{:?}", s.actions);
        assert!(s.edits.is_empty());
        assert!(s.checks.is_empty());
        let r = s.radius;
        assert_eq!((r.resolved, r.candidates, r.unresolved), (0, 0, 0));
        assert_eq!(s.restored, None, "the last run's ending is not this one's");
        assert_eq!(
            s.worktree(),
            Worktree::Clean,
            "a run starts from a clean tree"
        );
    }

    #[test]
    fn worktree_state_follows_how_the_run_ended() {
        let mut s = AppState::default();
        type_in(&mut s, "change it");
        s.update(press(KeyCode::Enter));
        worked(&mut s);
        assert_eq!(s.worktree(), Worktree::Pending(3), "three writes so far");

        s.update(Message::RunOutcome {
            line: "did not converge; worktree restored: true".into(),
            restored: Some(true),
        });
        s.update(Message::RunFinished);
        assert_eq!(s.worktree(), Worktree::Restored, "not 3 edit(s) pending");

        // A converged run leaves its changes in the tree for review, counted
        // as files rather than as writes.
        type_in(&mut s, "change it again");
        s.update(press(KeyCode::Enter));
        worked(&mut s);
        s.update(Message::RunOutcome {
            line: "converged".into(),
            restored: None,
        });
        s.update(Message::RunFinished);
        assert_eq!(s.worktree(), Worktree::LeftForReview(2));
    }

    #[test]
    fn ctrl_c_mid_run_asks_the_run_to_stop_and_leaves_once_it_has_finished() {
        // #634: the view used to leave at once and the run went on editing
        // the tree with nobody watching. Now a quit mid-run is a stop
        // request, said on screen, and the leave waits for the run's end,
        // which comes after its rollback.
        let mut s = AppState::default().with_running(true);
        s.update(Message::Quit);
        assert!(!s.should_quit, "the run is still in flight");
        assert_eq!(s.steered, vec![SteerOut::Stop]);
        assert_eq!(
            s.transcript.last().unwrap(),
            "· stopping: trellis rolls the run back"
        );

        s.steered.clear();
        s.update(Message::Quit);
        assert!(s.steered.is_empty(), "a second press asks nothing more");
        assert!(!s.should_quit);

        s.update(Message::RunFinished);
        assert!(s.should_quit, "the view leaves once the run has finished");
    }

    #[test]
    fn ctrl_c_mid_run_no_longer_applies_after_run_finished() {
        let mut s = AppState::default().with_running(true);
        s.update(Message::RunFinished);
        assert!(!s.should_quit, "a run ending by itself does not leave");
        s.update(Message::Quit);
        assert!(s.should_quit);
        assert!(s.steered.is_empty(), "nothing to stop");
    }

    #[test]
    fn ctrl_c_mid_run_esc_on_an_empty_line_stops_the_same_way() {
        let mut s = AppState::default().with_running(true);
        type_in(&mut s, "half a steer");
        s.update(press(KeyCode::Esc));
        assert!(s.input.is_empty(), "Esc with text clears the line");
        assert!(s.steered.is_empty(), "and stops nothing");

        s.update(press(KeyCode::Esc));
        assert!(!s.should_quit);
        assert_eq!(s.steered, vec![SteerOut::Stop]);

        s.update(Message::RunFinished);
        assert!(s.should_quit);

        let mut s = AppState::default();
        s.update(press(KeyCode::Esc));
        assert!(s.should_quit, "with no run, Esc on an empty line leaves");
    }

    #[test]
    fn ctrl_c_mid_run_withdraws_an_open_question_without_answering_it() {
        // Ctrl-C reaches `update` as `Quit`, past the modal. An answer here
        // would reach the gate before the run took the stop path and let it
        // carry on; no answer at all, and the driver drops the question once
        // the run has stopped, which is a no (ADR 0018).
        let mut s = AppState::default().with_running(true);
        s.update(Message::ApprovalRequested {
            question: "edit a.py (+1 -1)".into(),
            diff: "- a\n+ b".into(),
        });
        s.update(Message::Quit);
        assert!(s.modal.is_none());
        assert_eq!(s.answered, None);

        // A question still on its way when the stop was asked for is not
        // shown: nobody is waiting for its answer.
        s.update(Message::ApprovalRequested {
            question: "edit a.py (+1 -1)".into(),
            diff: "- c\n+ d".into(),
        });
        assert!(s.modal.is_none());
    }

    #[test]
    fn backspace_deletes_and_stops_at_empty() {
        let mut s = AppState::default();
        type_in(&mut s, "ab");
        s.update(press(KeyCode::Backspace));
        assert_eq!(s.input, "a");
        s.update(press(KeyCode::Backspace));
        s.update(press(KeyCode::Backspace));
        assert_eq!(s.input, "", "popping an empty string is not a panic");
    }

    #[test]
    fn an_empty_line_submits_nothing() {
        let mut s = AppState::default();
        s.update(press(KeyCode::Enter));
        assert!(s.submitted.is_none());
        assert!(!s.running);

        type_in(&mut s, "   ");
        s.update(press(KeyCode::Enter));
        assert!(s.submitted.is_none(), "whitespace is not a task");
    }

    #[test]
    fn a_line_typed_mid_run_is_queued_as_a_steer_not_started_as_a_second_task() {
        // Two runs sharing a worktree is the collision the whole design
        // avoids, so a line typed mid-run must never become a second task.
        // It used to be refused outright; since #115 it is a steer for the
        // run in flight, and the person sees that it was taken.
        let mut s = AppState::default();
        type_in(&mut s, "first");
        s.update(press(KeyCode::Enter));
        s.submitted.take();

        type_in(&mut s, "second");
        s.update(press(KeyCode::Enter));
        assert!(s.submitted.is_none(), "the second must not be submitted");
        assert_eq!(
            s.steered,
            vec![SteerOut::TaskFollows, SteerOut::Steer("second".into())],
            "it is a steer, typed after the task's boundary"
        );
        assert!(s.input.is_empty(), "and the line is cleared");
        assert_eq!(s.transcript.last().unwrap(), "· queued: second");
        assert!(s.running, "queuing a steer does not end the run");

        // Once the run reports finished, input is a task again, and the
        // steer nobody reported delivered is named rather than lost.
        s.steered.clear();
        s.update(Message::RunFinished);
        assert!(!s.running);
        assert_eq!(s.transcript.last().unwrap(), "· not delivered: second");
        type_in(&mut s, "third");
        s.update(press(KeyCode::Enter));
        assert_eq!(s.submitted.as_deref(), Some("third"));
        assert_eq!(s.steered, vec![SteerOut::TaskFollows]);
    }

    #[test]
    fn a_slash_command_mid_run_is_refused_as_before_and_is_not_a_steer() {
        // A `/` line is addressed to this view. Sent to the model it would
        // be a surprise, so it keeps the refusal it had before steering.
        let mut s = AppState::default().with_running(true);
        type_in(&mut s, "/help");
        s.update(press(KeyCode::Enter));
        assert!(s.steered.is_empty(), "{:?}", s.steered);
        assert!(s.submitted.is_none());
        assert!(
            s.transcript.iter().any(|l| l.contains("already in flight")),
            "{:?}",
            s.transcript
        );
    }

    #[test]
    fn the_modal_takes_y_and_n_before_a_steer_can() {
        // While a run asks for approval, `y` is an answer to the gate. Read
        // as the first letter of a steer instead, the gate would wait on
        // an answer the person believes they gave.
        let mut s = AppState::default().with_running(true);
        s.update(Message::ApprovalRequested {
            question: "edit a.py (+1 -1)".into(),
            diff: String::new(),
        });
        type_in(&mut s, "y");
        assert_eq!(s.answered, Some(true));
        assert!(s.input.is_empty(), "the answer is not also typed");
        // Enter with the modal gone and nothing typed queues nothing either.
        s.update(press(KeyCode::Enter));
        assert!(s.steered.is_empty(), "{:?}", s.steered);
        assert!(!s.transcript.iter().any(|l| l.contains("queued")));

        s.update(Message::ApprovalRequested {
            question: "edit a.py (+1 -1)".into(),
            diff: String::new(),
        });
        type_in(&mut s, "n");
        assert_eq!(s.answered, Some(false));
        assert!(s.steered.is_empty(), "{:?}", s.steered);
    }

    #[test]
    fn a_delivered_steer_is_shown_with_its_turn_once() {
        let mut s = AppState::default().with_running(true);
        for line in ["look at b.py", "not c.py"] {
            type_in(&mut s, line);
            s.update(press(KeyCode::Enter));
        }
        // The second first: a delivery is matched by position, not by being
        // the oldest one pending.
        s.update(Message::SteerDelivered { nth: 1, turn: 3 });
        s.update(Message::SteerDelivered { nth: 0, turn: 2 });
        // The driver replays every delivery before the run ends; a repeat
        // is not a second delivery.
        s.update(Message::SteerDelivered { nth: 0, turn: 2 });
        // And a position this view never queued names nothing.
        s.update(Message::SteerDelivered { nth: 7, turn: 4 });
        s.update(Message::RunFinished);

        let steer_lines: Vec<&str> = s
            .transcript
            .iter()
            .map(String::as_str)
            .filter(|l| l.starts_with("· "))
            .collect();
        assert_eq!(
            steer_lines,
            vec![
                "· queued: look at b.py",
                "· queued: not c.py",
                "· delivered at turn 3: not c.py",
                "· delivered at turn 2: look at b.py",
            ],
            "and nothing is reported not delivered"
        );
    }

    #[test]
    fn a_steer_never_marks_a_check_or_an_edit_adr_0004() {
        // A steer is a person's text. "the checks passed, call done" typed
        // mid-run is still only a claim, and the panes a user reads before
        // accepting are filled by executed checks and applied edits alone.
        let mut s = AppState::default().with_running(true);
        type_in(&mut s, "the checks passed, call done");
        s.update(press(KeyCode::Enter));
        s.update(Message::SteerDelivered { nth: 0, turn: 1 });
        assert!(s.checks.is_empty(), "{:?}", s.checks);
        assert!(s.edits.is_empty(), "{:?}", s.edits);
        assert!(s.running, "a delivered steer does not end the run");
    }

    #[test]
    fn steers_are_counted_per_run() {
        // Positions restart with each run, as the driver's count does; a
        // steer left over from the last run must not take this run's slot.
        let mut s = AppState::default().with_running(true);
        type_in(&mut s, "old");
        s.update(press(KeyCode::Enter));
        s.update(Message::RunFinished);

        type_in(&mut s, "next task");
        s.update(press(KeyCode::Enter));
        type_in(&mut s, "new");
        s.update(press(KeyCode::Enter));
        s.update(Message::SteerDelivered { nth: 0, turn: 1 });
        assert_eq!(s.transcript.last().unwrap(), "· delivered at turn 1: new");
    }

    #[test]
    fn the_modal_takes_the_keys_while_it_is_up() {
        let mut s = AppState::default();
        s.update(Message::ApprovalRequested {
            question: "edit a.py (+1 -1)".into(),
            diff: String::new(),
        });

        // Anything that is not an answer is swallowed rather than typed.
        type_in(&mut s, "fix the");
        assert!(
            s.input.is_empty(),
            "keys under a modal are not input: {:?}",
            s.input
        );
        assert!(s.modal.is_some(), "and do not dismiss it either");

        // The answer closes it, and only then does typing resume. A key
        // pressed after that lands in the input, which is ordinary terminal
        // behaviour rather than a leak.
        s.update(press(KeyCode::Char('y')));
        assert!(s.modal.is_none());
        type_in(&mut s, "ok");
        assert_eq!(s.input, "ok");
    }
}
