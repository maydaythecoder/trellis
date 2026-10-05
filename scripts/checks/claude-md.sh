#!/usr/bin/env bash
# claude-md.sh: CLAUDE.md stays short enough to be read whole, and keeps the
# parts that must never move out of it.
#
#   scripts/checks/claude-md.sh [--self-test]
#
# CLAUDE.md is loaded into every session and every subagent. Past about 200
# lines it is followed less reliably (code.claude.com/docs/en/memory), so
# material needed only some of the time lives in `.claude/rules/` with a
# `paths:` scope or in docs/ (#388). What may not move: the ten rules that
# need an ADR to change, and the invariants the PR #1 review rounds put in
# place, because they bind every change.
#
# Exit 0: under 200 lines, all ten numbered rules present, both sections
# present. Exit 1: each failure is named.
set -uo pipefail

check_file() {
  local f=$1 bad=0 n lines
  lines=$(wc -l < "$f")
  if [ "$lines" -ge 200 ]; then echo "FAIL: $f is $lines lines; keep it under 200"; bad=1; fi
  for section in "## Rules that need an ADR to change" "## Invariants the PR #1 review rounds put in place"; do
    grep -qxF "$section" "$f" || { echo "FAIL: $f has lost the section '$section'"; bad=1; }
  done
  for n in 1 2 3 4 5 6 7 8 9 10; do
    grep -qE "^$n\. \*\*" "$f" || { echo "FAIL: $f has lost rule $n"; bad=1; }
  done
  return $bad
}

if [ "${1:-}" = "--self-test" ]; then
  tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
  fails=0
  {
    echo "## Invariants the PR #1 review rounds put in place"
    echo "## Rules that need an ADR to change"
    for n in 1 2 3 4 5 6 7 8 9 10; do echo "$n. **rule $n**"; done
  } > "$tmp/good.md"
  check_file "$tmp/good.md" >/dev/null || { echo "self-test: a good file was refused"; fails=1; }
  grep -v '^7\. ' "$tmp/good.md" > "$tmp/no7.md"
  check_file "$tmp/no7.md" >/dev/null && { echo "self-test: a missing rule passed"; fails=1; }
  { cat "$tmp/good.md"; for i in $(seq 1 200); do echo "line $i"; done; } > "$tmp/long.md"
  check_file "$tmp/long.md" >/dev/null && { echo "self-test: a 212-line file passed"; fails=1; }
  [ $fails -eq 0 ] && echo "claude-md self-test: ok"
  exit $fails
fi

root=$(git rev-parse --show-toplevel) || exit 2
if check_file "$root/CLAUDE.md"; then
  echo "OK: CLAUDE.md is $(wc -l < "$root/CLAUDE.md") lines and keeps its ten rules and invariants"
else
  exit 1
fi
