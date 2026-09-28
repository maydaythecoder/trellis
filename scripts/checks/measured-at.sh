#!/usr/bin/env bash
# measured-at.sh: a document names a commit at or after a base.
#
#   scripts/checks/measured-at.sh <file-or-prefix> <base> [<text> ...]
#   scripts/checks/measured-at.sh --self-test
#
# CLAUDE.md "Before claiming a number" wants the method and the commit beside
# every figure. This lets a Verify line hold a document to that: it passes
# only when the document names a commit C with <base> <= C <= HEAD, and
# contains every <text> (case-insensitive). It proves the figure was
# re-measured after <base>; it says nothing about whether the figure is good.
#
# Exit 0: holds. Exit 1: does not, with the failed condition printed.
# Exit 2: wrong arguments.
set -uo pipefail

usage() {
  echo "usage: measured-at.sh <file-or-prefix> <base> [<text> ...] | --self-test"
  exit 2
}

self_test() {
  local self fails=0 a b c
  self="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
  # Global, so the EXIT trap still sees it after this function returns.
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  # The throwaway repository must not pick up the user's hooks, signing or
  # identity; a self-test that depends on ~/.gitconfig is not a test of this.
  export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null
  local g=(git -C "$tmp" -c user.name=t -c user.email=t@example.invalid -c commit.gpgsign=false)
  "${g[@]}" init -q || { echo "FAIL: git init"; exit 1; }
  for n in a b c; do
    echo "$n" > "$tmp/$n.txt"
    "${g[@]}" add "$n.txt" && "${g[@]}" commit -q -m "$n" || { echo "FAIL: commit $n"; exit 1; }
  done
  c=$("${g[@]}" rev-parse HEAD)
  b=$("${g[@]}" rev-parse HEAD~1)
  a=$("${g[@]}" rev-parse HEAD~2)

  mkdir -p "$tmp/notes"
  printf 'Measured at %s.\n' "${a:0:7}" > "$tmp/notes/only-a.md"
  printf 'Line coverage 80%%, measured at `%s`.\n' "${c:0:9}" > "$tmp/notes/c.md"
  printf 'Hashes that are not commits: deadbeef1234 0123456789abcdef.\n' > "$tmp/notes/nohash.md"
  # Prefix mode: m-1 names a commit before the base, m-2 one after it, so
  # only picking the lexicographically last file passes.
  printf 'at %s\n' "$a" > "$tmp/notes/m-1.md"
  printf 'at %s\n' "$c" > "$tmp/notes/m-2.md"

  expect() { # <want-exit> <name> <args...>
    local want="$1" name="$2" got
    shift 2
    (cd "$tmp" && "$self" "$@") > "$tmp/out" 2>&1
    got=$?
    if [ "$got" = "$want" ]; then
      echo "ok   $name (exit $got)"
    else
      echo "FAIL $name: want exit $want, got $got"
      sed 's/^/     /' "$tmp/out"
      fails=$((fails + 1))
    fi
  }
  expect 1 "names only A, base B" notes/only-a.md "$b"
  expect 0 "names C, base B" notes/c.md "$b"
  expect 0 "names C, base B, text present in other case" notes/c.md "$b" "LINE COVERAGE"
  expect 1 "names C, base B, required text missing" notes/c.md "$b" "Line coverage" "mutation score"
  expect 1 "only hex tokens are not commits" notes/nohash.md "$a"
  expect 0 "prefix picks the lexicographically last file" notes/m- "$b"
  expect 1 "prefix matches nothing" notes/zz "$b"
  expect 2 "base is not a commit" notes/c.md deadbeef1234
  expect 2 "missing base argument" notes/c.md

  if [ "$fails" -ne 0 ]; then
    echo "FAIL: $fails self-test case(s)"
    exit 1
  fi
  echo "OK: measured-at.sh self-test"
  exit 0
}

[ "${1:-}" = "--self-test" ] && { [ $# -eq 1 ] || usage; self_test; }
[ $# -ge 2 ] || usage
target="$1" base_arg="$2"
shift 2
[ -n "$target" ] && [ -n "$base_arg" ] || usage

ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || { echo "not inside a git repository"; exit 2; }
# The prefix is given relative to where the caller stands; ls-files paths
# are relative to the root, where every git call below runs.
cwd_prefix="$(git rev-parse --show-prefix)"

if [ -f "$target" ]; then
  file="$(cd "$(dirname "$target")" && pwd)/$(basename "$target")"
  cd "$ROOT" || exit 2
else
  cd "$ROOT" || exit 2
  want="$cwd_prefix$target"
  # Tracked and untracked-but-not-ignored: a measurement document is often
  # written before it is committed. LC_ALL=C makes "last" byte order, not
  # the locale's collation.
  rel=$(git ls-files -z --cached --others --exclude-standard \
    | tr '\0' '\n' | awk -v p="$want" 'index($0, p) == 1' | LC_ALL=C sort -u | tail -n 1)
  if [ -z "$rel" ] || [ ! -f "$rel" ]; then
    echo "no file matches $target"
    exit 1
  fi
  file="$ROOT/$rel"
fi

base="$(git rev-parse --verify --quiet "$base_arg^{commit}")" || {
  echo "base '$base_arg' is not a commit"
  usage
}

found=""
for tok in $(grep -oE '\b[0-9a-f]{7,40}\b' "$file" | sort -u); do
  c="$(git rev-parse --verify --quiet "$tok^{commit}")" || continue
  if git merge-base --is-ancestor "$base" "$c" && git merge-base --is-ancestor "$c" HEAD; then
    found="$tok"
    break
  fi
done

status=0
if [ -z "$found" ]; then
  echo "FAIL: $file names no commit at or after $base_arg and at or before HEAD"
  status=1
fi
# awk rather than `grep -iF`: Git for Windows' grep aborts on -i -F in some
# locales, and an aborted grep would read as "text missing". ENVIRON, not -v,
# so a backslash in the text is not taken as an escape.
has_text() {
  NEEDLE="$1" awk 'BEGIN { n = tolower(ENVIRON["NEEDLE"]) } index(tolower($0), n) { f = 1; exit } END { exit !f }' "$file"
}
for text in "$@"; do
  if ! has_text "$text"; then
    echo "FAIL: $file does not contain \"$text\""
    status=1
  fi
done
[ "$status" -eq 0 ] && echo "OK: $file names $found (at or after $base_arg)"
exit "$status"
