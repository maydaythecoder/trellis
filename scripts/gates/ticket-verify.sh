#!/usr/bin/env bash
# G6: the ticket's own Verify command passes.
#
# It runs text from an issue body, which is the arbitrary-command surface the
# PR #1 review rounds could not close, so it is fenced: every line must start
# with an allowed program, provider keys are already removed (lib.sh), and it
# runs from the repository root.
source "$(dirname "$0")/lib.sh"
n="${1:?usage: ticket-verify.sh <issue-number>}"

cmd=$(gh issue view "$n" --json body -q .body \
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
done <<< "$cmd"

echo "running #$n Verify:"
printf '%s\n' "$cmd" | sed 's/^/  /'
bash -euo pipefail -c "$cmd"
echo "OK: #$n Verify passed"
