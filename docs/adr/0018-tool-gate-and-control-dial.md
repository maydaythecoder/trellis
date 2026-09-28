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
  mode needs an interactive terminal; pass --auto`. It does not start and then
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
