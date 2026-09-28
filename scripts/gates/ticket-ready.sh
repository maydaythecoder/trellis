#!/usr/bin/env bash
# G0: the ticket is workable. It has a done-condition a stranger can check, a
# Verify command that proves it, and nothing blocking it.
source "$(dirname "$0")/lib.sh"
n="${1:?usage: ticket-ready.sh <issue-number>}"

# gh's built-in --jq, so the gates need nothing beyond gh, git, cargo and python.
state=$(gh issue view "$n" --json state -q .state) || fail "cannot read issue #$n"
body=$(gh issue view "$n" --json body -q .body)
labels=$(gh issue view "$n" --json labels -q '[.labels[].name] | join(" ")')

problems=()
[ "$state" = "OPEN" ] || problems+=("issue is $state")
printf '%s' "$body" | grep -qE '^## Done when' || problems+=("no '## Done when' section")
printf '%s' "$body" | awk '/^## Verify/{v=1; next} v && /^```/{print "fence"; exit}' | grep -q fence \
  || problems+=("no fenced command under '## Verify'")
case " $labels " in *" triage:blocked "*) problems+=("labelled triage:blocked") ;; esac
case " $labels " in *" triage:needs-human "*) problems+=("labelled triage:needs-human: a person routes this one") ;; esac
case " $labels " in *" split "*) problems+=("split: work happens in its child tickets") ;; esac

# Every blocked_by in the generated triage block must be closed.
# An entry that is not an issue number blocks: a dependency the gate cannot
# look up is not a satisfied one.
for b in $(printf '%s' "$body" | sed -n 's/^blocked_by: \[\(.*\)\]/\1/p' | tr ',"' '  '); do
  case "$b" in
    *[!0-9]*) problems+=("blocked_by entry '$b' is not an issue number"); continue ;;
  esac
  s=$(gh issue view "$b" --json state -q .state 2>/dev/null || echo UNKNOWN)
  [ "$s" = "CLOSED" ] || problems+=("blocked_by #$b is $s")
done

if [ ${#problems[@]} -gt 0 ]; then
  echo "FAIL: #$n is not ready:"
  printf '  - %s\n' "${problems[@]}"
  exit 1
fi
echo "OK: #$n has Done when, Verify, and nothing blocking it"
