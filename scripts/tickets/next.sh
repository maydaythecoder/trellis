#!/usr/bin/env bash
# Prints the next ticket an agent may start: highest priority, oldest first.
#
#   scripts/tickets/next.sh         the next ticket, as "<number> <title>"
#   scripts/tickets/next.sh --why   every open ticket and why it is or is not next
#
# Eligibility is read from the issue itself, by the same rules as G0
# (scripts/gates/ticket-ready.sh), not from the `ready` label: a label the
# triage writer computed from an older body would let the queue and the gate
# disagree. On top of G0 it requires `triage:auto` and excludes what a person
# decides (docs/BUILD_SYSTEM.md, "Where a person is needed").
set -euo pipefail
why="${1:-}"

# gh's built-in --jq; unit separator between fields because `read` collapses
# runs of tab, which drops empty fields.
gh issue list --state open --limit 300 --json number,title,labels,body --jq '
  def prio: ([.labels[].name] | if index("P0") then 0 elif index("P1") then 1 else 2 end);
  sort_by(prio, .number)[] |
  [(.number | tostring), (prio | tostring), ([.labels[].name] | join(" ")),
   ([.body | scan("(?m)^blocked_by: \\[([^\\]]*)\\]") | .[0]] | join(" ")),
   (if (.body | test("(?m)^## Done when")) then "y" else "n" end),
   (if (.body | test("(?ms)^## Verify\\s*\\n+```")) then "y" else "n" end),
   .title] | join("\u001f")' |
while IFS=$'\x1f' read -r n p labels blocked done_when verify title; do
  reason=""
  case " $labels " in
    *" type:story "*) reason="a story, not a ticket" ;;
    *" split "*) reason="split into children" ;;
    *" triage:needs-human "*) reason="needs a person" ;;
    *" parked "*) reason="parked: waits on the owner's answer" ;;
    *" risk:high "*) reason="risk:high: a person decides" ;;
    *" surface:public "*) reason="surface:public: a person approves" ;;
    *" area:checks "*) reason="area:checks: the Verdict seal" ;;
    *" !security "*) reason="!security: a person reviews" ;;
    *" triage:auto "*) ;;
    # Blocked only on other issues: eligible once every blocked_by is closed
    # (checked below). With no blocked_by there is nothing a script can see
    # to wait for, so it stays out.
    *" triage:blocked "*) [ -n "$blocked" ] || reason="triage:blocked with no blocked_by" ;;
    *) reason="not triage:auto" ;;
  esac
  [ -z "$reason" ] && [ "$done_when" != y ] && reason="no '## Done when'"
  [ -z "$reason" ] && [ "$verify" != y ] && reason="no fenced '## Verify'"
  if [ -z "$reason" ]; then
    # A dependency that is not an issue number cannot be looked up, so it blocks.
    for b in $(echo "$blocked" | tr ',"' '  '); do
      case "$b" in *[!0-9]*) reason="blocked_by '$b' is not an issue number"; break ;; esac
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
