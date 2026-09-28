# Shared by every gate. Sourced, not run.
#
# A gate is a script that exits 0 when what it proves holds and non-zero with
# the reason on stdout when it does not. There is no skip: a gate that cannot
# run fails (docs/BUILD_SYSTEM.md).

set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

# What "this change" is measured against. While PR #1 is open, a ticket
# branch is cut from `inherit` and answers only for what it adds to it;
# `inherit` itself answers to the PR base. Measuring a ticket against the PR
# base would fail it on PR #1's debt, which it did not add.
#
# `--base <ref>` as a gate's first argument sets it for that run, so a ticket
# whose work is cleaning up PR #1 itself (issue #13) can have a Verify line
# that measures against the PR base; G6 allows no `VAR=` prefix. Sourcing
# shares the caller's arguments, so the shift here is the caller's.
if [ "${1:-}" = "--base" ]; then
  GATE_BASE="${2:?--base needs a ref}"
  shift 2
fi
if [ -z "${GATE_BASE:-}" ]; then
  if [ "$(git rev-parse --abbrev-ref HEAD)" = "inherit" ]; then
    GATE_BASE="origin/v0.1-resolver"
  else
    GATE_BASE="origin/inherit"
  fi
fi

# Gates run commands a ticket or a repository names; none of them needs a
# provider key, and none may pass one on (ADR 0024 §3). Every variable that
# looks like a key is removed before anything runs.
for v in $(env | sed -n 's/^\([A-Za-z_][A-Za-z0-9_]*\)=.*/\1/p' | grep -E '(_API_KEY|_TOKEN|_SECRET)$' || true); do
  case "$v" in GH_TOKEN|GITHUB_TOKEN) ;; *) unset "$v" ;; esac
done
unset OPENAI_API_KEY ANTHROPIC_API_KEY 2>/dev/null || true

# Python for qa/: `py -3` on Windows, `python3` elsewhere.
if command -v python3 >/dev/null 2>&1 && python3 -c 'import sys' >/dev/null 2>&1; then
  PY="python3"
elif command -v py >/dev/null 2>&1; then
  PY="py -3"
else
  PY=""
fi

fail() { echo "FAIL: $*"; exit 1; }

# Rust files this change added lines to, outside tests, examples and fixtures.
changed_production_rs() {
  git diff --name-only --diff-filter=AM "$GATE_BASE"...HEAD -- '*.rs' \
    | grep -Ev '(^|/)(tests|examples|benches|fixtures)/|_test\.rs$' || true
}

# `file:line:text` for every line this change added to production Rust, above
# the file's first column-0 `#[cfg(test)]` (trellis keeps unit tests in a
# module at the bottom of the file).
added_production_lines() {
  local f cutoff
  for f in $(changed_production_rs); do
    [ -f "$f" ] || continue
    cutoff=$(grep -n '^#\[cfg(test)\]' "$f" | head -1 | cut -d: -f1)
    cutoff=${cutoff:-999999999}
    git diff -U0 "$GATE_BASE"...HEAD -- "$f" | awk -v f="$f" -v cut="$cutoff" '
      /^@@/ { split($3, a, ","); line = substr(a[1], 2) + 0; next }
      /^\+\+\+/ { next }
      /^\+/ { if (line < cut) print f ":" line ":" substr($0, 2); line++; next }
      /^-/ { next }'
  done
}
