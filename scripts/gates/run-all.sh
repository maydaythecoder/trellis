#!/usr/bin/env bash
# Runs the gates in order and stops at the first failure.
#
#   scripts/gates/run-all.sh [<issue>]           every gate (G0 and G6 need an issue)
#   scripts/gates/run-all.sh --only G3 [<issue>] one gate
#
# Exit 0: every gate passed. Exit 1: a gate failed (named in the output).
# Exit 3: every gate passed and G7 applies: the ticket is risk:high or
# surface:public, so a person approves before merge.
#
# Each gate's full output goes to <GATE_LOG_DIR>/<gate>.log, and the report
# prints the path beside the last 40 lines. The report alone lost the evidence
# once (#178): a G2 failure's test name and panic sat above the last 40 lines,
# and a rerun passed. GATE_LOG_DIR defaults to ${TMPDIR:-/tmp}/trellis-gates/
# <HEAD sha>; it must be outside the repository, because the gates diff a
# clean tree.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"

# A failure here stops before any gate, and says STOPPED like a gate failure.
stop() { echo "FAIL: $*"; echo "STOPPED before G0"; exit 1; }
root="$(git -C "$here" rev-parse --show-toplevel)" || stop "not in a git repository"
sha="$(git -C "$root" rev-parse HEAD)" || stop "no HEAD commit to name the log directory after"
logdir="${GATE_LOG_DIR:-${TMPDIR:-/tmp}/trellis-gates/$sha}"
mkdir -p "$logdir" || stop "cannot create the log directory $logdir"
logdir="$(cd "$logdir" && pwd -P)"
case "$logdir/" in "$(cd "$root" && pwd -P)"/*)
  rmdir "$logdir" 2>/dev/null
  stop "the log directory $logdir is inside the repository; the gates need a clean tree" ;;
esac

# Every `---- <test> stdout ----` block of a cargo test run, up to the next
# block or the `failures:` list that follows them, and then every
# `panicked at` line wherever it is: a panic outside the harness (a crashed
# test binary, a build script) has no stdout block.
test_failures() {
  echo "FAILING TESTS (from the full log):"
  awk '
    /^---- .* stdout ----$/ { inblock = 1; found = 1; print; next }
    inblock && /^failures:$/ { inblock = 0; next }
    inblock { print }
    END { if (!found) print "(no `---- <test> stdout ----` block in the log)" }' "$1"
  echo "PANICS (from the full log):"
  grep -n "panicked at" "$1" || echo "(no \`panicked at\` line in the log)"
}

only=""
if [ "${1:-}" = "--only" ]; then only="$2"; shift 2; fi
n="${1:-}"

# A full run replaces the previous run's logs for this commit, so a log left
# by a gate this run never reached cannot be read as this run's.
[ -z "$only" ] && rm -f "$logdir"/G*.log

gates=(
  "G0 ticket-ready.sh needs-issue"
  "G1 build.sh"
  "G2 test.sh"
  "G2b guards.sh"
  "G3 lint.sh"
  "G4 no-stubs.sh"
  "G5 security.sh"
  "G6 ticket-verify.sh needs-issue"
)

for g in "${gates[@]}"; do
  set -- $g
  id="$1"; script="$2"; needs="${3:-}"
  [ -n "$only" ] && [ "$only" != "$id" ] && continue
  if [ "$needs" = "needs-issue" ] && [ -z "$n" ]; then
    echo "GATE: $id ($script)"; echo "SKIPPED: no issue number given (run with one to include it)"; echo; continue
  fi
  args=(); [ "$needs" = "needs-issue" ] && args=("$n")
  echo "GATE: $id"
  echo "COMMAND: scripts/gates/$script ${args[*]:-}"
  log="$logdir/$id.log"
  bash "$here/$script" "${args[@]}" >"$log" 2>&1; code=$?
  echo "EXIT: $code"
  echo "LOG: $log"
  echo "OUTPUT:"; tail -40 "$log"
  echo
  if [ $code -ne 0 ] && [ "$id" = "G2" ]; then test_failures "$log"; echo; fi
  if [ $code -ne 0 ]; then echo "STOPPED at $id"; exit 1; fi
done

if [ -n "$n" ] && [ -z "$only" ]; then
  labels=$(gh issue view "$n" --json labels -q '[.labels[].name] | join(" ")')
  case " $labels " in *" risk:high "*|*" surface:public "*)
    echo "GATE: G7"; echo "HUMAN APPROVAL REQUIRED: #$n is risk:high or surface:public"; exit 3 ;;
  esac
fi
echo "ALL GATES PASSED"
