# ADR 0043 — Frontier escalation policy

**Status:** Accepted. Policy only; nothing here is built.
**Date:** 2026-09-29
**Decided by:** the owner, via the coordinator, on issue #29 (2026-09-28).
**Holds:** ADR 0010 and ADR 0024 §3 unchanged (the key comes from the
user's environment and goes only to an endpoint the user chose), ADR 0004
(the model never issues a verdict), ADR 0008 (one run budget), ADR 0007
(trails read back as untrusted data). It loosens none of the ten rules in
CLAUDE.md "Rules that need an ADR to change".

This ADR is written within ADR 0010. Frontier escalation sends repository
content to a hosted provider with the user's key, directly from the user's
machine, exactly as a run the user started with `--model
anthropic:https://api.anthropic.com#<model>` does today. No proxy, relay or
store of ours enters the path, and no rule here names a key or an endpoint
that ADR 0024 §3 would refuse.

**Two meanings of "escalate".** ADR 0008 and `trellis-loop`
(`agent.rs`, "budget exhausted; escalating once") already use the word for
something else: the one extra turn the *same* model is granted on budget
exhaustion, with the final notice set. That behaviour is built and this ADR
does not change it. Below it is called **the final turn**. **Frontier
escalation** is the new thing: handing an unconverged local run to a hosted
model the user configured. A grep for "escalat" in the code finds only the
final turn; frontier escalation has no code.

## Context

Roadmap v1.x and brief S7 name a "frontier escalation policy". The case it
serves: a local model ends a run with a failing grounded check, and the user
has a hosted model that might finish it. Today the user can do this by hand:
`trellis resume` prints `trellis run --resume --model <spec> "<task>"`, which
starts a new run whose prompt carries the previous trail fenced as data
(ADR 0038). The policy has to say when trellis may do that on its own, what
it sends, and whose budget pays.

Facts the decision rests on, read from the code on drain at 8782cc0:

- An unconverged run ends in `Outcome::NotConverged { reason, .. }`
  (`crates/trellis-loop/src/agent.rs`). The reasons are
  `StoppedWithoutPassingChecks`, `BudgetExhausted`, `ModelFailed`,
  `OrchestratorGone`, `ChangedAfterLastCheck`, `AfterRunChangedTree`,
  `TrustChanged`, `GitDirChanged`, `HeadMoved`, `GovernanceViolation`,
  `NoProgress` and `AskedHuman`.
- The trail records checks as `CheckOutcome { kind, passed }` and each
  action's result as one line (`Action.outcome`, whose doc comment says a
  trail that stored check output "would be mostly type-checker output"). It
  records the paths a run proposed (`Factual.proposed`), not the diff.
  **Neither the failing check's output nor the attempted diff is in the
  trail today.**
- What the model is shown of an observation is its last 2,000 characters
  (`OBSERVATION_TAIL` in `crates/trellis-loop/src/adapter.rs`; ADR 0024
  "Context").
- The trail keeps completion tokens (`tokens`) and prompt tokens
  (`prompt_tokens`) as separate fields and never sums them. A hosted reply
  with no reported count is charged the reply ceiling in-run (ADR 0024 §2).
- How local runs end unconverged depends on the model. Method: count, per
  committed floor benchmark directory, the `t*.log` files containing
  `budget exhausted after escalation` (ADR 0008's final turn used and the
  budget still spent) and those containing `no progress`. Result, all on
  one corpus (poetry) under the benchmark's own budget:
  `floor-qwen2.5-0.5b-q4km` 16 of 20 logs budget-exhausted;
  `floor-qwen2.5-0.5b-q4km-fixed-harness` 9 of 9;
  `floor-qwen2.5-coder-3b-q4km` 4 of 18, with the other 14 ending on
  `no progress` after 3 iterations. Whether those 14 had run a failing
  check was not counted. Nothing has been measured for any larger local
  model.

## Decision

1. **Off by default.** Frontier escalation happens only when the user's own
   configuration file (ADR 0025 §7, e.g. `%APPDATA%\trellis\config.toml` on
   Windows) has an `[escalation]` table naming a spec. With no such table,
   trellis never escalates and says nothing about escalation.

