#!/usr/bin/env bash
# exec-bit.sh: each named file is stored executable.
#
#   scripts/checks/exec-bit.sh <file>...
#   scripts/checks/exec-bit.sh --self-test
#
# A script that is invoked directly needs mode 100755 in git, or a Linux
# checkout fails with "Permission denied" while a Windows one, which has no
# execute bit on disk (core.fileMode=false), never notices. So this reads
# the index (`git ls-files -s`), not the disk: `git add --chmod=+x` or
# `git update-index --chmod=+x` is what sets it. Run from the repository.
#
# Exit 0: every file is 100755 in the index. Exit 1: some are not, and each
# is named with its mode. Exit 2: wrong arguments, a file that is not
# tracked, or not in a git repository.
set -uo pipefail

usage='usage: exec-bit.sh <file>... | --self-test'
# Git Bash rewrites a path-looking argument otherwise.
export MSYS_NO_PATHCONV=1

if [ "${1:-}" = "--self-test" ]; then
  [ $# -eq 1 ] || { echo "$usage"; exit 2; }
  self="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
  tmp=$(mktemp -d) || { echo "FAIL: mktemp failed"; exit 2; }
  trap 'rm -rf "$tmp"' EXIT
  fail=0
  (
    cd "$tmp" || exit 2
    git init -q . && git config core.autocrlf false || exit 2
    printf '#!/bin/sh\n' > run.sh
    printf '#!/bin/sh\n' > plain.sh
    printf '#!/bin/sh\n' > untracked.sh
    git add --chmod=+x run.sh || exit 2
    git add --chmod=-x plain.sh || exit 2
  ) || { echo "FAIL: could not build the temp repository"; exit 2; }
  run() { (cd "$tmp" && bash "$self" "$@") >/dev/null 2>&1; echo $?; }
  expect() {
    local want=$1 got
    shift
    got=$(run "$@")
    [ "$got" = "$want" ] || { echo "FAIL: exec-bit.sh $* exited $got, want $want"; fail=1; }
  }
  expect 0 run.sh
  expect 1 plain.sh
  expect 1 run.sh plain.sh            # one bad file fails the set
  expect 2 untracked.sh
  expect 2 run.sh untracked.sh
  expect 2                            # no arguments
  out=$( (cd "$tmp" && bash "$self" run.sh plain.sh) 2>&1)
  case "$out" in
    *plain.sh*) ;;
    *) echo "FAIL: the non-executable file is not named: $out"; fail=1 ;;
  esac
  [ "$fail" -eq 0 ] || exit 1
  echo "OK: self-test (100755 passes, 100644 fails and is named, untracked and bad args exit 2)"
  exit 0
fi

[ $# -ge 1 ] || { echo "$usage"; exit 2; }
git rev-parse --show-toplevel >/dev/null 2>&1 || { echo "FAIL: not in a git repository"; exit 2; }

bad=0
untracked=0
for f in "$@"; do
  [ -n "$f" ] || { echo "$usage"; exit 2; }
  entry=$(git ls-files -s -- "$f") || { echo "FAIL: git ls-files failed"; exit 2; }
  if [ -z "$entry" ]; then
    echo "FAIL: $f is not tracked"
    untracked=1
    continue
  fi
  # A directory argument lists many entries; this check is for files.
  if [ "$(printf '%s\n' "$entry" | wc -l | tr -d ' ')" -ne 1 ]; then
    echo "FAIL: $f names more than one tracked file"
    untracked=1
    continue
  fi
  mode=${entry%% *}
  if [ "$mode" != 100755 ]; then
    echo "FAIL: $f is mode $mode in the index, not 100755 (git update-index --chmod=+x $f)"
    bad=1
  fi
done
[ "$untracked" -eq 0 ] || exit 2
[ "$bad" -eq 0 ] || exit 1
echo "OK: $# file(s) are mode 100755 in the index"
