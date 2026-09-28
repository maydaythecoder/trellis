# ADR 0018 — A tool gate inside the orchestrator, and the control dial

**Status:** Accepted.
**Date:** 2026-09-27
**Reference design:** pi (`earendil-works/pi`, MIT), whose extensions intercept
`tool_call` and may block it. Borrowed as a design; no pi code is used.

## Context

The owner asked for fine-grained and hands-off control to be the user's choice
rather than the harness's. Today there is exactly one interception point:
governance `deny_write`, hard-coded in `Orchestrator::apply_edit`. Nothing asks
a human before an edit lands. `trellis session` offers `/diff`, `/accept` and
`/revert`, which approve a finished change as a whole, not the steps that
produced it. The loop-to-UI path is observe-only: `tracing` events rendered
by `tui_bridge`.

## Decision

### 1. `ToolGate`, consulted by the orchestrator actor before dispatch

```rust
pub enum GateDecision { Allow, Deny { reason: String } }

pub type GateFuture<'a> = Pin<Box<dyn Future<Output = GateDecision> + Send + 'a>>;

pub trait ToolGate: Send + Sync {
    fn check<'a>(&'a mut self, call: &'a ToolCall) -> GateFuture<'a>;
    /// Whether the decision just returned was a person's answer.
    fn human_decided(&self) -> bool { false }
}
```

*Amended at implementation.* As first accepted, `check` was synchronous and
the trait was `Send` only. Neither survived contact with the code:

- **`check` returns a future.** The approval gate waits on a
  `tokio::sync::oneshot` inside the orchestrator actor, which runs on a tokio
  runtime. A synchronous wait there (`blocking_recv`) panics, and
  `block_in_place` panics on the current-thread runtime `trellis run` builds.
  The future is boxed so the gate list stays a list of trait objects.
- **`Sync` as well as `Send`.** The actor lends `&self` across the `.await`
  of a running check subprocess, and the gates are part of `self`.
- **`human_decided`.** The trail records human decisions (see Consequences),
  and a gate that let a call through without asking anyone has not made one.
  Only the approval gate overrides it. It carries no verdict and cannot affect
  convergence.
- **The convergence audit is not gated.** The loop's own governance audit at
  `done` used to travel the same request as a model's `run_checks`. A gate
  that refused it (with `run_checks` in the approve list, a person pressing
  `n`) would have turned the audit into an error, which the loop reads as no
  audit, and the run would converge over a broken rule. The audit has its own
  ungated request; only model-originated calls pass through gates.

The orchestrator holds an ordered list of gates, and the first `Deny` wins. A
denial reaches the model as a `ToolResult::Error` observation that carries the
reason, the same way a governance refusal does today.

- **No `Mutate`.** A gate that rewrites an `Edit` would be a second writer, and
  ADR 0005 names the orchestrator as the only one.
- **No pass.** A gate returns `Allow` or `Deny`. It cannot produce a `Verdict`
  and cannot mark a run converged (ADR 0004).
- **Governance becomes the first gate.** `deny_write` moves behind the trait,
  and its semantics do not change. The existing governance tests must pass
  unmodified.

### 2. The control dial

`.trellis/config.toml`:

```toml
[control]
mode    = "auto"          # "auto" | "approve"
approve = ["edit"]        # tools that need a human yes in approve mode
```

`trellis run` also accepts the flags `--approve` and `--auto`, which override
the config for that run.

- **`auto`:** no human gate. Budgets (ADR 0008), governance and the grounded
  checks still apply. This is the hands-off end of the dial.
- **`approve`:** an `ApprovalGate` sends each listed call to the human through
  a request/response channel (`tokio::sync::oneshot`) and waits. A "no"
  becomes `Deny { reason: "declined by the user" }`. This is the fine-grained
  end. The tools that can be listed are `edit`, `read_file`, `run_checks` and
  `blast_radius`. The default list is `["edit"]`, because an edit is the only
  call that changes the tree.
- **The channel is not `tracing`.** An approval is a request that needs an
  answer, not telemetry, and ADR 0008 keeps `tracing` observe-only.
- **With no human to ask, a run in approve mode refuses to start.** A plain
  `trellis run` in approve mode with no terminal attached exits with `approve
  mode needs an interactive terminal; pass --auto, or set [control] mode =
  "auto"` (the second way out because `trellis session` reaches the same
  refusal and has no `--auto` flag). It does not start and then
  deny every edit, because that would be a run that looks as though it worked
  without doing the work.

Where the human answers:
- `trellis session` asks on the line: `edit src/x.py (+12 -3)? [y/N]`, with the
  diff shown first.
- `trellis tui` shows the existing `Modal::Approval`, now wired to the real
  channel instead of only the mock.

`/accept` and `/revert` remain the whole-change approval. Per-call approval
sits underneath them; it does not replace them.

### 3. Executable hooks are not built

