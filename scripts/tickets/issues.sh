# Reads issues through GitHub's REST API (`gh api`). Sourced, not run.
#
# `gh issue view` and `gh issue list` go through GraphQL, which Claude Code
# cloud sessions refuse (HTTP 403), so a gate that read issues that way could
# not pass in the session an unattended runner works in (#924). REST works on
# the owner's host too, so this is the only path.
#
# No `set` options and no GitHub call happen at source time: the caller owns
# its shell options. `{owner}/{repo}` is filled in by gh from the git remote.

# OPEN or CLOSED, upper-cased as GraphQL reported it so callers compare as
# before. Fails (non-zero, nothing printed) when the issue cannot be read.
issue_state_rest() {
  gh api "repos/{owner}/{repo}/issues/$1" --jq '.state | ascii_upcase'
}

issue_body() {
  gh api "repos/{owner}/{repo}/issues/$1" --jq '.body // ""'
}

# Label names joined by single spaces.
issue_labels() {
  gh api "repos/{owner}/{repo}/issues/$1" --jq '[.labels[].name] | join(" ")'
}

# Applies the jq filter in $1 to each open issue (number, title, labels, body)
# and prints what it emits. gh runs the filter once per page, so it sees one
# issue at a time: sort its output, not the issues. The REST list holds pull
# requests too; they are dropped.
open_issues_each() {
  gh api --paginate "repos/{owner}/{repo}/issues?state=open&per_page=100" \
    --jq ".[] | select(.pull_request | not) | .body |= (. // \"\") | $1"
}
