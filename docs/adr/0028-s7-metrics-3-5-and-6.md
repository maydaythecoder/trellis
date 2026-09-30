# ADR 0028 — S7 metrics 3, 5 and 6: one replaced, one waived at temperature 0, one kept

**Status:** Accepted
**Date:** 2026-09-29
**Decided:** 2026-09-28 by the owner (issue #146, split from #4)
**Relates to:** `BENCHMARK_AND_EVAL.md` §5 and §5.4, ADR 0004 (grounding
contract), `STATUS.md` row 7

## Context

`BENCHMARK_AND_EVAL.md` §5 designs the S7 model-floor spike around seven
metrics. The two configurations measured so far (§5.2, §5.6) left three of
them unmeasured, and §5.4 recorded why:

- **Metric 3, argument correctness**, is graded against a gold trace. The
  corpus is a hash-pinned poetry 2.5.0 sdist with no `.git`, the tasks are
  synthetic mutations (`docs/bench/floor_tasks.json`), and there is no
  known-good call sequence to compare against.
- **Metric 5, run-to-run variance**, needs five repetitions per task. Decoding
  in both configurations was greedy at temperature 0, and repetitions were
  observed byte-identical across four tasks that share a prompt.
- **Metric 6, context degradation**, runs the same task at 25 / 50 / 75%
  window fill. Neither configuration repaired a task, so there is
  no baseline for degradation to be measured from.

Leaving the three as "not measured" indefinitely would let S7 be called
resolved on four metrics without anyone having decided that four was enough.
This ADR is that decision.

## Decision

### Metric 5 — waived for temperature-0 configurations only

A configuration decoded greedily at temperature 0 is measured with one run
per task. Five repetitions of a deterministic decode repeat one number five
times; they add wall clock (about 70 minutes per 20-task sweep on this
machine, §5.5) and no information.

The waiver is scoped to the decoding setting, not to the model or the
runtime. **Any configuration sampled at a temperature above 0 still needs the
five repetitions per task that §5 designed**, and its floor is set on the
worst case or p25 of those five, as §5 states.

The temperature is read from what the adapter sends, not assumed from the
model. `RuntimeModel` (`trellis-model/src/runtime.rs`) sends `temperature: 0.0`
in every dialect it speaks, which is how both configurations in §5.2 were
driven. The hosted adapter (`hosted.rs`) sends no temperature, so the
provider's default applies; a hosted configuration is treated as sampled and
gets the five repetitions.

What would reopen it: evidence that a temperature-0 configuration is not
deterministic on the runtime used (for example batched inference changing
results between runs). Then the waiver does not hold for that runtime, and the
five repetitions apply.

### Metric 3 — the gold-trace comparison is replaced by a per-call argument audit

Argument correctness is measured by auditing each tool call the model makes,
against the corpus as it stood when the call was made:

- **path**: the call's `path` names a file that exists in the corpus;
- **anchor**: for an `edit` call, its `old` string occurs in that file's
  content.

The audit is counted **per task**: for each task, the number of calls, the
number whose path exists, and, over its `edit` calls, the number whose anchor
is present. The counts are reported side by side and are not folded into one
rate across tasks, because a task with forty calls would otherwise outweigh a
task with three. A call the grammar or the adapter rejected before it reached
a tool is a format failure (§5.2's column), not an argument-correctness
result, and is not counted here.

This measures less than a gold trace would. It establishes that an argument
refers to something real; it does not establish that it refers to the right
thing — an edit to an existing line of the wrong function passes both checks.
Whether the change was right is what check-recovery (metric 1) and the checker
already decide, and neither the audit nor the model issues that verdict
(ADR 0004).

It is chosen because it measures exactly the failure §5.6 recorded: a
grammar-valid call whose path does not exist (`src/poeetry/`). Tier 2
guarantees the call parses, never that its arguments refer to anything real,
and this audit is the direct measure of that gap.

The audit is computed from each task's record of its calls under
`docs/bench/floor-*/`. **`docs/bench/metric3_audit.py` computes it** (#167),
and `scripts/checks/metric3-audit-selftest.sh` holds it to two hand-counted
trails. The calls are read from the per-task `tNN.log` (the run's printed
turns, each call with its arguments), not from the `.json` trail, which in
the committed runs records no calls. The three runs already in §5.2 are
re-scored by it in §5.4, with the command, commit and method beside the
table. The trails of #144's run are next.

What would reopen it: a corpus with real history, so that a gold trace exists
again. Then the gold-trace comparison is the stronger measure and returns.

### Metric 6 — kept

Context degradation is not waived. It is measured on the first configuration
that repairs at least one task — #144's qwen3:8b run, or a later one if that
run repairs nothing — at 25 / 50 / 75% window fill on the tasks it repaired at
low fill. Until then it is pending, and S7 is not resolved.

## Consequences

- S7 can be resolved on six measured metrics plus metric 5 waived for
  temperature-0 configurations. It cannot be resolved while metric 6 is
  pending, and it cannot be resolved without a configuration that passes,
  which the floor needed anyway.
- A floor report states its decoding temperature, read from the adapter as
  above. The metric 5 waiver applies only where that is 0.
- Metric 3's counts are not argument correctness in the gold-trace sense. The
  failing calls are a lower bound on argument error; the passing calls are an
  upper bound on argument correctness, since a pass can still name the wrong
  line. Either must be labelled as the per-call audit wherever it is quoted.

## Alternatives rejected

- **Waive metric 3 outright.** The §5.6 failure is an argument-correctness
  failure; waiving the metric would drop the one number that names it.
- **Waive metric 5 for every configuration.** Would carry a property of greedy
  decoding over to sampled decoding, where it is false.
- **Waive metric 6.** Its output feeds the context budget (S5); nothing else
  measures it.
