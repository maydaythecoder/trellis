# Dependency Audit — 2026-09-28

Predecessor: `audit/DEPENDENCIES-2026-09-22.md`. This file re-audits what
changed in the manifests since, measured with that file's commands. It applies
no verdict: the one TRIM below is its own ticket.

## Verdict

Every dependency line the inherit PR added is used and is KEEP, except `jiff`,
which is TRIM (#154) for a measured 21,504 B. The release graph grew from 157 to
176 crates, 19 in all: 3 on `v0.1-resolver` after the last audit, 16 from the
inherit PR, 10 of which are the `hosted` TLS stack.

**The binary more than doubled, and dependencies are not most of it.** The
default release build is 10,861,056 B against the 4,810,240 B baseline
(+6,050,816 B). The `--no-default-features` build, which links none of the TLS
stack, is 9,519,616 B, so `hosted` accounts for 1,341,440 B of the growth. The
remaining ~4.7 MB came with 9 crates that are either first-party
(`trellis-queue`, `trellis-vault`) or small (`jiff`, `unicode-normalization`,
`tracing-serde`, and their dependencies). ADR 0024 recorded 7,084,032 B at
`3234bee`, before any TLS crate was added. Most of the growth is therefore
first-party code: the agent loop, queue, vault, RPC and hosted-protocol work.
That is inferred from these figures and was not attributed per crate. The
`cargo bloat` run that would attribute it was stopped (see Unverified).

## Numbers

**Measured on:** Windows 11 Home 10.0.26200, target `x86_64-pc-windows-msvc`,
rustc/cargo 1.93.0. Release profile as the workspace defines it: `lto = "fat"`,
`codegen-units = 1`, `strip = "symbols"`. Tree at `inherit` = `969c059`.

**Method.** These are the predecessor's commands, unchanged:

```
release graph   cargo tree -p trellis-cli --edges normal --prefix none | sed 's/ (\*)//' | awk '{print $1}' | sort -u | wc -l
whole workspace cargo tree --prefix none | sed 's/ (\*)//' | awk '{print $1" "$2}' | sort -u | wc -l
tree lines      cargo tree | wc -l          # whole workspace, not -p
binary          cargo build --release -p trellis-cli ; ls -la target/release/trellis.exe
duplicates      cargo tree -p trellis-cli --edges normal -d
```

`--no-default-features` is added to `cargo tree` and to `cargo build` for the
local-only rows. **Evidence that the method is the same:** the release-graph
command run at `7ad9b2d` (the commit that added the predecessor) prints 157,
the predecessor's figure.

| | Baseline (2026-09-22, `7ad9b2d`) | `origin/v0.1-resolver` (`e64b80c`) | `inherit` (`969c059`), default features | `inherit`, `--no-default-features` |
|---|---|---|---|---|
| Unique crates, release graph (names) | 157 | 160 | **176** | 166 |
| Unique crates, release graph (name+version) | 159 | — | 178 | — |
| Unique crates, whole workspace | 258 | — | 273 | — |
| `cargo tree` lines | 660 | — | 717 | — |
| `trellis.exe` bytes | 4,810,240 | not built | **10,861,056** | 9,519,616 |
| Duplicate versions, release graph | 2 pairs | — | 2 pairs (`syn` 2/3, `hashbrown` 0.16/0.17) | — |

- The default build was built twice from the same tree, with the
  `--no-default-features` build between them. Both default builds gave
  10,861,056 B.
- **The default build is what ships** (`release.yml` passes no feature flags,
  ADR 0024). The `--no-default-features` figure is the local-only binary, given
  separately. It does not replace the default figure.
- Compared with ADR 0024's measurements at `29528e2` (8,685,568 B default /
  7,357,440 B local-only, same OS, target and profile): +2,175,488 B default
  and +2,162,176 B local-only since then. The `hosted` cost is 1,328,128 B there
  and 1,341,440 B here.
- The ADR 0024 table's "178 crates" is the name+version count. The two counts
  agree.

**Where the 19 crates came from** (the `comm` of the sorted name lists):

- `v0.1-resolver` after the audit (+3), all from `1d3d06f` (2026-09-27) and the
  vault work: `tracing-serde`, `trellis-queue`, `trellis-vault`.
- The inherit PR, `hosted` only (+10): `rustls`, `rustls-pki-types`,
  `rustls-webpki`, `ring`, `untrusted`, `subtle`, `zeroize`, `tokio-rustls`,
  `webpki-roots`, `getrandom` (0.2, through `ring`). This is the same ten that
  ADR 0024 lists.
- The inherit PR, both builds (+6): `jiff`, `jiff-core`, `jiff-tzdb`,
  `jiff-tzdb-platform`, `unicode-normalization`, `tinyvec`.
