#!/usr/bin/env bash
# Runs the gates in order and stops at the first failure.
#
#   scripts/gates/run-all.sh [<issue>]           every gate (G0 and G6 need an issue)
#   scripts/gates/run-all.sh --only G3 [<issue>] one gate
#
# Exit 0: every gate passed. Exit 1: a gate failed (named in the output).
# Exit 3: every gate passed and G7 applies: the ticket is risk:high or
# surface:public, so a person approves before merge.
set -uo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
only=""
if [ "${1:-}" = "--only" ]; then only="$2"; shift 2; fi
n="${1:-}"

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
  out=$(bash "$here/$script" "${args[@]}" 2>&1); code=$?
  echo "EXIT: $code"
  echo "OUTPUT:"; printf '%s\n' "$out" | tail -40
  echo
  if [ $code -ne 0 ]; then echo "STOPPED at $id"; exit 1; fi
done

if [ -n "$n" ] && [ -z "$only" ]; then
  labels=$(gh issue view "$n" --json labels -q '[.labels[].name] | join(" ")')
  case " $labels " in *" risk:high "*|*" surface:public "*)
    echo "GATE: G7"; echo "HUMAN APPROVAL REQUIRED: #$n is risk:high or surface:public"; exit 3 ;;
  esac
fi
echo "ALL GATES PASSED"
