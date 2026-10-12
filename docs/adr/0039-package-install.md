# ADR 0039 — `trellis install`: skills and prompt templates, never code

**Status:** Accepted.
**Date:** 2026-09-29
**Issue:** #118 (split from #51). Related: #120 (no package code).
**Amended:** 2026-09-29 (#119): §10, how runs read installed packages.
**Amended:** 2026-09-30 (#120): §5 and §9, every skipped path named and
the trust store untouched, each held by a test.
**Amended:** 2026-09-30 (#200): §8, the tree listing bounded while it is
read.
**Amended:** 2026-10-12 (#617): §4, each entry carries a per-file content
digest and `--list` reports it.
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

   *2026-10-12 (#617):* an entry also carries `digest`, a table of
   `<package path> = <sha256>` over the bytes of each file as the install
   wrote them (ADR 0044's rule: the bytes on disk are what count), one per
   file rather than one over all of them so a change can be named by path.
   `--list` ends each line with `digest ok`, `modified: <paths>` (an edited
   path bare, a missing one `(missing)`, one not installed `(added)`; a
   link or an unreadable file counts as modified) or `not recorded`. The
   field is optional: a manifest from before this date still loads, its
   entries read "not recorded" and never "unchanged", and a save does not
   invent one. The digest is informational and never a trust decision; §9
   holds and the trust store is untouched. The hashing is `sha2`, already
   in the tree through ADR 0044. Evidence: `install_digest_*`
   (`cargo test -p trellis-cli install_digest_`).

5. **What is copied.** Exactly two shapes, and only as plain files (git
   mode `100644`):
   - `skills/<skill>/...` files ending `.md` or `.txt`, where `<skill>`
     passes ADR 0019's name alphabet and `skills/<skill>/SKILL.md` exists;
   - `prompts/<name>.md`, one level deep.

   Everything else is skipped and named in the output: hooks, tools,
   `.trellis/`, scripts inside a skill, executables (`100755`), links
   (`120000`), submodules (`160000`). **A package never carries code.**
   There is no flag to install one that does. Every skipped path is named
   on its own line, with no cap and no "and N more" (#120): a count would
   hide exactly the hook the user wanted to see. The listing is already in
   memory from `ls-tree`, so naming all of it reads nothing new. Evidence:
   `install_skips_code_and_names_every_skipped_path`.

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

   The listing those bounds are applied to is bounded too, while
   `ls-tree` is read and before the entry list is complete (#200): at most
   10,000 entries in the source's tree, copied or not, and 8 MiB of
   listing. Without it a source with a huge tree was held whole in memory,
   and #120 prints one line per skipped entry. Over either, the install is
   refused with a message naming the limit, and git is stopped.
   - *10,000 entries* is chosen, not measured: five times the 2,000 files
     that may be copied, leaving room for the README, licence, CI and other
     paths a package carries and never copies. No package corpus was
     surveyed; a real package over it is the evidence that would move it.
   - *8 MiB* is derived, not measured: about 100 bytes of fixed fields per
     `-z` record (mode, type, a 64-hex id, size, separators) plus an
     assumed average path of about 700 bytes, over 10,000 records. It is
     there because a count alone does not bound memory: a tree entry's path
     has no length limit this code can rely on, so one entry can be large.

9. **Why the trust store is not the gate.** Row 10 assumed it would be.
   The argument ADR 0025 made for user-level skills holds here: the trust
   store vouches for what one repository makes trellis execute, keyed by
   that repository's root, and a package is in no repository and executes
   nothing. What makes a package safe to install is 5 and 6, not a digest
   of a repository's hooks. So install neither reads nor writes the trust
   store: it is byte-identical before and after (#120). Trusted code lives
   under a repository's `.trellis/` only; `packages/` holds no code for a
   digest to cover. Evidence:
   `install_skips_code_and_leaves_the_trust_store_byte_identical`, with the
   store where ADR 0023 puts it, beside `packages/` (the two share a
   directory unless `TRELLIS_CONFIG_DIR` is set, which moves `packages/`
   and not the store). Letting a package
   carry code would reverse the owner's answer on #120 and needs its own
   ADR, which would have to put that code under the trust digest by
   content before anything of it runs.

10. **How runs read installed packages** (#119; the owner's order,
    decided 2026-09-28: repository, installed package, `~/.agents/skills`).
    - **Which packages.** Those `packages.toml` records, in package-name
      order, whatever order they were installed in. A directory under
      `packages/` the manifest does not name (a staging directory
      mid-install, a stray copy) is not read. A manifest that cannot be
      read or parsed refuses the run and `rpc`'s `get_commands`, and
      `doctor` prints it on its line, rather than a run going ahead
      without skills the user installed. A manifest name is held to the
      alphabet in 3 before it becomes a path.
    - **Skills.** The lookup order is the repository's `.trellis/skills`,
      then `<user config dir>/skills`, then each package's `skills/`, then
      `~/.agents/skills`. The first source holding a name wins it, in the
      listing, in `read_skill` and for a forced `--skill` alike, because
      all three take the same directory list. The user's own
      `<user config dir>/skills` sits above packages, which the owner's
      order does not name: a skill the user wrote on this machine is a
      more specific statement than one fetched. Between two packages the
      name order above decides.
    - **The `repo:` line** counts installed skills apart from the user's
      own, says neither was gated on repository trust, and names every
      collision with the source that won it and the ones it won over.
    - **Not trust-gated,** for the reason in 9, which is the one ADR 0025
      ("Integrated") gives for user-level skills: the trust store vouches
      for what a repository makes trellis execute, and a package is in no
      repository and executes nothing.
    - **One cap.** ADR 0019's 4,096-byte disclosure cap applies to the
      merged, name-sorted list of every source, unchanged. Installing a
      package does not buy a second allowance.
    - **Templates.** `.trellis/prompts/<name>.md`, else the first package
      in name order with `prompts/<name>.md`. The same name check guards
      both, `/prompts` and `get_commands` list each name once from the
      source that would be read, and an installed template is still a
      task, never configuration (ADR 0025 §3). The manifest is read only
      when the repository has no template of that name.
    - Installed files are the user's own, outside any repository, so they
      are read as they are, as user-level skills are; the
      repository-input rule (ADR 0025) is for what a repository supplies.
    - Evidence: tests `installed_skill_*` in `crates/trellis-cli/src/context_files.rs`
      (`cargo test -p trellis-cli installed_skill_`) and
      `installed_template_is_listed_and_read_and_the_repository_wins_a_name`.

## Not built

- **Upgrade in place.** Reinstalling a name is refused; `--remove` then
  install.
- **The fetch itself is not bounded.** `git clone` downloads the whole
  source before the bounds in 8 apply, and a slow or huge source is waited
  on. The bounds cap what is listed and written, not what is fetched.

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
  repository and never touch the network. The listing bound in 8:
  `install_tree_listing_bounded_*`
  (`cargo test -p trellis-cli install_tree_listing_bounded_`).
