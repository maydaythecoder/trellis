//! The state, and the one function that changes it.
//!
//! **`update` is synchronous and does no IO.** No `.await`, no file reads,
//! no HTTP. It is a pure state transition, which is what makes every
//! behaviour in this file testable without a terminal, a model or a
//! repository — and what stops a slow read from freezing the frame.
//!
//! Side effects leave through `effects`: `update` pushes an `Effect`, the
//! runtime drains it and does the work, and the answer comes back as
//! another `Message`. A command is a *request* here, never an action.

use crate::commands::{Command, CommandSpec, Registry};
use crate::tui::message::Message;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use std::collections::VecDeque;

/// Conversation lines kept. A session that runs for hours otherwise grows
/// memory without limit, and the oldest lines are the ones nobody scrolls
/// back to.
pub const TRANSCRIPT_CAP: usize = 4000;
/// Samples behind the tokens-per-second sparkline.
pub const RATE_WINDOW: usize = 64;
/// Log lines kept for the in-app pane.
pub const LOG_CAP: usize = 500;

/// Work the runtime must do. `update` produces these; it never performs
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    /// Send a user turn to the model.
    Prompt(String),
    /// Carry out a command that needs the filesystem, git, or a process.
    Run(Command),
    /// Leave the alternate screen and stop, as Ctrl-Z does everywhere else.
    ///
    /// An effect rather than something `update` does: suspending means
    /// restoring the terminal and raising a signal, and `update` owns
    /// neither. The event loop holds the terminal, so it performs this one
    /// itself rather than passing it to the runtime.
    Suspend,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Chat,
    Tools,
    Logs,
    State,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::Chat, Tab::Tools, Tab::Logs, Tab::State];

    pub fn label(self) -> &'static str {
        match self {
            Tab::Chat => "chat",
            Tab::Tools => "tools",
            Tab::Logs => "logs",
            Tab::State => "state",
        }
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|t| *t == self).unwrap_or(0)
    }
}

/// Which pane the keyboard is pointed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Focus {
    Conversation,
    Input,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    User,
    Assistant,
    System,
}

#[derive(Debug, Clone)]
pub struct Line {
    pub role: Role,
    pub text: String,
}

/// A destructive command waiting on `y`/`n`.
///
/// Held as state rather than asked inline, because the answer must be
/// explicit each time. Nothing here remembers a previous yes: a `/clear`
/// confirmed an hour ago says nothing about this one.
#[derive(Debug, Clone)]
pub struct Confirm {
    pub command: Command,
    pub prompt: String,
}

/// The completion popup over the input line.
#[derive(Debug, Clone, Default)]
pub struct Completion {
    pub open: bool,
    pub selected: usize,
    /// Names only — the view looks the specs back up, so the popup cannot
    /// show a command the registry does not have.
    pub matches: Vec<String>,
}

pub struct AppState {
    pub tab: Tab,
    pub focus: Focus,
    pub registry: Registry,

    pub conversation: VecDeque<Line>,
    pub input: String,
    pub completion: Completion,
    pub confirm: Option<Confirm>,

    pub model: String,
    pub tier: String,
    pub turns: u32,
    pub context_used: u32,
    pub context_window: u32,

    /// Tokens seen in each recent sample period, newest last.
    pub token_rate: VecDeque<u64>,
    /// Tokens in the sample currently accumulating.
    current_sample: u64,

    /// Tool name to call count, kept sorted by name so the pane does not
    /// reorder itself as counts change.
    pub tool_calls: Vec<(String, u32)>,

    pub log: VecDeque<String>,
    pub streaming: bool,
    /// Ticks since the stream opened, for the spinner.
    pub spinner: usize,
    /// How many rows the view sits **above the newest line**.
    ///
    /// Counted from the bottom, not the top, and that is the whole point:
    /// an absolute offset is meaningless while the content is growing, so
    /// the first `Up` after following jumped to the very beginning — the
    /// stored offset was still 0 and one decrement saturated there. From
    /// the bottom, `Up` is always "one line further back" whatever the
    /// transcript has done since.
    pub scroll_back: u16,
    /// Whether the conversation pane tracks the newest line.
    ///
    /// True by default, and the reason is what happens without it: the view
    /// pinned to offset 0, so once the transcript grew past the pane every
    /// new line arrived below the fold and the user watched the top of the
    /// conversation while the model answered off-screen. It looked correct
    /// for exactly as long as the content fitted.
    ///
    /// Scrolling up turns it off — someone reading back does not want the
    /// floor moving under them — and `End`, or sending a turn, turns it
    /// back on.
    pub follow: bool,
    pub should_quit: bool,
    pub last_error: Option<String>,

