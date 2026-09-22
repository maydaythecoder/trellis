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
    RunFinished,
    // internal
    Quit,
}
