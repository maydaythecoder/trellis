//! Per-run ceilings on iterations, wall-clock, and tokens.
//!
//! Tracked through `tracing` spans so accounting and observability are one
//! mechanism rather than two (4.11). On exhaustion the orchestrator
//! escalates once with the current state, then rolls back.

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

/// Which resource is actually scarce, which inverts by configuration.
///
/// On a frontier API, tokens cost money and are the binding constraint. On
/// local weights, marginal token cost approaches zero and wall-clock is the
/// budget. The budget code has to know which regime it is in, because
/// warning about token spend on a local model is noise and *not* warning
/// about it on an API is expensive (S8).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CostRegime {
    /// Metered API: tokens are the scarce resource.
    Metered,
    /// Local weights: wall-clock is the scarce resource.
    Local,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Budget {
    pub max_iterations: u32,
    pub max_wall_clock: Duration,
    /// Applies regardless of regime. The user's key means a runaway loop
    /// spends the user's money, so the global ceiling is not optional even
    /// when tokens are notionally free.
    pub max_tokens: u64,
    pub regime: CostRegime,
}

impl Default for Budget {
    fn default() -> Self {
        // ASSUMPTION A-7: these are placeholders pending calibration from
        // real usage (S8). They are not measurements and must not be cited
        // as any. See ASSUMPTIONS.md. ADR 0008's 2026-10-02 amendment
        // records them and the rule that will replace them (p90 of
        // converged runs + 50%), which had no converged run to apply to;
        // `default_budget_matches_adr_0008` keeps the two in step.
        Self {
            max_iterations: 40,
            max_wall_clock: Duration::from_secs(20 * 60),
            max_tokens: 2_000_000,
            regime: CostRegime::Local,
        }
    }
}

/// The clock a run's wall-clock ceiling is checked against: monotonic time
/// since the loop started, less the time spent waiting on a person in
/// approve mode. A diff read carefully should not be what ends a run.
///
/// One clock for the ceiling and for the figure the run reports
/// (`elapsed_ms` on its events and `run_end`), so a client draws elapsed
/// against the ceiling with the number the ceiling is checked against.
#[derive(Debug)]
pub(crate) struct WallClock {
    started: Instant,
    waited_on_human: Duration,
}

impl WallClock {
    pub(crate) fn start() -> Self {
        Self {
            started: Instant::now(),
            waited_on_human: Duration::ZERO,
        }
    }

    /// What the run has spent of its wall clock so far.
    pub(crate) fn spent(&self) -> Duration {
        self.started.elapsed().saturating_sub(self.waited_on_human)
    }

    /// Takes a person's answer time out of what the run has spent.
    pub(crate) fn leave_out(&mut self, waited_on_human: Duration) {
        self.waited_on_human += waited_on_human;
    }
}

/// Running total against a `Budget`.
#[derive(Debug, Clone, Default)]
pub struct Spend {
    pub iterations: u32,
    pub elapsed: Duration,
    pub tokens: u64,
}

/// Why a run stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Exhaustion {
    Iterations,
    WallClock,
    Tokens,
}

impl Exhaustion {
    /// A legible reason, for the user and for the trail. Failure is
    /// supposed to be boring (goal state 4).
    pub fn explain(self) -> &'static str {
        match self {
            Exhaustion::Iterations => "iteration ceiling reached without converging; rolled back",
            Exhaustion::WallClock => "wall-clock ceiling reached; rolled back",
            Exhaustion::Tokens => "token ceiling reached; rolled back",
        }
    }
}

impl Budget {
    pub fn exceeded(&self, spend: &Spend) -> Option<Exhaustion> {
        if spend.iterations >= self.max_iterations {
            Some(Exhaustion::Iterations)
        } else if spend.elapsed >= self.max_wall_clock {
            Some(Exhaustion::WallClock)
        } else if spend.tokens >= self.max_tokens {
            Some(Exhaustion::Tokens)
        } else {
            None
        }
    }

