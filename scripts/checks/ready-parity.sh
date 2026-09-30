#!/usr/bin/env bash
# ready-parity.sh: `trellis-tickets ready` and G0 (scripts/gates/ticket-ready.sh)
# give the same G0 verdict on every open issue.
#
#   scripts/checks/ready-parity.sh
#
# ADR 0040 decision 7: the shell script stays G0 until this shows the two
# agree. `ready` is stricter than G0 by four named extras (`no verify
# command`, `decision words: ...`, `effort:l`, `needs ADR`) and reports
# `abstained: ...` when a rule could not decide, so its heading verdict is
# not G0's. G0's verdict is read out of `ready` as its failure lines less
# those five kinds: none left is a G0 pass. An issue whose only failures are
# extras or abstentions agrees on G0 and is listed by name under its own
# heading, neither counted as plain agreement nor hidden (decision 7).
#
# Only the verdict is compared, not the reasons: `ready` appends the lookup
# error to a `blocked_by #n is UNKNOWN`, and G0 does not.
#
# Read-only. The only GitHub calls are `gh issue list` here and the
# `gh issue view` reads the two tools make. The binary is built first, so a
# stale one in target/ is never what gets compared.
#
# A classifier self-test over canned outputs runs before the live pass, so
# a parser that drifted from either tool's output fails here rather than
# reporting agreement it did not check.
#
# Exit 0: the self-test held and every open issue got the same G0 verdict
# from both. Exit 1: a verdict differed, an output could not be read as a
# verdict (an unreadable issue is not agreement), the issue list could not
# be read whole, the build failed, or the self-test failed. Exit 2: wrong
# arguments, or not in a git repository.
set -uo pipefail

[ $# -eq 0 ] || { echo "usage: ready-parity.sh"; exit 2; }
ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || { echo "FAIL: not in a git repository"; exit 2; }
cd "$ROOT" || exit 2

# G0's verdict from ticket-ready.sh's exit code and stdout: `pass`, `fail`,
# or `error` for anything that is neither (`FAIL: cannot read issue`, a
# `set -e` death mid-script, output for another issue).
g0_verdict() {
  local n="$1" rc="$2" out="$3" first
  first="${out%%$'\n'*}"
  if [ "$rc" -eq 0 ] && [ "$first" = "OK: #$n has Done when, Verify, and nothing blocking it" ]; then
    echo pass
  elif [ "$rc" -eq 1 ] && [ "$first" = "FAIL: #$n is not ready:" ]; then
    echo fail
  else
    echo error
  fi
}

# G0's verdict from `trellis-tickets ready`'s exit code and stdout, and the
# extras it failed or abstained on. Prints `pass`, `fail` or `error` on the
# first line; `extras` on a second when an extra failed or abstained.
ready_verdict() {
  local n="$1" rc="$2" out="$3" heading line item g0=0 extras=0 headed
  heading="${out%%$'\n'*}"
  # `: not ready` is tested first: a title may itself end in `: ready`.
  case "$heading" in
    "#$n "*": not ready") headed=fail ;;
    "#$n "*": ready") headed=pass ;;
    *) echo error; return ;;
  esac
  while IFS= read -r line; do
    case "$line" in
      "  - "*) item="${line#  - }" ;;
      "") continue ;;
      *) echo error; return ;;
    esac
    case "$item" in
      "no verify command" | "effort:l" | "needs ADR" | "decision words: "* | "abstained: "*) extras=1 ;;
      *) g0=1 ;;
    esac
  done < <(printf '%s\n' "$out" | tail -n +2)
  # The heading and exit code must agree with the lines: `ready` with no
  # line under it, or a line under `ready`, is output this does not know.
  if [ "$headed" = pass ]; then
    if [ "$rc" -ne 0 ] || [ "$g0" -ne 0 ] || [ "$extras" -ne 0 ]; then echo error; return; fi
    echo pass
    return
  fi
  if [ "$rc" -ne 1 ] || { [ "$g0" -eq 0 ] && [ "$extras" -eq 0 ]; }; then echo error; return; fi
  if [ "$g0" -eq 1 ]; then echo fail; else echo pass; fi
  [ "$extras" -eq 1 ] && echo extras
  return 0
}

