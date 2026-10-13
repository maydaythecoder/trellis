# Architecture decision records

Every non-obvious choice in TRELLIS is recorded here, each with the
alternatives it rejected. `CLAUDE.md` lists the ten rules that need an ADR to
change; this index lists every ADR. Status is the first clause of each file's
`Status` line as written; where a status goes on to list amendments, read the
file. There is no ADR 0030: numbers need not be contiguous, and
`scripts/checks/adr-numbers.sh` (G3) holds that each number is unique and
stays where it is.

| ADR | Title | Status | What it holds |
|---|---|---|---|
| [0001](0001-resolver-scope-and-build-not-consume.md) | Resolver scope, and building the index rather than consuming one | Accepted | The index is built in-process with tree-sitter over first-party names only; anything resolving outside the repository root never enters the graph. |
| [0002](0002-edge-confidence-model.md) | Edge confidence model, and the thresholds that govern it | Accepted | Every edge is `Resolved`, `Candidates` or `Unresolved(Reason)`, never two-valued; the recall thresholds were fixed before measuring. |
| [0003](0003-degradation-policy.md) | Degradation policy | Accepted | Unresolved sites are never suppressed, only aggregated with a visible count; `Verified` and `Reviewed` never collapse into one number (rule 4). |
| [0004](0004-grounding-contract.md) | The grounding contract | Accepted | Correctness verdicts come only from processes that execute; `Verdict` is sealed in `trellis-checks` and `ToolResult` has no pass variant (rules 1, 2). |
| [0005](0005-loop-shape.md) | Loop shape, and why judgment is never parallelized | Accepted | One loop, one model, a flat tool list; no agent judges another's output; uncertain edges are unpartitionable (rules 5, 6). |
| [0006](0006-adapter-tiers-and-inference-path.md) | Adapter tiers, and the inference path | Partially accepted. Tiers **Accepted**; inference path **Proposed**, pending v0.2 measurement (S17). | The four adapter tiers, with llguidance as the single grammar authority. ADR 0013 records the inference path as since settled. |
| [0007](0007-trail-format.md) | Trail format | Accepted | One JSON trail per run under `.trellis/runs/`, split into `factual` and `judgment`, read back as untrusted data (rule 10). |
| [0008](0008-budgets.md) | Budgets | Accepted, values uncalibrated | Hard ceilings on iterations, wall clock and tokens, accounted through `tracing`; budgets divide across partitions (rule 9). |
| [0009](0009-clean-worktree-requirement.md) | The clean-worktree requirement | Accepted | `trellis run` refuses a dirty worktree, so rollback is a reset to a recorded commit (rule 7). |
| [0010](0010-byok-and-key-handling.md) | BYOK and key handling | Accepted | The user's API key never touches infrastructure the project controls (rule 8). |
| [0011](0011-recall-measured-and-the-s16-decision.md) | Measured recall, and the S16 decision it forces | Proposed. | The first recall measurement fired the pre-registered S16 trigger; the decision was deferred to the owner and taken in ADR 0012. |
| [0012](0012-receiver-typing-backstop.md) | Deviating from the S16 rule to build a receiver-typing backstop | Accepted. Outcome recorded 2026-09-19 — the attempt succeeded. | A recorded deviation: receiver typing was built and recall re-measured on a fresh sample. |
| [0013](0013-inference-path.md) | The inference path for harness-side constraint | Accepted. | Tier 2 (the runtime's own guided decoding) first and default for local models; tier 3 is not built. |
| [0014](0014-radius-coverage.md) | A radius reports two facts, never a ratio | Accepted | `RadiusCoverage` is distinct from `Coverage` and has no `total()` or fraction (rule 3). |
| [0015](0015-typed-receivers-leave-the-denominator.md) | A receiver with a demonstrably non-first-party type leaves the denominator | Accepted | A call site leaves the recall denominator only on positive evidence that its receiver is not first-party. |
| [0016](0016-staged-partitioning.md) | Narrowing the partitionability test, and staging the plan | Accepted. | A plan is staged: shared work first, then disjoint partitions. |
| [0017](0017-vault-context.md) | Vault context: one focus note, compiled in memory | Accepted | What a run is shown from a vault is a pure function of files on disk, held against committed goldens; `trellis-vault` never depends on `trellis-model`. |
| [0018](0018-tool-gate-and-control-dial.md) | A tool gate inside the orchestrator, and the control dial | Accepted. | Ordered `ToolGate`s that can allow or deny but never rewrite or pass; `[control] mode` auto or approve. |
| [0019](0019-skills.md) | Skills, disclosed progressively | Accepted. | `.trellis/skills/<name>/SKILL.md`; descriptions in the prompt, body only on `read_skill`. |
| [0020](0020-local-work-queue.md) | A local work queue and a heartbeat | Accepted. | The queue lives in the git directory as files; a claim is a hard link, so one claimer wins. |
| [0021](0021-capability-matrix.md) | Inheriting pi, Apex and Paperclip: the capability matrix | Accepted. | Every pi, Apex and Paperclip capability, and whether it is built, deferred or excluded, and by which clause. |
| [0022](0022-vault-parity.md) | Apex vault parity: lenient reading, strict run gate, `trellis vault` | Accepted. | Reading follows Apex leniently; the run gate stays strict about the two-key schema; `trellis vault` edits. |
| [0023](0023-tools-trust-and-hooks.md) | A composable tool set, a trust store, and executable hooks | Accepted. | `ToolSet` built once per run; repository code runs only after `trellis trust`, recorded outside the repository; hooks; trusted code under `.trellis/`. |
| [0024](0024-hosted-providers-and-compaction.md) | Hosted providers over HTTPS with native tool calls, and deterministic compaction | Accepted. | Tier 1 over TLS behind the `hosted` feature; only the user decides where a key is sent (§3); compaction by elision (§5). |
| [0025](0025-context-files-and-run-modes.md) | Context files, system prompt files, templates, and run modes | Accepted. | `AGENTS.md`/`CLAUDE.md` context, `SYSTEM.md`, templates, `--json`, `trellis rpc`; repository inputs resolved inside the repository. |
| [0026](0026-local-orchestration.md) | Local orchestration: task model, agent profiles, routines, budgets | Accepted. | Task fields, profiles, token budgets, approvals, routines, the activity log, process agents (§11), services. |
| [0027](0027-drain-run-decision-log.md) | Drain-run decision log | Accepted, appended to during agent runs. | Decisions an agent may make without the owner, each with its reason and what reverses it. |
| [0028](0028-s7-metrics-3-5-and-6.md) | S7 metrics 3, 5 and 6: one replaced, one waived at temperature 0, one kept | Accepted | How the model-floor metrics are counted, including one run per task at temperature 0. |
| [0029](0029-loop-keeps-the-call-graph.md) | `trellis-loop` keeps a hard dependency on trellis-index | Accepted. | No feature gates `blast_radius` out of the loop. |
| [0031](0031-pyright-recall-backstop.md) | Pyright over LSP as the recall backstop | Accepted. | Pyright is asked only about `Unresolved` and `Candidates` sites, opt-in behind `[index] backstop = "pyright"`. |
| [0032](0032-throwaway-clone-for-untrusted-writers.md) | A throwaway clone for the writers trellis does not control | Accepted. | `bash`, custom tools, hooks and process agents run in a disposable clone; only their change to tracked files comes back through trellis's write path. |
| [0033](0033-minimum-context-window.md) | The smallest supported context window, and a radius sized to fit | Accepted. | 4,096 tokens is the smallest supported window; an observation carrying coverage is never cut to fit. |
| [0034](0034-steering-in-the-trail.md) | Steering and follow-up text in the trail | Accepted. | Steering text is recorded verbatim up to 2,000 characters and read back as untrusted. |
| [0035](0035-windows-job-objects.md) | Windows Job Objects through `process-wrap` | Accepted. Built. | On Windows, children spawn into a Job Object with kill-on-close, so what they start ends with them. |
| [0036](0036-v0.1-resolver-protection-is-a-local-hook.md) | v0.1-resolver is protected by a local pre-push hook | Accepted. | Branch protection is a tracked `pre-push` hook, because server-side protection needs a paid plan. |
| [0037](0037-partition-worktrees.md) | Amends ADR 0016: running a staged plan in partition worktrees | Accepted. | `Supervisor` runs each partition in its own git worktree and merges in plan order. |
| [0038](0038-resume-takes-a-run-id.md) | `trellis resume` takes an optional run id | Accepted. | `resume [<run_id>]` matches a run id exactly; the trail is still untrusted data. |
| [0039](0039-package-install.md) | `trellis install`: skills and prompt templates, never code | Accepted. | Installed packages carry skills and templates only. |
| [0040](0040-ticketing-tool.md) | The ticketing tool is a separate `trellis-tickets` binary | Accepted. | Ticket triage is a separate crate and binary depending only on `trellis-core`, so the `trellis` CLI does not grow. |
| [0041](0041-image-input-on-hosted-tier-1.md) | `trellis run --image`, hosted tier 1 only | Accepted. | Image input is refused on every tier but hosted tier 1. |
| [0042](0042-integrated-check-after-partition-merge.md) | Amends ADR 0037: the integrated check after partitions merge | Accepted. | `Supervisor::run_integrated` is the only public way to run a plan; only checks over the merged union converge it. |
| [0043](0043-frontier-escalation-policy.md) | Frontier escalation policy | Accepted. Policy only; nothing here is built. | Escalation to a frontier model is off unless the user's own config names a spec. |
| [0044](0044-model-pull-pins-revision-and-sha256.md) | `trellis model pull` pins a revision and verifies SHA-256 | Accepted. Built. | Each catalogue entry names a commit and a SHA-256; a URL at `main` is rejected. |
| [0045](0045-work-units-for-multi-agent-runs.md) | Work units for `trellis run --agents N` are named targets | Accepted. | The user names the work units with `--target`; an unknown or ambiguous symbol is refused before anything runs. |
| [0046](0046-rollback-under-a-clock-that-steps-back.md) | Rollback re-reads and verifies what the run wrote | Accepted. Built. | Every path the run wrote is re-read by the reset and compared by content, so rollback is never reported complete when it is not. |
| [0047](0047-colibri-as-a-model-runtime.md) | colibri as a TRELLIS model runtime | Accepted (owner-directed) | TRELLIS is a client of colibri's HTTP gateway and nothing more; no inference code is added. |
| [0048](0048-a-tauri-desktop-interface.md) | A Tauri desktop interface, in `desktop/`, as a `trellis rpc` client | Accepted (owner-directed). Not built (story S15, #365). | The desktop is its own Cargo workspace and lock, and talks to the engine only through `trellis rpc`. |
| [0049](0049-model-writes-stay-out-of-the-vault.md) | Model writes stay out of the configured vault | Accepted (owner-directed). Built. | Every writer is refused at the configured vault's notes root. |
| [0050](0050-starting-trellis.md) | Starting trellis: plain `trellis`, or `--tui`, `--dsk`, `--web` | Accepted (owner-directed). Not built (story S19, #377). | Bare `trellis` and three interface flags are the whole start-up surface. Parts are wired in `main.rs` since; check `docs/STATUS.md` before relying on the status line. |
| [0051](0051-branching-main-dev-audit.md) | Branches: main, dev, audit, and ticket branches cut from dev | Accepted (owner-directed). | `main`, `dev`, `audit` are the only permanent branches; work is cut from `dev`, merged into `audit`, promoted to `dev`; `main` moves only by the owner's pull request. |
| [0052](0052-two-rpc-clients-and-go-for-the-tui.md) | Two `trellis rpc` clients: a rendering Tauri desktop, and a Go TUI that replaces ratatui | Accepted (owner-directed). | Two clients, one engine. The desktop's views are built in `desktop/` (story S15); the Go TUI is not built (#514). |
| [0053](0053-a-worktree-per-queued-task.md) | Amends ADR 0020 §5: `work` runs each queued task in a worktree of its own | Accepted (owner-directed) when the owner approves #624's pull request. Built (#709, #710). Amended 2026-10-13: an asked task reuses its trail-only branch. | Each task runs on `trellis/task/<id>` in `.trellis/work/task/<id>/`; a converged one is kept for a person and `work` continues; the queue stays in the starting worktree's git directory. |
| [0054](0054-conflicting-non-negotiable-rules-are-escalated-not-arbitrated.md) | Conflicting non-negotiable rules are escalated verbatim, not arbitrated | Proposed. | A failed audit whose summary names another non-negotiable rule shows both rules, attributed and verbatim, and changes no outcome (#11). |
| [0055](0055-os-sandbox-around-the-throwaway-clone.md) | An OS sandbox around the throwaway clone's command: bwrap on Linux, sandbox-exec on macOS | Accepted (owner-decided, #621). Built for Linux; macOS compiled and its profile tested, not run on a Mac. | `bash`, custom tools, hooks and process agents run with no network, the home directory hidden and writes only to the clone and the temporary directory; a missing or failing sandbox is a warning (`sandbox: none (<reason>)`). Windows stays on ADR 0035. |

## Writing a new one

Take the next unused number and name the file `NNNN-short-slug.md`; never
renumber or reuse one (G3 checks). Use the sections `docs/00-index.md`
names: Context / Decision / Consequences / Alternatives rejected / Status /
Reversibility, with at least one rejected alternative and its reason.
State what is not built and what would change the decision, cite the
issue and the owner's decision where there is one, and add a row here.
