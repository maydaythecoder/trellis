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
# Go (ADR 0052, #504): every `go build`, `go test`, `go vet`, `go run` and
# `go install` passes `-mod=readonly` on its line, or runs in a job (or a
# workflow) whose `env:` sets `GOFLAGS` to include `-mod=readonly`; go.sum
# is the pin, and without readonly a build may rewrite go.mod and go.sum on
# the runner. A `go install` also names an exact `@vX.Y.Z` for every
# package, never `@latest` or no version, so a tool is the release a
# reviewer saw. Zero go commands is not a failure: CI ran no Go before
# tui/ existed (#505).
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

# The Go scan, per file. The file is read twice (pass 1 finds which jobs,
# and whether the whole workflow, set GOFLAGS with -mod=readonly in an
# `env:` block; pass 2 judges each go command). The indentation is this
# repository's: jobs at 2 spaces, a job's keys at 4, its env entries at 6.
# Prints `BAD <file>:<line>: <why>: <text>` per violation and `GOCOUNT <n>`.
scan_go_file() {
  awk '
    FNR == 1 { pass++; injobs = 0; job = ""; inwfenv = 0; injobenv = 0 }
    {
      line = $0; sub(/\r$/, "", line)
      if (line ~ /^[ \t]*#/) next
      if (line ~ /^[^ \t]/) { inwfenv = 0; injobs = 0 }
      if (line ~ /^env:[ \t]*$/) { inwfenv = 1; next }
      if (line ~ /^jobs:[ \t]*$/) { injobs = 1; next }
      if (injobs && line ~ /^  [A-Za-z0-9_-]+:/) { job = line; sub(/^  /, "", job); sub(/:.*/, "", job); injobenv = 0 }
      if (injobs && line ~ /^    [^ ]/) injobenv = (line ~ /^    env:[ \t]*$/)
      if (pass == 1) {
        if (inwfenv && line ~ /^  GOFLAGS:.*-mod=readonly/) wfok = 1
        if (injobenv && line ~ /^      GOFLAGS:.*-mod=readonly/) jobok[job] = 1
        next
      }
      rest = line
      while (match(rest, /(^|[^A-Za-z0-9_.\/-])go +(build|test|vet|run|install)([^A-Za-z0-9_-]|$)/)) {
        m = substr(rest, RSTART, RLENGTH)
        tail = substr(rest, RSTART + RLENGTH)
        nxt = match(tail, /(^|[^A-Za-z0-9_.\/-])go +/)
        seg = m (nxt ? substr(tail, 1, nxt - 1) : tail)
        # A shell separator ends the command.
        if (match(seg, /&&|\|\||;|\|/)) seg = substr(seg, 1, RSTART - 1)
        count++
        why = ""
        if (seg !~ /(^|[ \t])-mod=readonly([ \t]|$)/ && !wfok && !jobok[job]) why = "no -mod=readonly (on the line, or GOFLAGS in the job or workflow env)"
        if (seg ~ /go +install/) {
          n = split(seg, w, /[ \t]+/)
          seen = 0; pkgs = 0
          for (i = 1; i <= n; i++) {
            if (!seen) { if (w[i] == "install") seen = 1; continue }
            if (w[i] == "" || w[i] ~ /^-/) continue
            pkgs++
            if (w[i] !~ /@v[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$/) why = (why == "" ? "" : why "; ") "go install of " w[i] " names no exact @vX.Y.Z"
          }
          if (!pkgs) why = (why == "" ? "" : why "; ") "go install names no package@vX.Y.Z"
        }
        if (why != "") print "BAD " FILENAME ":" FNR ": " why ": " line
        rest = nxt ? substr(tail, nxt) : ""
      }
    }
    END { print "GOCOUNT " count + 0 }
  ' "$1" "$1"
}

# Every file through scan_go_file; one GOCOUNT total.
scan_go() {
  local f total=0 out c
  for f in "$@"; do
    out=$(scan_go_file "$f") || return 1
    printf '%s\n' "$out" | grep '^BAD ' || true
    c=$(printf '%s\n' "$out" | sed -n 's/^GOCOUNT //p')
    total=$((total + ${c:-0}))
  done
  echo "GOCOUNT $total"
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
  # Go: each refused form must be flagged.
  for bad in \
    '        run: go build ./...' \
    '        run: go test ./...' \
    '        run: go vet ./...' \
    '        run: go run ./cmd/trellis-tui --version' \
    '        run: go install golang.org/x/vuln/cmd/govulncheck@v1.8.0' \
    '        run: go install -mod=readonly golang.org/x/vuln/cmd/govulncheck@latest' \
    '        run: go install -mod=readonly golang.org/x/vuln/cmd/govulncheck' \
    '        run: go build -mod=readonly ./... && go test ./...'; do
    printf '%s\n' "$bad" > "$tmp/w.yml"
    out=$(scan_go "$tmp/w.yml")
    printf '%s\n' "$out" | grep -q '^BAD ' \
      || { echo "FAIL: self-test: go: not flagged: $bad"; fail=1; }
  done
  # Go: each accepted form passes and is counted.
  for ok in \
    '        run: go build -mod=readonly ./...' \
    '        run: go test -mod=readonly ./...' \
    '        run: go vet -mod=readonly ./...' \
    '        run: go run -mod=readonly ./cmd/trellis-tui --version' \
    '        run: go install -mod=readonly golang.org/x/vuln/cmd/govulncheck@v1.8.0'; do
    printf '%s\n' "$ok" > "$tmp/w.yml"
    out=$(scan_go "$tmp/w.yml")
    if printf '%s\n' "$out" | grep -q '^BAD '; then
      echo "FAIL: self-test: go: flagged an accepted command: $ok"; fail=1
    fi
    printf '%s\n' "$out" | grep -q '^GOCOUNT 0$' \
      && { echo "FAIL: self-test: go: did not see the go command in: $ok"; fail=1; }
  done
  # Go: a job-level GOFLAGS covers that job only; a step-level one covers
  # nothing; a workflow-level one covers every job.
  printf '%s\n' 'jobs:' '  a:' '    env:' '      GOFLAGS: -mod=readonly' '    steps:' \
    '      - run: go test ./...' '  b:' '    steps:' '      - run: go vet ./...' > "$tmp/w.yml"
  out=$(scan_go "$tmp/w.yml")
  printf '%s\n' "$out" | grep -q ':6: ' && { echo "FAIL: self-test: go: job-level GOFLAGS not honoured"; fail=1; }
  printf '%s\n' "$out" | grep -q ':9: ' || { echo "FAIL: self-test: go: job-level GOFLAGS leaked into another job"; fail=1; }
  printf '%s\n' 'jobs:' '  a:' '    steps:' '      - run: go test ./...' '        env:' \
    '          GOFLAGS: -mod=readonly' > "$tmp/w.yml"
  out=$(scan_go "$tmp/w.yml")
  printf '%s\n' "$out" | grep -q '^BAD ' || { echo "FAIL: self-test: go: a step-level GOFLAGS was taken as the job's"; fail=1; }
  printf '%s\n' 'env:' '  GOFLAGS: "-mod=readonly"' 'jobs:' '  a:' '    steps:' '      - run: go test ./...' > "$tmp/w.yml"
  out=$(scan_go "$tmp/w.yml")
  printf '%s\n' "$out" | grep -q '^BAD ' && { echo "FAIL: self-test: go: workflow-level GOFLAGS not honoured"; fail=1; }
  for skip in \
    '        run: scripts/checks/go-test.sh TestRPC 3' \
    '      # go test without readonly would rewrite go.sum' \
    '        run: go version' \
    '        run: go mod verify'; do
    printf '%s\n' "$skip" > "$tmp/w.yml"
    out=$(scan_go "$tmp/w.yml")
    printf '%s\n' "$out" | grep -q '^GOCOUNT 0$' \
      || { echo "FAIL: self-test: go: counted as a go build/test/vet/run/install: $skip"; fail=1; }
  done
  [ "$fail" -eq 0 ] || exit 1
  echo "OK: self-test (flags 8 unlocked cargo forms, passes 5 locked ones, ignores fmt, audit and comments; flags 8 go forms without -mod=readonly or an exact install version, passes 5, honours job- and workflow-level GOFLAGS only)"
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
gout=$(scan_go "${files[@]}") || { echo "FAIL: awk failed (go scan)"; exit 1; }
gn=$(printf '%s\n' "$gout" | sed -n 's/^GOCOUNT //p')
gbad=$(printf '%s\n' "$gout" | sed -n 's/^BAD //p')
if [ -n "$gbad" ]; then
  echo "FAIL: go commands in workflows without -mod=readonly, or a go install without an exact @vX.Y.Z (go.sum is the pin):"
  printf '%s\n' "$gbad" | sed 's/^/  /'
  exit 1
fi
echo "OK: all $n cargo build/check/clippy/test/bench/run/doc/install/deny commands in .github/workflows/ pass --locked; all ${gn:-0} go build/test/vet/run/install commands are -mod=readonly and every go install names an exact version"
