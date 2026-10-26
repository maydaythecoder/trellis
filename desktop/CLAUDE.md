# desktop/CLAUDE.md

Rules for work under `desktop/`, the Tauri desktop (ADR 0048). They bind
only here, so they live here and stay out of every other session's context.
Each names the file, script or test that already holds it; a rule with no
holder is a wish, so none is written here without one.

## The workspace is its own

- `desktop/` is its own Cargo workspace with its own `desktop/Cargo.lock`.
  Every cargo command on it takes `--manifest-path desktop/Cargo.toml` and
  `--locked`: build, test, clippy and run. The root workspace excludes it,
  so the root `Cargo.lock` never sees Tauri (ADR 0048 §1; held by
  `scripts/checks/desktop-guards.sh`, whose `desktop-lock-current` rule
  refuses a stale `desktop/Cargo.lock` and whose `root-lock` rule refuses
  Tauri in the root lock, and by the `desktop` job in
  `.github/workflows/ci.yml`, which builds, lints and tests it on Windows and
  Linux with `--locked`).
- It depends on no `trellis-*` crate and reaches trellis only by spawning,
  from `src/child.rs` alone: `trellis --root <repo> rpc`, `trellis --root
  <repo> --json log --limit <n>`, `trellis --version`, and `trellis [--root
  <repo>] --json overview` (ADR 0048 §1 and §2, §2 amended 2026-10-07 for
  the overview; held by `desktop-guards.sh`'s `desktop-trellis-dep` and
  `src-command` rules and by `child_argument_lists_are_fixed` in
  `src/child.rs`).
- It never reads trellis's stderr as data. What it shows comes from JSON on
  stdout; stderr is shown in the capped "trellis messages" pane as human
  text (ADR 0048 §2, amended by ADR 0052; held by
  `stdout_lines_become_frames_and_a_bad_line_stays_text` in
  `src/commands.rs` and `stderr_lines_are_kept_apart_and_capped` in
  `src/state.rs`).

## Children, environment and keys

- It never kills a trellis child. `quit` comes only from the core: closing
  the window or opening another repository sends it and waits, so a run in
  flight rolls back and writes its trail (ADR 0025 §5, ADR 0048 §3 and §9;
  held by the `src-kill` rule in `desktop-guards.sh`,
  `rpc_send_refuses_a_type_rpc_does_not_list` and
  `shutdown_sends_quit_and_waits_for_the_child_to_exit` in
  `src/commands.rs`).
- It adds nothing to a child's environment and reads no environment
  variable; only test code sets `TRELLIS_CONFIG_DIR`, after a file's first
  `#[cfg(test)]` line, so a test never reads the developer's own config
  (ADR 0010, ADR 0024 §3, ADR 0048 §3; held by `desktop-guards.sh`'s
  `src-envs`, `src-env-clear`, `src-env-var`, `src-api-key`, `src-key-env`
  and `src-env` rules).
- It forwards a hosted spec (`openai-chat:` or `anthropic:`) as `set_model`
  or as a prompt's `model` only when it is byte-equal to `configured.user`,
  the user's own config's spec from the child's answer to the core's
  `desktop-models` question, because rpc counts a model the client names as
  the user's choice (ADR 0024 §3, ADR 0048 §4; held by the `model_guard_`
  tests in `src/commands.rs`).
- No `prompt` is forwarded until the child has answered a `set_model` with
  success, and the page serves prompt input `disabled` until then (ADR 0048
  §5, amended 2026-10-07; held by the `model_gate_` tests in
  `src/commands.rs`, `src/state.rs` and `src/lockdown.rs`).

## The page

- Every trellis-, model-, trail- or repository-supplied string reaches the
  DOM through `textContent`, `createElement` or `createElementNS` only: no
  `innerHTML`, `outerHTML`, `insertAdjacentHTML`, `document.write`, `eval`,
  `Function`, inline handler, `style` attribute or `.style` (ADR 0007, S10,
  ADR 0048 §5; held by `desktop-guards.sh`'s `ui-*` rules and
  `page_has_no_style_attribute` in `src/lockdown.rs`).
- The page has no key field and no endpoint field (ADR 0010, ADR 0024 §3;
  held by `page_has_no_key_or_endpoint_field` in `src/lockdown.rs`).
- The page draws the design system's components: `ui/ds.css` is
  `docs/design/system/components/bundle.css` byte for byte below its
  header, and `ui/ds.js` alone writes a component's markup (DESIGN.md §2.1,
  #909; held by `ds_css_is_the_design_systems_bundle` and
  `ds_components_draw_the_bundles_markup` in `src/lockdown.rs`). A change to
  the system reaches the page by copying the file again.
- Nothing is added without amending ADR 0048: no Tauri plugin (the
  `desktop-plugin` rule in `desktop-guards.sh`); no capability permission,
  CSP source or navigation target (`main_capability_grants_only_listen_unlisten_and_the_apps_commands`,
  `csp_has_no_unsafe_source_and_no_network_origin` and
  `navigation_stays_on_the_app_origin` in `src/lockdown.rs`). The page is
  static HTML, CSS and ES modules with no build step and no npm package
  (ADR 0048 §5; `no_dev_url_and_no_build_hooks` in `src/lockdown.rs`).

## What it shows

- Radius is three counts and never a ratio, and no view key names a ratio,
  a percentage or a total (CLAUDE.md rule 3, ADR 0048 §8; held by
  `radius_holds_three_counts_and_no_ratio` in `src/state.rs`).
- A check row shows the verdict trellis reported, and nothing in the
  desktop computes one (ADR 0004, ADR 0048 §8; held by
  `check_row_comes_only_from_a_some_verdict` in `src/state.rs`).
- A figure trellis did not report is `null` in the view and is said to be
  unreported on the page, never drawn as 0 (ADR 0048 §8; held by
  `budget_reports_prompt_bytes_not_tokens` and
  `session_tokens_unknown_is_null_never_zero` in `src/state.rs`).

## Auditing its dependencies

- `desktop/deny.toml` holds every line of the root `deny.toml`, in order,
  and between its `# desktop-only: begin` and `# desktop-only: end` markers
  only the entries ADR 0048 §7 allows: the two supported targets, the
  per-crate licence exceptions it names, an `ignore` for each `unmaintained`
  advisory, and RUSTSEC-2024-0429. No `audit.toml` exists, so a
  `vulnerability` advisory is never ignored, and G5 fails on one until a
  lock update, in its own commit, clears it (ADR 0048 §7; held by
  `desktop-guards.sh`'s `deny-copy` and `audit-toml` rules, and by G5's
  `cargo deny` and `cargo audit` on `desktop/Cargo.lock`).
- A new crate dependency needs an ADR, here as at the root.

## Commands

```
cargo build -p trellis-cli                                   # the trellis the tests spawn
cargo build  --locked --manifest-path desktop/Cargo.toml
cargo test   --locked --manifest-path desktop/Cargo.toml
cargo clippy --locked --manifest-path desktop/Cargo.toml --all-targets
cargo fmt    --manifest-path desktop/Cargo.toml --check
scripts/checks/desktop-guards.sh                             # and --self-test
scripts/tickets/verify-tests.sh trellis-desktop <filter> <min>
```

`verify-tests.sh` for `trellis-desktop` fails while `desktop/` has changes
git sees, since a test that dirties the tree would block the next run (ADR
0009): run it after committing.
