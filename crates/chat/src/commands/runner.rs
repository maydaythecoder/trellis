//! Running a generated loop, with the halts enforced **here** rather than
//! described in a prompt.
//!
//! Every rule in this module exists because the prompt-level version of it
//! is a suggestion:
//!
//! - the **cycle cap** is checked by `LoopRunner` before each cycle, not by
//!   the text the model reads;
//! - **`/stop` always works**, including mid-cycle: a `CancellationToken` is
//!   checked between every step, so a loop cannot be in a state where the
//!   only way out is killing the process;
//! - **every cycle is written to the state file before the next begins**, so
//!   an interrupted loop is resumable and a crash is diagnosable;
//! - **no progress halts it**: two cycles with an identical error set means
//!   iterating is not working, and continuing to the cap would burn four
//!   more cycles to learn nothing.
//!
//! The runner is generic over the work so it can be tested without running
//! a compiler.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Cancellation, checked between steps.
///
/// A plain `AtomicBool` behind an `Arc` rather than a dependency: the whole
/// contract is "set it from anywhere, read it everywhere", and `/stop` must
/// not be able to fail because a channel was full.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(std::sync::Arc<std::sync::atomic::AtomicBool>);

impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn cancel(&self) {
        self.0.store(true, std::sync::atomic::Ordering::SeqCst);
    }

    /// Clears the flag, so the token can be reused for the next piece of
    /// work.
    ///
    /// Without this a single `/stop` ends the session: the token stays
    /// cancelled, every later stream breaks out of its receive loop on the
    /// first chunk, and the user gets no reply and no error. Observed
    /// against a real model — one Ctrl-C and every turn after it was
    /// silently empty.
    ///
    /// Reset at the *start* of work rather than at the end of a stop, so
    /// two stops in a row cannot race a reset into re-enabling something
    /// that was meant to die.
    pub fn reset(&self) {
        self.0.store(false, std::sync::atomic::Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(std::sync::atomic::Ordering::SeqCst)
    }
}

/// The persisted form of `.harness/loops/<name>.json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoopState {
    pub cycles: u32,
    /// One entry per cycle, each the error set that cycle produced.
    #[serde(default)]
    pub errors: Vec<Vec<String>>,
}

impl LoopState {
    /// Reads the file, or the documented initial value if it is absent.
    ///
    /// A *corrupt* file is an error rather than a reset: silently starting
    /// from zero would discard the record of why a loop was interrupted,
    /// which is the one thing the file exists for.
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read_to_string(path) {
            Ok(text) => serde_json::from_str(&text)
                .map_err(|e| format!("{} is not readable loop state: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(LoopState::default()),
            Err(e) => Err(format!("{}: {e}", path.display())),
        }
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("{}: {e}", parent.display()))?;
        }
        let text = serde_json::to_string(self).map_err(|e| e.to_string())?;
        std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Why a loop stopped. Every variant is a *reported* outcome — a loop that
/// ended without one of these ended by crashing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Halt {
    /// Zero errors. The only success.
    Converged { cycles: u32 },
    /// Hit the cap with errors outstanding.
    CapReached { cycles: u32, remaining: usize },
    /// Two consecutive cycles produced the same errors.
    NoProgress { cycles: u32, repeated: Vec<String> },
    /// `/stop`, or a shutdown signal.
    Cancelled { cycles: u32 },
    /// The verification step itself could not run.
    Failed { cycles: u32, reason: String },
}

impl Halt {
    /// One line for the log pane.
    pub fn summary(&self) -> String {
        match self {
            Halt::Converged { cycles } => format!("converged after {cycles} cycle(s)"),
            Halt::CapReached { cycles, remaining } => {
                format!("halted at the {cycles}-cycle cap with {remaining} error(s) left")
            }
            Halt::NoProgress { cycles, repeated } => format!(
                "halted after {cycles} cycles: the same {} error(s) twice, so iterating is \
                 not working",
                repeated.len()
            ),
            Halt::Cancelled { cycles } => format!("stopped during cycle {}", cycles + 1),
            Halt::Failed { cycles, reason } => {
                format!("verification could not run at cycle {cycles}: {reason}")
            }
        }
    }

