#!/usr/bin/env bash
# floor-tools-selftest.sh: the floor run's report and bytes-per-token figures
# count what they should.
#
#   scripts/checks/floor-tools-selftest.sh
#
# Runs docs/bench/run_floor.py --self-test (model-call time from `turn`
# events: median, p90 and count, an untimed turn left out rather than counted
# as 0) and docs/bench/bytes_per_token.py --self-test (`--no-runtime-log` on a
# hosted and a runtime adapter log, with the pooled ratio fixed in the script).
#
# Exit 0: both self-tests passed. Exit 1: either failed, or no Python on PATH.
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

rc=0
"${py[@]}" docs/bench/run_floor.py --self-test || rc=1
"${py[@]}" docs/bench/bytes_per_token.py --self-test || rc=1
exit "$rc"
