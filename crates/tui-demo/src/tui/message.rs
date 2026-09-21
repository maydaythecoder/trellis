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
