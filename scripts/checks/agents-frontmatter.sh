#!/usr/bin/env bash
# agents-frontmatter.sh: every agent in .claude/agents/ says what it is,
# what it may use and which model runs it, and none re-reads CLAUDE.md.
#
#   scripts/checks/agents-frontmatter.sh [--self-test]
#
# An agent without `model:` inherits the session's model and effort, so a
# gate-runner that only reports output ran on the most expensive model there
# is (Claude-setup audit, 2026-10-04, #387). And every subagent already has
# CLAUDE.md in its context; an instruction to read it again loads it twice.
#
# Exit 0: every file has `name:`, `description:`, `tools:` and `model:` in
# its frontmatter and no "Read `CLAUDE.md`". Exit 1: each failure is named.
set -uo pipefail

check_dir() {
  local dir=$1 bad=0 f key
  shopt -s nullglob
  local files=("$dir"/*.md)
  [ ${#files[@]} -gt 0 ] || { echo "FAIL: no agent files in $dir"; return 1; }
  for f in "${files[@]}"; do
    # The frontmatter is the block between the first two `---` lines.
    local head
    head=$(awk 'NR==1 && $0!="---" {exit} NR>1 && $0=="---" {exit} NR>1 {print}' "$f")
    for key in name description tools model; do
      if ! printf '%s\n' "$head" | grep -q "^$key:"; then
        echo "FAIL: $f has no '$key:' in its frontmatter"; bad=1
      fi
    done
    if grep -qE 'Read `?CLAUDE\.md`?' "$f"; then
      echo "FAIL: $f tells the agent to read CLAUDE.md, which it already has"; bad=1
    fi
  done
  return $bad
}

if [ "${1:-}" = "--self-test" ]; then
  tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
  fails=0
  mkdir -p "$tmp/good" "$tmp/nomodel" "$tmp/reread"
  printf -- '---\nname: a\ndescription: d\ntools: Read\nmodel: haiku\n---\nbody\n' > "$tmp/good/a.md"
  printf -- '---\nname: a\ndescription: d\ntools: Read\n---\nmodel: in the body does not count\n' > "$tmp/nomodel/a.md"
  printf -- '---\nname: a\ndescription: d\ntools: Read\nmodel: opus\n---\nRead `CLAUDE.md` first.\n' > "$tmp/reread/a.md"
  check_dir "$tmp/good" >/dev/null || { echo "self-test: a complete agent was refused"; fails=1; }
  check_dir "$tmp/nomodel" >/dev/null && { echo "self-test: a missing model passed"; fails=1; }
  check_dir "$tmp/reread" >/dev/null && { echo "self-test: a CLAUDE.md re-read passed"; fails=1; }
  [ $fails -eq 0 ] && echo "agents-frontmatter self-test: ok"
  exit $fails
fi

root=$(git rev-parse --show-toplevel) || exit 2
if check_dir "$root/.claude/agents"; then
  echo "OK: every agent names its model and tools, and none re-reads CLAUDE.md"
else
  exit 1
fi
