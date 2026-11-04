#!/usr/bin/env bash
# G1: it compiles. Every target, with default features, and the local-only
# binary without the `hosted` (TLS) feature, which ships as a supported build
# (ADR 0024).
source "$(dirname "$0")/lib.sh"
# G2's time limit where neither coreutils `timeout` nor `gtimeout` is on
# PATH (macOS, #1070). Checked here, before G2 depends on it, so a watchdog
# that cannot fire is found by a failed self-test, not by a gate that never
# ends. Where `timeout` exists G2 does not use it, and G1 is unchanged.
wdmsg=""
if ! command -v timeout >/dev/null 2>&1 && ! command -v gtimeout >/dev/null 2>&1; then
  perl scripts/gates/watchdog.pl --self-test || fail "watchdog.pl self-test failed"
  wdmsg="; watchdog.pl self-test (no timeout or gtimeout here)"
fi
cargo build --workspace --all-targets
cargo build -p trellis-cli --no-default-features
# Go branch (ADR 0052, #504): tui/ builds from its committed go.sum.
if go_module_present; then
  go_require
  (cd tui && go build ./...)
  gomsg="; tui/ (go build)"
else
  gomsg="; Go branch skipped: tui/ is absent"
fi
echo "OK: workspace (all targets) and the local-only trellis-cli build$gomsg$wdmsg"
