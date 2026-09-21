# SESSION_LOG

One block per session, per §9 of the session prompt. Created late — the
prompt has asked for it throughout, and the block below is the first.

## 2026-09-21 — correctness campaign and quality audit (v1.1 nominal)

**Did:** `94c285c..ad3d42c`, 55 commits. Ran the correctness campaign's
tiers and the quality audit's passes end to end. Tiers 0–5 and 7–11 closed;
Tier 6 measured 7 of 8 crates. Quality audit Passes 0–7 closed. Nineteen
correctness findings (C-001…C-020) and nine quality findings (Q-001…Q-009),
each recorded with what closed it. Tests 328 → 415.

**Verified:** `cargo test --workspace` 415 passing, 0 failed, after every
commit. `cargo clippy --workspace --all-targets --all-features -D warnings`
0 warnings, now including `unreachable_pub`. `cargo audit` no
vulnerabilities; `cargo deny check` advisories/bans/licenses/sources ok.
`cargo llvm-cov` 82.74% lines at `94c285c`. Mutation: seven crates, 593
caught / 195 missed as self-test scores. Every fix attributed to a mutant was
verified by applying that exact mutant and watching the test fail — four of
them only after the first version of the test passed against the mutant it
targeted.

**Did not:** The `main.rs` split (2,527 lines), because §9 wants a mutation
score to compare against and `trellis-cli` has none. `trellis-index` and
`trellis-loop` keep 117 untriaged survivors between them — 67 of `trellis-index`'s 92 and 50 of `trellis-loop`'s 57, all in display
strings, counter arithmetic and accessors — the highest-value ones in both
crates were taken. No fuzz target for `UntrustedTrail::from_json`, which is
the one first-party parser of attacker-influenced input.

**Blocked:** `trellis-cli`'s mutation run, stopped four times by system
memory — twice in a whole-workspace run, once at `-j 2`, once per-crate with
corrupt partial output. Options: run it on a machine with headroom; run it
with `CLAUDE_CODE_DISABLE_BG_SHELL_PRESSURE_REAP=1` if the reaper rather than
the OS is the constraint; or accept seven of eight crates and say so.

**Unverified:** No workspace mutation score exists, and the per-crate figures
are not one. The four crates not re-run with `--test-workspace` still carry
self-test scores that understate by an unmeasured amount — for the two that
were re-run, by 20 and 33 points. `trellis-model`'s re-run is unusable (five
timeouts) and has not been repeated since the stub hang that caused them was
fixed. Coverage is measured at `94c285c` and is stale by 56 commits. Tier 4
is not an independent review: most of this code was written in the sessions
that audited it.

**Next:** Complete `trellis-cli`'s mutation run, which is the only thing
standing between this campaign and a Tier 6 that covers the workspace.

**Open soft spots touched:** S3, S7, S10, S11, S18, S19.
