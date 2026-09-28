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
