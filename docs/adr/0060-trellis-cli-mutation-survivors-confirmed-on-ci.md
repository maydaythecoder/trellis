# ADR 0060 — trellis-cli's mutation survivors: one workspace run on CI at cf821cd, dispositioned shard by shard

**Status:** Accepted (owner-directed). Not built.
**Date:** 2026-10-31
**Issue:** #248 (story S04, from #17).
**Decided by:** the owner: on 2026-10-06, that the `WORKSPACE_TESTS=1`
cargo-mutants run of trellis-cli happens on CI by manual dispatch and that
#248 closes when it confirms the kills; on 2026-10-26, that G7 is
pre-approved for #248's branches; on 2026-10-31, to work #248 now, this ADR
first, then the survivors in batches, each merged into `audit` only when
G0–G6 exit 0. Which commit the run is measured at, what a disposition must
carry and how the batches are cut were chosen by the architect under that
instruction; they are reversible.
**Holds:** every rule in CLAUDE.md "Rules that need an ADR to change"; none
is loosened. ADR 0004 (`trellis-checks` owns `Verdict`) is untouched: a
mutation run says whether a test notices a change, not whether code is
correct. CLAUDE.md "Before claiming a number": every figure below has its
method beside it.

## Context

#248 lists 407 mutants of trellis-cli that survived a self-test run
(`cargo mutants -p trellis-cli`, only trellis-cli's tests) at `989b2a2`.
Four passes and a ledger backfill (#431, #455, #459, #462, #471, #668) gave
every one a line in CORRECTNESS_LEDGER.md. Counted with
`grep '| crates' <the trellis-cli 407 block> | sed 's/ (#248).*//; s/ |.*//'
| sort | uniq -c` at `112af193` (origin/dev, 2026-10-31): 353 killed, 19
unexercisable plus 1 "unexercisable by `cargo test`", 17 not examined, 8
equivalent, 5 unix-only, 4 not applicable. 407 in all.

Three things are open.

1. **The kills were shown by hand, on Windows.** Each pass applied a mutant
   by hand, ran the named test with `cargo test -p trellis-cli` and saw it
   fail. No cargo-mutants run has confirmed them with the whole workspace
   suite, which is what #248's Done-when asks for.
2. **22 lines need another operating system.** 17 are `#[cfg(unix)]` code in
   process_agent.rs and services.rs, which cargo-mutants mutated on a
   Windows host that never compiles it ("not examined"). 5 are `probe_ps` in
   liveness.rs, chosen at run time only on a Unix that is not Linux
   ("unix-only").
3. **The CI run has started and found new survivors.** `mutants.yml`
   (`--test-workspace true --timeout 300 -j 1`, cargo-mutants 27.1.0) ran
   shard 0/10 on `ubuntu-latest` at `cf821cd` on 2026-10-25 (run
   40208964899). Its summary line: `252 mutants tested in 4h: 12 missed, 217
   caught, 22 unviable, 1 timeouts`. None of the 13 missed or timed-out
   lines is in the ledger (each checked with `git grep -F` against
   CORRECTNESS_LEDGER.md at `112af193`): they are in code written after
   `989b2a2`. Shards 1–9 have not run. The artifact is kept 30 days, to
   about 2026-11-24.

`main` has moved since `cf821cd` (#898). cargo-mutants generates its mutant
list from the source tree and `--shard k/n` splits that list, so shards
dispatched against a different tree do not complete shard 0's run: they
split a different list.

The sibling tickets for trellis-model, -vault and -queue (#245, #246, #247)
closed on hand-shown kills with no workspace run. #248 differs only by the
owner's 2026-10-06 decision.

## Decision

1. **One measurement, at `cf821cd`.** Shards 1/10 to 9/10 run on Linux at
   the same commit as shard 0, with the same inputs (`crate=trellis-cli
   os=linux`, `n=10`). A dispatch names a ref, not a commit, so a temporary
   branch `t248-mutants-6b804cb` is pushed at `cf821cd` (an ancestor of
   `origin/dev`, checked with `git merge-base --is-ancestor`) and every
   shard is dispatched with `--ref t248-mutants-6b804cb`. The artifact name
   carries the short sha; a shard whose artifact does not name `cf821cd` is
   not part of the run. The branch is deleted when shard 9 has uploaded.
   Shards are dispatched one after another, as docs/BUILD_SYSTEM.md "Mutation
   testing on CI" says, by the worker on the dispatch ticket under the
   2026-10-06 decision. The repository is public (`gh repo view`,
   2026-10-31).

2. **Each shard's outcome is committed as text.** Its `caught.txt`,
   `missed.txt`, `timeout.txt` and `unviable.txt`, and a `COMMIT` file
   reading `cf821cd`, go under
   `docs/bench/mutants-by-crate-ws/trellis-cli/shard-<k>-of-10/`, the layout
   the other crates' workspace results already use. The ledger is the
   record; the artifact expires and the repository does not.

3. **Scope is every line those shards report missed or timed out**, not only
   the 407. A survivor in code written after `989b2a2` is still a trellis-cli
   survivor, and the owner's 2026-10-31 instruction is to kill the survivors.
   The 407-line block stays as it is, a record of `989b2a2`; the new lines go
   in a second block in CORRECTNESS_LEDGER.md, a `<details>` whose summary
   reads `trellis-cli at cf821cd, workspace suite on Linux (#248)`. Each line
   has the existing form, `<disposition> (#248)…: <reason> | <line exactly
   as missed.txt or timeout.txt prints it>`.

4. **What a disposition must carry.** The vocabulary is the one the four
   passes used; what is new is the evidence each word needs.
   - **killed (#248) by `<module path>::<test name>`.** The test exists at
     the batch's head. The mutant was applied by hand at the batch's base,
     the test run and seen to fail, and the source restored (pass 2's
     method). A test that runs only on Linux, or only on Windows, is shown
     instead by a `mutants.yml` dispatch with `file=<that source file>` and
     that `os`, at the batch's branch; the line cites the run id.
   - **equivalent (#248): `<why no input distinguishes the two>`.** Any
     difference in printed output, in an exit code or in a file written
     disqualifies it. Pass 1 called 7 mutants equivalent and 4 of them were
     later killed because each changed what was printed (ledger, the
     paragraph under "trellis-cli: 407 survivors"), so the word is used
     last.
   - **unexercisable (#248): `<the facility a test would need and does not
     have>`** (a pty, a signal this host cannot send, a race with no hook).
   - **windows-only / unix-only (#248): `<the cfg or runtime check>`**, for
     code the Linux run does not compile or never reaches. It stands only
     with a run on the other system cited (a `file=`/`os=windows` dispatch,
     or a 989b2a2 pass line that killed it on Windows), or with the reason
     no hosted runner reaches it.
   - **not applicable (#248): `<the commit that rewrote or removed it>`**,
     for a mutant at `cf821cd` whose code no longer exists at the batch's
     base.
   A timed-out mutant gets a disposition like a missed one. A hang is not a
   kill: "killed" needs a test that fails within the 300 s limit, which for
   `config.rs:1208:9` (`model_timeout -> Default::default()`) means a test
   that reads the default and fails on zero rather than waiting on it.
   A 989b2a2 line that said "killed" and whose mutant (function and
   mutation, `line:col` ignored) appears in a cf821cd `missed.txt` is a
   refuted kill: the new line says so and gives the new disposition.

5. **A check script holds it.** `scripts/checks/mutants-ledger.sh
   <shard-dir>` exits 0 exactly when every line of that directory's
   `missed.txt` and `timeout.txt` appears, after ` | `, on one line inside
   the cf821cd block, that line starts with one of the dispositions above,
   and, for "killed … by `<name>`", `fn <last segment of name>` exists under
   `crates/`. `COMMIT` must read `cf821cd`. `--all <dir>` does the same for
   every `shard-*-of-10` directory and fails unless all ten are present.
   `--self-test` runs it against fixtures, as the other `scripts/checks/`
   scripts do. A script and not a Verify line of greps, because G6 refuses a
   Verify line containing `|` or `&`, and most mutant names contain `&&`,
   `||` or the ledger's ` | `.

6. **The 22 lines of Context item 2 are settled by the Linux
   run.** Each of the 17 `#[cfg(unix)]` lines is in some shard's
   `caught.txt` or `missed.txt`. Caught: its 989b2a2 line becomes "killed
   (#248) by the workspace suite: caught at cf821cd, shard <k> of 10".
   Missed: it gets a cf821cd line under point 4. `probe_ps` is compiled on
   every target (its choice is `cfg!` at run time, liveness.rs lines 29–45 at
   `112af193`), so a Linux-only test calling it directly against the
   runner's procps `ps`, on the test's own pid and on a pid that is not
   running, is tried first; "unix-only" stays only if that test cannot tell
   the mutants apart, with the reason.

7. **Batches.** Each fits one branch and merges into `audit` when G0–G6
   exit 0.
   - **Shard 0** (13 lines, workable now): commit its outcome files, write
     the check script, give each line a disposition. It goes first because
     its artifact expires and every later batch uses the script.
   - **Dispatch** (no cargo): push the pinned branch, run shards 1–9,
     commit their outcome files. Runs beside the shard 0 batch.
   - **One batch per shard with at least one line**, filed from the
     template below by whoever lands the dispatch batch, and added to
     #248's `blocked_by`. Two adjacent shards may share a ticket when
     together they hold 40 lines or fewer (pass 4 dispositioned 89 lines on
     one branch, so 40 leaves room).
   - **The 22 lines of point 6**, after the dispatch batch.
   - **#248 itself** closes when the children are closed and `--all` passes
     with no `not examined (#248)` line left in the ledger.

   Template for a shard batch:

       ## Done when
       - docs/bench/mutants-by-crate-ws/trellis-cli/shard-<k>-of-10/ is tracked and its COMMIT reads cf821cd.
       - Every line of its missed.txt and timeout.txt has a line in CORRECTNESS_LEDGER.md's "trellis-cli at cf821cd" block with a disposition and its evidence as ADR 0060 §4 defines them.

       ## Verify
       ```
       scripts/checks/mutants-ledger.sh --self-test
       scripts/checks/mutants-ledger.sh docs/bench/mutants-by-crate-ws/trellis-cli/shard-<k>-of-10
       ```

## Consequences

- One commit's worth of evidence for the whole crate: the 353 hand-shown
  kills are confirmed or refuted by `caught.txt` (matched with `line:col`
  ignored, as the ledger already matches moved mutants), and the 17 Windows-blind
  lines get an answer, from one run.
- The run costs about 33 runner hours more. Method: shard 0's `cargo
  mutants` step took 3h42m for 252 mutants (about 53 s each, run
  40208964899's log); nine more shards of similar size is 9 × 3h42m. Shards
  split the list by position, not by cost, so a shard can take longer; the
  job limit is 360 minutes.
- Kills written after `cf821cd` are shown by hand or by a per-file dispatch,
  not by the cf821cd run. The ledger names the base each was shown at.
- The ledger gains a second trellis-cli block, and `docs/bench/` gains ten
  directories of plain text, about 260 lines each (shard 0's four files hold
  252 lines, `wc -l`).
- #248 stays open for days: nine sequential dispatches at about four hours
  each, then the batches.

### Failure modes of this decision

- **`cf821cd` is a week old.** A survivor in code changed since is
  dispositioned "not applicable" with the commit, which is bookkeeping
  spent on nothing. The cost was judged lower than rerunning shard 0.
- **A shard reaches the step's 345-minute limit.** Its mutants with no
  outcome are in none of the four files, and the check cannot see them. The
  dispatch batch compares each shard's outcome count with the summary line
  of its log, and a short shard is finished with `file=` dispatches at the
  pinned branch for the files it did not reach.
- **The shard 0 artifact expires before it is committed** (about
  2026-11-24). Then shard 0 is rerun at the pinned branch, about four hours.
- **A hand-shown kill that cargo-mutants would not reproduce**, for example
  a test compiled out on Linux. §4 routes those through a per-file dispatch;
  a kill shown only on Windows of code that runs on both is still a hand
  claim until the next full run.

## Alternatives rejected

- **Finish the run on `--ref main` or `--ref dev`.** Both have moved past
  `cf821cd`, so shards 1–9 would split a different mutant list from shard
  0's and the ten would not cover the crate exactly once. `dev` also moves
  between dispatches.
- **Rerun all ten shards on current `dev`.** One consistent run at a newer
  commit, for about 37 runner hours (10 × 3h42m) instead of 33, and shard
  0's 13 survivors would wait days for a result already in hand.
- **Per-file dispatches for every file instead of shards.** Each file's run
  is at the commit its dispatch saw, so the results would not add up to one
  measurement, and the largest files' mutant counts at 53 s each are not
  known to fit in 360 minutes.
- **Close on the hand-shown kills, as #245–#247 did.** The owner decided on
  2026-10-06 that #248 closes on the CI run; this ADR does not reopen that.
- **A Windows run as well.** The 407 came from Windows and were
  dispositioned there; the new information a run adds is on Linux, where
  the 17 unix lines compile. A `windows-only` line from the Linux run is
  settled by a per-file `os=windows` dispatch, not by a second full run.
- **A macOS runner for `probe_ps`.** `mutants.yml` has no macOS option and
  adding one is a workflow change for five mutants; a direct Linux test is
  tried first (§6).
- **Verify by one `git grep` per survivor.** G6 refuses `|` and `&`, which
  most mutant names and every ledger line contain.

## Reversibility

Cheap. The decision is a ledger block, a check script, ten directories of
text and a temporary branch; deleting them undoes it. The runner hours
spent are not recoverable, and switching to a different commit later means
rerunning all ten shards there (about 37 hours by shard 0's rate).

## What is not built

- The check script, the outcome directories and the ledger block. #248's
  child tickets build them.
- No change to `mutants.yml` or `scripts/mutate-by-crate.sh`.
- No mutation score is computed: the outcome files are counts of different
  kinds, and this ADR asks for a disposition per line, not a ratio.

## What would change it

- The shard 0 artifact expiring uncommitted, or a shard of the cf821cd run
  failing in a way a rerun cannot finish: then all ten shards run at one
  newer commit, and this ADR's commit is replaced.
- The owner deciding #248 may close on hand-shown kills, as #245–#247 did:
  then the dispatch batch is dropped and the shard batches stop at shard 0.