    pub fn converged(&self) -> bool {
        matches!(self, Halt::Converged { .. })
    }
}

/// One cycle of work: verify, then attempt fixes. Returns the errors still
/// outstanding at the end of the cycle.
pub trait Cycle {
    fn run(&mut self, cycle: u32) -> Result<Vec<String>, String>;
}

impl<F> Cycle for F
where
    F: FnMut(u32) -> Result<Vec<String>, String>,
{
    fn run(&mut self, cycle: u32) -> Result<Vec<String>, String> {
        self(cycle)
    }
}

/// Drives a loop to one of the halts above.
pub struct LoopRunner {
    pub state_file: PathBuf,
    pub halt_threshold: u32,
    pub cancel: CancelToken,
}

impl LoopRunner {
    pub fn new(state_file: PathBuf, halt_threshold: u32, cancel: CancelToken) -> Self {
        Self {
            state_file,
            halt_threshold,
            cancel,
        }
    }

    /// Runs until a halt. Never returns without one.
    pub fn run(&self, work: &mut dyn Cycle) -> Halt {
        let mut state = match LoopState::load(&self.state_file) {
            Ok(s) => s,
            Err(reason) => {
                return Halt::Failed { cycles: 0, reason };
            }
        };

        loop {
            // Cancellation before the cap, so `/stop` is honoured even on a
            // loop that has already exhausted its cycles.
            if self.cancel.is_cancelled() {
                return Halt::Cancelled {
                    cycles: state.cycles,
                };
            }
            // The cap, checked by the runtime before every cycle. This is
            // the line that makes it a cap rather than a suggestion.
            if state.cycles >= self.halt_threshold {
                let remaining = state.errors.last().map(Vec::len).unwrap_or(0);
                return Halt::CapReached {
                    cycles: state.cycles,
                    remaining,
                };
            }

            let errors = match work.run(state.cycles) {
                Ok(e) => e,
                Err(reason) => {
                    return Halt::Failed {
                        cycles: state.cycles,
                        reason,
                    };
                }
            };

            // Recorded *before* any of the halts below, including
            // cancellation. The cycle really ran, and the errors it found
            // are the diagnostic a resumed run starts from — discarding
            // them because `/stop` arrived a moment later would make an
            // interrupted loop less resumable than a crashed one.
            let previous = state.errors.last().cloned();
            state.cycles += 1;
            state.errors.push(errors.clone());
            if let Err(reason) = state.save(&self.state_file) {
                return Halt::Failed {
                    cycles: state.cycles,
                    reason,
                };
            }

            // Convergence first: a cycle that reached zero errors succeeded,
            // and a `/stop` arriving in the same instant does not undo that.
            if errors.is_empty() {
                return Halt::Converged {
                    cycles: state.cycles,
                };
            }
            // Checked again here: a cycle can take minutes, and a `/stop`
            // during one must not be discovered only after the next has
            // started.
            if self.cancel.is_cancelled() {
                return Halt::Cancelled {
                    cycles: state.cycles,
                };
            }
            // No progress: the same set twice means the next cycle has
            // nothing new to work from, and four more would learn nothing.
            if previous.as_ref().is_some_and(|p| same_set(p, &errors)) {
                return Halt::NoProgress {
                    cycles: state.cycles,
                    repeated: errors,
                };
            }
        }
    }
}

