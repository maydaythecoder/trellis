#!/usr/bin/env bash
# G6: the ticket's own Verify command passes.
#
# It runs text from an issue body, which is the arbitrary-command surface the
# PR #1 review rounds could not close, so it is fenced: every line must start
# with an allowed program, provider keys are already removed (lib.sh), and it
# runs from the repository root.
source "$(dirname "$0")/lib.sh"
n="${1:?usage: ticket-verify.sh <issue-number>}"

# shellcheck source=../tickets/issues.sh
source "$ROOT/scripts/tickets/issues.sh"
cmd=$(issue_body "$n" \
  | awk '/^## Verify/{v=1; next} v && /^```/{ if (in_) exit; in_=1; next } in_{print}')
[ -n "$cmd" ] || fail "#$n has no fenced command under '## Verify'"

while IFS= read -r line; do
  l="${line#"${line%%[![:space:]]*}"}"
  [ -z "$l" ] && continue
  case "$l" in
    \#*) continue ;;
    cargo\ *|git\ *|scripts/*|"py -3 qa/"*|"python3 qa/"*) ;;
    *) fail "#$n Verify line not allowed (cargo, git, scripts/, py -3 qa/, python3 qa/ only): $l" ;;
  esac
  case "$l" in *'$('*|*'`'*|*'|'*|*';'*|*'&'*|*'>'*|*'<'*)
    fail "#$n Verify line uses shell composition; put it in a script under scripts/: $l" ;;
  esac
  # `scripts/../x` matches the scripts/ prefix and runs anything.
  case "$l" in *..*) fail "#$n Verify line contains '..': $l" ;; esac
done <<< "$cmd"

# A ticket may say `python3 qa/` or `py -3 qa/`; which one exists depends on
# the machine (python3 is a Store stub on Windows), so both run as $PY.
if printf '%s\n' "$cmd" | grep -qE '^[[:space:]]*(python3|py -3) '; then
  [ -n "$PY" ] || fail "#$n Verify needs Python and none is on PATH"
  cmd=$(printf '%s\n' "$cmd" | sed -E "s/^([[:space:]]*)(python3|py -3) /\\1$PY /")
fi

echo "running #$n Verify:"
printf '%s\n' "$cmd" | sed 's/^/  /'
bash -euo pipefail -c "$cmd"
echo "OK: #$n Verify passed"