# One of `agree`, `extras` (agree on G0; `ready` also failed or abstained on
# an extra), `differ`, or `error`.
compare() {
  local n="$1" g0_rc="$2" g0_out="$3" rd_rc="$4" rd_out="$5" g rv r
  g="$(g0_verdict "$n" "$g0_rc" "$g0_out")"
  rv="$(ready_verdict "$n" "$rd_rc" "$rd_out")"
  r="${rv%%$'\n'*}"
  if [ "$g" = error ] || [ "$r" = error ]; then
    echo error
  elif [ "$g" != "$r" ]; then
    echo differ
  elif [ "$rv" != "$r" ]; then
    echo extras
  else
    echo agree
  fi
}

# --- self-test ------------------------------------------------------------

st_fail=0
expect() {
  local name="$1" want="$2" got
  shift 2
  got="$(compare "$@")"
  if [ "$got" = "$want" ]; then
    echo "ok: self-test: $name"
  else
    echo "FAIL: self-test: $name: want $want, got $got"
    st_fail=1
  fi
}
G0_OK='OK: #7 has Done when, Verify, and nothing blocking it'
G0_NO=$'FAIL: #7 is not ready:\n  - blocked_by #5 is OPEN'

expect "both ready" agree 7 0 "$G0_OK" 0 '#7 A title: ready'
expect "both fail G0" agree 7 1 "$G0_NO" 1 $'#7 A title: not ready\n  - blocked_by #5 is OPEN'
# The reason is not compared: `ready` names the lookup error, G0 does not.
expect "UNKNOWN worded differently" agree 7 1 $'FAIL: #7 is not ready:\n  - blocked_by #5 is UNKNOWN' \
  1 $'#7 A title: not ready\n  - blocked_by #5 is UNKNOWN (fetching #5: gone)'
# decision 7: failing only an extra is not a G0 difference, and not hidden.
expect "extra only" extras 7 0 "$G0_OK" 1 $'#7 A title: not ready\n  - no verify command\n  - effort:l'
expect "decision words only" extras 7 0 "$G0_OK" 1 $'#7 A title: not ready\n  - decision words: either, or'
expect "abstention only" extras 7 0 "$G0_OK" 1 $'#7 A title: not ready\n  - abstained: needs ADR (no table maps x)'
expect "G0 fail plus extras" extras 7 1 "$G0_NO" 1 $'#7 A title: not ready\n  - parked: waits on the owner\'s answer\n  - needs ADR'
# The split this check exists to find, both ways round.
expect "G0 fails, ready passes G0" differ 7 1 "$G0_NO" 0 '#7 A title: ready'
expect "G0 passes, ready fails G0" differ 7 0 "$G0_OK" 1 $'#7 A title: not ready\n  - no \'## Done when\' section'
# A title that ends in `: ready` must not turn `not ready` into `ready`.
expect "title ends ': ready'" agree 7 1 "$G0_NO" 1 $'#7 Mark it: ready: not ready\n  - blocked_by #5 is OPEN'
# An issue neither could read is not agreement: both erroring is an error.
expect "G0 cannot read" error 7 1 'FAIL: cannot read issue #7' 1 $'#7 A title: not ready\n  - x'
expect "ready printed nothing" error 7 0 "$G0_OK" 1 ''
expect "both unreadable" error 7 1 'FAIL: cannot read issue #7' 1 ''
# Output for another issue is not a verdict on this one.
expect "G0 names another issue" error 7 0 'OK: #8 has Done when, Verify, and nothing blocking it' 0 '#7 A title: ready'
expect "ready names another issue" error 7 0 "$G0_OK" 0 '#8 A title: ready'
# Heading, lines and exit code that contradict each other are unknown output.
expect "ready heading with a failure line" error 7 0 "$G0_OK" 0 $'#7 A title: ready\n  - effort:l'
expect "not ready with no line" error 7 0 "$G0_OK" 1 '#7 A title: not ready'
expect "ready exit 0 on not ready" error 7 0 "$G0_OK" 0 $'#7 A title: not ready\n  - effort:l'
expect "G0 OK with exit 1" error 7 1 "$G0_OK" 0 '#7 A title: ready'

