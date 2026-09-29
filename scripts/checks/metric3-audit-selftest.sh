#!/usr/bin/env bash
# metric3-audit-selftest.sh: the S7 metric 3 audit counts what it should.
#
#   scripts/checks/metric3-audit-selftest.sh
#
# Runs docs/bench/metric3_audit.py --self-test: two hand-written trails over a
# three-file corpus (docs/bench/metric3-selftest/), with their counts fixed in
# the script, once with the corpus and once without it. Without it every path
# and anchor check must come out unknown, never absent.
#
# Exit 0: both runs gave the hand-counted rows. Exit 1: otherwise, or no
# Python on PATH.
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

"${py[@]}" docs/bench/metric3_audit.py --self-test
