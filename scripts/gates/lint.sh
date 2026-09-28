#!/usr/bin/env bash
# G3: formatter and linter clean, with warnings as errors, and zero lint
# suppressions added by this change (CLAUDE.md; a suppression is a warning
# silenced, not fixed).
source "$(dirname "$0")/lib.sh"
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
added=$(git diff -U0 "$GATE_BASE"...HEAD -- '*.rs' | grep -E '^\+[^+].*#!?\[allow\(' || true)
if [ -n "$added" ]; then
  echo "FAIL: this change adds lint suppressions (against $GATE_BASE):"
  printf '%s\n' "$added"
  exit 1
fi
echo "OK: fmt, clippy -D warnings, no #[allow] added"
