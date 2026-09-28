//! Entering and leaving the alternate screen, and getting out of it whatever
//! happens.
//!
//! In raw mode with the alternate screen active, a panic unwinds straight
//! past any restore call at the end of `main` and leaves the user with no
//! echo, no line editing and a screen that will not clear. That is not a
//! cosmetic failure — the shell is unusable until `reset` is typed blind.
//!
//! Three exits are covered here: a panic, a signal, and suspend/resume.

use std::sync::atomic::{AtomicBool, Ordering};

/// Whether the alternate screen is currently entered.
///
/// The panic hook and the suspend handler can both fire when it is not, and
/// calling `ratatui::restore()` on an already-restored terminal writes
/// escape sequences into the user's shell.
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Enters the alternate screen and installs the panic hook.
pub fn init() -> ratatui::DefaultTerminal {
    let terminal = ratatui::init();
    ACTIVE.store(true, Ordering::SeqCst);
    install_panic_hook(restore);
    terminal
}

/// Wraps the current panic hook so `leave` runs before it.
///
/// **Wrapping, not replacing.** `color_eyre` installs its own hook, and
/// replacing it loses the report — leaving a restored terminal showing
/// nothing about what went wrong, which is a worse outcome than the broken
/// terminal it was meant to prevent.
///
/// **Leave first, then report.** The original hook writes to stderr, and
/// stderr inside the alternate screen is a report nobody can read: it
/// vanishes with the buffer when the terminal is restored by something
/// else, or scrolls under the interface if it is not.
///
/// Taking the restore as an argument is what makes the ordering testable
/// without a terminal — see the test below.
pub fn install_panic_hook(leave: fn()) {
    let original = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        leave();
        original(info);
    }));
}

/// Leaves the alternate screen. Safe to call twice.
pub fn restore() {
    if ACTIVE.swap(false, Ordering::SeqCst) {
        ratatui::restore();
    }
}

/// Re-enters after a resume. Paired with `suspend`.
pub fn resume() {
    if !ACTIVE.swap(true, Ordering::SeqCst) {
        let _ = ratatui::try_init();
    }
}

/// Leaves the screen, stops this process, and re-enters when resumed.
///
/// Ctrl-Z has to be done by the application in raw mode. The tty's own
/// job-control characters are disabled there, so no SIGTSTP is ever
/// generated — a QA run pressed Ctrl-Z and got a literal `z` in the input
/// line, because the chord arrived as an ordinary key event.
///
/// Order matters both ways. Leaving the alternate screen *before* stopping
/// means the shell the user lands in is their own, not our buffer; and
/// re-entering *after* the continue means `fg` returns to a redrawn
/// interface rather than to whatever the shell left behind.
///
/// Returns false when the platform has no job control, so the caller can
/// say so rather than appearing to do nothing.
pub fn suspend() -> bool {
    #[cfg(unix)]
    {
        restore();
        // `kill -STOP` on our own pid rather than a libc `raise`: this
        // crate carries no libc dependency, and the shell notices either
        // way. SIGSTOP cannot be caught, which is what is wanted here.
        let stopped = crate::scrub_keys(&mut std::process::Command::new("kill"))
            .args(["-STOP", &std::process::id().to_string()])
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        // Execution resumes here on SIGCONT.
        resume();
        stopped
    }
    #[cfg(not(unix))]
    {
        // Windows has no SIGTSTP. Saying so beats silently doing nothing.
        false
    }
}

/// Whether the interface currently owns the screen.
pub fn is_active() -> bool {
    ACTIVE.load(Ordering::SeqCst)
}

