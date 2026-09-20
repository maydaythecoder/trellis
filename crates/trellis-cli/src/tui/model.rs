//! Application state. No I/O, no drawing, no terminal.
//!
//! Every transition is a plain method taking `&mut self`, so the whole
//! behaviour of the interface can be exercised without a terminal attached —
//! which is what the tests at the bottom of this file do.
//!
//! The view takes `&App` and may not mutate it. Anything the view would have
//! to compute per frame is computed here instead, once, when the state that
//! feeds it changes: a `draw` that formats strings runs sixty times a second
//! to produce the same bytes.

use super::events::{LogLine, RunReport};
use std::collections::VecDeque;

/// How many log lines are kept.
///
/// A bound rather than a growing buffer: a long run emits thousands of lines
/// and nobody scrolls back past a few hundred, but the memory is held for the
/// life of the session either way.
const LOG_CAPACITY: usize = 2_000;

/// Which pane has the keyboard.
///
/// An enum rather than a pair of bools, so "both focused" and "neither
/// focused" cannot be written down.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Focus {
    /// Typing a task.
    Input,
    /// Scrolling the transcript.
    Log,
}

/// Whether a run is in flight.
///
/// The state a TUI most often gets wrong is "a job is running": a `bool` plus
/// a separate `Option<JoinHandle>` can disagree, and the pair that says
/// running-with-no-handle is the one that hangs the interface. One enum, and
/// the impossible pairs are unrepresentable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Job {
    Idle,
    Running {
        task: String,
        /// Frames since the run started, for the spinner. Advanced by the
        /// tick, so the animation speed is the tick rate and not the speed of
        /// whatever the run happens to be doing.
        frames: usize,
    },
    Done(RunOutcome),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RunOutcome {
    pub(crate) task: String,
    pub(crate) converged: bool,
    pub(crate) summary: String,
    pub(crate) proposed: Vec<String>,
}

pub(crate) struct App {
    /// Pre-rendered, because the header draws it every frame and it changes
    /// only when the index is rebuilt.
    pub(crate) index_line: String,
    pub(crate) model: Option<String>,
    pub(crate) focus: Focus,
    pub(crate) input: String,
    pub(crate) log: VecDeque<LogLine>,
    /// Lines scrolled back from the bottom. Zero follows the tail.
    pub(crate) scroll: usize,
    pub(crate) job: Job,
    /// Set when the user asks to leave. The loop reads it and stops; nothing
    /// calls `exit()` from inside a key handler, so there is exactly one exit
    /// path and the terminal guard is always unwound through it.
    pub(crate) quit: bool,
    pub(crate) status: String,
}

impl App {
    pub(crate) fn new(index_line: String, model: Option<String>) -> Self {
        Self {
            index_line,
            model,
            focus: Focus::Input,
            input: String::new(),
            log: VecDeque::with_capacity(256),
            scroll: 0,
            job: Job::Idle,
            quit: false,
            status: "ready".into(),
        }
    }

    pub(crate) fn push_log(&mut self, line: LogLine) {
        if self.log.len() == LOG_CAPACITY {
            self.log.pop_front();
            // Keep the viewport over the same text rather than letting it
            // drift up by one every time the buffer evicts.
            self.scroll = self.scroll.saturating_sub(1);
        }
        self.log.push_back(line);
    }

    pub(crate) fn tick(&mut self) {
        if let Job::Running { frames, .. } = &mut self.job {
            *frames = frames.wrapping_add(1);
        }
    }

    /// Whether a task may be started.
    ///
    /// Asked rather than assumed: starting a second run while one is in
    /// flight would have two writers on one worktree, which is the thing the
    /// orchestrator's single-owner design exists to prevent.
    pub(crate) fn can_start(&self) -> bool {
        !matches!(self.job, Job::Running { .. }) && !self.input.trim().is_empty()
    }

    /// Takes the typed task and marks a run started.
    pub(crate) fn start_run(&mut self) -> Option<String> {
        if !self.can_start() {
            return None;
        }
        let task = std::mem::take(&mut self.input).trim().to_string();
        self.job = Job::Running {
            task: task.clone(),
            frames: 0,
        };
        self.status = "running".into();
        self.scroll = 0;
        Some(task)
    }

    pub(crate) fn finish_run(&mut self, report: RunReport) {
        self.status = if report.converged {
            "converged — review the diff, then accept".into()
        } else {
            "not converged — the tree was rolled back".into()
        };
        self.job = Job::Done(RunOutcome {
            task: report.task,
            converged: report.converged,
            summary: report.summary,
            proposed: report.proposed,
        });
    }