    /// This budget's iterations and tokens dealt out across `n` partitions,
    /// exactly.
    ///
    /// Budgets **divide, they do not multiply**. N agents each granted the
    /// full ceiling is an N-times faster way to spend the user's money
    /// (4.13). Flooring each share would leave iterations nobody may spend,
    /// and clamping each to one would grant a wide plan more than the
    /// ceiling, ADR 0008's "multiply" by another route. So the remainder
    /// goes to the first partitions in plan order, the shares sum to the
    /// run's ceiling, and a plan wider than the ceiling is refused rather
    /// than granted more than the user set.
    ///
    /// Wall-clock is copied whole: partitions in one stage run
    /// concurrently. The supervisor narrows it for a later stage.
    pub fn shares(&self, n: u32) -> Result<Vec<Budget>, BudgetTooSmall> {
        if n == 0 {
            return Ok(Vec::new());
        }
        if self.max_iterations < n || self.max_tokens < u64::from(n) {
            return Err(BudgetTooSmall {
                partitions: n,
                iterations: self.max_iterations,
                tokens: self.max_tokens,
            });
        }
        let (it_each, it_rest) = (self.max_iterations / n, self.max_iterations % n);
        let (tok_each, tok_rest) = (
            self.max_tokens / u64::from(n),
            self.max_tokens % u64::from(n),
        );
        Ok((0..n)
            .map(|i| Budget {
                max_iterations: it_each + u32::from(i < it_rest),
                max_wall_clock: self.max_wall_clock,
                max_tokens: tok_each + u64::from(u64::from(i) < tok_rest),
                regime: self.regime,
            })
            .collect())
    }
}

/// The ceilings a person asks to raise during a run (ADR 0058 §2). `None`
/// leaves a ceiling as it is.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Raise {
    pub max_iterations: Option<u32>,
    pub max_wall_clock: Option<Duration>,
    pub max_tokens: Option<u64>,
}

/// Why a [`Raise`] changed nothing.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RaiseRefused {
    #[error("name a ceiling to raise: max_iterations, max_wall_clock_ms or max_completion_tokens")]
    NothingNamed,
    /// Lowering is refused whole: a limit below the spend ends the run,
    /// which `abort` already does, and a lowered ceiling has no defined
    /// share for partitions part-way through a stage (ADR 0058).
    #[error(
        "{ceiling} {asked} is below the run's ceiling of {current}; a ceiling is raised \
         during a run, never lowered, and nothing changed"
    )]
    Lower {
        ceiling: &'static str,
        asked: u64,
        current: u64,
    },
    /// No loop has adopted the budget yet, so nothing knows the ceiling a
    /// raise would be measured against.
    #[error("the run has not reached its first budget check yet; nothing changed")]
    NotStarted,
    /// No loop will check the budget again: the reason, as given.
    #[error("{0}")]
    Closed(String),
}

impl Budget {
    /// This budget with `r`'s ceilings, or the first one `r` would lower.
    /// All or nothing, so a refused request leaves no ceiling half-moved; a
    /// value equal to the ceiling changes nothing. The regime is kept.
    pub fn raised(&self, r: &Raise) -> Result<Budget, RaiseRefused> {
        if *r == Raise::default() {
            return Err(RaiseRefused::NothingNamed);
        }
        let lower = |ceiling, asked: u64, current: u64| {
            (asked < current).then_some(RaiseRefused::Lower {
                ceiling,
                asked,
                current,
            })
        };
        let ms = |d: Duration| u64::try_from(d.as_millis()).unwrap_or(u64::MAX);
        let refused = [
            r.max_iterations.and_then(|v| {
                lower(
                    "max_iterations",
                    u64::from(v),
                    u64::from(self.max_iterations),
                )
            }),
            r.max_wall_clock
                .and_then(|v| lower("max_wall_clock_ms", ms(v), ms(self.max_wall_clock))),
            r.max_tokens
                .and_then(|v| lower("max_completion_tokens", v, self.max_tokens)),
        ];
        if let Some(e) = refused.into_iter().flatten().next() {
            return Err(e);
        }
        Ok(Budget {
            max_iterations: r.max_iterations.unwrap_or(self.max_iterations),
            max_wall_clock: r.max_wall_clock.unwrap_or(self.max_wall_clock),
            max_tokens: r.max_tokens.unwrap_or(self.max_tokens),
            regime: self.regime,
        })
    }
}

/// A run's whole ceiling as a person may raise it while the run goes on
/// (ADR 0058 §2), shared between the loop and whoever takes the person's
/// request.
///
/// It is the **whole run's** ceiling, never a partition's share: a loop that
/// adopted it with a share would let one raise be granted to every
/// partition, ADR 0008's "multiply". A partitioned run deals the raised
/// whole through [`Budget::shares`], as it dealt the first, so the shares sum
/// to the raised ceiling (rule 9).
///
/// Refused until a loop adopts it, since until then nothing knows the
/// ceiling a raise would be measured against, and refused once closed, as
/// the loop closes it on return. A raise accepted during the run's last
/// turn reaches no check and changes nothing: nothing can know which turn
/// is the last until the model has chosen its call.
#[derive(Debug, Default)]
pub struct LiveBudget {
    state: std::sync::Mutex<LiveState>,
}

