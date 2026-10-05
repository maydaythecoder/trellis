#!/usr/bin/env bash
# touches.sh: every named file differs between the gates' base and HEAD.
#
#   scripts/checks/touches.sh <file>...
#   scripts/checks/touches.sh --self-test
#
# A Verify line for a ticket whose work is a list of edits to named files,
# such as a line review's findings (#390): the ticket is not done while one
# of its files is untouched. It proves an edit happened, not that the edit is
# right; the gates and the review of the diff answer that.
#
# The base is the gates' (scripts/gates/lib.sh) unless GATE_BASE is set.
# Exit 0: each file changed. Exit 1: the unchanged ones are named. Exit 2:
# no files given, or not in a git repository.
set -uo pipefail

check() {
  local base=$1; shift
  local bad=0 f
  for f in "$@"; do
    if git diff --quiet "$base"...HEAD -- "$f"; then
      echo "FAIL: $f is unchanged since $base"; bad=1
    fi
  done
  return $bad
}

if [ "${1:-}" = "--self-test" ]; then
  tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
  fails=0
  (
    cd "$tmp" && git init -q && git config core.autocrlf false && git config user.email t@t && git config user.name t &&
    printf 'a\n' > a && printf 'b\n' > b && git add a b && git commit -qm base &&
    git branch base && printf 'a2\n' > a && git commit -qam change
  ) || { echo "self-test: cannot build the fixture"; exit 1; }
  (cd "$tmp" && check base a >/dev/null) || { echo "self-test: a changed file was refused"; fails=1; }
  (cd "$tmp" && check base a b >/dev/null) && { echo "self-test: an unchanged file passed"; fails=1; }
  [ $fails -eq 0 ] && echo "touches self-test: ok"
  exit $fails
fi

[ $# -gt 0 ] || { echo "usage: touches.sh <file>... | --self-test" >&2; exit 2; }
git rev-parse --show-toplevel >/dev/null 2>&1 || exit 2
if [ -z "${GATE_BASE:-}" ]; then
  # shellcheck source=../gates/lib.sh
  source "$(git rev-parse --show-toplevel)/scripts/gates/lib.sh" >/dev/null
fi
check "$GATE_BASE" "$@" && echo "OK: all $# file(s) changed since $GATE_BASE"
