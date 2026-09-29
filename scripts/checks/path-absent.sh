#!/usr/bin/env bash
# path-absent.sh: no file under a path is tracked.
#
#   scripts/checks/path-absent.sh <path>
#
# A Verify line for a deletion. It reads the index, not the disk: a build
# directory or an ignored file left under the path does not count, and a
# file deleted from disk but still tracked does. Run from the repository.
#
# Exit 0: nothing under <path> is tracked. Exit 1: something is, and the
# first few tracked files are printed. Exit 2: wrong arguments, or not in
# a git repository.
set -uo pipefail

[ $# -eq 1 ] && [ -n "$1" ] || { echo "usage: path-absent.sh <path>"; exit 2; }
# Git Bash rewrites a path-looking argument otherwise.
export MSYS_NO_PATHCONV=1
tracked=$(git ls-files -- "$1") || { echo "FAIL: git ls-files failed"; exit 2; }
if [ -n "$tracked" ]; then
  n=$(printf '%s\n' "$tracked" | wc -l | tr -d ' ')
  echo "FAIL: $n tracked file(s) under $1:"
  printf '%s\n' "$tracked" | head -5 | sed 's/^/  /'
  exit 1
fi
echo "OK: nothing under $1 is tracked"
