//! Entering and leaving the alternate screen, and surviving a panic.

use ratatui::crossterm::event::{DisableBracketedPaste, EnableBracketedPaste};
use ratatui::crossterm::execute;
use std::io::{self, Write};
use std::panic;

/// Enters the alternate screen and raw mode, then installs the panic hook.
///
/// The order matters: the hook must be in place before anything can panic
/// while the terminal is in raw mode.
///
/// Bracketed paste is turned on after that (#639): `ratatui::init` leaves
/// it off, and a terminal without it types a paste key by key, so a pasted
/// task's line breaks reached the input as Enters. With it the paste
/// arrives as one `Event::Paste`.
pub fn init() -> ratatui::DefaultTerminal {
    let terminal = ratatui::init();
    install_panic_hook();
    // The same stdout `ratatui::init` just wrote the alternate-screen
    // sequence to, so a failure here is one `init` would have panicked on
    // already; and it panics under the hook, which restores the terminal.
    enable_paste(&mut io::stdout()).expect("failed to enable bracketed paste");
    terminal
}

pub fn restore() {
    // Before leaving the alternate screen, and whatever the write's result:
    // the terminal is being handed back, and the rest of `restore` must run.
    let _ = disable_paste(&mut io::stdout());
    ratatui::restore();
}

/// The enable sequence, onto `w`. Separate from `init` so a test can read
/// the bytes without a terminal.
fn enable_paste(w: &mut impl Write) -> io::Result<()> {
    execute!(w, EnableBracketedPaste)
}

/// The disable sequence, onto `w`. Left on, a shell that does not expect
/// bracketed paste would get `\x1b[200~` typed around every paste after
/// this exits.
fn disable_paste(w: &mut impl Write) -> io::Result<()> {
    execute!(w, DisableBracketedPaste)
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
        restore();
        original(info);
    }));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// #639: a terminal left in bracketed-paste mode after exit wraps every
    /// paste the shell gets in `\x1b[200~`; one never put in it types a
    /// paste as keystrokes. The bytes `init` and `restore` write are the
    /// mode's enable and disable codes.
    #[test]
    fn paste_mode_is_enabled_by_init_and_disabled_by_restore() {
        let mut on = Vec::new();
        enable_paste(&mut on).unwrap();
        assert_eq!(on, b"\x1b[?2004h");

        let mut off = Vec::new();
        disable_paste(&mut off).unwrap();
        assert_eq!(off, b"\x1b[?2004l");
    }
}
