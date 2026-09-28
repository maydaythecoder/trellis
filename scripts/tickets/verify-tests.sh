#!/usr/bin/env bash
# A Verify command for "these tests exist and pass".
#
#   scripts/tickets/verify-tests.sh <package> <filter> [<min>]
#
# `cargo test -p pkg filter` exits 0 when the filter matches nothing, so a
# ticket whose tests were never written would pass its own Verify. This fails
# unless at least <min> (default 1) tests ran, and all passed.
set -euo pipefail
pkg="${1:?usage: verify-tests.sh <package> <filter> [<min>]}"
filter="${2:?usage: verify-tests.sh <package> <filter> [<min>]}"
min="${3:-1}"
cd "$(git rev-parse --show-toplevel)"
out=$(cargo test -p "$pkg" "$filter" 2>&1) || { printf '%s\n' "$out" | tail -40; echo "FAIL: tests failed"; exit 1; }
ran=$(printf '%s\n' "$out" | sed -n 's/^test result: ok\. \([0-9]*\) passed.*/\1/p' | awk '{s+=$1} END {print s+0}')
printf '%s\n' "$out" | grep -E '^test .* \.\.\. ' | tail -40 || true
if [ "$ran" -lt "$min" ]; then
  echo "FAIL: $ran test(s) matched '$filter' in $pkg; need at least $min"
  exit 1
fi
echo "OK: $ran test(s) matched '$filter' in $pkg and passed"