pi also allows arbitrary code to act as a gate. Doing the same here, with
`.trellis/hooks/pre-tool`, would mean that cloning a repository and running
trellis executes code the repository supplied. pi puts a `project_trust` prompt
in front of that. Trellis has no trust store yet, and a hook that runs silently
is worse than no hook. Until a trust mechanism exists and has its own ADR,
`ToolGate` is extended in Rust only.

## Alternatives rejected

- **Gate outside the actor, in the loop.** Rejected: the loop is not the
  writer. A check made before the actor sees the call can go stale by the time
  the actor applies it.
- **Approval through a prompt instruction ("ask before editing").** Rejected:
  that is a suggestion. The same reasoning put `deny_write` at the write path
  rather than in the prompt.
- **Deny silently when there is no terminal.** Rejected, for the reason in §2.

## Consequences

- Hands-off and fine-grained runs go through the same loop, checks and trail.
  The only difference is which gates are installed, so the trail records the
  mode.
- The trail records each human decision (`approved` or `declined`) as a factual
  action, so a declined edit is visible in `trellis why`.

## As built

Decisions the text above left open, made when this was implemented. Each is
tested; the test names are in `docs/STATUS.md`.

- **The mode is recorded** as `factual.control` in the trail: `auto`, or
  `approve (<tools>)`. The field is omitted when unknown, so older trails keep
  their shape and their digest.
- **A decision is an action.** Each answer is an `Action` with tool
  `approval`, target `<tool> <target>`, outcome `approved` or `declined`,
  recorded under the same turn and just before the call it decided.
  `trellis why <path>` lists decisions about calls aimed at that path, since a
  declined edit never reaches `proposed`.
- **An edit the orchestrator would refuse is not put to the human.** When the
  file is missing, the anchor is absent or ambiguous, or `old == new`, the
  approval gate allows the call and `apply_edit` refuses it in its usual
  words. Such an edit cannot change the tree, and asking someone to approve a
  refusal spends their attention on nothing. The diff is computed the way
  `apply_edit` matches, on LF, so a CRLF checkout previews the edit that then
  applies.
- **Only an explicit yes counts.** `y` or `yes`, any case. An empty line,
  anything else, end of input, a dropped reply channel, or an interface that
  quit mid-question is a no.
- **Time spent waiting on a person is not charged to the wall-clock budget.**
  That ceiling bounds compute and a model's wandering; a diff read carefully
  should not be what ends a run.
- **The terminal answer is read on tokio's blocking pool**, so Ctrl-C while
  a question is open still interrupts and rolls back the run.
- **`trellis session` asks on the line** when its stdin is a terminal, and
  refuses a task in approve mode with the same message when it is not. The
  message says `pass --auto`, which is a `trellis run` flag; in a session the
  way out is `[control] mode = "auto"`.
- **`trellis tui` is wired.** The gate's question becomes `Modal::Approval`
  with the diff and the question line; `y` or `n` on the modal goes back on
  the request's `oneshot`. The modal's key handling was already there; what
  is new is `AppState::answered` and `events::run_with_answers`, which carry
  the key out of the pure state transition to the gate. The interface always
  has a human, so it never takes the no-terminal refusal.
- **`trellis doctor`** prints the configured mode, or the configuration error.
- **`read_skill` is approvable too.** ADR 0019 added the tool after this ADR
  listed the four above; it is read-only, like `read_file`.
- **`deny_write` binds the path a write lands on, not the string the model
  wrote** (review round 8). A rule was matched against the raw argument, so
  `.//setup.py`, `vendor//x.py`, `lnk/x.py` through a committed link or
  junction to `vendor/`, and on Windows `setup.py.` or `Setup.py`, each
  wrote the protected file unrefused. Both sides are now folded the same
  way before comparison: `/` separators, empty and `.` components dropped,
  and on Windows the trailing dots and spaces Win32 strips removed; case is
  folded on Windows and macOS, whose default filesystems ignore it (on a
  case-sensitive macOS volume that denies a file the rule did not name,
  the direction a deny list may err in). Then the path is resolved through
  any link to its repository-relative real path, the one `write_refusal`
  computes, and the rule is asked again. `Governance::denies_write_at(root,
  rel)` does both, and every place `deny_write` is consulted asks it: the
  governance gate (so it now holds the root) for `edit` and `write_file`,
  and a process agent's audit of what its command wrote. The approval
  gate's prediction needs no copy: governance is first by construction, so
  a denied write never reaches it. What this does not cover: `bash`,
  custom tools and hooks in a model run write without passing any gate,
  and `deny_write` is not audited over their writes (ADR 0023 §5 states
  this for a model run's ignored writes; it holds for tracked ones too).
  Evidence: `governance::tests::a_spelling_that_lands_on_a_protected_path_is_denied`,
  `on_windows_a_case_or_trailing_dot_spelling_is_the_same_file` and
  `gate::tests::deny_write_holds_for_the_path_a_write_really_lands_on`
  (a junction on Windows) each failed before the change; the last wrote
  `lnk/new.py` into `vendor/`.
