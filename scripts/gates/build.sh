#!/usr/bin/env bash
# G1: it compiles. Every target, with default features, and the local-only
# binary without the `hosted` (TLS) feature, which ships as a supported build
# (ADR 0024).
source "$(dirname "$0")/lib.sh"
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
echo "OK: workspace (all targets) and the local-only trellis-cli build$gomsg"
