//! Entering and leaving the alternate screen, and surviving a panic.

use std::panic;

/// Enters the alternate screen and raw mode, then installs the panic hook.
///
/// The order matters: the hook must be in place before anything can panic
/// while the terminal is in raw mode.
pub fn init() -> ratatui::DefaultTerminal {
    let terminal = ratatui::init();
    install_panic_hook();
    terminal
}

pub fn restore() {
    ratatui::restore();
}

/// Restores the terminal *before* the original hook prints.
///
/// Wrapping rather than replacing: whatever hook was already installed still
/// prints its report, onto a terminal that echoes. In `trellis tui` that is
/// the std default — `color-eyre` is behind this crate's `demo` feature and is
/// not compiled into the shipped binary. Without this a panic leaves the
/// user with no echo, no line editing and a stuck alternate screen — the
/// state where the only way out is `reset` typed blind.
fn install_panic_hook() {
    let original = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        ratatui::restore();
        original(info);
    }));
}
