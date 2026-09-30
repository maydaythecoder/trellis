#!/usr/bin/env bash
# crlf-blocked-by.sh: G0's blocked_by parser reads a CRLF issue body the same
# as its LF form, and still blocks on an entry that is not an issue number.
#
#   scripts/checks/crlf-blocked-by.sh
#
# #205: #114's body was edited from Windows and arrived with CRs. G0 parsed
# `blocked_by: [113]` as `113\r` and refused the ticket with "blocked_by
# entry '113' is not an issue number" (the CR does not show in a terminal).
#
# Bodies on GitHub end their lines in two shapes: `\r\n`, and `\r\r\n` (#106,
# #107 and #108 had `blocked_by: [105]\r\r\n`). Each case runs in both.
# Git Bash's sed drops the one CR that precedes a newline, so there only
# `\r\r\n` shows the bug; GNU sed on Linux keeps every CR, so both show it.
#
# This sources scripts/tickets/blocked-by.sh, the parser ticket-ready.sh runs,
# and replaces its issue_state with a canned lookup, so nothing here calls
# GitHub. Each body is checked to contain the CRs it was built with before it
# is used: a body that lost them would test the LF path and pass for the
# wrong reason.
#
# Exit 0: every case held. Exit 1: a case failed, or a fixture lost its CRs.
set -uo pipefail

ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || { echo "FAIL: not in a git repository"; exit 1; }
# shellcheck source=../tickets/blocked-by.sh
source "$ROOT/scripts/tickets/blocked-by.sh" || { echo "FAIL: cannot source scripts/tickets/blocked-by.sh"; exit 1; }

# Canned states: #113 closed (as in #205), #114 closed, #115 open. Any other
# number is UNKNOWN, as the real lookup reports an issue it cannot read.
issue_state() {
  case "$1" in
    113 | 114) echo CLOSED ;;
    115) echo OPEN ;;
    *) echo UNKNOWN ;;
  esac
}

CR=$'\r'
# A ticket body in #114's shape, with the given blocked_by line inside the
# triage block, every line ended by $2 (`\r\n` or `\r\r\n`). printf -v, not
# `$(...)`: Git Bash's command substitution strips a trailing `\r\n`.
eol_body() {
  local line out=""
  for line in 'Title line' '' '## Done when' '- it works' '' '## Verify' '```' 'true' '```' '' \
    '<!-- triage -->' "$1" 'ready: true' '<!-- /triage -->'; do
    printf -v out '%s%s%s' "$out" "$line" "$2"
  done
  printf '%s' "$out"
}

fail=0
bad() { echo "FAIL: $*"; fail=1; }
# Shows invisible bytes, so a stray CR is in the log rather than guessed at.
bytes() { printf '%s' "$1" | od -c | sed 's/^/    /'; }

# $1 case name, $2 blocked_by line, $3 expected entries (space-separated),
# $4 expected problems (newline-separated, empty for none).
check() {
  local eol
  for eol in "$CR"$'\n' "$CR$CR"$'\n'; do
    check_one "$@" "$eol"
  done
}
check_one() {
  local name="$1" line="$2" want_entries="$3" want_problems="$4" eol="$5" shape body entries problems
  [ "${#eol}" -eq 3 ] && shape='\r\r\n' || shape='\r\n'
  name="$name, $shape"
  body="$(eol_body "$line" "$eol"; echo .)"
  body="${body%.}"
  # A bash match, not grep: Git Bash's grep drops a CR at the end of a line.
  case "$body" in
    *"$line$eol"*) ;;
    *) bad "$name: fixture lost its CRs; the CR path would not be tested"; bytes "$body"; return ;;
  esac
  # The parser's output byte for byte: the trailing `.` stops Git Bash's
  # `$(...)` from stripping a trailing `\r\n`, which would hide a leftover CR.
  # Then word-split on IFS, which has no CR, and rejoin with one space.
  entries="$(printf '%s' "$body" | blocked_by_entries; echo .)"
  entries="${entries%.}"
  entries="$(set -f; printf '%s ' $entries; echo .)"
  entries="${entries% .}"
  problems="$(printf '%s' "$body" | blocked_by_problems)"
  if [ "$entries" != "$want_entries" ]; then
    bad "$name: entries are not '$want_entries'; got:"; bytes "$entries"
  elif [ "$problems" != "$want_problems" ]; then
    bad "$name: problems are not as expected; want:"; bytes "$want_problems"
    echo "  got:"; bytes "$problems"
  else
    echo "ok: $name"
  fi
}

# The #205 case: read as 113, and #113 is closed, so nothing blocks.
check "[113], #113 closed" 'blocked_by: [113]' '113' ''
# An open dependency: the message names #115 with no CR attached.
check "[115], #115 open" 'blocked_by: [115]' '115' 'blocked_by #115 is OPEN'
check "[113, 114], both closed" 'blocked_by: [113, 114]' '113 114' ''
# Fail-closed: an entry that is still not a number once CR is gone blocks.
check "[abc] blocks" 'blocked_by: [abc]' 'abc' "blocked_by entry 'abc' is not an issue number"
check "[113, 12a] blocks on 12a" 'blocked_by: [113, 12a]' '113 12a' "blocked_by entry '12a' is not an issue number"
check "[999], unknown issue blocks" 'blocked_by: [999]' '999' 'blocked_by #999 is UNKNOWN'

if [ "$fail" -ne 0 ]; then
  exit 1
fi
echo "OK: a CRLF body's blocked_by parses as its LF form; a non-numeric entry still blocks"
