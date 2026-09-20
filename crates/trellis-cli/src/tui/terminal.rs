//! Entering and leaving raw mode, exactly once each.
//!
//! Terminal setup is the one piece of a TUI that damages something outside
//! the process when it goes wrong: a program that exits without leaving the
//! alternate screen and disabling raw mode hands the user a shell that no
//! longer echoes, and the usual response is to close the window.
//!
//! Two mechanisms, because they cover different failures and neither covers
//! both:
//!
//! - `ratatui::try_init` installs a panic hook that restores before the
//!   original hook runs. That covers a panic, and only a panic.
//! - [`TerminalGuard`] restores on `Drop`. That covers every ordinary exit
//!   path — including the `?` that returns an error four frames deep, which
//!   no panic hook ever sees.
//!
//! The guard owns the terminal rather than sitting beside it, so there is no
//! way to hold one without the other and no "remember to call restore".

use crossterm::event::{DisableMouseCapture, EnableMouseCapture};
use ratatui::DefaultTerminal;
use std::io;

/// A live terminal in raw mode and the alternate screen.
///
/// Dereferences to the terminal, so callers draw through it without being
/// able to take it out and outlive the restoration.
pub(crate) struct TerminalGuard {
    inner: Option<DefaultTerminal>,
}

impl TerminalGuard {
    /// Enters raw mode and the alternate screen, and arms both restore paths.
    pub(crate) fn enter() -> io::Result<Self> {
        // Installs the panic hook as well as switching modes.
        let inner = ratatui::try_init()?;

        // Mouse capture is ours: `try_init` does not enable it, and neither
        // does `restore` disable it. Anything enabled here is disabled in
        // `restore` below, or a terminal that outlives this process keeps
        // reporting mouse movement as escape sequences into the user's shell.
        if let Err(e) = crossterm::execute!(io::stdout(), EnableMouseCapture) {
            // Undo the half-entered state rather than leave a terminal in raw
            // mode because one of two steps failed.
            ratatui::restore();
            return Err(e);
        }
        Ok(Self { inner: Some(inner) })
    }

    pub(crate) fn terminal(&mut self) -> &mut DefaultTerminal {
        self.inner
            .as_mut()
            .expect("the terminal is taken only in Drop")
    }

    /// Restores early, so the caller can print to a normal terminal before
    /// the guard goes out of scope.
    ///
    /// Idempotent: `Drop` checks whether this already ran. Restoring twice is
    /// harmless to the terminal but the bookkeeping matters, because a second
    /// restore after the caller has started writing normal output would clear
    /// what they just wrote.
    pub(crate) fn restore(&mut self) {
        if self.inner.take().is_some() {
            // Mirrors `enter` exactly, and in reverse order. Ignored on
            // failure: this runs on the way out, including from `Drop` during
            // an unwind, and a panic here would abort the process while
            // replacing the message the user needed to read.
            let _ = crossterm::execute!(io::stdout(), DisableMouseCapture);
            ratatui::restore();
        }
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        self.restore();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The guard must be usable without a terminal attached, because tests
    /// and CI have none — and a guard that panics when stdout is not a TTY
    /// would make every one of them a terminal test.
    #[test]
    fn entering_without_a_terminal_is_an_error_rather_than_a_panic() {
        // Whether this succeeds depends on how the test harness is attached,
        // so the assertion is about the shape of the outcome, not which one.
        if let Ok(mut g) = TerminalGuard::enter() {
            // If it did enter, it must leave cleanly and twice over.
            g.restore();
            g.restore();
        }
    }
}
