//! Everything that can reach the interface, as one type.
//!
//! Terminal events and work events arrive on different channels and are
//! merged into this before anything sees them, so `AppState::update` has a
//! single entry point and the event loop has one thing to match on.

use ratatui::crossterm::event::KeyEvent;

#[derive(Debug, Clone)]
pub enum Message {
    // terminal
    Key(KeyEvent),
    Resize(u16, u16),
    Tick,
    // work events, mocked for now
    AgentToken(String),
    EditProposed {
        path: String,
    },
    CheckFinished {
        name: String,
        passed: bool,
    },
    /// What a probe found the adapter tier to be.
    ///
    /// The status bar used to show a tier derived from the spec string, which
    /// for every runtime spec meant the constant `2`. Whether a runtime
    /// honours a schema cannot be read off its address — it has to be asked —
    /// so the label arrives once something has asked.
    TierKnown(String),
    /// One thing the agent did. The action graph is drawn from these.
    ///
    /// Carried as its parts rather than as the rendered call label the
    /// transcript shows: the graph groups by tool and counts writes, and
    /// re-parsing a string assembled for display is not a way to know either.
    ActionTaken {
        tool: String,
        target: String,
        wrote: bool,
    },
    RadiusUpdated {
        resolved: usize,
        candidates: usize,
        unresolved: usize,
    },
    BudgetUpdated {
        iteration: u32,
        tokens: u64,
        elapsed_secs: u64,
    },
    ApprovalRequested {
        diff: String,
    },
    /// A steer the view queued reached the model (#113, #115).
    ///
    /// `nth` is its position among the steers queued during this run, counted
    /// from 0, and not the text: the loop reports a delivery without the text,
    /// and naming it by position rather than taking the oldest one still
    /// pending means a lost or repeated message cannot pin one steer's
    /// delivery on another. `turn` is the turn whose request carried it.
    SteerDelivered {
        nth: usize,
        turn: usize,
    },
    RunFinished,
    // internal
    Quit,
}

/// What the event loop sends on the steer channel (#115).
///
/// Steers and tasks leave on different channels, and a driver that has to
/// tell the last run's leftover steers from the next run's first ones
/// cannot do it by timing: a pasted task and steer can both be sent before
/// the driver next runs. `TaskFollows` puts the boundary on the steer
/// channel itself, in the order the lines were typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SteerOut {
    /// A line typed while a run was in flight, for that run.
    Steer(String),
    /// Sent immediately before a task: every steer ahead of this was typed
    /// for an earlier task, and every steer after it for this one or later.
    TaskFollows,
}
