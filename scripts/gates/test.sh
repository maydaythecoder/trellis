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

# Test threads per test binary, for unit, integration and doc tests alike.
# libtest defaults to one per core, and many tests spawn git, a shell or
# PowerShell, so on a 16-core host the gate ran up to 16 tests and their
# children at once and failed timing tests on its own load (#226: #207,
# #212, #221, #222 and integrate's 15 s merge wait). cargo runs the test
# binaries one after another, so this bounds the whole gate, not each
# binary on top of the others. 4 is a quarter of the 16-core host the gate
# runs on, beside up to three implementer builds at CARGO_BUILD_JOBS=4.
# Measured on a quiet host, same commit (#226): the gate took 558 s at the
# default 16, 923 s at 8 and 1075 s at 4, so most of the cost is paid by 8
# and halving the load again costs about 150 s more. Set unconditionally: a
# caller's larger value would undo the bound. It only changes how many run
# at once; every test still runs (2118 at #226, the same count at 16, 8 and 4).
#
# GATE_TEST_TIMEOUT_S above is per `cargo test`; at 4 threads the longer one
# (the workspace) spent 645 s in its test binaries, so 2400 still leaves
# more than 3x for a loaded host and was not changed.
export RUST_TEST_THREADS=4

bounded() {
  local code=0
  # `timeout` from coreutils (Git Bash and Linux alike). --kill-after, for a
  # cargo that ignores TERM; 124 is timeout's own code for an expiry, and
  # 137 is the KILL that followed it.
  timeout --kill-after=30 "$limit" "$@" || code=$?
  case "$code" in
    0) ;;
    124 | 137) fail "\`$*\` did not finish within GATE_TEST_TIMEOUT_S=${limit}s: a test waiting on something, or a host too loaded to finish (#173)" ;;
    *) exit "$code" ;;
  esac
}

bounded cargo test --workspace --no-fail-fast
bounded cargo test -p trellis-cli --no-default-features --no-fail-fast
# Go branch (ADR 0052, #504): every package in tui/, under the same time
# limit. tui/'s rpc tests spawn target/debug/trellis, which the cargo runs
# above have built.
if go_module_present; then
  go_require
  (cd tui && bounded go test -count=1 ./...)
  gomsg="; tui/ (go test)"
else
  gomsg="; Go branch skipped: tui/ is absent"
fi
echo "OK: workspace tests and the local-only trellis-cli tests$gomsg"