#[derive(Debug, Default)]
struct LiveState {
    /// The whole run's ceiling, with every raise accepted so far.
    current: Option<Budget>,
    /// A raise accepted and not yet taken by the loop.
    raised: bool,
    /// Set once; from then on every raise is refused with it.
    closed: Option<String>,
}

impl LiveBudget {
    fn state(&self) -> std::sync::MutexGuard<'_, LiveState> {
        // Every holder writes the state whole, so a poisoned lock still
        // guards a consistent one.
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Raises the run's ceilings, answered with all of them after the
    /// change. The loop enforces them from its next budget check.
    pub fn raise(&self, r: &Raise) -> Result<Budget, RaiseRefused> {
        let mut s = self.state();
        if let Some(why) = &s.closed {
            return Err(RaiseRefused::Closed(why.clone()));
        }
        let Some(current) = &s.current else {
            return Err(RaiseRefused::NotStarted);
        };
        let next = current.raised(r)?;
        if !same_ceilings(&next, current) {
            s.raised = true;
            s.current = Some(next.clone());
        }
        Ok(next)
    }

    /// Ends raising for good; later raises are refused with `why`. The
    /// first reason stands.
    pub fn close(&self, why: &str) {
        let mut s = self.state();
        s.closed.get_or_insert_with(|| why.to_string());
    }

    /// The loop's start: the ceiling raises are measured against.
    pub(crate) fn adopt(&self, whole: &Budget) {
        self.state().current = Some(whole.clone());
    }

    /// The raised ceiling, once per raise, for the loop's budget check.
    pub(crate) fn take_raised(&self) -> Option<Budget> {
        let mut s = self.state();
        if !std::mem::take(&mut s.raised) {
            return None;
        }
        s.current.clone()
    }
}

fn same_ceilings(a: &Budget, b: &Budget) -> bool {
    a.max_iterations == b.max_iterations
        && a.max_wall_clock == b.max_wall_clock
        && a.max_tokens == b.max_tokens
}

/// A plan with more partitions than the run has iterations or tokens.
///
/// Every partition needs at least one of each to do anything, and granting
/// that past the ceiling would spend more than the user set.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error(
    "{partitions} partitions cannot share a budget of {iterations} iterations and {tokens} tokens \
     without exceeding it; raise the budget or run fewer partitions"
)]
pub struct BudgetTooSmall {
    pub partitions: u32,
    pub iterations: u32,
    pub tokens: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The risk: the defaults move in code and the ADR that justifies them
    /// keeps describing the old ones, or the reverse, and a reader cites
    /// whichever they found first. The ADR's "Recorded defaults" line is the
    /// written source, so the test reads it rather than a copy of it.
    #[test]
    fn default_budget_matches_adr_0008() {
        const ADR: &str = include_str!("../../../docs/adr/0008-budgets.md");
        let recorded = ADR
            .lines()
            .filter(|l| l.starts_with("Recorded defaults:"))
            .collect::<Vec<_>>();
        assert_eq!(
            recorded.len(),
            1,
            "ADR 0008 must record the defaults exactly once: {recorded:?}"
        );
        let b = Budget::default();
        let shipped = format!(
            "Recorded defaults: {} iterations, {} minutes, {} tokens.",
            b.max_iterations,
            b.max_wall_clock.as_secs() / 60,
            b.max_tokens
        );
        assert_eq!(
            b.max_wall_clock.as_secs() % 60,
            0,
            "the ADR records whole minutes"
        );
        assert_eq!(recorded[0].trim_end(), shipped);
    }

    /// The risk: N partitions each granted the whole ceiling (ADR 0008).
    #[test]
    fn budgets_divide_across_partitions() {
        let b = Budget {
            max_iterations: 40,
            max_wall_clock: Duration::from_secs(600),
            max_tokens: 1_000_000,
            regime: CostRegime::Metered,
        };
        let shares = b.shares(4).unwrap();
        assert_eq!(shares.len(), 4);
        for s in &shares {
            assert_eq!(s.max_iterations, 10);
            assert_eq!(s.max_tokens, 250_000);
            assert_eq!(
                s.max_wall_clock,
                Duration::from_secs(600),
                "partitions run concurrently, so wall-clock is shared not divided"
            );
        }
    }

