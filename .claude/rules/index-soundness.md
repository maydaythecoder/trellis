---
paths:
  - "crates/trellis-index/**"
  - "docs/bench/**"
---

# Index soundness

Loaded when a change touches the resolver or its benchmarks; moved here from
CLAUDE.md (#388) so it is in context exactly when it can prevent a mistake.

## The number that is easy to misread

`trellis index` prints what the resolver *bound*. That is a **lower bound on
recall, not recall**, and it must never be compared to the thresholds in
`BENCHMARK_AND_EVAL.md` §3. An `obj.method()` whose receiver type is unknown is
counted as a miss even when its true callee is third-party, which §2 excludes
from the denominator outright. Only the hand-audited sample produces a
comparable figure. See A-30.

That figure moved from 37.6% to 43.7% on poetry when ADR 0015 corrected which
sites belong in the denominator. **`resolved` did not change**, hand-audited
recall did not change, and no additional call site became resolvable. Any
before/after of this number must state both halves or it reads as an
improvement that did not happen.

## Receiver typing, and why it is allowed to be wrong in only one direction

`trellis-index` types receivers from annotations and constructor assignments
(ADR 0012). Every rule there can only *add* resolutions, so every one is a new
chance to emit an unsound edge. Three invariants hold it in place, and all
three earned their place by catching a real defect:

- A **declared type is an upper bound, not an identity.** If any first-party
  subclass overrides the method, the result is `Candidates` over the base and
  every override. Deliberately not applied to `self.m()`, where the audit
  protocol resolves to the enclosing class.
- A type fact is honoured **only when it resolves to a first-party class**, so
  `x = make_env()` types nothing.
- **Conflicting statements type nothing**, and `list[Env]` types `list`.

Before changing any of them, run the audit: `examples/check_soundness.rs` over
the corpus must stay at zero violations, and precision on the audited sample
must stay at 100%.
