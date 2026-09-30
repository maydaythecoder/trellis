# Reads the `blocked_by: [...]` line of a ticket body. Sourced, not run; G0
# (scripts/gates/ticket-ready.sh) and scripts/checks/crlf-blocked-by.sh share it
# so the check exercises the parser the gate runs.
#
# No `set` options, no `cd` and no GitHub call happen at source time: the
# caller owns its shell options, and the check replaces issue_state.

# The issue's state as gh reports it (OPEN, CLOSED), or UNKNOWN when it cannot
# be read. The check redefines this after sourcing, so it never calls GitHub.
issue_state() {
  gh issue view "$1" --json state -q .state 2>/dev/null || echo UNKNOWN
}

# Body on stdin; prints the blocked_by entries separated by spaces.
# An issue body edited from Windows arrives CRLF (#205). The sed below is not
# anchored at `$`, so `blocked_by: [113]\r` left `113\r`, which is not in IFS
# and so reached the numeric test with the CR attached. Stripping CR first
# makes a CRLF body parse the same as its LF form.
blocked_by_entries() {
  tr -d '\r' | sed -n 's/^blocked_by: \[\(.*\)\]/\1/p' | tr ',"' '  '
}

# Body on stdin; prints one line per blocked_by entry that blocks, and nothing
# when every entry is a closed issue. An entry that is not an issue number
# blocks: a dependency that cannot be looked up is not a satisfied one.
blocked_by_problems() {
  local b s
  for b in $(blocked_by_entries); do
    case "$b" in
      *[!0-9]*) echo "blocked_by entry '$b' is not an issue number"; continue ;;
    esac
    s=$(issue_state "$b")
    [ "$s" = "CLOSED" ] || echo "blocked_by #$b is $s"
  done
}
