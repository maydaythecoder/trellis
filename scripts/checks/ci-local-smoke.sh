#!/usr/bin/env bash
# ci-local-smoke.sh: scripts/ci-local.py runs a WSL leg twice without touching
# the checkout it was started from.
#
#   scripts/checks/ci-local-smoke.sh
#
# #166: the first run of ci-local.py cloned the repository into the Windows
# checkout (./trellis) instead of the WSL home, because wsl.exe's `--` hands
# the command to a login shell that expanded `$W` before bash assigned it.
# Every later WSL leg then failed at checkout. This runs the smallest Linux
# job, index-invariants, twice (the second must reuse the clone the first
# made) and fails unless both pass and `git status --porcelain --ignored` of
# this checkout is byte-identical before and after.
#
# It does not run the other jobs: the host is shared with other builds and a
# local model, and index-invariants is enough to exercise clone, reuse and a
# cargo step inside WSL.
#
# Exit 0: both runs passed their WSL leg and the checkout is unchanged.
# Exit 1: otherwise, with the failed condition printed.
set -uo pipefail

root="$(git rev-parse --show-toplevel)" || exit 1
cd "$root" || exit 1

if command -v py >/dev/null 2>&1; then
  py=(py -3)
else
  py=(python3)
fi

before="$(git status --porcelain --ignored)"
fail=0

for n in 1 2; do
  echo "== ci-local run $n: --job index-invariants"
  out="$("${py[@]}" scripts/ci-local.py --job index-invariants 2>&1)"
  code=$?
  printf '%s\n' "$out"
  if [ "$code" -ne 0 ]; then
    echo "FAIL: run $n exited $code"
    fail=1
  elif ! printf '%s\n' "$out" | grep -Eq '^index-invariants \| ubuntu-latest \| pass \|'; then
    # The job is Linux-only, so exit 0 without this row would mean the leg
    # was never run, not that it passed.
    echo "FAIL: run $n printed no passing index-invariants (ubuntu-latest) row"
    fail=1
  fi
  after="$(git status --porcelain --ignored)"
  if [ "$after" != "$before" ]; then
    echo "FAIL: run $n changed the checkout at $root:"
    diff <(printf '%s\n' "$before") <(printf '%s\n' "$after")
    fail=1
  fi
done

if [ "$fail" -eq 0 ]; then
  echo "ci-local-smoke: ok (WSL leg passed twice; checkout unchanged)"
fi
exit "$fail"
