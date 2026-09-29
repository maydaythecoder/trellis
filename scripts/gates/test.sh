#!/usr/bin/env bash
# G2: the full suite, not just the new tests, in both feature configurations.
source "$(dirname "$0")/lib.sh"

# Each `cargo test` gets a time limit, so a test that hangs fails the gate
# instead of stalling the queue behind it (issue #173: a trellis-loop unit
# test waited at zero CPU for 20 and 55 minutes under load). The default,
# 40 minutes, is headroom for a loaded host, not a measured bound.
limit="${GATE_TEST_TIMEOUT_S:-2400}"
case "$limit" in
  '' | *[!0-9]* | 0) fail "GATE_TEST_TIMEOUT_S must be a whole number of seconds above 0, not '$limit'" ;;
esac

bounded() {
  local code=0
  # `timeout` from coreutils (Git Bash and Linux alike). --kill-after, for a
  # cargo that ignores TERM; 124 is timeout's own code for an expiry, and
  # 137 is the KILL that followed it.
  timeout --kill-after=30 "$limit" "$@" || code=$?
  case "$code" in
    0) ;;
    124 | 137) fail "\`$*\` did not finish within GATE_TEST_TIMEOUT_S=${limit}s; a test is waiting on something (#173)" ;;
    *) exit "$code" ;;
  esac
}

bounded cargo test --workspace --no-fail-fast
bounded cargo test -p trellis-cli --no-default-features --no-fail-fast
echo "OK: workspace tests and the local-only trellis-cli tests"
