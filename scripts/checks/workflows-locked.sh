#!/usr/bin/env bash
# workflows-locked.sh: every cargo command in .github/workflows/ that
# resolves dependencies passes --locked (or --frozen).
#
#   scripts/checks/workflows-locked.sh [--self-test]
#
# Without it, a change that edits a Cargo.toml requirement and not the lock
# is built and tested on versions cargo resolved on the runner, which no
# reviewer saw, and CI passes on them (audit 2026-09-30, T1; #215). Cargo.lock
# is the pin (CLAUDE.md), and --locked is what makes a runner honour it.
#
# Checked: build, check, clippy, test, bench, run, doc, install, deny, each
# on the line that names it, so a multi-line `run:` must keep --locked on the
# cargo line itself (for `cargo run`, before its `--`). Not checked: `cargo
# fmt` and `cargo audit` (neither resolves; neither takes --locked), comment
# lines, and cargo commands inside scripts a workflow calls.
#
# Zero cargo commands found also fails: a scan that matched nothing proves
# nothing, and a renamed directory or a broken pattern would otherwise pass.
#
# Exit 0: holds. Exit 1: a cargo command without --locked, each printed as
# file:line, or no cargo command found, or the self-test did not hold.
# Exit 2: wrong arguments, or not in a git repository.
set -uo pipefail

# One awk program for the check and its self-test, so the self-test proves
# the scan that actually runs. POSIX awk only (no \b, no {n}): the Linux
# runner's awk is mawk. Prints `BAD <file>:<line>: <text>` per violation and
# `COUNT <n>` last.
scan() {
  awk '
    function check(rest,    seg, m, tail, nxt) {
      # One segment per cargo command, so `cargo build --locked && cargo
      # test` is two commands and the second is judged on its own.
      while (match(rest, /(^|[^A-Za-z0-9_-])cargo( +\+[^ ]+)? +(build|check|clippy|test|bench|run|doc|install|deny)([^A-Za-z0-9_-]|$)/)) {
        m = substr(rest, RSTART, RLENGTH)
        tail = substr(rest, RSTART + RLENGTH)
        nxt = match(tail, /(^|[^A-Za-z0-9_-])cargo +/)
        seg = m (nxt ? substr(tail, 1, nxt - 1) : tail)
        # After a bare `--`, arguments belong to the program, not to cargo.
        if (match(seg, /[ \t]--([ \t]|$)/)) seg = substr(seg, 1, RSTART)
        count++
        if (seg !~ /(^|[ \t])--(locked|frozen)([ \t=]|$)/) bad = 1
        rest = nxt ? substr(tail, nxt) : ""
      }
    }
    {
      sub(/\r$/, "")
      if ($0 ~ /^[ \t]*#/) next
      bad = 0
      check($0)
      if (bad) print "BAD " FILENAME ":" FNR ": " $0
    }
    END { print "COUNT " count + 0 }
  ' "$@"
}

usage='usage: workflows-locked.sh [--self-test]'
if [ "${1:-}" = "--self-test" ]; then
  [ $# -eq 1 ] || { echo "$usage"; exit 2; }
  tmp=$(mktemp -d) || { echo "FAIL: mktemp"; exit 1; }
  trap 'rm -rf "$tmp"' EXIT
  fail=0
  # Each must be flagged.
  for bad in \
    '        run: cargo test --workspace' \
    '          cargo build --release -p trellis-cli' \
    '        run: cargo clippy --workspace --all-targets' \
    '        run: cargo run --release -p x --example y -- --locked' \
    '        run: cargo build --locked -p a && cargo test -p a' \
    '        run: cargo +stable build -p a' \
    '        run: cargo deny check' \
    '        run: cargo install cargo-deny --version 1'; do
    printf '%s\n' "$bad" > "$tmp/w.yml"
    out=$(scan "$tmp/w.yml")
    printf '%s\n' "$out" | grep -q '^BAD ' \
      || { echo "FAIL: self-test: not flagged: $bad"; fail=1; }
  done
  # None may be flagged, and each still counts as a cargo command seen
  # (except the ones that are not cargo commands this check covers).
  for ok in \
    '        run: cargo test --locked --workspace' \
    '        run: cargo run --locked --release -p x --example y -- .bench' \
    '        run: cargo build --frozen -p a' \
    '        run: cargo build --locked -p a && cargo test --locked -p a' \
    '        run: cargo deny --locked check'; do
    printf '%s\n' "$ok" > "$tmp/w.yml"
    out=$(scan "$tmp/w.yml")
    if printf '%s\n' "$out" | grep -q '^BAD '; then
      echo "FAIL: self-test: flagged a locked command: $ok"; fail=1
    fi
    printf '%s\n' "$out" | grep -q '^COUNT 0$' \
      && { echo "FAIL: self-test: did not see the cargo command in: $ok"; fail=1; }
  done
  for skip in \
    '        run: cargo fmt --all -- --check' \
    '        run: cargo audit --file fuzz/Cargo.lock' \
    '      # without it cargo test resolves new versions' \
    '          cargo-audit does not read deny.toml'; do
    printf '%s\n' "$skip" > "$tmp/w.yml"
    out=$(scan "$tmp/w.yml")
    printf '%s\n' "$out" | grep -q '^COUNT 0$' \
      || { echo "FAIL: self-test: counted as a cargo command: $skip"; fail=1; }
  done
  [ "$fail" -eq 0 ] || exit 1
  echo "OK: self-test (flags 8 unlocked forms, passes 5 locked ones, ignores fmt, audit and comments)"
  exit 0
fi
[ $# -eq 0 ] || { echo "$usage"; exit 2; }

root=$(git rev-parse --show-toplevel 2>/dev/null) || { echo "FAIL: not in a git repository"; exit 2; }
cd "$root" || exit 2
shopt -s nullglob
files=(.github/workflows/*.yml .github/workflows/*.yaml)
[ ${#files[@]} -gt 0 ] || { echo "FAIL: no workflow files under .github/workflows/"; exit 1; }
out=$(scan "${files[@]}") || { echo "FAIL: awk failed"; exit 1; }
n=$(printf '%s\n' "$out" | sed -n 's/^COUNT //p')
bad=$(printf '%s\n' "$out" | sed -n 's/^BAD //p')
if [ -n "$bad" ]; then
  echo "FAIL: cargo commands in workflows without --locked (Cargo.lock is the pin; add --locked on the cargo line):"
  printf '%s\n' "$bad" | sed 's/^/  /'
  exit 1
fi
if [ "${n:-0}" -eq 0 ]; then
  echo "FAIL: no cargo command found in .github/workflows/; a scan that sees nothing proves nothing"
  exit 1
fi
echo "OK: all $n cargo build/check/clippy/test/bench/run/doc/install/deny commands in .github/workflows/ pass --locked"
