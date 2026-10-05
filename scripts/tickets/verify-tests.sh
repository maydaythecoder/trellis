#!/usr/bin/env bash
# A Verify command for "these tests exist and pass".
#
#   scripts/tickets/verify-tests.sh <package> <filter> [<min>]
#   scripts/tickets/verify-tests.sh --self-test
#
# `cargo test -p pkg filter` exits 0 when the filter matches nothing, so a
# ticket whose tests were never written would pass its own Verify. This fails
# unless at least <min> (default 1) tests ran, and all passed.
#
# `trellis-desktop` is the package of the desktop's own Cargo workspace
# (ADR 0048 section 1), which the root workspace excludes, so `-p` cannot
# reach it: it runs as `cargo test --locked --manifest-path
# desktop/Cargo.toml`, and then fails if a test left anything under desktop/
# that git sees (#286), since a build or test that dirties the tree would
# block the next run (ADR 0009).
set -euo pipefail

run() { # <package> <filter> <min>
  local pkg="$1" filter="$2" min="$3" out ran dirty
  cd "$(git rev-parse --show-toplevel)"
  if [ "$pkg" = "trellis-desktop" ]; then
    out=$(cargo test --locked --manifest-path desktop/Cargo.toml "$filter" 2>&1) ||
      { printf '%s\n' "$out" | tail -40; echo "FAIL: tests failed"; exit 1; }
  else
    out=$(cargo test -p "$pkg" "$filter" 2>&1) ||
      { printf '%s\n' "$out" | tail -40; echo "FAIL: tests failed"; exit 1; }
  fi
  ran=$(printf '%s\n' "$out" | sed -n 's/^test result: ok\. \([0-9]*\) passed.*/\1/p' | awk '{s+=$1} END {print s+0}')
  printf '%s\n' "$out" | grep -E '^test .* \.\.\. ' | tail -40 || true
  if [ "$ran" -lt "$min" ]; then
    echo "FAIL: $ran test(s) matched '$filter' in $pkg; need at least $min"
    exit 1
  fi
  if [ "$pkg" = "trellis-desktop" ]; then
    dirty=$(git status --porcelain -- desktop)
    if [ -n "$dirty" ]; then
      printf '%s\n' "$dirty"
      echo "FAIL: the desktop tests left these paths under desktop/"
      exit 1
    fi
  fi
  echo "OK: $ran test(s) matched '$filter' in $pkg and passed"
}

self_test() {
  local self fails=0
  # Cases are expected to fail; their exit codes are read, not obeyed.
  set +e
  self="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
  # Global, so the EXIT trap still sees it after this function returns.
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null
  # A scratch target directory, so the fixture's builds stay out of the
  # fixture tree and out of the repository's own target/.
  export CARGO_TARGET_DIR="$tmp/target"
  local repo="$tmp/repo"
  local g=(git -C "$repo" -c user.name=t -c user.email=t@example.invalid -c commit.gpgsign=false)
  mkdir -p "$repo/rootcrate/src" "$repo/desktop/src"
  cat > "$repo/Cargo.toml" <<'EOF'
[workspace]
members = ["rootcrate"]
exclude = ["desktop"]
resolver = "2"
EOF
  cat > "$repo/rootcrate/Cargo.toml" <<'EOF'
[package]
name = "rootcrate"
version = "0.0.0"
edition = "2021"
publish = false
EOF
  printf '%s\n' '#[test]' 'fn root_works() {}' > "$repo/rootcrate/src/lib.rs"
  cat > "$repo/desktop/Cargo.toml" <<'EOF'
[package]
name = "trellis-desktop"
version = "0.0.0"
edition = "2021"
publish = false

[workspace]
EOF
  cat > "$repo/desktop/src/lib.rs" <<'EOF'
#[test]
fn desktop_works() {}

#[test]
fn desktop_dirties() {
    if std::env::var_os("DIRTY").is_some() {
        std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/left-behind.txt"), "x").unwrap();
    }
}
EOF
  printf '%s\n' 'target/' > "$repo/.gitignore"
  "${g[@]}" init -q
  (cd "$repo" && cargo generate-lockfile -q && cargo generate-lockfile -q --manifest-path desktop/Cargo.toml)
  "${g[@]}" add -A && "${g[@]}" commit -qm fixture

  expect() { # <want-exit> <name> [env...] -- <args...>
    local want="$1" name="$2" got
    shift 2
    local envs=()
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    # The subshell clears the trap: it must not remove the fixture when it
    # exits.
    (trap - EXIT; cd "$repo" && env "${envs[@]}" bash "$self" "$@") > "$tmp/out" 2>&1
    got=$?
    if [ "$got" = "$want" ]; then
      echo "ok   $name (exit $got)"
    else
      echo "FAIL $name: want exit $want, got $got"
      sed 's/^/     /' "$tmp/out" | tail -15
      fails=$((fails + 1))
    fi
  }
  expect 0 "the desktop mode passes with min 1" -- trellis-desktop desktop_works 1
  expect 1 "the desktop mode fails for a filter matching nothing" -- trellis-desktop no_such_test 1
  expect 1 "the desktop mode fails when a test writes under desktop/" DIRTY=1 -- trellis-desktop desktop_dirties 1
  rm -f "$repo/desktop/left-behind.txt"
  expect 0 "the root mode passes for the root crate" -- rootcrate root_works 1

  [ "$fails" -eq 0 ] && { echo "OK: self-test"; exit 0; }
  echo "FAIL: $fails self-test case(s)"
  exit 1
}

if [ "${1:-}" = "--self-test" ]; then
  self_test
fi
pkg="${1:?usage: verify-tests.sh <package> <filter> [<min>] | --self-test}"
filter="${2:?usage: verify-tests.sh <package> <filter> [<min>] | --self-test}"
run "$pkg" "$filter" "${3:-1}"
