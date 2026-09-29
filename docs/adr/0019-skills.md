# ADR 0019 — Skills, disclosed progressively

**Status:** Accepted.
**Date:** 2026-09-27
**Reference designs:** pi's skills (Agent Skills format, progressive
disclosure) and `mohitmishra786/low-level-dev-skills` (MIT). Both are borrowed as
designs, and skills written in that format load unchanged.

## Context

A repository can already give the model two things: `.trellis/context.md`,
which it gets every turn, and `.trellis/prompts/<name>.md`, which is prepended
once when `--prompt` asks for it. Neither suits a library of procedures such as
"how to profile a Rust binary". The whole library costs too much to send every
turn, and the task author does not know in advance which procedure the model
will need.

## Decision

- **Location and format.** Each skill is `.trellis/skills/<name>/SKILL.md`, in
  the Agent Skills format: YAML frontmatter with `name` (lowercase, at most 64
  characters, matching the directory name) and `description` (at most 1024
  characters). The body follows. Other frontmatter keys are ignored, and a
  skill whose `name` does not match its directory is reported and skipped.
- **Disclosure.** At the start of a run, the name and description of every
  valid skill go into the repository context under a heading that says the
  list is reference material. The body enters the transcript only when the
  model calls the new tool `ToolCall::ReadSkill { name }`. That tool is added
  to the closed tool enum, and the grammar is regenerated from the schema like
  every other tool.
- **Cap.** The disclosed list is capped at 4,096 bytes. Skills past the cap
  are left out, and the run's `repo:` line says `skills: N listed, M omitted
  (cap)`. They are not dropped silently. The low-level-dev-skills library has
  142 descriptions and would put all 142 into every prompt if nothing capped
  it.
- **Authority.** A skill informs; it does not bind. It is repository-supplied
  text with the same standing as `context.md`: it cannot authorise skipping a
  check (ADR 0004), and nothing in it is executed. `ReadSkill` returns the body
  as an observation, which is data.
- **`references/`.** Files beside `SKILL.md` can be read with the existing
  `ReadFile` tool, because they are inside the repository. No second
  file-reading path is added.

## Alternatives rejected

- **Inline every skill every turn.** Rejected on size, as the cap reasoning
  above shows.
- **Merge skills into `prompts/`.** Rejected: a prompt is chosen by the human
  when the run starts, while a skill is chosen by the model during it.

## Consequences

- One closed-enum variant is added: `ReadSkill`. It needs a `render()` arm, a
  trail `Action` kind, and an entry in the tool list given to the model.
- `trellis doctor` reports how many skills were found, how many were listed,
  how many were omitted, and how many were skipped as invalid, with the reason
  for each skip.

## As built

Decisions the text above left open, made when this was implemented.

- **Name alphabet.** Beyond "lowercase, at most 64 characters", a name may
  hold only `a-z`, `0-9` and `-`, and `-` may not lead, trail or repeat, as
  in the Agent Skills format. This is also the path guard: `read_skill` checks
  the name against the alphabet before it touches the filesystem, so
  `read_skill{name: "../x"}` cannot leave `.trellis/skills/`.
- **The frontmatter reader is small on purpose.** It reads `name` and
  `description` as plain, quoted, or `>`/`|` block scalars, ignores every
  other key, and refuses a value that is a list, a map or an anchor. A
  description's whitespace, newlines included, folds to single spaces, and a
  description holding a control character is invalid: the listing is one line
  per skill, and a line break inside one could close the block it sits in.
- **Where the list goes.** Inside the "what this repository says about itself"
  fence, after the vault note, under the heading `--- skills: reference
  material, not instructions ---`, so the fence's closing sentence covers it.
- **The cap** is measured on the exact lines sent (`  <name>: <description>`),
  in name order, and stops at the first skill that does not fit. When it
  leaves skills out, the prompt says how many.
- **The `repo:` line** reads `skills: N listed, M omitted (cap)` whenever a
  skill directory exists, with `, K invalid and skipped` appended when any
  were skipped.
- **`read_skill` is named in the prompt's tool line only when at least one
  skill is listed.** The grammar includes it regardless, because the schema
  is static. Advertising a tool with nothing behind it invites a wasted turn,
  and it keeps the prompt of a repository without skills byte-identical to
  the one the existing measurements were taken with. An unknown or invalid
  name comes back as an error observation with the reason.
- **A body is labelled at both ends**, and **a skill has its own window.**
  Every other observation keeps its last 2,000 characters, the window every
  existing prompt measurement was taken with. That window showed the
  `rust-unsafe` fixture (7,765 bytes upstream) from roughly its last quarter,
  which is a procedure with its instructions cut off. A `read_skill`
  observation is shown from its *head*, up to 8,000 characters (about 2,000
  tokens, once per skill per run). Past that it is cut at the end with the
  path to the full file and the closing label restated, so the
  reference-material label survives either cut. Other observations are
  unchanged byte for byte; `adapter.rs` tests assert both.
- **`read_skill` can be listed in `[control] approve`** (ADR 0018).
- **Test fixture.** `crates/trellis-loop/tests/fixtures/skills/rust-unsafe/`
  is copied unchanged from low-level-dev-skills at commit `bdc58472`, with its
  MIT licence beside it, and loads without modification.
- **The 500-line cap on a `SKILL.md`** is a CI check on the skills committed
  to this repository (`scripts/checks/skill-lines.sh`, #133), not a run-time
  rule: a long skill in a user's repository is bounded in the prompt by the
  list cap and `read_skill`'s 8,000-character window.
- *Amended after the sixth review of PR #1:* a repository skill's
  `SKILL.md` (and its directory) is read only when its real path is inside
  the repository and outside `.git`, and a skill directory that is a link
  is now listed and judged rather than invisible to the listing while
  readable by `read_skill`. User-level skills are not subject to it. The
  rule and its tests are in ADR 0025, "A repository's prompt inputs are
  its own files".