/// Order-insensitive comparison. A checker that lists the same failures in a
/// different order has not made progress, and comparing as sequences would
/// let a reordering look like one.
fn same_set(a: &[String], b: &[String]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut a: Vec<&str> = a.iter().map(String::as_str).collect();
    let mut b: Vec<&str> = b.iter().map(String::as_str).collect();
    a.sort_unstable();
    b.sort_unstable();
    a == b
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("chat-runner-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("state.json")
    }

    #[test]
    fn a_loop_halts_at_the_cap_rather_than_running_forever() {
        let f = tmp("cap");
        let r = LoopRunner::new(f.clone(), 5, CancelToken::new());
        let mut n = 0;
        // Errors that keep changing, so the no-progress halt cannot fire and
        // the cap is the only thing that can stop this.
        let halt = r.run(&mut |_: u32| {
            n += 1;
            Ok(vec![format!("E{n}")])
        });
        assert_eq!(
            halt,
            Halt::CapReached {
                cycles: 5,
                remaining: 1
            }
        );
        assert_eq!(n, 5, "the cap is checked before each cycle, not after");
    }

    #[test]
    fn a_loop_halts_on_no_progress_long_before_the_cap() {
        // Two cycles with the same error set means iterating is not working.
        // Running to the cap anyway costs four more cycles to learn nothing.
        let f = tmp("noprogress");
        let r = LoopRunner::new(f, 5, CancelToken::new());
        let mut n = 0;
        let halt = r.run(&mut |_: u32| {
            n += 1;
            Ok(vec!["E0308".to_string()])
        });
        assert!(
            matches!(halt, Halt::NoProgress { cycles: 2, .. }),
            "got {halt:?}"
        );
        assert_eq!(n, 2);
    }

    #[test]
    fn reordered_errors_are_not_progress() {
        let f = tmp("reorder");
        let r = LoopRunner::new(f, 5, CancelToken::new());
        let mut n = 0;
        let halt = r.run(&mut |_: u32| {
            n += 1;
            Ok(if n % 2 == 1 {
                vec!["a".into(), "b".into()]
            } else {
                vec!["b".into(), "a".into()]
            })
        });
        assert!(
            matches!(halt, Halt::NoProgress { .. }),
            "a checker listing the same failures in a different order has \
             not made progress: {halt:?}"
        );
    }

    #[test]
    fn zero_errors_is_the_only_success() {
        let f = tmp("converge");
        let r = LoopRunner::new(f, 5, CancelToken::new());
        let mut n = 0;
        let halt = r.run(&mut |_: u32| {
            n += 1;
            Ok(if n < 3 { vec![format!("E{n}")] } else { vec![] })
        });
        assert_eq!(halt, Halt::Converged { cycles: 3 });
        assert!(halt.converged());
    }

    #[test]
    fn a_token_can_be_reused_after_a_stop() {
        // A cancelled token that stays cancelled ends the session: every
        // later stream breaks out on its first chunk and the user gets no
        // reply and no error. Seen against a real model — one Ctrl-C and
        // every turn after it was empty.
        let t = CancelToken::new();
        t.cancel();
        assert!(t.is_cancelled());
        t.reset();
        assert!(!t.is_cancelled(), "the next turn has to be able to run");
        // Clones share the flag, which is what makes one reset enough.
        let c = t.clone();
        t.cancel();
        assert!(c.is_cancelled());
        c.reset();
        assert!(!t.is_cancelled());
    }

    #[test]
    fn stop_works_mid_cycle_and_not_only_between_them() {
        // The difference that matters: a cycle can take minutes, and a
        // `/stop` the runner only notices at the top of the next one leaves
        // the user with a process they have to kill.
        let f = tmp("cancel");
        let f2 = f.clone();
        let cancel = CancelToken::new();
        let r = LoopRunner::new(f, 5, cancel.clone());
        let mut n = 0;
        let halt = r.run(&mut |_: u32| {
            n += 1;
            cancel.cancel(); // as if /stop arrived while this cycle ran
            Ok(vec![format!("E{n}")])
        });
        assert_eq!(halt, Halt::Cancelled { cycles: 1 });
        assert_eq!(n, 1, "the second cycle must not start");
        // And what that cycle found is on disk, not thrown away because the
        // stop arrived a moment after it finished.
        assert_eq!(
            LoopState::load(&f2).unwrap().errors,
            vec![vec!["E1".to_string()]]
        );
    }

    #[test]
    fn a_cycle_that_converged_is_not_reported_as_cancelled() {
        // Both conditions are true at once here. Reporting the stop would
        // lose a real success and send the user back to re-run a loop that
        // had already finished.
        let f = tmp("converge-and-cancel");
        let cancel = CancelToken::new();
        let r = LoopRunner::new(f, 5, cancel.clone());
        let halt = r.run(&mut |_: u32| {
            cancel.cancel();
            Ok(vec![])
        });
        assert_eq!(halt, Halt::Converged { cycles: 1 });
    }

    #[test]
    fn stop_is_honoured_even_on_a_loop_that_is_already_at_its_cap() {
        let f = tmp("cancel-at-cap");
        LoopState {
            cycles: 5,
            errors: vec![vec!["E".into()]],
        }
        .save(&f)
        .unwrap();
        let cancel = CancelToken::new();
        cancel.cancel();
        let r = LoopRunner::new(f, 5, cancel);
        let halt = r.run(&mut |_: u32| Ok(vec![]));
        assert_eq!(halt, Halt::Cancelled { cycles: 5 });
    }

    #[test]
    fn every_cycle_is_on_disk_before_the_next_one_starts() {
        // What makes an interrupted loop resumable and a crash diagnosable.
        let f = tmp("persist");
        let r = LoopRunner::new(f.clone(), 5, CancelToken::new());
        let seen = std::cell::RefCell::new(Vec::new());
        let mut n = 0;
        let _ = r.run(&mut |_: u32| {
            // What is on disk at the *start* of each cycle.
            seen.borrow_mut().push(LoopState::load(&f).unwrap().cycles);
            n += 1;
            Ok(vec![format!("E{n}")])
        });
        assert_eq!(
            seen.into_inner(),
            vec![0, 1, 2, 3, 4],
            "each cycle must see the previous one already written"
        );
        assert_eq!(LoopState::load(&f).unwrap().errors.len(), 5);
    }

    #[test]
    fn a_resumed_loop_counts_the_cycles_it_already_ran() {
        let f = tmp("resume");
        LoopState {
            cycles: 4,
            errors: vec![vec!["old".into()]],
        }
        .save(&f)
        .unwrap();
        let r = LoopRunner::new(f, 5, CancelToken::new());
        let mut n = 0;
        let halt = r.run(&mut |_: u32| {
            n += 1;
            Ok(vec!["new".to_string()])
        });
        assert_eq!(n, 1, "four cycles are already spent");
        assert!(
            matches!(halt, Halt::CapReached { cycles: 5, .. }),
            "{halt:?}"
        );
    }

    #[test]
    fn corrupt_state_is_reported_rather_than_silently_reset() {
        // Starting from zero would discard the record of why a loop was
        // interrupted, which is the one thing the file exists for.
        let f = tmp("corrupt");
        std::fs::write(&f, "{ not json").unwrap();
        let r = LoopRunner::new(f, 5, CancelToken::new());
        let halt = r.run(&mut |_: u32| Ok(vec![]));
        assert!(matches!(halt, Halt::Failed { cycles: 0, .. }), "{halt:?}");
    }

    #[test]
    fn a_verification_step_that_cannot_run_is_not_a_pass() {
        // The whole value of a self-verifying loop is the verification. A
        // checker that failed to start must never read as zero errors.
        let f = tmp("failed");
        let r = LoopRunner::new(f, 5, CancelToken::new());
        let halt = r.run(&mut |_: u32| Err("cargo: not found".to_string()));
        assert!(matches!(halt, Halt::Failed { .. }), "{halt:?}");
        assert!(!halt.converged());
    }

    #[test]
    fn every_halt_says_something_a_user_can_act_on() {
        for h in [
            Halt::Converged { cycles: 2 },
            Halt::CapReached {
                cycles: 5,
                remaining: 3,
            },
            Halt::NoProgress {
                cycles: 2,
                repeated: vec!["E".into()],
            },
            Halt::Cancelled { cycles: 1 },
            Halt::Failed {
                cycles: 0,
                reason: "x".into(),
            },
        ] {
            assert!(!h.summary().is_empty(), "{h:?}");
        }
    }
}