    /// Scrolls back through the transcript.
    ///
    /// Clamped against the buffer rather than the viewport, because the view
    /// owns the height and the model must not need to know it. The view
    /// clamps again to what it can show, which is the only place that can.
    pub(crate) fn scroll_up(&mut self, n: usize) {
        self.scroll = (self.scroll + n).min(self.log.len());
    }

    pub(crate) fn scroll_down(&mut self, n: usize) {
        self.scroll = self.scroll.saturating_sub(n);
    }

    pub(crate) fn follow_tail(&mut self) {
        self.scroll = 0;
    }

    pub(crate) fn toggle_focus(&mut self) {
        self.focus = match self.focus {
            Focus::Input => Focus::Log,
            Focus::Log => Focus::Input,
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn app() -> App {
        App::new("index".into(), None)
    }

    fn log(text: &str) -> LogLine {
        LogLine {
            level: tracing::Level::INFO,
            text: text.into(),
        }
    }

    #[test]
    fn a_second_run_cannot_start_while_one_is_in_flight() {
        // Two runs would mean two writers on one worktree, which the
        // orchestrator's single-owner design exists to prevent. The interface
        // must not be the thing that arranges it.
        let mut a = app();
        a.input = "first".into();
        assert_eq!(a.start_run().as_deref(), Some("first"));

        a.input = "second".into();
        assert!(!a.can_start());
        assert_eq!(a.start_run(), None);
        assert_eq!(
            a.input, "second",
            "the typed text is not eaten by a refusal"
        );
    }

    #[test]
    fn an_empty_task_is_not_a_run() {
        let mut a = app();
        a.input = "   ".into();
        assert!(!a.can_start());
        assert_eq!(a.start_run(), None);
    }

    #[test]
    fn the_log_is_bounded_and_scrolling_does_not_drift_when_it_evicts() {
        // A viewport pinned to a line must keep showing that line as the
        // buffer rolls, or a long run slowly scrolls itself under the reader.
        let mut a = app();
        for i in 0..LOG_CAPACITY {
            a.push_log(log(&format!("line {i}")));
        }
        a.scroll_up(10);
        let before = a.scroll;
        a.push_log(log("one more"));

        assert_eq!(a.log.len(), LOG_CAPACITY, "the buffer is bounded");
        assert_eq!(
            a.scroll,
            before - 1,
            "the viewport tracks the text it was on"
        );
    }

    #[test]
    fn scrolling_is_clamped_at_both_ends() {
        let mut a = app();
        for i in 0..5 {
            a.push_log(log(&format!("{i}")));
        }
        a.scroll_down(99);
        assert_eq!(a.scroll, 0, "cannot scroll past the tail");
        a.scroll_up(999);
        assert_eq!(a.scroll, 5, "cannot scroll past the start");
    }

    #[test]
    fn starting_a_run_returns_to_the_tail() {
        // Scrolled back, then a run starts: the interesting text is the text
        // about to arrive.
        let mut a = app();
        for i in 0..50 {
            a.push_log(log(&format!("{i}")));
        }
        a.scroll_up(20);
        a.input = "go".into();
        a.start_run();
        assert_eq!(a.scroll, 0);
    }

    #[test]
    fn a_finished_run_reports_which_way_it_went() {
        let mut a = app();
        a.input = "t".into();
        a.start_run();
        a.finish_run(RunReport {
            task: "t".into(),
            converged: false,
            summary: "stopped".into(),
            proposed: vec![],
        });
        assert!(a.status.contains("rolled back"), "{}", a.status);
        assert!(matches!(a.job, Job::Done(_)));
        // A finished job no longer blocks the next one. Typed text is the
        // other half of `can_start`, and `start_run` consumed it.
        assert!(!a.can_start(), "with the box empty there is nothing to run");
        a.input = "next".into();
        assert!(a.can_start(), "a finished run does not block the next task");
    }

    #[test]
    fn the_tick_advances_only_a_running_job() {
        let mut a = app();
        a.tick();
        assert_eq!(a.job, Job::Idle, "an idle job has nothing to animate");

        a.input = "t".into();
        a.start_run();
        a.tick();
        a.tick();
        assert!(matches!(a.job, Job::Running { frames: 2, .. }));
    }
}
