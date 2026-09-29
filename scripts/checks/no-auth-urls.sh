#!/usr/bin/env bash
# no-auth-urls.sh: no tracked source line holds a `scheme://user:pass@host`
# literal.
#
#   scripts/checks/no-auth-urls.sh [--self-test | --pattern]
#
# A secret scanner reads such a literal as a leaked basic-auth string on
# every push, fake or not (#185: GitGuardian on #150's proxy tests). A test
# that needs one builds it at run time, `format!("http://u:{pw}@host")`, so
# the password segment holds a `{`: the pattern refuses `{` and `}` there,
# which is what lets that form through. docs/ is prose that describes the
# shape, and this script names it, so both are left out.
#
# --pattern prints the pattern, so scripts/gates/security.sh scans added
# lines with this one rather than a copy that can drift.
#
# Exit 0: none found. Exit 1: found, each printed as file:line. Exit 2:
# wrong arguments, or not in a git repository.
set -uo pipefail

AUTH_URL='[A-Za-z][A-Za-z0-9+.-]*://[A-Za-z0-9._%~-]+:[^@/"{}[:space:]]+@[A-Za-z0-9.[-]'

usage='usage: no-auth-urls.sh [--self-test | --pattern]'
if [ "${1:-}" = "--pattern" ]; then
  [ $# -eq 1 ] || { echo "$usage"; exit 2; }
  printf '%s\n' "$AUTH_URL"
  exit 0
fi
if [ "${1:-}" = "--self-test" ]; then
  [ $# -eq 1 ] || { echo "$usage"; exit 2; }
  # Assembled from parts so this file never holds the literal it looks for.
  at='@'
  planted="\"http://alice:s3cret-pw${at}proxy.example:3128\""
  formatted="format!(\"http://alice:{pw}${at}proxy.example:3128\")"
  positional="format!(\"http://alice:{}${at}proxy.example:3128\", pw)"
  plain='"http://proxy.example:3128"'
  fail=0
  printf '%s\n' "$planted" | grep -qE "$AUTH_URL" \
    || { echo "FAIL: pattern misses a planted literal: $planted"; fail=1; }
  for ok in "$formatted" "$positional" "$plain"; do
    if printf '%s\n' "$ok" | grep -qE "$AUTH_URL"; then
      echo "FAIL: pattern matches a run-time form: $ok"; fail=1
    fi
  done
  [ "$fail" -eq 0 ] || exit 1
  echo "OK: self-test (matches a planted literal, not the format! forms)"
  exit 0
fi
[ $# -eq 0 ] || { echo "$usage"; exit 2; }

git rev-parse --show-toplevel >/dev/null 2>&1 || { echo "FAIL: not in a git repository"; exit 2; }
cd "$(git rev-parse --show-toplevel)" || exit 2
# git grep exits 1 on no match and >1 on error; only the latter is a failure
# to run.
hits=$(git grep -nIE "$AUTH_URL" -- . ':(exclude)docs/' ':(exclude)scripts/checks/no-auth-urls.sh')
rc=$?
if [ "$rc" -gt 1 ]; then
  echo "FAIL: git grep failed ($rc)"
  exit 2
fi
if [ -n "$hits" ]; then
  echo "FAIL: basic-auth URL literal(s) in tracked source (build them at run time):"
  printf '%s\n' "$hits" | cut -d: -f1-2 | sed 's/^/  /'
  exit 1
fi
echo "OK: no scheme://user:pass@host literal in tracked source"
