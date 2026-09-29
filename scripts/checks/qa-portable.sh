#!/usr/bin/env bash
# qa-portable.sh: the guard suite runs on a machine that is not the owner's.
#
#   scripts/checks/qa-portable.sh
#
# Two things made qa/ run on one machine only: absolute paths to that
# machine's home directory (the default `--repo`, and every `scripted:` model
# spec in behaviour.json), and a fixture repository that existed only there.
# This fails if either comes back.
#
# qa/REPORT-*.md, qa/BASELINE.md and qa/logs/ are excluded: they are records
# of a run on a particular machine, and the paths in them are part of what
# was recorded.
#
# Exit 0: holds. Exit 1: does not, with the offending lines or the bundle
# failure printed.
set -uo pipefail

ROOT="$(git rev-parse --show-toplevel)" || { echo "FAIL: not in a git repository"; exit 1; }
cd "$ROOT"

BUNDLE="qa/fixtures/trellis-demo.bundle"
# The fixture commit the behaviour and guard expectations were written
# against (behaviour.json `_baseline_commit`).
COMMIT="31daaaf"
fails=0

hits=$(git grep -n -I -E '[A-Za-z]:[\\/]+Users|/home/' -- qa \
  ':!qa/REPORT-*.md' ':!qa/BASELINE.md' ':!qa/logs')
code=$?
# git grep: 0 = matches, 1 = none, anything else = it could not search, which
# must not read as "none found".
case $code in
  0)
    echo "FAIL: machine-specific absolute paths under qa/:"
    printf '%s\n' "$hits"
    fails=1
    ;;
  1) ;;
  *)
    echo "FAIL: git grep exited $code; qa/ was not searched"
    fails=1
    ;;
esac

if [ ! -f "$BUNDLE" ]; then
  echo "FAIL: $BUNDLE is missing; the guard suite has no fixture"
  fails=1
else
  if ! out=$(git bundle verify "$BUNDLE" 2>&1); then
    echo "FAIL: git bundle verify $BUNDLE:"
    printf '%s\n' "$out"
    fails=1
  fi
  # list-heads prints full object names; the fixture is named by its prefix.
  if ! git bundle list-heads "$BUNDLE" 2>/dev/null | grep -q "^$COMMIT"; then
    echo "FAIL: $BUNDLE does not list $COMMIT as a head"
    fails=1
  fi
fi

[ $fails -eq 0 ] || exit 1
echo "OK: no machine-specific paths under qa/; $BUNDLE verifies and lists $COMMIT"