    pub effects: Vec<Effect>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            tab: Tab::Chat,
            focus: Focus::Input,
            registry: Registry::builtins_only(),
            conversation: VecDeque::new(),
            input: String::new(),
            completion: Completion::default(),
            confirm: None,
            model: "mock".into(),
            tier: "mock".into(),
            turns: 0,
            context_used: 0,
            context_window: 32_768,
            token_rate: VecDeque::new(),
            current_sample: 0,
            tool_calls: Vec::new(),
            log: VecDeque::new(),
            streaming: false,
            spinner: 0,
            scroll_back: 0,
            follow: true,
            should_quit: false,
            last_error: None,
            effects: Vec::new(),
        }
    }
}

impl AppState {
    /// The one entry point. Synchronous, no IO, no `.await`.
    pub fn update(&mut self, msg: Message) {
        match msg {
            Message::Key(k) => self.on_key(k),
            // The layout is recomputed from `frame.area()` every frame, so
            // there is nothing to invalidate here. The variant exists so a
            // resize still forces the redraw the loop would otherwise skip.
            Message::Resize(..) => {}
            Message::Tick => self.on_tick(),
            Message::StreamToken(t) => self.on_token(&t),
            Message::StreamDone => {
                self.streaming = false;
                self.flush_sample();
                self.push_log("stream closed");
            }
            Message::ToolCalled { name } => {
                self.push_log(&format!("tool {name}"));
                match self.tool_calls.iter_mut().find(|(n, _)| *n == name) {
                    Some((_, c)) => *c += 1,
                    None => {
                        self.tool_calls.push((name, 1));
                        self.tool_calls.sort_by(|a, b| a.0.cmp(&b.0));
                    }
                }
            }
            Message::Error(e) => {
                self.streaming = false;
                self.push_log(&format!("error: {e}"));
                self.push(Role::System, &format!("! {e}"));
                self.last_error = Some(e);
            }
            Message::Notice(text) => {
                for line in text.lines() {
                    self.push(Role::System, line);
                }
            }
            Message::Command(c) => self.dispatch(c),
            Message::CommandsLoaded { specs, errors } => {
                let n = specs.len();
                let rejected = self.registry.register_custom(specs);
                self.push(Role::System, &format!("· {n} custom command(s) loaded"));
                for r in rejected {
                    self.push(
                        Role::System,
                        &format!("! `{r}` ignored: a custom command cannot replace a built-in"),
                    );
                }
                for e in errors {
                    self.push(Role::System, &format!("! {e}"));
                }
            }
            Message::ContextUsed { tokens } => self.context_used = tokens,
            Message::Quit => self.should_quit = true,
        }
    }

    // -- keys ------------------------------------------------------------

