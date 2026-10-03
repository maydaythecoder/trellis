//! Per-run ceilings on iterations, wall-clock, and tokens.
//!
//! Tracked through `tracing` spans so accounting and observability are one
//! mechanism rather than two (4.11). On exhaustion the orchestrator
//! escalates once with the current state, then rolls back.

use serde::{Deserialize, Serialize};
use std::time::Duration;

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

    /// Splits this budget across `n` partitions.
    ///
    /// Budgets **divide, they do not multiply**. N agents each granted the
    /// full ceiling is an N-times faster way to spend the user's money
    /// (4.13). Wall-clock is the exception: partitions run concurrently, so
    /// the run's elapsed time is bounded by the whole ceiling, not a share
    /// of it.
    pub fn split(&self, n: u32) -> Budget {
        let n = n.max(1);
        Budget {
            max_iterations: (self.max_iterations / n).max(1),
            max_wall_clock: self.max_wall_clock,
            max_tokens: (self.max_tokens / u64::from(n)).max(1),
            regime: self.regime,
        }
    }

    /// This budget's iterations and tokens dealt out across `n` partitions,
    /// exactly.
    ///
    /// [`Budget::split`] floors, so three partitions of a 40-iteration run
    /// get 13 each and a 40th iteration nobody may spend; and it clamps to
    /// one, so sixteen partitions of a 2-iteration run get sixteen between
    /// them. The second is ADR 0008's "multiply" by another route. Here the
    /// remainder goes to the first partitions in plan order, so the shares
    /// sum to the run's ceiling, and a plan wider than the ceiling is
    /// refused rather than granted more than the user set.
    ///
    /// Wall-clock is copied whole, as in `split`: partitions in one stage
    /// run concurrently. The supervisor narrows it for a later stage.
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

    #[test]
    fn budgets_divide_across_partitions() {
        let b = Budget {
            max_iterations: 40,
            max_wall_clock: Duration::from_secs(600),
            max_tokens: 1_000_000,
            regime: CostRegime::Metered,
        };
        let split = b.split(4);
        assert_eq!(split.max_iterations, 10);
        assert_eq!(split.max_tokens, 250_000);
        assert_eq!(
            split.max_wall_clock,
            Duration::from_secs(600),
            "partitions run concurrently, so wall-clock is shared not divided"
        );
    }

    #[test]
    fn splitting_never_yields_a_zero_budget() {
        let b = Budget {
            max_iterations: 2,
            max_wall_clock: Duration::from_secs(1),
            max_tokens: 3,
            regime: CostRegime::Local,
        };
        let s = b.split(16);
        assert_eq!(s.max_iterations, 1);
        assert_eq!(s.max_tokens, 1);
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
