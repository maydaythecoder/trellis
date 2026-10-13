# Shared by every gate. Sourced, not run.
#
# A gate is a script that exits 0 when what it proves holds and non-zero with
# the reason on stdout when it does not. There is no skip: a gate that cannot
# run fails (docs/BUILD_SYSTEM.md).

set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

# What "this change" is measured against (ADR 0051). Ticket (`t<n>-<slug>`)
# and `feature/<slug>` branches are cut from `dev` and merge into the
# integration branch `audit`, where they meet and conflicts are resolved;
# `audit` is promoted into `dev`, and `dev` reaches the launch branch `main`
# only through a PR the owner merges. A ticket branch and `audit` answer only
# for what they add to `dev`; `dev` answers to `main` (its PR's diff).
# Measuring a ticket against main would fail it on work it did not add.
#
# `--base <ref>` as a gate's first argument sets it for that run, so a ticket
# whose work is cleaning up an older change (issue #13) can have a Verify line
# that measures against that change's base; G6 allows no `VAR=` prefix.
# Sourcing shares the caller's arguments, so the shift here is the caller's.
INTEGRATION_BRANCH="audit"
DEV_BRANCH="dev"
if [ "${1:-}" = "--base" ]; then
  GATE_BASE="${2:?--base needs a ref}"
  shift 2
fi
if [ -z "${GATE_BASE:-}" ]; then
  case "$(git rev-parse --abbrev-ref HEAD)" in
    t[0-9]*-*|feature/*|"$INTEGRATION_BRANCH") GATE_BASE="origin/$DEV_BRANCH" ;;
    *) GATE_BASE="origin/main" ;;
  esac
fi
# A base this clone has not fetched is a failure, not a reason to measure
# against some other branch: the legacy branches are frozen (ADR 0051), and a
# diff against them would pass or fail a change on work it did not touch.
if ! git rev-parse -q --verify "$GATE_BASE^{commit}" >/dev/null 2>&1; then
  echo "FAIL: base $GATE_BASE not fetched; run git fetch origin" >&2
  exit 1
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

# Go (ADR 0052, #504). tui/ is a Go module; each gate's Go branch runs only
# when tui/go.mod exists and says it skipped otherwise, so the gates could
# carry these branches before tui/ existed. With tui/ present a missing `go`
# fails the gate: a gate that cannot run fails.
go_module_present() { [ -f "$ROOT/tui/go.mod" ]; }

# Fail unless `go` is on PATH, then hold every go command the gate runs to
# the committed pins: go.sum read-only, and no toolchain download for a
# `toolchain` line (decision 2.4).
go_require() {
  command -v go >/dev/null 2>&1 \
    || fail "tui/go.mod exists but go is not on PATH (scripts/runners/install-go.sh installs the pinned one)"
  export GOFLAGS=-mod=readonly GOTOOLCHAIN=local
}

# Go files this change added lines to, outside tests and testdata.
changed_production_go() {
  git diff --name-only --diff-filter=AM "$GATE_BASE"...HEAD -- 'tui/*.go' 'tui/**/*.go' \
    | grep -Ev '_test\.go$|(^|/)testdata/' | sort -u || true
}

# `file:line:text` for every line this change added to production Go. Go
# keeps tests in `_test.go` files, so there is no in-file cutoff.
added_production_go_lines() {
  local f
  for f in $(changed_production_go); do
    [ -f "$f" ] || continue
    git diff -U0 "$GATE_BASE"...HEAD -- "$f" | awk -v f="$f" '
      /^@@/ { split($3, a, ","); line = substr(a[1], 2) + 0; next }
      /^\+\+\+/ { next }
      /^\+/ { print f ":" line ":" substr($0, 2); line++; next }
      /^-/ { next }'
  done
}

# deny.toml's licence allow-list as one comma-separated line, so go-licenses
# holds the Go tree to the list cargo deny holds the Rust tree to (ADR 0052
# section 5).
deny_allowed_licenses() {
  awk '
    /^\[licenses\]/ { in_l = 1; next }
    /^\[/ { in_l = 0 }
    in_l && /^allow[ \t]*=[ \t]*\[/ { in_a = 1; next }
    in_a && /^\]/ { exit }
    in_a && match($0, /^[ \t]*"[^"]+"/) { s = substr($0, RSTART, RLENGTH); gsub(/[ \t"]/, "", s); print s }
  ' "$ROOT/deny.toml" | paste -sd, -
}
