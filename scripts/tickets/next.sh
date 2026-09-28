#!/usr/bin/env bash
# Prints the next ticket for the orchestrator: highest priority, oldest first,
# labelled `ready`, not blocked, not needing a person, not split, not a story,
# and every `blocked_by` in its triage block closed.
#
#   scripts/tickets/next.sh         the next ticket, as "<number> <title>"
#   scripts/tickets/next.sh --why   every open ready issue and why it is or is not next
set -euo pipefail
why="${1:-}"

# gh's built-in --jq; unit separator between fields because `read` collapses
# runs of tab, which drops empty fields.
gh issue list --state open --label ready --limit 200 --json number,title,labels,body --jq '
  def prio: ([.labels[].name] | if index("P0") then 0 elif index("P1") then 1 else 2 end);
  sort_by(prio, .number)[] |
  [(.number | tostring), (prio | tostring), ([.labels[].name] | join(" ")),
   ([.body | scan("blocked_by: \\[([0-9, ]*)\\]") | .[0]] | join(" ")),
   .title] | join("\u001f")' |
while IFS=$'\x1f' read -r n p labels blocked title; do
  reason=""
  case " $labels " in
    *" triage:blocked "*) reason="triage:blocked" ;;
    *" triage:needs-human "*) reason="needs a person" ;;
    *" split "*) reason="split into children" ;;
    *" type:story "*) reason="a story, not a ticket" ;;
  esac
  if [ -z "$reason" ]; then
    for b in $(echo "$blocked" | tr ',' ' '); do
      s=$(gh issue view "$b" --json state -q .state 2>/dev/null || echo UNKNOWN)
      [ "$s" = "CLOSED" ] || { reason="blocked by #$b ($s)"; break; }
    done
  fi
  if [ "$why" = "--why" ]; then
    printf 'P%s #%s %s: %s\n' "$p" "$n" "${reason:-ELIGIBLE}" "$title"
  elif [ -z "$reason" ]; then
    printf '%s %s\n' "$n" "$title"
    exit 0
  fi
done
