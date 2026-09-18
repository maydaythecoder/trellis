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
        // as any. See ASSUMPTIONS.md.
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
            Exhaustion::Iterations => {
                "iteration ceiling reached without converging; rolled back"
            }
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
}

#[cfg(test)]
mod tests {
    use super::*;

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
