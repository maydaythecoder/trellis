#!/usr/bin/env bash
# G2: the full suite, not just the new tests, in both feature configurations.
source "$(dirname "$0")/lib.sh"
cargo test --workspace --no-fail-fast
cargo test -p trellis-cli --no-default-features --no-fail-fast
echo "OK: workspace tests and the local-only trellis-cli tests"
