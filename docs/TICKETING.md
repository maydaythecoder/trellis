# The ticketing standard

The text between the two rules below is the owner's ticketing standard,
copied verbatim. The owner pasted it into the coordinating session on
2026-09-28 (20:09 UTC); every issue in story S13 cites it as "the owner's
ticketing standard (2026-09-28)". It is committed here so that each rule in
`crates/trellis-tickets` has a written source to be checked against
(ADR 0040, decision 3; issue #76), not someone's memory of it.

Do not edit the quoted text to match the code. Where this repository applies
it differently, the difference is recorded in "How this repository applies
it" after the quote, with the reason.

The standard opens by saying it "extends the ticketing standard". That base
document was never committed or pasted where this repository can read it, so
it is not reproduced here. Where the text below refers to it (for `effort:`),
the definitions given in the text are the whole of what is known.

---

# TRELLIS — Ticket Creation, Labeling, and Auto-Triage

Extends the ticketing standard. Covers where tickets come from, how they get labeled, and what the machine decides without you.

---

## 1. Principles

**Triage is rules, not a model call.** Same ticket in, same labels out, every time. A model in the triage path makes it slow, nondeterministic, expensive, and untestable — and triage runs on every ticket, constantly. Deterministic rules run in milliseconds and can be unit-tested. The one place a model earns its keep is semantic duplicate detection (§8), and even there it's advisory.

**Dimensions, not a label bag.** A flat pile of labels rots into 200 overlapping tags nobody trusts. Each dimension holds exactly one value. That keeps it queryable and stops the sprawl.

**Fail to human, never guess.** When triage can't classify something confidently, it marks `triage: needs-human` and stops. A wrong label that looks authoritative is worse than no label, because you'll act on it.

**Triage never changes status.** It labels, enriches, and gates. Moving a ticket to `todo` is a decision, and decisions stay yours.

---

## 2. Creation

### Sources

| Source | How | Lands as |
|---|---|---|
| Manual | `trellis ticket new` | `draft` |
| From a plan | Planning-loop output → one ticket per task | `draft` |
| Code marker | `TODO(T)` / `FIXME(T)` scanned from source | `draft`, `source: code` |
| Failing check | A check that fails twice on the same target | `draft`, `source: check` |
| Agent-noticed | An agent files it mid-run | `draft`, `source: agent` |
| QA finding | Each unfixed finding from a QA report | `draft`, `source: qa` |
| Dropped-then-revived | Copy of a `dropped` ticket, new ID, `revives: <old-id>` | `draft` |

**Everything lands in `draft`.** Nothing enters `todo` without passing readiness. That's the whole gate.

### Agent-created tickets are marked and treated as drafts

`source: agent` tickets carry `triage: needs-human` unconditionally. An agent noticing something is a useful signal; an agent adding work to your queue unsupervised is how a backlog fills with plausible noise you didn't ask for. Review before promotion, always.

### Code markers

```rust
// TODO(T): resolve re-export chains deeper than one hop
```

`trellis ticket scan` finds these, creates a ticket, and rewrites the marker with the assigned ID:

```rust
// TODO(T0244): resolve re-export chains deeper than one hop
```

An already-numbered marker is skipped. A marker whose ticket is `done` or `dropped` is reported as stale — that's a real cleanup signal.

### Scaffold

```
trellis ticket new ticket --title "Parse the frontmatter parent field" \
  --story S014 --files crates/trellis-context/src/frontmatter.rs
```

Writes the file, assigns the ID, runs triage (§4), prints what it inferred and what it couldn't.

---

## 3. Labels

Six dimensions, one value each. Plus flags, which are boolean and rare.

### `area:` — which component

Derived from `files:`. One per ticket; a ticket spanning three areas is three tickets or a story.

```
area:context   area:model   area:checks    area:runtime
area:skills    area:agents  area:protocol  area:tui
area:graph     area:cli     area:build     area:docs
```

### `kind:` — what sort of work

```
kind:feature    new capability
kind:bug        something is wrong
kind:refactor   behavior unchanged, structure improved
kind:chore      deps, tooling, CI
kind:test       test coverage only
kind:docs       documentation only
kind:spike      a question (spikes only)
```

### `risk:` — blast radius if it goes wrong

Drives the gate. See §5 for derivation.

```
risk:low        contained, reversible, tested
risk:medium     crosses a module boundary, or touches shared state
risk:high       touches a trust boundary, a protocol, or rollback
```

### `surface:` — who sees a change here

```
surface:internal   private to a crate
surface:cli        user-visible command behavior
surface:protocol   the JSON Lines wire format
surface:public     a published API or file format
```

Anything above `internal` needs an ADR. See §5.

### `effort:` — size, from the ticketing standard

```
effort:s   under 2h
effort:m   one session
effort:l   split it — never allowed in todo
```

### `origin:` — where it came from

```
origin:manual  origin:plan  origin:code  origin:check
origin:agent   origin:qa    origin:revived
```

### Flags

Boolean, absent by default, each one meaningful:

```
!agent-ok      an agent may execute this unsupervised
!needs-adr     requires a recorded decision before starting
!breaking      changes something downstream depends on
!security      touches secrets, keys, or sandbox boundaries
!blocked-ext   waiting on something outside the repo
```

---

## 4. The triage pipeline

Five stages, in order. Each is a pure function. Any stage can set `triage: needs-human` and short-circuit the rest.

```
intake → classify → enrich → gate → route
```

**1. Intake** — parse frontmatter, validate required fields, assign an ID if absent. Malformed frontmatter is a hard fail, not a guess.

**2. Classify** — derive `area:`, `kind:`, `surface:` from files, title, and template. (§5)

**3. Enrich** — derive `risk:`, `effort:`, flags. Detect duplicates. Resolve `blocked_by` against current statuses.

**4. Gate** — run the readiness checks. Set `ready: true|false` and list every failure by name.

**5. Route** — assign `triage:` — one of `auto`, `needs-human`, `blocked`.

Triage is idempotent. Running it twice changes nothing. Run it on every create, every edit, and in CI on every commit that touches `tickets/`.

---

## 5. The rules

### Area, from file paths

```
crates/trellis-context/**   → area:context
crates/trellis-model/**     → area:model
crates/trellis-checks/**    → area:checks
crates/trellis-runtime/**   → area:runtime
crates/trellis-skills/**    → area:skills
crates/trellis-agents/**    → area:agents
protocol/**, crates/trellis-protocol/** → area:protocol
tui/**, cmd/trellis-tui/**  → area:tui
```

Zero files, or files spanning more than two areas → `needs-human`. Spanning areas usually means the ticket is really a story.

### Kind, from the title verb and the files

| Signal | Kind |
|---|---|
| Title starts with fix/repair/correct, or `source: check` | `kind:bug` |
| Only `*_test.rs` / `*_test.go` touched | `kind:test` |
| Only `*.md` touched | `kind:docs` |
| Only `Cargo.toml`, `go.mod`, CI files touched | `kind:chore` |
| Title contains rename/extract/inline/move/simplify | `kind:refactor` |
| Type is spike | `kind:spike` |
| Otherwise | `kind:feature` |

### Risk — the important one

**`risk:high` if any of:**
- `area:checks` — **always**. The Verdict seal is the one thing making the harness trustworthy. Every change there is high risk, no exceptions, no matter how small it looks.
- `area:protocol` — two languages depend on it; a change breaks one silently.
- Touches rollback, worktree teardown, or session persistence.
- Introduces `unsafe`.
- `!breaking` or `!security` set.
- Changes an on-disk format that existing data uses.

**`risk:medium` if:**
- Files span two areas.
- Touches a crate boundary or a public-in-crate signature.
- `surface:` is above `internal`.
- More than five files.

**`risk:low`** otherwise.

### Surface

| Signal | Surface |
|---|---|
| `protocol/**` touched | `surface:protocol` |
| A `pub fn` or `pub struct` diff in a lib crate | `surface:public` |
| `cli/**` or a command definition touched | `surface:cli` |
| Otherwise | `surface:internal` |

### Effort

Heuristic, and you may override — but the override is recorded, not silent.

| Files | New tests | Effort |
|---|---|---|
| 1–2 | 0–1 | `effort:s` |
| 3–5 | 2+ | `effort:m` |
| 6+ | — | `effort:l` → **split it** |

### `!agent-ok` — all six, or it's absent

1. `## Verify` is a real command.
2. `## Done when` has no line needing judgment.
3. No decision words in the body: *design*, *decide*, *figure out*, *investigate*, *explore*, *consider*.
4. `effort:` is `s` or `m`.
5. `risk:` is not `high`.
6. `area:` is not `checks`.

Rules 5 and 6 overlap deliberately. The Verdict seal is never modified by an unsupervised agent, and stating it twice means a future refactor of the risk rules can't quietly remove the protection.

### `!needs-adr`

- `surface:` is `protocol` or `public`
- A dependency is added, removed, or version-changed
- An on-disk format changes
- `risk:high` and `kind:` is not `bug`

### `ready: true` — all of these

- Status is `draft` or `todo`
- `## Done when` present and non-empty
- `## Verify` present and parses as a command
- No decision words
- `effort:` is not `l`
- Every `blocked_by` is `done`
- `!needs-adr` absent, or the ADR exists and is `accepted`

Each failure is reported by name. "Not ready" with no reason is useless.

---

## 6. Routing

| `triage:` | When | Then |
|---|---|---|
| `auto` | Classified confidently, `ready: true` | Eligible for `todo`; `!agent-ok` ones can be picked up unsupervised |
| `needs-human` | Any classifier abstained, or `source: agent`, or `risk:high` | Stays `draft`. Reason recorded. |
| `blocked` | A `blocked_by` is unresolved, or `!blocked-ext` | Stays `draft`. Re-triaged automatically when the blocker closes. |

**Closing a ticket re-triages everything it blocks.** That's the one piece of automation that actually saves you time — the queue unblocks itself instead of you remembering what was waiting.

---

## 7. What triage writes back

Appended to frontmatter, regenerated each run, never hand-edited:

```yaml
---
id: T0231
title: Parse the frontmatter parent field
status: draft
# ── triage block, generated ──
labels: [area:context, kind:feature, risk:low, surface:internal, effort:s, origin:manual]
flags: [agent-ok]
triage: auto
ready: true
ready_failures: []
triaged_at: 2026-09-28T16:05:00Z
triage_version: 3
---
```

`triage_version` matters: bump it when rules change, and everything below the current version gets re-triaged on the next sweep. Without it, old tickets keep stale labels from rules that no longer exist.

---

## 8. Duplicates

Three checks, cheapest first:

1. **Exact title match** against open tickets → flag.
2. **File overlap** — same `files:` and same `area:` → flag as possible overlap.
3. **Semantic similarity** — optional, advisory, model-backed. Embed titles plus `## Do`, flag anything above threshold.

**Never auto-close a duplicate.** Add `possible_duplicate: [T0198]` and let a human decide. Auto-closing on a false positive loses work silently, and you won't notice for weeks.

---

## 9. Staleness

`trellis ticket sweep`, run weekly or in CI:

| Condition | Action |
|---|---|
| `doing` for more than 3 days | Flag — it's stuck or it's really an `l` |
| `review` for more than 7 days | Flag |
| `blocked` with all blockers `done` | Auto-move to `draft`, re-triage |
| `draft` for more than 30 days, never triaged `auto` | Flag for drop |
| Code marker points at a `done` ticket | Report — stale comment |
| `triage_version` below current | Re-triage |
| Two `doing` at once | **Error.** WIP limit of one. |

The sweep reports. It never drops anything on its own.

---

## 10. Implementation

One crate, `trellis-tickets`. Pure functions, no I/O in the rules layer.

```rust
pub fn triage(t: &Ticket, ctx: &RepoContext) -> Triage {
    let area    = classify_area(&t.files)?;
    let kind    = classify_kind(&t.title, &t.files, t.source);
    let surface = classify_surface(&t.files, ctx);
    let risk    = assess_risk(area, surface, &t.files, ctx);
    let effort  = estimate_effort(&t.files, t.new_tests);
    let flags   = derive_flags(t, area, risk, surface);
    let ready   = check_ready(t, &flags, ctx);
    Triage { labels: ..., flags, ready, route: route(t, &ready, risk) }
}
```

Every classifier returns `Option` — `None` means abstain, and abstain means `needs-human`. That's the whole fail-to-human design in one type.

**Table-driven tests.** A directory of fixture tickets with expected triage output, compared exactly. When a rule changes, the diff shows every ticket it reclassifies, which is exactly the review you want before shipping a rule change.

**CLI:**

```
trellis ticket triage <id>       # one ticket, show the reasoning
trellis ticket triage --all      # everything below current triage_version
trellis ticket ready <id>        # gate check only, with named failures
trellis ticket sweep             # staleness report
trellis ticket scan              # code markers → tickets
trellis ticket index             # regenerate INDEX.md
```

`triage <id>` prints *why* it assigned each label. A triage system you can't interrogate is one you'll stop trusting the first time it's wrong.

---

## 11. Don't automate these

**Priority.** Order in `INDEX.md` is priority, and ordering is judgment. A computed priority score is a number you'll argue with and then override.

**Status transitions.** Except `blocked → draft` when blockers clear. Everything else is a decision.

**Closing anything.** Not duplicates, not stale, not "obviously done." Flag and let a human pull the trigger.

**Assignment.** You're one person. There's nothing to assign.

**Estimation beyond s/m/l.** Any finer and you're doing estimation theater against a sample size of one.

**LLM classification of area or kind.** Paths and title verbs are deterministic and correct. A model here would be slower, nondeterministic, and occasionally confidently wrong about something a regex gets right every time.

---

## How this repository applies it

The standard is written for ticket files with YAML frontmatter. trellis's
tickets are GitHub issues on `maydaythecoder/trellis`, organised in the
Trellis roadmap project (#2), so each input the standard names is read from
the issue instead:

| The standard says | Here it is | Why |
|---|---|---|
| `files:` | the `**Files:**` line of the issue body | issues have no frontmatter; the line is where every ticket already lists its files |
| `status:` (`draft`, `todo`, `doing`, ...) | the project board's Status field; the rules read GitHub's `OPEN`/`CLOSED` state, never the board | triage never changes status (§1), so it does not need to read the board to label |
| the triage block in frontmatter (§7) | the YAML between `<!-- triage:start ... -->` and `<!-- triage:end -->` in the issue body | same fields; `blocked_by` is read from there |
| `trellis ticket <command>` | `trellis-tickets <command>`, a separate binary | the owner decided on 2026-09-28 that the tool does not grow the `trellis` CLI; the commands and their arguments are ADR 0040's, not §10's list |
| `triage_version: 3` (§7's example) | `triage_version: 2` | every triage block on the issues says 2; the library pins `TRIAGE_VERSION = 2` |
| `flags: [agent-ok]` (§7's example) | `!agent-ok`, with the `!` | the flags are GitHub labels, and §3 spells them with the prefix |

A rule that cannot decide returns `Outcome::Abstained { rule, reason }`
rather than §10's `None`: the same fail-to-human design, with the reason
carried so that `triage <n>` can print why (§10, last paragraph).

### Where the rules could not be applied as written

`crates/trellis-tickets` implements §5 and §6 over an issue's title, body,
labels and state. The standard assumes inputs (a diff, a repository
context) that an issue does not carry. Each rule abstains only when an input
it cannot read could change its answer. Where that would make a rule abstain
on every ticket, or where the standard names no signal, the table records
what the rule does instead.

| Rule | What the code does | Why |
|---|---|---|
| Files | The `**Files:**` line is split on `;` and `,`; parentheticals and a word after a path (`§4`) are commentary; `src/...` after `crates/<name>/...` continues that crate. An entry that is not a path (`if built: ...`, `no crate files`) makes the line unreadable. | Dropping an entry would undercount files and could lower effort and risk. |
| `area:` | §5's table as written, plus `crates/trellis-cli/**` → `area:cli` and `docs/**` → `area:docs`, plus the owner's mapping of this repository's other crates onto the same twelve values (#194, 2026-10-04): `crates/trellis-index/**` → `area:graph`; `crates/trellis-loop/**` and `crates/trellis-vcs/**` → `area:runtime`; `crates/trellis-vault/**` → `area:context`; `crates/trellis-queue/**` → `area:agents`; `crates/trellis-trail/**` → `area:protocol`; `crates/trellis-core/**`, `scripts/**` and `.github/**` → `area:build`. `desktop/**`, the Tauri desktop workspace, → `area:tui` (#249). On 2026-10-05, the root `CLAUDE.md` → `area:docs`, `crates/tui-demo/**` → `area:tui` and `crates/trellis-tickets/**` → `area:build` (#282). Every other path (`Cargo.toml`, `fuzz/`, `examples/`, ...) maps to nothing, and the rule abstains. With two areas, the first-listed file's area labels the ticket. | §5 names crates this repository does not have and omits most that it does. The owner mapped them onto existing values rather than adding new ones, so the dimension keeps its fixed set (§1, "dimensions, not a label bag"). §5 allows two areas without saying which one wins. |
| `kind:` | §5's table in order. If the files cannot be read and the title does not decide `bug`, the rule abstains. A directory entry leaves "only `*.md`" and the other "only" rows unknown. | The file rows come before `feature` in the table, so an unreadable file list could change the answer. |
| `surface:` | `protocol/**` → protocol. A file in a library crate's `src/` (any crate but `trellis-cli`, which is binary-only) abstains. Otherwise `crates/trellis-cli/src/**` → cli, else internal. | The public row needs a `pub fn`/`pub struct` diff, and an issue has none. |
| `risk:` | High on `!breaking`, `!security`, a file under `crates/trellis-checks` or `protocol/`, even when `area` abstained. A `Cargo.toml` in the files abstains: the workspace forbids `unsafe`, so only a manifest edit can introduce it. Touching rollback, worktree teardown or session persistence is read from the owner's path table (#196, 2026-10-04): `crates/trellis-vcs/**`, `crates/trellis-trail/**`, `crates/trellis-loop/src/{integrate,partition,supervise}.rs` and `crates/trellis-cli/src/session*.rs`, or a directory entry containing one of them. An on-disk format change is expected to carry `!breaking`. | These are the signals the files and labels carry. |
| `effort:` | `s` needs both 1-2 files and 0-1 new tests; 6+ files is `l`; anything else is `m`. New tests are the `<min>` of the Verify block's `scripts/tickets/verify-tests.sh` lines. A directory entry abstains. | §5's table leaves the mixed rows open; `m` is the only value left when `s` fails and `l` does not apply. The Verify line is the one test count an issue records. |
| `origin:` | Read from the one `origin:` label. None, two, or an unknown value abstains. | §2 has the creation path set it; it is not in the text. |
| `!breaking`, `!security`, `!blocked-ext` | Carried from the labels. | §5 uses them as inputs and gives no derivation. |
| `!needs-adr` | §5's list. A `Cargo.toml` or `Cargo.lock` in the files abstains unless another condition already set the flag. | Whether a dependency changes needs the diff. |
| `!agent-ok` | **Never set by the tool; hand-set only.** The owner decided this on 2026-10-04 (#195). | Rule 2 ("no line needing judgment") is itself a judgment, so it has no deterministic check, and a heuristic would label confidently and wrongly, which §1 forbids ("fail to human, never guess"). The flag asserts all six conditions were shown, and rule 2 cannot be shown by a rule, only by the person who sets the label. `scripts/tickets/next.sh` does not read the flag, so what agents start is unchanged. |
| Decision words | The `## Decision` section and the generated triage block are not scanned, nor is a path component: a word that touches a `/` or carries a file extension (`docs/design/DESIGN.md`) is a file name (2026-10-25). The rest of the body is scanned as §5 lists it, so prose that mentions a past decision ("ADR 0031 decided ...") still fails. | Those two record decisions already made; a path names a file, not a decision the work still has to make. |
| `ready:` | "Status is draft or todo" is read as the issue being open. `!needs-adr` always fails the gate as `needs ADR`, because no ADR is linked from an issue. A blocker whose state the caller did not supply abstains. G0's label checks (`parked`, `split`, `triage:needs-human`) are not in §5's list and are not in this rule. | `trellis-tickets ready <n>` (#78) runs G0's checks as `scripts/gates/ticket-ready.sh` reads them, beside `no verify command`, `decision words`, `effort:l` and `needs ADR`; when `effort:l` or `needs ADR` abstains it says so, and the issue is not ready. |
| `triage:` | A ticket that is not ready and not blocked matches no row of §6's table and goes to `needs-human`, with its failures as the reasons. A story abstains. | Fail to human, never guess (§1). |
| Sweep (§9) | `trellis-tickets sweep` (#80) reports label drift only: a dry run unless `--apply`, and it skips `parked` and `split` issues. A dimension whose rule abstained keeps its labels and is printed as `abstained`. It never writes `ready` or the block's `ready:`/`ready_failures:` lines, never sets or clears `!agent-ok`, and never lowers `risk:` (a lower value is printed as abstained). `triage:` is applied only when every other dimension was decided and applied. On an issue whose labels change, the block's `labels:`, `flags:`, `triaged_at:` and `triage_version:` lines are rewritten, and `triage:`/`triage_reasons:` when the route was applied; no other byte of the body changes, and an issue that changed after it was read is not written. §9's staleness rows (`doing`, `review`, age) read the board, which the tool does not. | `ready` is `scripts/tickets/sync-ready.py`'s until `sweep --apply` reproduces it (ADR 0040 decision 7). `!agent-ok` has no rule (#195). The risk rule cannot see rollback, worktree teardown or session persistence (#196). A route that follows from an abstention would move issues out of `triage:auto` on an open question. |
| Scan (§2) | `trellis-tickets scan` (#80) reports G4's markers, read from `scripts/gates/no-stubs.sh`, with G4's exclusions, as `file:line` proposals. It files nothing and does not rewrite the marker with an ID. It walks the files and skips `target/` and dot-directories, which a diff never shows G4. | §2 has scan file and renumber; ADR 0040 has it propose only. It cannot spawn `git` (ADR 0040 decision 6). |
| Index (§10) | `trellis-tickets index [--out <p>]` (#81) writes JSON, not §10's `INDEX.md`, with `"triage_version": 2` at the top; the schema is below. It indexes the open issues, the list `sweep` reads. It reports what the rules compute and adds two abstentions of its own: `!agent-ok` on every issue, and a computed `risk:` below the recorded label, with `triage` (and, when the recorded risk is high, `flags` and `ready`, through `!needs-adr`). The order is by story, then issue number, and holds no timestamp, so a re-run over the same issues writes the same bytes. | The owner decided the JSON shape for S13 on 2026-09-28 (#81). `!agent-ok` has no rule (#195), so its absence from `flags` would read as a decision. The risk rule cannot see rollback, worktree teardown or session persistence (#196), and `sweep` never lowers the label for the same reason. An area the table does not map arrives abstained from the rule. A `triaged_at` would change every run and hide the lines that moved. §11 keeps priority out of the order, and it is. |

### The `index` schema

`trellis-tickets index` prints, or writes to `--out`, one JSON object,
pretty-printed with two-space indents and one final newline. DEL and the
C1 control characters, which JSON allows raw, are written as `\u` escapes,
so the document parses to the same strings and cannot drive a terminal.

```
{
  "triage_version": 2,
  "issues": [ <entry>, ... ]
}
```

`triage_version` is the rule set's version (`TRIAGE_VERSION`), the value
every triage block carries. `issues` holds one entry per open issue, sorted
by story number, then by the id as a string (`S07` before `S7`), then by issue number; an
issue with no story comes after every story.

Each entry has these fields, in this order:

| Field | Type | What it is |
|---|---|---|
| `number` | integer | The issue number. |
| `title` | string | The title as on GitHub. |
| `story` | string or null | `S<n>`. A `type:story` issue's comes from its title's `S<n>:` prefix. Any other issue's comes from the `story:` line of its triage block, or, when it has no block, from a line that starts `story:` anywhere in the body (as G0 reads `blocked_by:`). Null when there is none; null with a note when the line is not `S` and digits, or there are two. |
| `labels` | array of strings | The labels on GitHub now, sorted, each once. |
| `blocked_by` | array of integers, or null | The `blocked_by:` entries in the order written; null, with a note, when they cannot be read. |
| `area`, `kind`, `risk`, `surface`, `effort`, `origin`, `flags`, `ready`, `triage` | line | Each rule's outcome, the same object `triage <n> --json` prints for that dimension. |
| `agent_ok` | line | Always abstained, with the reason (#195). |
| `notes` | array of strings | Intake that could not be read: a blocker whose state could not be fetched (its `ready` and `triage` then abstain), a `story:` or `blocked_by:` that is malformed. Empty when there is none. |

A line is one of two objects:

```
{ "outcome": "decided", "value": <value>, "rule": "<the rule that set it>" }
{ "outcome": "abstained", "rule": "<the rule that abstained>", "reason": "<why>" }
```

`value` is a label string (`"area:cli"`) for the six dimensions; an array
of flag labels, sorted, for `flags`; `{"ready": bool, "failures": [string]}`
for `ready`; and `{"route": "triage:<route>", "reasons": [string]}` for
`triage`.

There is no `triaged_at`: the index is for diffing two runs, and a field
that changes on every run would be a diff on every issue.