    /// The risk (rule 9, ADR 0058 §2): a person's raise granted to every
    /// partition, so raising a four-partition run's ceiling by 20 iterations
    /// spends 80 more. The raise is to the run's whole ceiling, and the
    /// shares are dealt from that whole, so they sum to the raised ceiling.
    #[test]
    fn budget_shares_divide_a_raised_budget() {
        let first = Budget {
            max_iterations: 40,
            max_wall_clock: Duration::from_secs(600),
            max_tokens: 1_000,
            regime: CostRegime::Metered,
        };
        let live = LiveBudget::default();
        live.adopt(&first);
        let whole = live
            .raise(&Raise {
                max_iterations: Some(60),
                max_wall_clock: Some(Duration::from_secs(900)),
                max_tokens: Some(3_001),
            })
            .unwrap();
        assert_eq!(
            live.take_raised().map(|b| b.max_iterations),
            Some(60),
            "the loop takes the raised whole"
        );
        let shares = whole.shares(4).unwrap();
        let its: u32 = shares.iter().map(|s| s.max_iterations).sum();
        let toks: u64 = shares.iter().map(|s| s.max_tokens).sum();
        assert_eq!(
            (its, toks),
            (60, 3_001),
            "the shares sum to the raised ceiling, never to four times it"
        );
        for s in &shares {
            assert_eq!(s.max_wall_clock, Duration::from_secs(900));
        }
        // Each share is the raised whole's, not the first share plus the raise.
        let first_shares = first.shares(4).unwrap();
        assert_eq!(first_shares[0].max_iterations, 10);
        assert_eq!(shares[0].max_iterations, 15);
    }

    /// The risk (ADR 0058 §2): a request that lowers one ceiling and raises
    /// another half-applied, or a lower ceiling accepted, so a run ends on a
    /// limit the person never meant to set.
    #[test]
    fn a_raise_that_lowers_any_ceiling_changes_nothing() {
        let first = Budget {
            max_iterations: 40,
            max_wall_clock: Duration::from_secs(600),
            max_tokens: 1_000,
            regime: CostRegime::Local,
        };
        let live = LiveBudget::default();
        assert_eq!(
            live.raise(&Raise {
                max_iterations: Some(50),
                ..Raise::default()
            })
            .err(),
            Some(RaiseRefused::NotStarted),
            "nothing to measure a raise against before the loop adopts it"
        );
        live.adopt(&first);
        assert_eq!(
            live.raise(&Raise {
                max_iterations: Some(50),
                max_tokens: Some(999),
                ..Raise::default()
            })
            .err(),
            Some(RaiseRefused::Lower {
                ceiling: "max_completion_tokens",
                asked: 999,
                current: 1_000,
            })
        );
        assert_eq!(
            live.raise(&Raise::default()).err(),
            Some(RaiseRefused::NothingNamed)
        );
        // Equal is not lower, and changes nothing for the loop to take.
        let same = live
            .raise(&Raise {
                max_iterations: Some(40),
                ..Raise::default()
            })
            .unwrap();
        assert_eq!(same.max_iterations, 40);
        assert!(live.take_raised().is_none(), "nothing was raised");
        live.close("the run ended");
        assert_eq!(
            live.raise(&Raise {
                max_iterations: Some(50),
                ..Raise::default()
            })
            .err(),
            Some(RaiseRefused::Closed("the run ended".into()))
        );
    }

    /// The risk: a floored share leaves iterations nobody may spend.
    #[test]
    fn the_remainder_is_dealt_so_shares_sum_to_the_ceiling() {
        let b = Budget {
            max_iterations: 40,
            max_wall_clock: Duration::from_secs(600),
            max_tokens: 1_000,
            regime: CostRegime::Metered,
        };
        let shares = b.shares(3).unwrap();
        let its: Vec<u32> = shares.iter().map(|s| s.max_iterations).collect();
        let toks: Vec<u64> = shares.iter().map(|s| s.max_tokens).collect();
        assert_eq!(its, [14, 13, 13], "the first partitions get the remainder");
        assert_eq!(toks, [334, 333, 333]);
    }

    /// The risk: a plan wider than the ceiling granted one of each per
    /// partition, so sixteen partitions of a 2-iteration run spend sixteen.
    #[test]
    fn a_plan_wider_than_the_budget_is_refused_not_multiplied() {
        let b = Budget {
            max_iterations: 2,
            max_wall_clock: Duration::from_secs(1),
            max_tokens: 3,
            regime: CostRegime::Local,
        };
        assert_eq!(
            b.shares(16).unwrap_err(),
            BudgetTooSmall {
                partitions: 16,
                iterations: 2,
                tokens: 3,
            }
        );
    }

    #[test]
    fn token_ceiling_applies_even_on_local_weights() {
        let b = Budget {
            regime: CostRegime::Local,
            max_tokens: 100,
            ..Budget::default()
        };
        let spend = Spend {
            tokens: 100,
            ..Spend::default()
        };
        assert_eq!(b.exceeded(&spend), Some(Exhaustion::Tokens));
    }
}