2. **Only the user's config or `--model` can name the frontier spec.**
   `[escalation] spec` is read
   from the user's file only. A repository `.trellis/config.toml` that sets
   `[escalation]` is refused at load, naming the file, by the same mechanism
   that refuses a repository `key_env` (ADR 0024 §3, ADR 0025 §7). The spec
   then resolves as a spec the user chose, so ADR 0024 §3's key rules apply
   to it unchanged: a `provider:<name>` resolves on the user's entry when
   both files define the name, and the key comes from the environment
   variable the protocol or the user's own `key_env` names. `--model` is the
   user's other channel: a run the user starts on a hosted `--model` is
   already on a frontier model and has nothing to escalate to, so frontier
   escalation applies only to a run whose spec is not a hosted one (not
   `openai-chat:`, `anthropic:` or `provider:`; ADR 0024 §2). The budget's
   `CostRegime` is configured separately and is not the test.
   No new flag is added; the CLI surface does not grow.

3. **The trigger is a failing `Verdict`, never the model's claim.** Frontier
   escalation is considered only when all of these hold:
   - the local run ended `NotConverged`;
   - at least one grounded check ran in it, and the last one recorded in
     `check_outcomes` has `passed: false` (the verdict came from
     `trellis-checks`, which alone can produce one, CLAUDE.md rule 1);
   - the reason is `StoppedWithoutPassingChecks`, `BudgetExhausted`,
     `ModelFailed`, `NoProgress` or `GovernanceViolation`. The governance
     audit is a recorded check: the loop pushes it as a turn with its
     verdict, and the CLI builds `check_outcomes` from those turns, so a
     violation is a last entry with `passed: false`.

   Excluded: `ChangedAfterLastCheck` means the last verdict *passed* and
   the tree changed after it (`pass_went_stale` in `agent.rs`), so there is
   no failing check to hand over; the answer is to run the checks again,
   not a frontier model. `TrustChanged`, `GitDirChanged`, `HeadMoved` and
   `AfterRunChangedTree` end a run because the tree or trusted code may have
   been tampered with, and sending a possibly tampered diff to a third party
   is the wrong response to that. `AskedHuman` asked a person, and the
   answer is a person, not another model. `OrchestratorGone` has no state to
   hand over. No tool triggers it: `ToolResult` has no pass variant (rule 2)
   and gains no escalation variant; the loop decides from the recorded
   verdicts alone, after the run has ended.

4. **What leaves the machine.** The handoff to the frontier run carries
   three things and nothing else from the local run:
   - the task text, as the user gave it;
   - the failing checks' output **as the trail records it**: each check's
     kind and result, and, once the trail holds it (see 7), the tail of the
     failing check's output, bounded by the same 2,000-character tail the
     model is shown;
   - the attempted diff, **as the trail records it** (see 7). It is not
     truncated, because a cut diff misreports what was tried; a diff too
     large for the frontier model's window is that run's ordinary context
     problem (ADR 0024 compaction, ADR 0033), not a new bound here.

   The rule under both "as the trail records it": nothing from the local run
   leaves the machine that a reader of the trail cannot see afterwards. The
   trail is the receipt. Not sent: the local transcript, the local model's
   summaries and reasoning, the trail's judgment section.

   The frontier run is then an ordinary trellis run on the hosted spec. Its
   system prompt carries the repository fences and every `read_file`,
   `blast_radius` or check observation it asks for goes to the provider, as
   in any hosted run under ADR 0024. The three items bound the handoff, not
   the whole run. The user accepted that disclosure when they named a
   hosted spec in their own file; this ADR says so here so it is not
   discovered later.

5. **Where the frontier run starts.** The local run rolls back as it does
   today (ADR 0008). The attempted diff is captured from the snapshot before
   rollback. The frontier run starts from the base commit on a clean tree
   (rule 7) and gets the diff as fenced data in its prompt, through the same
   path `run --resume` uses for a previous trail (ADR 0007, 0038). It is
   never applied: a frontier run that started from the local model's edits
   would entangle two writers, and rollback is complete only for trellis's
   own writes. The frontier run converges only on its own checks, run in its
   own session.

6. **Same run budget; spend reported separately.** The frontier run gets
   what the local run left on every dimension ADR 0008 ceilings (iterations,
   wall-clock, tokens), and no more. The final turn (above) happens on the
   local model first, as today; frontier escalation adds no second final
   turn to the local model. If any dimension has nothing left, or the
   remainder is below one hosted reply ceiling of tokens (ADR 0024 §2),
   trellis **does not escalate and reports escalation as available**: one
   line naming the spec, why it was not performed (budget spent, and which
   dimension), and the manual command, `trellis run --resume --model
   <spec> "<task>"`. Frontier tokens are recorded in the trail in their own
   fields, apart from the local run's `tokens` and `prompt_tokens`, and
   never summed with them in any output. They are counted against the one
   `max_tokens`, because that is what "same budget" means; the cost is that
   one ceiling then counts tokens from two tokenizers, which a user reading
   the ceiling should know.

