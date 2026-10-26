#!/usr/bin/env bash
# G0: the ticket is workable. It has a done-condition a stranger can check, a
# Verify command that proves it, and nothing blocking it.
source "$(dirname "$0")/lib.sh"
source "$ROOT/scripts/tickets/blocked-by.sh"
n="${1:?usage: ticket-ready.sh <issue-number>}"

# gh's built-in --jq, so the gates need nothing beyond gh, git, cargo and
# python. blocked-by.sh has sourced scripts/tickets/issues.sh (REST, #924).
state=$(issue_state_rest "$n") || fail "cannot read issue #$n"
# A body edited from Windows is CRLF; every test below reads it as LF (#205).
body=$(issue_body "$n" | tr -d '\r')
labels=$(issue_labels "$n")

problems=()
[ "$state" = "OPEN" ] || problems+=("issue is $state")
printf '%s' "$body" | grep -qE '^## Done when' || problems+=("no '## Done when' section")
printf '%s' "$body" | awk '/^## Verify/{v=1; next} v && /^```/{print "fence"; exit}' | grep -q fence \
  || problems+=("no fenced command under '## Verify'")
# triage:blocked waits on its blocked_by (checked below), so it clears when
# they close; with no blocked_by there is nothing to wait for that a gate can see.
case " $labels " in *" triage:blocked "*)
  printf '%s' "$body" | grep -qE '^blocked_by: \[[^]]' || problems+=("labelled triage:blocked with no blocked_by") ;;
esac
case " $labels " in *" parked "*) problems+=("parked: waits on the owner's answer") ;; esac
case " $labels " in *" triage:needs-human "*) problems+=("labelled triage:needs-human: a person routes this one") ;; esac
case " $labels " in *" split "*) problems+=("split: work happens in its child tickets") ;; esac

# Every blocked_by in the generated triage block must be closed
# (scripts/tickets/blocked-by.sh; a non-numeric entry blocks).
while IFS= read -r p; do
  if [ -n "$p" ]; then problems+=("$p"); fi
done < <(printf '%s' "$body" | blocked_by_problems)

if [ ${#problems[@]} -gt 0 ]; then
  echo "FAIL: #$n is not ready:"
  printf '  - %s\n' "${problems[@]}"
  exit 1
fi
echo "OK: #$n has Done when, Verify, and nothing blocking it"
