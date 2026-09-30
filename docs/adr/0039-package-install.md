# ADR 0039 — `trellis install`: skills and prompt templates, never code

**Status:** Accepted.
**Date:** 2026-09-29
**Issue:** #118 (split from #51). Related: #120 (no package code).
**Amends:** ADR 0021 row 10 ("packages: deferred ... needs the trust store
first"). Grows the CLI surface fixed by brief 4.12, as the owner directed.

## Context

ADR 0021 row 10 deferred pi's packages: an install command for skills and
prompt templates, on the grounds that it needed the ADR 0023 trust store
first. The owner decided on 2026-09-28 to build it narrowly: an explicit
ref, only skills and prompt templates, `--list` and `--remove`. On #120 the
owner's answer to "may a package carry code" is no.

What a package source holds is text from outside any repository the user
is working in, fetched over the network or read from a local repository.
Its paths, file modes and sizes are all chosen by whoever wrote it.

## Decision

1. **The surface.** One command, three forms:
   - `trellis install <git-url|path>@<ref>`
   - `trellis install --list`
   - `trellis install --remove <name>`

   It needs no repository: packages are user state, as models are.

2. **The ref is required.** The spec splits at its last `@` (a URL's user
   part holds one too); a part after it that holds `:` is the rest of an
   address, since git forbids `:` in a ref name, so `git@host:o/p.git`
   with no ref is refused as naming none. The ref is resolved to a commit
   (`<ref>^{commit}`), and both are recorded. An implicit default branch
   would record a commit nobody chose.

3. **Where it goes.** `<user config dir>/packages/<name>/`, with the user
   config directory resolved by ADR 0025 §7. `<name>` is the source's last
   path component less `.git`, and must be `[A-Za-z0-9._-]`, not begin or
   end with `.`, not be a Windows device name and not be `packages.toml`.
   A name already installed (compared without case) is refused; remove it
   first.

4. **`packages.toml`** in `<user config dir>/packages/` records one
   `[[package]]` per install: `name`, `source` (made absolute when it is a
   local directory), `ref` and `commit`. `--list` prints them. `--remove`
   deletes the package's directory and its entry. The manifest is written
   beside and renamed over. A manifest that cannot be read or parsed
   refuses the command rather than reading as empty, since the next save
   would drop every entry (ADR 0025 §7's rule for config files). A source
   URL carrying a password is refused, because it would be written here.

5. **What is copied.** Exactly two shapes, and only as plain files (git
   mode `100644`):
   - `skills/<skill>/...` files ending `.md` or `.txt`, where `<skill>`
     passes ADR 0019's name alphabet and `skills/<skill>/SKILL.md` exists;
   - `prompts/<name>.md`, one level deep.

   Everything else is skipped and named in the output: hooks, tools,
   `.trellis/`, scripts inside a skill, executables (`100755`), links
   (`120000`), submodules (`160000`). **A package never carries code.**
   There is no flag to install one that does.

6. **How it is fetched.** Nothing is checked out. Every git spawn goes
   through `trellis_vcs::command()`, so the environment is scrubbed of keys
   and `core.fsmonitor` and hooks are off. On top of that, for an
   untrusted source: `clone --bare --no-local` (a local source goes through
   the transport and is checked like a remote one), `protocol.ext.allow=never`
   (the `ext::` transport runs a command named in the URL),
   `transfer.fsckObjects=true`, `GIT_TERMINAL_PROMPT=0`, and `--` or
   `--end-of-options` before every source, ref and commit. A source or ref
   beginning with `-` is refused before git runs. Files are listed with
   `ls-tree -r -l -z` and read by blob id with `cat-file`, so a package path
   never reaches a git argument and a link is never followed on disk.

7. **Paths are an allowlist.** Each accepted path passes
   `trellis_core::paths::unsafe_repo_path` and every component is held to
   the alphabet in 3. Files are written with `create_new`, so two package
   paths one filesystem folds together (`SKILL.md`, `skill.md` on Windows)
   refuse the install instead of one replacing the other. The copy is built
   in a staging directory under `packages/` and renamed into place; the
   staging directory, bare clone included, is removed whatever the outcome.

8. **Bounds.** Before any file is read: at most 2,000 files, 1 MiB each,
   32 MiB in total. Over any of them the install is refused, not trimmed.

9. **Why the trust store is not the gate.** Row 10 assumed it would be.
   The argument ADR 0025 made for user-level skills holds here: the trust
   store vouches for what one repository makes trellis execute, keyed by
   that repository's root, and a package is in no repository and executes
   nothing. What makes a package safe to install is 5 and 6, not a digest
   of a repository's hooks.

## Not built

- **Runs do not read `packages/` yet.** Skills still load from
  `<user config dir>/skills` and `~/.agents/skills` (ADR 0025, integrated),
  and templates from `.trellis/prompts`. Adding `packages/*/skills` and
  `packages/*/prompts` to those lists changes what the model is shown and
  in what precedence, so it is its own change. The install command says so
  when it finishes, rather than looking like it did more than it did.
- **Upgrade in place.** Reinstalling a name is refused; `--remove` then
  install.
- **The fetch itself is not bounded.** `git clone` downloads the whole
  source before the bounds in 8 apply, and a slow or huge source is waited
  on. The bounds cap what is written, not what is fetched.

## Alternatives rejected

- **Clone with a working tree and copy files.** The checkout would write
  every path of the package to disk, links included, before any of them was
  vetted.
- **Install scripts or tools with a trust prompt.** Refused by the owner on
  #120: packages carry no code.

## Consequences

- ADR 0021 row 10 now reads "built in part".
- Evidence: `crates/trellis-cli/src/install.rs`, tests `install_*`
  (`cargo test -p trellis-cli install_`), which install from a local git
  repository and never touch the network.