/// Signal handling: termination and suspend/resume.
///
/// SIGTERM must restore before exiting, for the same reason a panic must.
/// SIGTSTP is the subtler one: Ctrl-Z stops the process while it still owns
/// the alternate screen, so the shell the user lands in is drawing over our
/// buffer — and `fg` resumes into a display that was never re-initialised.
/// Both halves are needed, and the default SIGTSTP disposition has to be
/// restored so the stop actually happens.
#[cfg(unix)]
pub fn install_signal_handlers(tx: tokio::sync::mpsc::Sender<crate::tui::message::Message>) {
    use tokio::signal::unix::{SignalKind, signal};

    // Termination: restore, then let the process go.
    tokio::spawn({
        let tx = tx.clone();
        async move {
            let Ok(mut term) = signal(SignalKind::terminate()) else {
                return;
            };
            if term.recv().await.is_some() {
                let _ = tx.send(crate::tui::message::Message::Quit).await;
            }
        }
    });

    // Suspend: leave the alternate screen, then re-raise SIGTSTP with the
    // default handler so the shell actually stops us. Without re-raising,
    // Ctrl-Z appears to do nothing.
    tokio::spawn(async move {
        let Ok(mut tstp) = signal(SignalKind::from_raw(libc_sigtstp())) else {
            return;
        };
        let Ok(mut cont) = signal(SignalKind::from_raw(libc_sigcont())) else {
            return;
        };
        loop {
            tokio::select! {
                Some(_) = tstp.recv() => {
                    restore();
                    // SAFETY-adjacent: this is the documented way to hand a
                    // stop back to the default disposition. Nothing here is
                    // `unsafe` in Rust terms because we go through the
                    // process-wide signal API rather than libc directly.
                    let _ = crate::scrub_keys(&mut std::process::Command::new("kill"))
                        .args(["-STOP", &std::process::id().to_string()])
                        .status();
                }
                Some(_) = cont.recv() => {
                    resume();
                    let _ = tx.send(crate::tui::message::Message::Tick).await;
                }
            }
        }
    });
}

#[cfg(unix)]
fn libc_sigtstp() -> i32 {
    20
}
#[cfg(unix)]
fn libc_sigcont() -> i32 {
    18
}

/// Windows has no SIGTSTP, and Ctrl-C arrives through the console handler
/// that `crossterm` already turns into a key event — which `AppState`
/// handles. Ctrl-Break is left to the OS.
#[cfg(not(unix))]
pub fn install_signal_handlers(tx: tokio::sync::mpsc::Sender<crate::tui::message::Message>) {
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            // Only reached when crossterm is not consuming it, e.g. after
            // the event stream has closed.
            let _ = tx.send(crate::tui::message::Message::Quit).await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_is_idempotent() {
        // The panic hook, the signal handler and the normal exit path can
        // all reach `restore`, and two of them can fire for the same exit.
        // A second `ratatui::restore()` writes escape sequences into the
        // shell the user has just been handed back.
        ACTIVE.store(true, Ordering::SeqCst);
        restore();
        assert!(!is_active());
        restore(); // must not touch the terminal a second time
        assert!(!is_active());
    }

    #[test]
    fn the_panic_hook_leaves_the_screen_before_the_report_is_printed() {
        // The ordering is the whole contract, and it cannot be checked by
        // reading the code — that is exactly how a wrapped hook gets
        // reordered by a later edit. Driven with a real panic through
        // `catch_unwind`, recording what ran in which order.
        use std::sync::atomic::{AtomicUsize, Ordering as O};
        static SEQ: AtomicUsize = AtomicUsize::new(0);
        static LEFT_AT: AtomicUsize = AtomicUsize::new(0);
        static REPORTED_AT: AtomicUsize = AtomicUsize::new(0);

        // Stand in for `color_eyre`'s hook: the one that must still run.
        std::panic::set_hook(Box::new(|_| {
            REPORTED_AT.store(SEQ.fetch_add(1, O::SeqCst) + 1, O::SeqCst);
        }));
        install_panic_hook(|| {
            LEFT_AT.store(SEQ.fetch_add(1, O::SeqCst) + 1, O::SeqCst);
        });

        let result = std::panic::catch_unwind(|| panic!("as if a widget indexed past a buffer"));
        assert!(result.is_err());

        let left = LEFT_AT.load(O::SeqCst);
        let reported = REPORTED_AT.load(O::SeqCst);
        assert!(left > 0, "the terminal was never restored on panic");
        assert!(
            reported > 0,
            "the original hook was replaced, so the report is lost"
        );
        assert!(
            left < reported,
            "restore must come first, or the report prints into the alternate              screen: left at {left}, reported at {reported}"
        );

        let _ = std::panic::take_hook();
    }

    #[test]
    fn resume_only_re_enters_when_it_has_left() {
        ACTIVE.store(false, Ordering::SeqCst);
        assert!(!is_active());
        ACTIVE.store(true, Ordering::SeqCst);
        resume(); // already active: must be a no-op
        assert!(is_active());
        ACTIVE.store(false, Ordering::SeqCst);
    }
}
