#!/usr/bin/env bash
# G3: formatter and linter clean, with warnings as errors, and zero lint
# suppressions added by this change (CLAUDE.md; a suppression is a warning
# silenced, not fixed).
source "$(dirname "$0")/lib.sh"
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
added=$(git diff -U0 "$GATE_BASE"...HEAD -- '*.rs' | grep -vE '^\+\+\+' | grep -E '^\+.*#!?\[allow\(' || true)
if [ -n "$added" ]; then
  echo "FAIL: this change adds lint suppressions (against $GATE_BASE):"
  printf '%s\n' "$added"
  exit 1
fi
# Every ADR has a number of its own, including against the base, which a
# branch cut before another ADR merged cannot see in its tree (#284).
bash scripts/checks/adr-numbers.sh --self-test > /dev/null || fail "adr-numbers.sh self-test failed"
# The exit-gate check holds a ticked box to a test that exists (#467, #484).
bash scripts/checks/gate-checked.sh --self-test > /dev/null || fail "gate-checked.sh self-test failed"
adr=$(bash scripts/checks/adr-numbers.sh "$GATE_BASE") || { printf '%s\n' "$adr"; fail "two ADRs share a number, or a held number is misused"; }
# Go branch (ADR 0052, #504): gofmt clean, go vet clean, and no `//nolint`
# added, Go's form of a lint suppression.
if go_module_present; then
  go_require
  unformatted=$(cd tui && gofmt -l .)
  [ -z "$unformatted" ] || { printf '%s\n' "$unformatted"; fail "gofmt -l lists the files above under tui/"; }
  (cd tui && go vet ./...)
  added=$(git diff -U0 "$GATE_BASE"...HEAD -- 'tui/*.go' 'tui/**/*.go' | grep -vE '^\+\+\+' | grep -E '^\+.*//[[:space:]]*nolint' || true)
  if [ -n "$added" ]; then
    echo "FAIL: this change adds //nolint suppressions (against $GATE_BASE):"
    printf '%s\n' "$added"
    exit 1
  fi
  gomsg="; tui/: gofmt, go vet, no //nolint added"
else
  gomsg="; Go branch skipped: tui/ is absent"
fi
echo "OK: fmt, clippy -D warnings, no #[allow] added; $adr$gomsg"
