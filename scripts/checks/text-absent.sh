#!/usr/bin/env bash
# text-absent.sh: no tracked file under a path contains a fixed string.
#
#   scripts/checks/text-absent.sh <text> <path>
#   scripts/checks/text-absent.sh --self-test
#
# A Verify line for a removed reference, the counterpart of path-absent.sh.
# <text> is matched as a fixed string, not a pattern, so a ref name such as
# `origin/legacy-x` or a dotted version means exactly what it says. Only tracked
# files count (their working-tree contents, as `git grep` reads them): a build
# directory or an ignored file under the path does not. Run from the
# repository; <path> is relative to the current directory, as git reads it.
#
# Exit 0: no tracked file under <path> contains <text>. Exit 1: some do, and
# the first few are printed. Exit 2: wrong arguments, or not in a git
# repository.
set -uo pipefail

usage='usage: text-absent.sh <text> <path> | --self-test'
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
    mkdir -p gates other
    printf 'base=origin/dev\n' > gates/clean.sh
    printf 'base=origin/legacy-x\n' > other/dirty.sh
    printf 'base=origin/legacy-x\n' > gates/untracked.sh
    git add gates/clean.sh other/dirty.sh || exit 2
  ) || { echo "FAIL: could not build the temp repository"; exit 2; }
  run() { (cd "$tmp" && bash "$self" "$@") >/dev/null 2>&1; echo $?; }
  expect() {
    local want=$1 got
    shift
    got=$(run "$@")
    [ "$got" = "$want" ] || { echo "FAIL: text-absent.sh $* exited $got, want $want"; fail=1; }
  }
  expect 0 origin/legacy-x gates      # only an untracked file holds it
  expect 1 origin/legacy-x other      # a tracked file holds it
  expect 1 origin/legacy-x .
  expect 0 'origin/legacy.x' .        # fixed string: `.` is not a wildcard
  expect 2 origin/legacy-x            # one argument
  expect 2 '' gates                   # empty text
  notrepo=$(mktemp -d) || exit 2
  got=$( (cd "$notrepo" && GIT_CEILING_DIRECTORIES="$(dirname "$notrepo")" bash "$self" x .) >/dev/null 2>&1; echo $?)
  rm -rf "$notrepo"
  [ "$got" = 2 ] || { echo "FAIL: outside a repository exited $got, want 2"; fail=1; }
  [ "$fail" -eq 0 ] || exit 1
  echo "OK: self-test (untracked ignored, tracked found, fixed-string match, bad args and no repo exit 2)"
  exit 0
fi

[ $# -eq 2 ] && [ -n "$1" ] && [ -n "$2" ] || { echo "$usage"; exit 2; }
text=$1
path=$2
git rev-parse --show-toplevel >/dev/null 2>&1 || { echo "FAIL: not in a git repository"; exit 2; }

# git grep exits 1 on no match and >1 on error; only the latter is a failure
# to run.
hits=$(git grep -F -l -- "$text" -- "$path")
rc=$?
if [ "$rc" -gt 1 ]; then
  echo "FAIL: git grep failed ($rc)"
  exit 2
fi
if [ -n "$hits" ]; then
  n=$(printf '%s\n' "$hits" | wc -l | tr -d ' ')
  echo "FAIL: $n tracked file(s) under $path contain '$text':"
  printf '%s\n' "$hits" | head -5 | sed 's/^/  /'
  exit 1
fi
echo "OK: no tracked file under $path contains '$text'"
