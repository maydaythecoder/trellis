#!/usr/bin/env bash
# skill-lines.sh: no SKILL.md committed to this repository runs past 500 lines.
#
#   scripts/checks/skill-lines.sh               check every tracked SKILL.md
#   scripts/checks/skill-lines.sh --self-test   prove the check bites at 501
#
# history/TRELLIS-direction.md keeps a skill at 500 lines or fewer. ADR 0019
# caps only the disclosed list, so nothing else holds a single SKILL.md to
# that. This is a check on this repository's own skills, not a run-time
# rule: a long skill in a user's repository is already bounded in the prompt
# by the disclosure cap and read_skill's 8,000-character window.
#
# Exit 0: every file is within the cap. Exit 1: at least one is over, each
# printed with its line count, or the self-test failed. Exit 2: wrong
# arguments, or git could not list the files.
set -uo pipefail

MAX_LINES=500

# check_files <file>...: prints each file over MAX_LINES with its count and
# returns 1 if there was one. awk's NR counts a final line with no trailing
# newline, which `wc -l` would miss, so a 501st line cannot hide there.
check_files() {
  local f n over=0
  for f in "$@"; do
    n=$(awk 'END { print NR }' "$f") || { echo "FAIL: cannot read $f"; over=1; continue; }
    if [ "$n" -gt "$MAX_LINES" ]; then
      echo "FAIL: $f has $n lines (cap $MAX_LINES)"
      over=1
    fi
  done
  return "$over"
}

self_test() {
  local out rc
  # Global, not local: the EXIT trap runs after this function's scope is gone.
  tmp=$(mktemp -d) || { echo "FAIL: self-test could not create a temporary directory"; return 1; }
  # Removed on every exit path, including a failed assertion below; the
  # script exits as soon as the self-test returns.
  trap 'rm -rf "$tmp"' EXIT
  mkdir -p "$tmp/at-cap" "$tmp/over-cap"
  awk 'BEGIN { for (i = 1; i <= 500; i++) print "line " i }' > "$tmp/at-cap/SKILL.md"
  awk 'BEGIN { for (i = 1; i <= 501; i++) print "line " i }' > "$tmp/over-cap/SKILL.md"

  out=$(check_files "$tmp/at-cap/SKILL.md" "$tmp/over-cap/SKILL.md"); rc=$?
  if [ "$rc" -ne 1 ]; then
    echo "FAIL: self-test: the check returned $rc with a 501-line SKILL.md present, want 1"
    return 1
  fi
  if ! printf '%s\n' "$out" | grep -qF "$tmp/over-cap/SKILL.md has 501 lines"; then
    echo "FAIL: self-test: the check did not name the 501-line file; it printed:"
    printf '%s\n' "$out"
    return 1
  fi
  if printf '%s\n' "$out" | grep -qF "$tmp/at-cap/SKILL.md"; then
    echo "FAIL: self-test: the check named the 500-line file; it printed:"
    printf '%s\n' "$out"
    return 1
  fi

  out=$(check_files "$tmp/at-cap/SKILL.md"); rc=$?
  if [ "$rc" -ne 0 ]; then
    echo "FAIL: self-test: the check failed a 500-line SKILL.md alone; it printed:"
    printf '%s\n' "$out"
    return 1
  fi
  echo "OK: self-test: 501 lines fails and is named, 500 lines passes"
}

case "$#:${1:-}" in
  0:) ;;
  1:--self-test) self_test; exit $? ;;
  *) echo "usage: skill-lines.sh [--self-test]"; exit 2 ;;
esac

# Checked first: the loop below cannot see git's exit status, and "not in a
# repository" must not read as "no skills".
git rev-parse --is-inside-work-tree >/dev/null 2>&1 || { echo "FAIL: not in a git repository"; exit 2; }
# The index, not the disk: an untracked or ignored SKILL.md is not committed.
# -z so a path with a space or a non-ASCII byte arrives unquoted and whole.
files=()
while IFS= read -r -d '' f; do
  case "$f" in SKILL.md | */SKILL.md) files+=("$f") ;; esac
done < <(git ls-files -z)

if [ "${#files[@]}" -eq 0 ]; then
  echo "OK: no tracked SKILL.md"
  exit 0
fi
check_files "${files[@]}" || exit 1
echo "OK: ${#files[@]} tracked SKILL.md file(s), none over $MAX_LINES lines"
