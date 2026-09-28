# Skill fixtures

`rust-unsafe/` is copied unchanged from
[`mohitmishra786/low-level-dev-skills`](https://github.com/mohitmishra786/low-level-dev-skills),
path `skills/rust/rust-unsafe/`, at commit
`bdc58472fa9f309ed1b3f7d985a0d8e9bd8f4608`. That repository is MIT-licensed;
its licence text is `LICENSE` in this directory, as the licence requires.

It is here as a test input, not as documentation for this project. ADR 0019
says skills written in the Agent Skills format load unchanged, and the only
way to test that is with a skill nobody wrote for this repository.
`trellis-loop/src/skills.rs` copies it into a temporary repository's
`.trellis/skills/` and reads it through discovery and `read_skill`.

Do not edit the copy. If it needs to change, re-copy it from a named upstream
commit and update the commit above.