- None were removed.

**Duplicates, two scopes.** The row above uses the predecessor's release-graph
scope and is unchanged at 2 pairs. `cargo deny check` looks at the whole
workspace, dev-dependencies included, and warns of 4: `syn`, `hashbrown`,
`getrandom` and `windows-sys`. ADR 0024 records that the two added pairs come
through `ring`.

## Every dependency line added

These rows are the `+` lines of
`git diff origin/v0.1-resolver...inherit -- '*Cargo.toml'`. They are followed
by the named items that no manifest line in that diff adds. Usage was checked by
grepping each crate's `src/` and `tests/` for the dependency's path. `cargo
machete` reports only `crossterm` in `tui-demo`, which is the false positive the
predecessor explains (it is reached through `ratatui::crossterm::`).

| Manifest | Line | Verdict | Reason |
|---|---|---|---|
| `trellis-checks` | `trellis-core.workspace = true` | KEEP | `trellis_core::secrets::scrub` on every spawned check (the child-env invariant in CLAUDE.md), plus `paths::symlink_for_test`. The predecessor cut this line for zero references. It now has references. No new crate: `trellis-core` was already in the graph |
| `trellis-vcs` | `trellis-core.workspace = true` | KEEP | `trellis_core::secrets::scrub` on every `git` spawn (`lib.rs`), the same invariant. Also re-added after the predecessor's cut, for the same reason |
| `trellis-vcs` | `[dev-dependencies] tempfile.workspace = true` | KEEP (dev) | Fixture repositories for `git_guard.rs` tests. Not in the release graph |
| `trellis-cli` | `serde_json.workspace = true` (moved from dev to normal) | KEEP | `--json`, `log --json`, `rpc` framing and the after-run hook's outcome JSON (44 references). No new crate: `serde_json` was already in the graph through the libraries |
| `trellis-cli` | `jiff = "0.2.37"` | **TRIM** → #154 | `trellis vault dates` needs *local* calendar days, and std has no time zones. It is used at two sites in `vault_cmd.rs`, and on Windows it needs the bundled tzdb, because Windows has no system zoneinfo. Keep the crate and drop the default features those calls do not use (`perf-inline`, `tzdb-concatenated`). Measured: −21,504 B, crate count unchanged (method under TRIM measurement). Adds 4 crates (`jiff`, `jiff-core`, `jiff-tzdb`, `jiff-tzdb-platform`) |
| `trellis-cli` | `[features] default = ["hosted"]` | KEEP | ADR 0024's decision: tier 1 over HTTPS is in the released binary. Cost measured here: +10 crates, +1,341,440 B (12.3% of the default binary). A local-only binary is one flag away |
| `trellis-cli` | `hosted = ["trellis-model/hosted"]` | KEEP | Forwards the feature. Without it `--no-default-features` could not drop TLS |
| `trellis-loop` | `trellis-vault.workspace = true` | KEEP | `search_notes`/`read_note` use the vault's reader and BM25 index (`notes.rs`) and do not keep a second copy. `trellis-vault` does not depend on `trellis-model`, so the CLAUDE.md boundary holds |
| `trellis-loop` | `trellis-vcs.workspace = true` | KEEP | The worktree fingerprint around `post_tool` hooks, and the `git grep` in `listing.rs` goes through `trellis_vcs::git_argv` (the every-git-spawn invariant) |
| `trellis-loop` | `sha2.workspace = true` | KEEP | The trust store's fingerprints (`trust.rs`, ADR 0023). Already in the graph for the trail |
| `trellis-loop` | `thiserror.workspace = true` | KEEP | `exec.rs` and `hooks.rs` error enums. The predecessor cut it for zero references. It now has them |
| `trellis-model` | `rustls` (0.23.45, `ring`, `std`, `tls12`, optional) | KEEP | The TLS implementation for `hosted`. Uses the `ring` provider, not the default `aws-lc-rs`, which needs cmake and nasm (ADR 0013's clean-machine rule). `tls12` has no stated reason in ADR 0024. Dropping it would refuse a user-chosen endpoint that only speaks TLS 1.2, which changes what a user can connect to. That makes it an owner decision, not a chore. Its byte cost was not measured. It is the next candidate if binary size becomes a gate |
| `trellis-model` | `tokio-rustls` (0.26.5, `ring`, `tls12`, optional) | KEEP | Runs rustls over the existing `tokio::net::TcpStream`, so the HTTPS path has the same timeout and cancellation as the plain one. The alternative is `reqwest`, which the predecessor kept out of the core |
| `trellis-model` | `webpki-roots` (1.0.9, optional) | KEEP | Mozilla's root set compiled in, so trust does not depend on the machine's certificate store. The alternative (`rustls-native-certs`) is a different trust policy, not a trim |
| `trellis-model` | dev-dep self-reference gains `"hosted"` | KEEP (dev) | Without it, `cargo test -p trellis-model` compiles out the TLS configuration test, which then runs only when another crate in the same invocation happens to turn the feature on |
| `trellis-model` | `[features] hosted = ["dep:rustls", "dep:tokio-rustls", "dep:webpki-roots"]` | KEEP | The gate that keeps the local-only build free of TLS (measured: 166 crates vs 176) |
| `trellis-queue` | `toml.workspace = true` | KEEP | Agent profiles and routines under `.trellis/` (`portable.rs`, ADR 0026). Already in the graph for `config.toml` |
| `trellis-vault` | `serde.workspace = true` | KEEP | `dates.json` is read with `deny_unknown_fields` at every level (`dates.rs`, ADR 0022 §5). Already in the graph |
| `trellis-vault` | `unicode-normalization = "0.1.25"` | KEEP | NFKD before stripping combining marks, matching Apex's `slugify`. Without it `Café` slugs to `caf` here and `cafe` in Apex, and an alias resolves in one tool and not the other (`refs.rs`). Correct NFKD needs the Unicode decomposition tables, and hand-writing them is not a trim. Adds `tinyvec` |

**Named items no manifest line in the diff adds:**

| Item | Verdict | Where it comes from, and why |
|---|---|---|
| `ring` | KEEP | Transitive. `rustls`'s `ring` feature and `rustls-webpki` pull it, and only under `hosted`. It is the crypto provider chosen over `aws-lc-rs` (see `rustls`). It builds with the C compiler already needed on MSVC, not with cmake or nasm. It brings `untrusted` and a second `getrandom` (0.2) |
| `tracing-serde` | KEEP | Not added by the inherit PR. The `json` feature on `tracing-subscriber` is a context line in the inherit diff. It came to `v0.1-resolver` in `1d3d06f` (2026-09-27), after the predecessor, for `--log-json` (ADR 0020 §7). One crate, and `serde` was already in the graph |
| `CDLA-Permissive-2.0` (licence) | KEEP | Only `webpki-roots@1.0.9` carries it (`cargo deny list -l crate`). The licence covers Mozilla's root certificates, which are data. `deny.toml` allows it with the comment that it "permits use and redistribution with no copyleft condition". It appears only in the `hosted` build |

## TRIM measurement (#154)

A detached worktree at `969c059` was used, with `jiff` changed to
`default-features = false, features = ["std", "tz-system",
"tzdb-bundle-platform", "tzdb-zoneinfo"]`. It was built with `cargo build
--release -p trellis-cli --target-dir <scratch>`. The manifest was then
restored and the unmodified tree built from the same path into the same target
directory. That second build is the control, because embedded source paths make
a build from a different directory a different size. Result: 10,840,064 B
trimmed vs 10,861,568 B control, **−21,504 B**. The control is 512 B larger
than the in-repo build (10,861,056 B), which is the path difference. The trial
compiled. Its tests were not run: #154 requires them.

## `cargo deny check` and `cargo audit`

- `cargo deny check` (cargo-deny 0.20.2): exit 0,
  `advisories ok, bans ok, licenses ok, sources ok`. It prints four `duplicate`
  warnings, listed above.
- `cargo audit` (cargo-audit 0.22.2): exit 0. It prints one allowed warning,
  RUSTSEC-2023-0089 (`atomic-polyfill` unmaintained, through `heapless` ←
  `postcard`). Unmaintained advisories are warnings, not failures, in
  cargo-audit. `deny.toml` ignores the same ID with a reason, and that entry
  dates from `bc705e1` (2026-09-21), before this ticket and the predecessor.

## Unverified

- **Per-crate size attribution was not produced.** `CARGO_PROFILE_RELEASE_STRIP=none
  cargo bloat --release -p trellis-cli --crates` (into a scratch target
  directory) was stopped by the host for low memory before it finished. The
  claim that the non-TLS growth is mostly first-party code rests on the
  `--no-default-features` figure and ADR 0024's `3234bee` figure, not on a
  per-crate table. What I would do next is rerun that command on an idle
  machine.
- **`tls12` size was not measured.** See the `rustls` row.
- **Only one OS and target.** Sizes are for Windows/MSVC. `jiff`'s bundled
  tzdb in particular is Windows-specific (`tzdb-bundle-platform` bundles
  nothing where a system zoneinfo exists), so its cost elsewhere differs.
- **Build times were not measured.** The ticket does not ask for them, and the
  predecessor's timing method (medians of repeated clean builds) was not
  repeated.
