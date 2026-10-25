---
name: trellis-ui
description: The rules, tokens and reference wireframe for trellis's two rpc clients. Use when touching desktop/ui/, desktop/src/state.rs, tui/, or any text a client shows a person, and when designing a new view or state.
---

# trellis-ui

The desktop (`desktop/`, Tauri) and the TUI (`tui/`, Go) are two clients of one
engine, `trellis rpc`. They draw what the engine reports. The reference for how
a run should look is `docs/design/run-view-wireframe.html` (#786); open it with
`sources` on to see what each region draws from and what is not built yet.

## Rules that are not stylistic

These come from ADRs and CLAUDE.md. A change that breaks one fails review.

1. **The client computes no verdict.** A check row exists only because a turn
   event carried a `Verdict` from trellis-checks (`verdict: Some(true|false)`,
   `check_kind`). The model's own text is shown as the model's words and never
   styled like a check result. → ADR 0004, ADR 0048 §8
2. **A figure trellis did not report is `unreported`, never `0`.** Prompt
   tokens, elapsed on the desktop, a slot count the runtime did not answer. The
   desktop holds this with `budget_reports_prompt_bytes_not_tokens` and
   `session_tokens_unknown_is_null_never_zero`. → desktop/CLAUDE.md
3. **`Verified` and `Reviewed` are two rows, never one number.** No total, no
   ratio. → ADR 0003, brief §4.6, S3
4. **Blast radius is three counts** (`reached_resolved`, `reached_ambiguous`,
   `widest_unbound_in_touched_files`) with "a floor on reach, not a boundary",
   never a ratio and never a bare "complete". → ADR 0002, ADR 0014
5. **Prompt and completion tokens are never summed**; prompt size is in bytes
   where that is what was measured. → `get_session_stats`, ADR 0024
6. **One outcome vocabulary**, `run_end`'s: `converged`, `not_converged`,
   `interrupted`, `error`, with the exit code. The trail's derived outcome is
   shown as the trail's. → #653
7. **No currency.** Under the Local regime spend is "not metered"; wall clock
   and tokens are the budget. → brief §4.11
8. **No percent-complete, no remaining-time estimate.** There is no
   denominator. Elapsed is shown counting up, from the engine's clock where it
   reports one; the desktop draws no client clock (status.js), the TUI's own
   clock is labelled as its own.
9. **textContent only.** Everything from the stream, a trail, a file name or
   the model goes into the DOM as text, never as markup; control characters
   are escaped, not dropped. Trail text is fenced and marked untrusted.
   → ADR 0048 §5, S10, rule 10
10. **Only contract fields.** A client relies on fields listed in
    `crates/trellis-cli/tests/client_contract.json`; anything else is
    unversioned. A new region that needs a new field needs the contract
    amended first. → ADR 0048 §6
11. **No marketing language.** State what is true and what is not known.
    "Done" is `run_end`; "verified" is the mode count; nothing is "complete".
12. **A steer never marks an edit or a check.** It is text delivered before
    the next model call, recorded in the trail as the user's (`steering[]`);
    the log shows it as `queued`, `delivered after turn N` or `not delivered`.

## What the run view is made of

The wireframe's regions, in the order a person reads them:

- **Now strip**: the state word (`idle`, `running`, `needs you` + reason,
  `paused`, `stopping`, `ended · <outcome>`), elapsed, an activity line in
  `-ing` form from the last event, `turn n / max`, the queued count, and the
  controls `Stop` (confirmed), `Pause` (not built), `Summary so far`.
- **Action log**: rows typed by role. `you · task` / `you · steer` (signal
  colour, left bar), `model said — its words, not a verdict`, tool calls as one
  sentence with the raw call and observation on demand, check rows with the
  command and `trellis-checks verdict`, notes (WARN/INFO lines), refusals,
  `model asks you` for `ask_human`. Turn dividers carry `model_ms`,
  `completion_tokens`, `prompt_bytes`. Filters and a quiet mode.
- **Compose**: three verbs stated on the button: `Steer` (next model call),
  `Queue` (after this run; `follow_up`), `Stop and send` (abort, then prompt).
  The queue lists each item with its state; take-back and cancel; reorder only
  among follow-ups.
- **Where it is**: phase, tool and target, turn, last call, prompt bytes
  against the context budget, compactions, model, tier, runtime, control, root.
- **Budget**: two numbers per row (used / ceiling); wall clock excludes time
  waiting on a person; `raise a limit` is not built (`set_budget`).
- **Code**: files touched with two line counts and a state (`in the worktree`,
  `awaiting your approval`, `left in the tree`, `rolled back`); `diff` and
  `whole file` line by line; approve/reject only while the run waits on that
  edit; a line comment becomes a steer naming `file:line`.
- **Summary** (reachable mid-run as "so far"): outcome and exit code, checks
  by kind with duration, the facts above, budget consumed, steers, the path by
  turn, files changed, trail id and integrity, the model's closing words
  fenced.

The TUI shows the same regions at 120x40 and folds the right column under
the transcript at 80 columns; same state words, same verbs, keyboard only.
Parity between the clients is tracked in `tui/PARITY.md`.

## Tokens

Both themes live in the wireframe's first `<style>` block. The desktop's
`desktop/ui/style.css` still has its own, older names (`--bg`, `--fg`,
`--accent`, `--passed`, `--failed`, …); mapping them onto these is part of
building the wireframe, not a change to make on the side. Use the variables,
never a literal:

- Surfaces: `--ground`, `--panel`, `--panel-raised`, `--panel-inset`; lines
  `--line`, `--line-strong`; frames `--frame`, `--frame-strong`,
  `--frame-signal`, `--frame-down`.
- Ink: `--ink`, `--ink-secondary`, `--ink-muted`, `--ink-inverse`.
- Meaning: `--signal` (the user and what needs them), `--cool` (tools and
  the engine's own activity), `--ok` (a passed check), `--down` (a failed
  check, a refusal, stop), `--warn` (waiting, a WARN line), `--play`
  (wire-only annotations). Each has a `-soft` fill where a row is tinted.
- Diff: `--diff-add`, `--diff-del`, `--diff-hunk` and their `-soft` fills.
- Type: `--font-mono` for anything the engine said or measured, `--font-sans`
  for prose a person or the model wrote. Figures use tabular numerals.
- Space and radius: `--space-1` … `--space-16`, `--radius-0` (panels are
  square), `--radius-sm`, `--radius-full` (dots).

## When you add a region

1. Name the event or command it draws from. If none exists, the first change
   is to the contract (`client_contract.json`) and ADR 0048 §6, then the
   engine, then the clients.
2. Put the region in the wireframe first, with its `sources` tag, and get the
   owner's read before building it in either client.
3. Build it in the Rust reducer (`desktop/src/state.rs`) and the Go model
   (`tui/internal/...`), never in page JavaScript; the page only renders.
4. Add the parity row to `tui/PARITY.md` and a test that names the risk
   (e.g. `a_check_row_comes_only_from_a_verdict`).