if [ "$st_fail" -ne 0 ]; then
  echo "FAIL: the classifier self-test did not hold; the live comparison was not run"
  exit 1
fi

# --- live pass --------------------------------------------------------------

echo "building trellis-tickets"
cargo build -q -p trellis-tickets || { echo "FAIL: cargo build -p trellis-tickets failed"; exit 1; }
target="${CARGO_TARGET_DIR:-$ROOT/target}"
case "$target" in /* | [A-Za-z]:*) ;; *) target="$ROOT/$target" ;; esac
BIN="$target/debug/trellis-tickets"
[ -x "$BIN" ] || BIN="$BIN.exe"
[ -x "$BIN" ] || { echo "FAIL: no trellis-tickets binary under $target/debug after the build"; exit 1; }

# Far above the open count; reaching it means the list may be cut short, and
# "every open issue" could not be claimed.
LIMIT=1000
list="$(gh issue list --state open --limit "$LIMIT" --json number -q '.[].number')" \
  || { echo "FAIL: cannot list open issues"; exit 1; }
issues="$(printf '%s\n' "$list" | sed '/^$/d' | sort -n)"
count="$(printf '%s\n' "$issues" | sed '/^$/d' | wc -l | tr -d ' ')"
[ "$count" -gt 0 ] || { echo "FAIL: gh listed no open issues; nothing was compared"; exit 1; }
[ "$count" -lt "$LIMIT" ] || { echo "FAIL: $count open issues reached the list limit $LIMIT; the list may be incomplete"; exit 1; }
while IFS= read -r n; do
  case "$n" in *[!0-9]* | "") echo "FAIL: gh listed '$n', which is not an issue number"; exit 1 ;; esac
done <<<"$issues"

errfile="$(mktemp)" || { echo "FAIL: mktemp"; exit 1; }
trap 'rm -f "$errfile"' EXIT

agree=() extras=() differ=() errors=()
show() {
  local label="$1" rc="$2" out="$3" err="$4"
  echo "  $label (exit $rc):"
  printf '%s\n' "$out" | sed 's/^/    | /'
  [ -n "$err" ] && printf '%s\n' "$err" | sed 's/^/    ! /'
  return 0
}
while IFS= read -r n; do
  g0_out="$(bash "$ROOT/scripts/gates/ticket-ready.sh" "$n" 2>"$errfile")"
  g0_rc=$?
  g0_err="$(cat "$errfile")"
  rd_out="$("$BIN" ready "$n" 2>"$errfile")"
  rd_rc=$?
  rd_err="$(cat "$errfile")"
  result="$(compare "$n" "$g0_rc" "$g0_out" "$rd_rc" "$rd_out")"
  case "$result" in
    agree) agree+=("$n") ;;
    extras) extras+=("$n") ;;
    differ | error)
      if [ "$result" = differ ]; then differ+=("$n"); echo "DIFFER: #$n"; else errors+=("$n"); echo "ERROR: #$n: an output is not a verdict"; fi
      show "scripts/gates/ticket-ready.sh $n" "$g0_rc" "$g0_out" "$g0_err"
      show "trellis-tickets ready $n" "$rd_rc" "$rd_out" "$rd_err"
      ;;
  esac
  if [ "$result" = extras ]; then
    echo "extras: #$n agrees on G0; ready also reports:"
    printf '%s\n' "$rd_out" | tail -n +2 | grep -E '^  - (no verify command|effort:l|needs ADR|decision words: |abstained: )' | sed 's/^/  /'
  fi
done <<<"$issues"

echo
echo "open issues compared: $count"
echo "agree on G0:                     ${#agree[@]}  ${agree[*]:-}"
echo "agree on G0, ready fails extras: ${#extras[@]}  ${extras[*]:-}"
echo "G0 verdicts differ:              ${#differ[@]}  ${differ[*]:-}"
echo "could not compare:               ${#errors[@]}  ${errors[*]:-}"
if [ "${#differ[@]}" -ne 0 ] || [ "${#errors[@]}" -ne 0 ]; then
  echo "FAIL: G0 and trellis-tickets ready do not agree on every open issue"
  exit 1
fi
echo "OK: G0 and trellis-tickets ready give the same G0 verdict on all $count open issues"