7. **The trail records the escalation and what it sent.** Before anything
   leaves, the implementation adds to the trail, each as an optional field
   that is omitted when absent (so older trails keep their shape and digest
   and `SCHEMA` stays 1, as `prompt_tokens` did): the tail of each failing
   check's output, the attempted diff, and an escalation record saying
   whether it was performed or reported as available and why (off, no
   trigger, which reason excluded it, budget spent). Read back, all of it is
   untrusted data (rule 10). Trails are committed, so checker output and the
   attempted diff go into the repository's history; that is the same
   repository's own code and tool output, and the user who turns escalation
   on is the one committing it.

8. **Partitioned runs.** Frontier escalation applies to the integrated run
   only. A partition that fails does not escalate on its own: N partitions
   escalating would be N frontier handoffs, and the integrated checks decide
   convergence anyway (rule 6). If the integrated run qualifies under 3, it
   escalates once, with the budget that remains of the whole run (rule 9).

## Alternatives rejected

- **Single-shot patch request.** Send the three items in one request, take a
  patch back, apply it and run the checks. It would keep "what leaves" to
  exactly three items for the whole escalation. Rejected: it needs a new
  adapter path beside the loop, the frontier model could not read a file or
  run a check before proposing, and the result has to be applied before it
  can be verified, which is a write outside the loop's serialized write
  path. The loop already does this job; decision 4 states honestly what the
  loop sends instead.
- **A reserved share of the budget for escalation**, divided off before the
  local run starts (consistent with rule 9: divide, never multiply). It would
  make escalation happen even when the local run exhausts its iterations,
  which was the usual ending for the 0.5B model in the benchmark logs
  above. Rejected for
  now: the owner's decision is "drawn from the same run budget", the right
  share is a number nobody has measured (pending, calibration per
  `COST_MODEL.md` after escalation is built), and a reserve silently shrinks
  every local run of a user who turned escalation on.
- **Continue from the local diff applied.** Rejected in decision 5: it breaks
  the clean-worktree start and entangles two writers' edits.
- **Let the model request escalation** (an `escalate` tool). Rejected: it
  makes the model's own claim the trigger, and a tool result would then carry
  a judgment about the run (rules 2 and 5).
- **Send the full check output.** Rejected: it is unbounded, it is more than
  the local model was ever shown, and it would not be in the trail.

## Consequences

- **Failure mode of this policy.** Escalation draws only on what the local
  run left, and a local run that ends by exhausting its budget leaves
  nothing. For the smallest benchmarked model that was most runs (Context),
  so for such a model under the placeholder budgets (ADR 0008) escalation
  will mostly be *reported as available* rather than performed. For the 3B
  model most runs ended early on `no progress` with budget left, which is
  the case escalation can serve; how often those runs had a failing check,
  and so met the trigger, is pending (measured when escalation is built,
  from the escalation record in the trail). That is the
  intended safe failure: the user learns a frontier model is configured and
  gets the command, and nothing is spent silently. If it turns out to be the
  only outcome in practice, the reserved-share alternative is the change to
  revisit, with a measured share.
- A second failure mode: a local model that stops early with a failing check
  (`StoppedWithoutPassingChecks`) escalates on nearly every run, so a weak
  local model plus escalation is in effect a hosted run with an extra
  handoff. Spend stays inside the one budget, and the trail's separate
  frontier token fields show it.
- ADR 0008's "escalate once" keeps its meaning (the final turn). Nothing in
  ADR 0008 is amended.
- **What an implementation ticket must build** (none of it exists): the
  `[escalation]` table in the user config with the repository-side refusal;
  the three trail fields in 7; capturing the diff before rollback; a seed
  path that fences the diff and check tails for the frontier run; the
  budget remainder hand-off and the "available" report; and tests that a
  repository `[escalation]` is refused, that no excluded reason escalates,
  that a run with budget spent reports rather than escalates, and that the
  handoff request contains nothing the trail does not.

## Reversibility

Cheap while unbuilt: this ADR is a document, and superseding it costs a new
ADR. Once built, turning it off is a configuration change for every user
(it is off by default), and removing it is contained to the loop's end-of-run
path and the config loader. The trail fields in 7 are the expensive part:
they are optional and additive, but once trails carrying them are committed
in users' repositories they must stay readable, so a reader must keep
accepting them even if escalation is removed. Loosening decision 2 (letting a
repository name the spec) would reopen the hole ADR 0024 §3 closed and needs
its own ADR against ADR 0010.
