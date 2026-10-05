#!/usr/bin/env bash
# ci-timeouts.sh: every job in .github/workflows/ names its own
# timeout-minutes.
#
#   scripts/checks/ci-timeouts.sh [--self-test]
#
# Without one, a job on an offline or stuck runner waits GitHub's default of
# 360 minutes before it is failed, on a plan whose minutes have already run
# out once (#156). The DevOps audit of 2026-10-04 found no job with a
# timeout (#381).
#
# Jobs are the two-space-indented keys under `jobs:`, as
# scripts/checks/ci-jobs-unchanged.sh reads them. A job holds when a
# `timeout-minutes:` key sits at job level (four spaces) inside its block; a
# step's timeout-minutes (deeper) does not count, since it bounds one step
# and leaves the job's other steps and its queueing at the default.
#
# Zero jobs found also fails: a scan that matched nothing proves nothing,
# and a renamed directory or a broken pattern would otherwise pass.
#
# Exit 0: holds. Exit 1: a job without timeout-minutes, each printed as
# file: job, or no job found, or the self-test did not hold. Exit 2: wrong
# arguments, or not in a git repository.
set -uo pipefail

# One awk program for the check and its self-test, so the self-test proves
# the scan that actually runs. POSIX awk only: the Linux runner's awk is
# mawk. Prints `BAD <file>: <job>` per job without a timeout and `COUNT <n>`
# last.
scan() {
  awk '
    function close_job() {
      if (job != "" && !has) print "BAD " file ": " job
      job = ""; has = 0
    }
    FNR == 1 { close_job(); injobs = 0; file = FILENAME }
    {
      sub(/\r$/, "")
      if ($0 ~ /^jobs:[ \t]*$/) { injobs = 1; next }
      if (!injobs) next
      # A top-level key ends the jobs block.
      if ($0 ~ /^[^ #]/) { close_job(); injobs = 0; next }
      if ($0 ~ /^  [A-Za-z0-9_-]+:[ \t]*$/) {
        close_job()
        job = $0; sub(/:.*/, "", job); sub(/^  /, "", job)
        count++
        next
      }
      if (job != "" && $0 ~ /^    timeout-minutes:[ \t]*[^ \t]/) has = 1
    }
    END { close_job(); print "COUNT " count + 0 }
  ' "$@"
}

usage='usage: ci-timeouts.sh [--self-test]'
if [ "${1:-}" = "--self-test" ]; then
  [ $# -eq 1 ] || { echo "$usage"; exit 2; }
  tmp=$(mktemp -d) || { echo "FAIL: mktemp"; exit 1; }
  trap 'rm -rf "$tmp"' EXIT
  fail=0
  cat > "$tmp/a.yml" <<'EOF'
name: A
on: push
jobs:
  timed:
    runs-on: ubuntu-latest
    timeout-minutes: 20
    steps:
      - run: true
  untimed:
    runs-on: ubuntu-latest
    steps:
      - run: true
  step-only:
    runs-on: ubuntu-latest
    steps:
      - run: true
        timeout-minutes: 5
  # a comment between jobs
  last-timed:
    timeout-minutes: 45
    runs-on: ubuntu-latest
env:
  timeout-minutes: 1
EOF
  # CRLF line endings, as a Windows checkout may leave them.
  printf 'on: push\r\njobs:\r\n  crlf-timed:\r\n    timeout-minutes: 10\r\n  crlf-untimed:\r\n    runs-on: x\r\n' > "$tmp/b.yml"
  printf 'name: no jobs here\non: push\n' > "$tmp/c.yml"
  out=$(scan "$tmp/a.yml" "$tmp/b.yml")
  for want in "BAD $tmp/a.yml: untimed" "BAD $tmp/a.yml: step-only" "BAD $tmp/b.yml: crlf-untimed" "COUNT 6"; do
    printf '%s\n' "$out" | grep -qxF "$want" \
      || { echo "FAIL: self-test: expected line missing: $want"; fail=1; }
  done
  for job in timed last-timed crlf-timed; do
    printf '%s\n' "$out" | grep -q "^BAD .*: $job\$" \
      && { echo "FAIL: self-test: flagged a job with timeout-minutes: $job"; fail=1; }
  done
  [ "$(printf '%s\n' "$out" | grep -c '^BAD ')" -eq 3 ] \
    || { echo "FAIL: self-test: expected exactly 3 flagged jobs, got:"; printf '%s\n' "$out"; fail=1; }
  printf '%s\n' "$(scan "$tmp/c.yml")" | grep -qx 'COUNT 0' \
    || { echo "FAIL: self-test: counted a job in a file with none"; fail=1; }
  [ "$fail" -eq 0 ] || exit 1
  echo "OK: self-test (flags a job with no timeout and one with only a step timeout, LF and CRLF; passes 3 timed jobs; counts none where there are none)"
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
  echo "FAIL: jobs without timeout-minutes (a stuck runner would wait the default 360 minutes; add it at job level):"
  printf '%s\n' "$bad" | sed 's/^/  /'
  exit 1
fi
if [ "${n:-0}" -eq 0 ]; then
  echo "FAIL: no job found in .github/workflows/; a scan that sees nothing proves nothing"
  exit 1
fi
echo "OK: all $n jobs in .github/workflows/ set timeout-minutes"