    fn on_key(&mut self, k: KeyEvent) {
        // Ctrl chords are commands, never text.
        //
        // Found through a pty: Ctrl-Z rendered as . In raw mode the tty
        // does not generate SIGTSTP, so the chord arrives as an ordinary key
        // event — and the `Char(ch)` arm below pushed the bare letter. Every
        // Ctrl+letter typed its letter: ^W, ^U and ^A all inserted text where
        // the user meant an editing command.
        if k.modifiers.contains(KeyModifiers::CONTROL)
            && let KeyCode::Char(c) = k.code
        {
            match c {
                'c' => {}
                'z' => {
                    // The tty will not do this for us in raw mode, so the
                    // app has to suspend itself.
                    self.effects.push(Effect::Suspend);
                    return;
                }
                // Swallowed rather than inserted. Doing nothing is the
                // honest response to a chord this interface does not bind.
                _ => return,
            }
        }
        // Ctrl-C is the interrupt, not a character. Checked first so no
        // other binding can shadow it.
        if k.modifiers.contains(KeyModifiers::CONTROL) && matches!(k.code, KeyCode::Char('c')) {
            if self.streaming {
                self.streaming = false;
                self.effects.push(Effect::Run(Command::Stop));
                self.push(Role::System, "· interrupted");
                return;
            }
            self.should_quit = true;
            return;
        }

        // A pending confirmation owns the keyboard. Anything but y/n leaves
        // it open rather than guessing, and there is no default: Enter does
        // not mean yes.
        if let Some(c) = &self.confirm {
            match k.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    let cmd = c.command.clone();
                    self.confirm = None;
                    self.perform(cmd);
                }
                KeyCode::Char('n') | KeyCode::Char('N') | KeyCode::Esc => {
                    self.confirm = None;
                    self.push(Role::System, "· cancelled");
                }
                _ => {}
            }
            return;
        }

        if self.completion.open {
            match k.code {
                KeyCode::Esc => {
                    self.completion.open = false;
                    return;
                }
                KeyCode::Up => {
                    self.completion.selected = self.completion.selected.saturating_sub(1);
                    return;
                }
                KeyCode::Down => {
                    let last = self.completion.matches.len().saturating_sub(1);
                    self.completion.selected = (self.completion.selected + 1).min(last);
                    return;
                }
                KeyCode::Tab => {
                    self.accept_completion();
                    return;
                }
                _ => {}
            }
        }

        match k.code {
            KeyCode::Tab => self.focus = self.next_focus(),
            KeyCode::Enter => self.submit(),
            KeyCode::Backspace => {
                self.input.pop();
                self.refresh_completion();
            }
            KeyCode::Char(ch) => {
                self.input.push(ch);
                self.refresh_completion();
            }
            // Esc clears a half-typed line first and only then quits, so the
            // key that means "get out of this" does not mean "get out of
            // everything" while there is something to get out of.
            KeyCode::Esc if !self.input.is_empty() => {
                self.input.clear();
                self.completion.open = false;
            }
            KeyCode::Esc => self.should_quit = true,
            // Reading back stops the pane following: the floor moving
            // under you mid-sentence is worse than having to press End.
            KeyCode::Up => self.scroll_up(1),
            KeyCode::PageUp => self.scroll_up(10),
            KeyCode::Down => self.scroll_down(1),
            KeyCode::PageDown => self.scroll_down(10),
            KeyCode::End => {
                self.scroll_back = 0;
                self.follow = true;
            }
            KeyCode::Home => {
                self.follow = false;
                // Clamped by the view, which is the only place the content
                // height is known.
                self.scroll_back = u16::MAX;
            }
            KeyCode::Left => self.tab = Tab::ALL[self.tab.index().saturating_sub(1)],
            KeyCode::Right => {
                self.tab = Tab::ALL[(self.tab.index() + 1).min(Tab::ALL.len() - 1)];
            }
            _ => {}
        }
    }

    fn scroll_up(&mut self, n: u16) {
        self.follow = false;
        self.scroll_back = self.scroll_back.saturating_add(n);
    }

    /// Scrolling back to the bottom resumes following, so a user who
    /// arrowed down to the newest line does not then watch it sit still.
    fn scroll_down(&mut self, n: u16) {
        self.scroll_back = self.scroll_back.saturating_sub(n);
        if self.scroll_back == 0 {
            self.follow = true;
        }
    }

    fn next_focus(&self) -> Focus {
        match self.focus {
            Focus::Conversation => Focus::Input,
            Focus::Input => Focus::Conversation,
        }
    }

    // -- input -----------------------------------------------------------

    fn refresh_completion(&mut self) {
        if let Some(prefix) = self.input.strip_prefix('/') {
            // Only while the line is still just a command name. Once there
            // is an argument the popup is in the way.
            if prefix.contains(char::is_whitespace) {
                self.completion.open = false;
                return;
            }
            self.completion.matches = self
                .registry
                .complete(prefix)
                .iter()
                .map(|s| s.name.clone())
                .collect();
            self.completion.open = !self.completion.matches.is_empty();
            self.completion.selected = self
                .completion
                .selected
                .min(self.completion.matches.len().saturating_sub(1));
        } else {
            self.completion.open = false;
        }
    }

    fn accept_completion(&mut self) {
        if let Some(name) = self.completion.matches.get(self.completion.selected) {
            self.input = format!("/{name}");
            // A command that takes an argument gets a trailing space, so the
            // next keystroke is the argument rather than a typo in the name.
            if self
                .registry
                .find(name)
                .is_some_and(|s| !matches!(s.args, crate::commands::ArgSpec::None))
            {
                self.input.push(' ');
            }
            self.completion.open = false;
        }
    }

    fn submit(&mut self) {
        let line = self.input.trim().to_string();
        if line.is_empty() {
            return;
        }
        self.input.clear();
        self.completion.open = false;

        if line.starts_with('/') {
            match self.registry.parse(&line) {
                Ok(cmd) => self.dispatch(cmd),
                // Reported, never sent. A mistyped command that fell through
                // to the model would become a turn the user did not ask for.
                Err(e) => self.push(Role::System, &format!("! {e}")),
            }
            return;
        }

        // You want to see the turn you just sent, wherever you had
        // scrolled to.
        self.follow = true;
        self.scroll_back = 0;
        self.push(Role::User, &line);
        self.turns += 1;
        self.streaming = true;
        self.spinner = 0;
        self.effects.push(Effect::Prompt(line));
    }

    // -- commands --------------------------------------------------------

    fn dispatch(&mut self, cmd: Command) {
        if cmd.destructive() {
            self.confirm = Some(Confirm {
                prompt: match &cmd {
                    Command::Clear => {
                        "drop the whole conversation? history is the only record of what \
                         was decided  [y/n]"
                            .into()
                    }
                    Command::Compact => {
                        "summarise history? the original wording is not recoverable  [y/n]".into()
                    }
                    other => format!("{other:?}? this cannot be undone  [y/n]"),
                },
                command: cmd,
            });
            return;
        }
        self.perform(cmd);
    }

    /// Applies a command that is either pure state, or hands it out as an
    /// effect. Reached only after any confirmation has been given.
    fn perform(&mut self, cmd: Command) {
        match cmd {
            Command::Clear => {
                self.conversation.clear();
                self.turns = 0;
                self.context_used = 0;
                self.push(Role::System, "· history dropped");
                // The *runtime* holds the history that is actually sent to
                // the model, and it is not this one. Clearing only the
                // display left every dropped turn still going out on the
                // next request while the pane, the turn count and the
                // context figure all said it was gone — a confirmation
                // prompt that lied about what it had done.
                self.effects.push(Effect::Run(Command::Clear));
            }
            // Everything else needs the filesystem, git, a process or the
            // model, and none of that happens here.
            other => self.effects.push(Effect::Run(other)),
        }
    }

    // -- stream ----------------------------------------------------------

    fn on_token(&mut self, t: &str) {
        self.current_sample += t.split_whitespace().count().max(1) as u64;
        // Tokens continue the assistant's line rather than starting one, so
        // a streamed sentence reads as a sentence.
        let continues = matches!(self.conversation.back(), Some(l) if l.role == Role::Assistant)
            && !self
                .conversation
                .back()
                .is_some_and(|l| l.text.ends_with('\n'));
        if continues
            && !t.starts_with('\n')
            && let Some(last) = self.conversation.back_mut()
        {
            last.text.push_str(t);
            return;
        }
        self.push(Role::Assistant, t.trim_start_matches('\n'));
    }

    fn on_tick(&mut self) {
        if self.streaming {
            self.spinner = self.spinner.wrapping_add(1);
        }
        self.flush_sample();
    }

    /// Closes the current sample into the sparkline window.
    fn flush_sample(&mut self) {
        self.token_rate.push_back(self.current_sample);
        self.current_sample = 0;
        while self.token_rate.len() > RATE_WINDOW {
            self.token_rate.pop_front();
        }
    }

    // -- helpers ---------------------------------------------------------

    fn push(&mut self, role: Role, text: &str) {
        self.conversation.push_back(Line {
            role,
            text: text.to_string(),
        });
        while self.conversation.len() > TRANSCRIPT_CAP {
            self.conversation.pop_front();
        }
    }

    fn push_log(&mut self, text: &str) {
        self.log.push_back(text.to_string());
        while self.log.len() > LOG_CAP {
            self.log.pop_front();
        }
    }

    /// Takes the queued effects, leaving none behind.
    pub fn drain_effects(&mut self) -> Vec<Effect> {
        std::mem::take(&mut self.effects)
    }

    /// Fraction of the context window in use, clamped to 1.
    pub fn context_ratio(&self) -> f64 {
        if self.context_window == 0 {
            return 0.0;
        }
        (f64::from(self.context_used) / f64::from(self.context_window)).min(1.0)
    }

    /// The specs the popup should show, looked up from the registry rather
    /// than stored, so it can never offer a command that is not registered.
    pub fn completion_specs(&self) -> Vec<&CommandSpec> {
        self.completion
            .matches
            .iter()
            .filter_map(|n| self.registry.find(n))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

    fn key(c: KeyCode) -> Message {
        Message::Key(KeyEvent::new(c, KeyModifiers::NONE))
    }

    fn type_line(s: &mut AppState, text: &str) {
        for c in text.chars() {
            s.update(key(KeyCode::Char(c)));
        }
    }

    #[test]
    fn typing_a_prompt_and_pressing_enter_hands_it_out_as_an_effect() {
        // `update` may not send anything itself. The test asserts the state
        // *and* that the work left as a request.
        let mut s = AppState::default();
        type_line(&mut s, "hello");
        s.update(key(KeyCode::Enter));

        assert!(s.input.is_empty());
        assert_eq!(s.turns, 1);
        assert!(s.streaming);
        assert_eq!(s.drain_effects(), vec![Effect::Prompt("hello".into())]);
        assert!(s.drain_effects().is_empty(), "effects drain exactly once");
    }

    #[test]
    fn an_empty_line_is_not_a_turn() {
        let mut s = AppState::default();
        s.update(key(KeyCode::Enter));
        assert_eq!(s.turns, 0);
        assert!(s.drain_effects().is_empty());
    }

    #[test]
    fn tokens_join_into_one_assistant_line() {
        let mut s = AppState::default();
        s.update(Message::StreamToken("It walks ".into()));
        s.update(Message::StreamToken("the scope ".into()));
        s.update(Message::StreamToken("chain".into()));
        assert_eq!(s.conversation.len(), 1);
        assert_eq!(s.conversation[0].text, "It walks the scope chain");
        assert_eq!(s.conversation[0].role, Role::Assistant);
    }

    #[test]
    fn the_transcript_is_capped() {
        let mut s = AppState::default();
        for i in 0..TRANSCRIPT_CAP + 50 {
            s.update(Message::Notice(format!("line {i}")));
        }
        assert_eq!(s.conversation.len(), TRANSCRIPT_CAP);
        assert!(
            s.conversation.front().unwrap().text.contains("line 50"),
            "the oldest lines go, not the newest"
        );
    }

    #[test]
    fn the_token_sparkline_window_is_bounded() {
        let mut s = AppState::default();
        for _ in 0..RATE_WINDOW * 3 {
            s.update(Message::StreamToken("x".into()));
            s.update(Message::Tick);
        }
        assert_eq!(s.token_rate.len(), RATE_WINDOW);
    }

    #[test]
    fn tool_counts_increment_and_stay_in_a_stable_order() {
        let mut s = AppState::default();
        for name in ["run_tests", "read_file", "read_file"] {
            s.update(Message::ToolCalled { name: name.into() });
        }
        assert_eq!(
            s.tool_calls,
            vec![("read_file".into(), 2), ("run_tests".into(), 1)],
            "sorted by name, so the pane does not reshuffle as counts change"
        );
    }

    #[test]
    fn the_context_gauge_never_exceeds_full() {
        let mut s = AppState {
            context_window: 100,
            ..Default::default()
        };
        s.update(Message::ContextUsed { tokens: 42 });
        assert!((s.context_ratio() - 0.42).abs() < 1e-9);
        s.update(Message::ContextUsed { tokens: 500 });
        assert_eq!(s.context_ratio(), 1.0, "a gauge over 1.0 panics ratatui");
    }

    #[test]
    fn slash_opens_the_popup_and_typing_filters_it() {
        let mut s = AppState::default();
        type_line(&mut s, "/");
        assert!(s.completion.open);
        let all = s.completion.matches.len();
        type_line(&mut s, "co");
        assert!(s.completion.open);
        assert!(s.completion.matches.len() < all);
        assert!(s.completion.matches.iter().all(|m| m.starts_with("co")));
    }

    #[test]
    fn the_popup_closes_once_the_line_has_an_argument() {
        let mut s = AppState::default();
        type_line(&mut s, "/add ");
        assert!(!s.completion.open, "the popup would cover the argument");
    }

    #[test]
    fn tab_accepts_the_selection_and_leaves_room_for_an_argument() {
        let mut s = AppState::default();
        type_line(&mut s, "/ad");
        s.update(key(KeyCode::Tab));
        assert_eq!(s.input, "/add ", "a command taking a path gets a space");

        let mut s = AppState::default();
        type_line(&mut s, "/conte");
        s.update(key(KeyCode::Tab));
        assert_eq!(s.input, "/context", "one that takes nothing does not");
    }

    #[test]
    fn a_non_destructive_command_leaves_as_an_effect_without_asking() {
        let mut s = AppState::default();
        type_line(&mut s, "/context");
        s.update(key(KeyCode::Enter));
        assert!(s.confirm.is_none());
        assert_eq!(s.drain_effects(), vec![Effect::Run(Command::Context)]);
    }

    #[test]
    fn a_destructive_command_asks_first_and_honours_n() {
        // No default-yes: the whole point is that the answer is given, not
        // assumed. Enter must not mean yes.
        let mut s = AppState::default();
        s.update(Message::Notice("keep me".into()));
        type_line(&mut s, "/clear");
        s.update(key(KeyCode::Enter));

        assert!(s.confirm.is_some(), "it must ask");
        s.update(key(KeyCode::Enter));
        assert!(s.confirm.is_some(), "Enter is not an answer");

        s.update(key(KeyCode::Char('n')));
        assert!(s.confirm.is_none());
        assert!(
            s.conversation.iter().any(|l| l.text.contains("keep me")),
            "`n` must leave the history alone"
        );
    }

    #[test]
    fn a_destructive_command_runs_on_y() {
        let mut s = AppState::default();
        s.update(Message::Notice("drop me".into()));
        s.turns = 4;
        type_line(&mut s, "/clear");
        s.update(key(KeyCode::Enter));
        s.update(key(KeyCode::Char('y')));

        assert!(s.confirm.is_none());
        assert_eq!(s.turns, 0);
        assert!(!s.conversation.iter().any(|l| l.text.contains("drop me")));
    }

    #[test]
    fn clearing_the_display_also_tells_the_runtime_to_drop_its_history() {
        // Found in a live run: after `/clear` the pane read `ctx 0, turns
        // 0`, and the very next turn reported 166 tokens rather than the
        // ~21 a fresh history costs. The display had been cleared and the
        // model was still being sent every dropped turn.
        //
        // The confirmation makes it worse, not better: the user is asked
        // to approve losing the history and it is not lost.
        let mut s = AppState::default();
        s.update(Message::Notice("something".into()));
        s.turns = 3;
        type_line(&mut s, "/clear");
        s.update(key(KeyCode::Enter));
        s.update(key(KeyCode::Char('y')));

        assert!(s.conversation.iter().all(|l| !l.text.contains("something")));
        assert_eq!(s.turns, 0);
        assert_eq!(s.context_used, 0);
        assert!(
            s.drain_effects().contains(&Effect::Run(Command::Clear)),
            "the runtime holds what is actually sent, and must be told"
        );
    }

    #[test]
    fn confirmation_is_never_remembered() {
        // A `/clear` confirmed a minute ago says nothing about this one.
        let mut s = AppState::default();
        for _ in 0..2 {
            type_line(&mut s, "/clear");
            s.update(key(KeyCode::Enter));
            assert!(s.confirm.is_some(), "it must ask every time");
            s.update(key(KeyCode::Char('y')));
        }
    }

    #[test]
    fn a_mistyped_command_is_reported_and_never_sent_to_the_model() {
        let mut s = AppState::default();
        type_line(&mut s, "/contxt");
        s.update(key(KeyCode::Enter));
        assert!(s.drain_effects().is_empty(), "a typo is not a turn");
        assert_eq!(s.turns, 0);
        assert!(
            s.conversation
                .iter()
                .any(|l| l.text.contains("unknown command")),
            "and the user has to be told why nothing happened"
        );
    }

    #[test]
    fn ctrl_chords_are_commands_and_never_text() {
        // Found through a pty: Ctrl-Z rendered as `> z`. In raw mode the tty
        // does not generate SIGTSTP, so the chord arrives as an ordinary key
        // event and the `Char` arm inserted the bare letter — ^W, ^U and ^A
        // all typed their letter where the user meant an editing command.
        let mut s = AppState::default();
        for c in ['z', 'w', 'u', 'a', 'd'] {
            s.update(Message::Key(KeyEvent::new(
                KeyCode::Char(c),
                KeyModifiers::CONTROL,
            )));
        }
        assert_eq!(s.input, "", "a chord must never leave text behind");
        assert!(!s.should_quit, "and none of those is a quit key");
    }

    #[test]
    fn ctrl_z_asks_to_suspend_rather_than_doing_it() {
        // Suspending restores the terminal and stops the process, and
        // `update` owns neither. It records the intent; the event loop,
        // which holds the terminal, performs it.
        let mut s = AppState::default();
        s.update(Message::Key(KeyEvent::new(
            KeyCode::Char('z'),
            KeyModifiers::CONTROL,
        )));
        assert_eq!(s.drain_effects(), vec![Effect::Suspend]);
        assert_eq!(s.input, "");
    }

    #[test]
    fn ctrl_c_interrupts_a_stream_before_it_quits() {
        // Two different meanings for one key, and the order matters: a user
        // stopping a runaway answer does not want the session to end.
        let mut s = AppState {
            streaming: true,
            ..Default::default()
        };
        s.update(Message::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
        assert!(!s.should_quit, "the first Ctrl-C stops the stream");
        assert!(!s.streaming);
        assert_eq!(s.drain_effects(), vec![Effect::Run(Command::Stop)]);

        s.update(Message::Key(KeyEvent::new(
            KeyCode::Char('c'),
            KeyModifiers::CONTROL,
        )));
        assert!(s.should_quit, "the second leaves");
    }

    #[test]
    fn esc_clears_a_half_typed_line_before_it_quits() {
        let mut s = AppState::default();
        type_line(&mut s, "half a thought");
        s.update(key(KeyCode::Esc));
        assert!(s.input.is_empty());
        assert!(!s.should_quit);
        s.update(key(KeyCode::Esc));
        assert!(s.should_quit);
    }

    #[test]
    fn tab_switches_focus_and_the_arrows_switch_tabs() {
        let mut s = AppState::default();
        assert_eq!(s.focus, Focus::Input);
        s.update(key(KeyCode::Tab));
        assert_eq!(s.focus, Focus::Conversation);
        s.update(key(KeyCode::Tab));
        assert_eq!(s.focus, Focus::Input);

        assert_eq!(s.tab, Tab::Chat);
        s.update(key(KeyCode::Right));
        assert_eq!(s.tab, Tab::Tools);
        s.update(key(KeyCode::Left));
        assert_eq!(s.tab, Tab::Chat);
        s.update(key(KeyCode::Left));
        assert_eq!(s.tab, Tab::Chat, "the ends do not wrap round");
    }

    #[test]
    fn the_conversation_follows_the_newest_line_until_you_scroll_up() {
        // Found through a pty: once the transcript grew past the pane, the
        // view stayed pinned at offset 0 and every new line arrived below
        // the fold. The model answered off-screen and the pane showed the
        // top of the conversation. It looked correct for exactly as long as
        // the content fitted.
        let mut s = AppState::default();
        assert!(s.follow, "following is the default");

        s.update(key(KeyCode::Up));
        assert!(!s.follow, "reading back must stop the floor moving");

        s.update(key(KeyCode::End));
        assert!(s.follow, "End returns to the tail");

        // And sending a turn re-follows, wherever you had scrolled to.
        s.update(key(KeyCode::Up));
        assert!(!s.follow);
        type_line(&mut s, "a question");
        s.update(key(KeyCode::Enter));
        assert!(s.follow, "you want to see the turn you just sent");
    }

    #[test]
    fn scrolling_does_not_run_off_either_end_of_a_u16() {
        let mut s = AppState::default();
        s.update(key(KeyCode::Down));
        assert_eq!(s.scroll_back, 0, "saturating at zero, not wrapping");
        s.scroll_back = u16::MAX;
        s.update(key(KeyCode::Up));
        assert_eq!(s.scroll_back, u16::MAX, "and not wrapping at the top");
    }

    #[test]
    fn the_first_up_scrolls_one_line_and_not_to_the_beginning() {
        // Found through a pty: ten `Up`s from a following view landed on the
        // very first line of the session. The offset was absolute and still
        // 0 while the view followed the tail, so one decrement saturated at
        // the top and every further press did nothing. Counting back from
        // the newest line makes `Up` mean the same thing whatever the
        // transcript has done since.
        let mut s = AppState::default();
        assert!(s.follow);
        s.update(key(KeyCode::Up));
        assert_eq!(s.scroll_back, 1, "one line back, not all of them");
        s.update(key(KeyCode::Up));
        assert_eq!(s.scroll_back, 2);
        s.update(key(KeyCode::PageUp));
        assert_eq!(s.scroll_back, 12);
    }

    #[test]
    fn scrolling_back_down_to_the_newest_line_resumes_following() {
        // Otherwise a user who arrowed down to the bottom watches it sit
        // still while the model keeps answering below them.
        let mut s = AppState::default();
        s.update(key(KeyCode::Up));
        s.update(key(KeyCode::Up));
        assert!(!s.follow);
        s.update(key(KeyCode::Down));
        assert!(!s.follow, "still one line back");
        s.update(key(KeyCode::Down));
        assert!(s.follow, "at the tail again");
    }

    #[test]
    fn a_stream_error_stops_the_stream_and_says_so_in_the_conversation() {
        let mut s = AppState {
            streaming: true,
            ..Default::default()
        };
        s.update(Message::Error("connection refused".into()));
        assert!(
            !s.streaming,
            "a failed stream that still says 'streaming' hangs the spinner"
        );
        assert!(
            s.conversation
                .iter()
                .any(|l| l.text.contains("connection refused"))
        );
        assert!(s.log.iter().any(|l| l.contains("connection refused")));
    }

    #[test]
    fn loaded_commands_reach_the_registry_and_the_rejections_are_shown() {
        let mut s = AppState::default();
        s.update(Message::CommandsLoaded {
            specs: vec![
                CommandSpec {
                    name: "test-loop".into(),
                    description: "d".into(),
                    args: crate::commands::ArgSpec::None,
                    destructive: false,
                    custom: true,
                },
                CommandSpec {
                    name: "clear".into(),
                    description: "impostor".into(),
                    args: crate::commands::ArgSpec::None,
                    destructive: false,
                    custom: true,
                },
            ],
            errors: vec!["broken.md: no frontmatter".into()],
        });
        assert!(s.registry.find("test-loop").is_some());
        let text: String = s.conversation.iter().map(|l| l.text.clone()).collect();
        assert!(text.contains("cannot replace a built-in"), "{text}");
        assert!(text.contains("broken.md"), "{text}");
    }

    #[test]
    fn update_is_reachable_with_no_terminal_at_all() {
        // The property the whole split exists for. If this test ever needs a
        // backend, `update` has grown IO.
        let mut s = AppState::default();
        for m in [
            Message::Tick,
            Message::Resize(10, 10),
            Message::StreamToken("x".into()),
            Message::StreamDone,
            Message::ContextUsed { tokens: 1 },
            Message::Notice("n".into()),
            Message::Quit,
        ] {
            s.update(m);
        }
        assert!(s.should_quit);
    }

    #[test]
    fn key_release_events_are_the_loops_problem_not_this_ones() {
        // Documented rather than filtered here: `events.rs` drops anything
        // that is not a Press, so Windows' duplicate Release does not reach
        // `update`. If that filter is removed this test still passes and the
        // interface types every character twice — which is why the filter
        // has its own assertion in `events.rs`.
        let mut s = AppState::default();
        s.update(Message::Key(KeyEvent::new(
            KeyCode::Char('a'),
            KeyModifiers::NONE,
        )));
        assert_eq!(s.input, "a");
        assert_eq!(KeyEventKind::Press, KeyEventKind::Press);
    }
}
