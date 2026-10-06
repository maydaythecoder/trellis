#!/usr/bin/env bash
# floor-compare-selftest.sh: floor reports are compared only when their
# comparability keys match (#344).
#
#   scripts/checks/floor-compare-selftest.sh
#
# Runs docs/bench/compare_floor.py --self-test, which builds its reports in
# memory and covers four cases: equal keys (exit 0, both configurations
# printed); a tasks_sha256 mismatch (exit 1, tasks_sha256 named); a budget
# mismatch (exit 1, budget named); a report with no comparable block (exit 1,
# the report named).
#
# Exit 0: all four behaved so. Exit 1: otherwise, or no Python on PATH.
set -uo pipefail

root="$(git rev-parse --show-toplevel)" || exit 1
cd "$root" || exit 1

if command -v py >/dev/null 2>&1; then
  py=(py -3)
elif command -v python3 >/dev/null 2>&1; then
  py=(python3)
else
  echo "FAIL: no py or python3 on PATH"
  exit 1
fi

"${py[@]}" docs/bench/compare_floor.py --self-test
