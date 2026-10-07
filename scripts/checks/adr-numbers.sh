#!/usr/bin/env bash
# adr-numbers.sh: every ADR has a number of its own (#284).
#
#   scripts/checks/adr-numbers.sh [<base>]
#   scripts/checks/adr-numbers.sh --self-test
#
# Exit 1, naming each file and the rule it breaks, when two docs/adr files
# share a number, when a file's first line does not begin "# ADR NNNN " with
# its own number, when a held number appears under another slug (or a held
# slug under another number), or, given <base>, when a file this change adds
# takes a number <base>'s tree already holds under another name: a branch cut
# before another ADR merged does not see it in its own tree. Exit 0
# otherwise; numbers need not be contiguous (there is no 0030). Exit 2 on
# wrong arguments or outside a git repository.
set -uo pipefail

# Numbers the 2026-10-04 upstream plan and #239 have taken, one per line:
# "<number> <slug>" and the ticket that writes the ADR. A ticket that closes
# without writing its ADR drops its line in the same commit.
HELD="
0046 rollback-under-a-clock-that-steps-back #239
0047 colibri-as-a-model-runtime COL-01a
0048 a-tauri-desktop-interface DESK-01
0049 model-writes-stay-out-of-the-vault APEX-08
0052 two-rpc-clients-and-go-for-the-tui #500
"

usage() {
  echo "usage: adr-numbers.sh [<base>] | --self-test"
  exit 2
}

check() { # [<base>]; run from the repository root
  local base="${1:-}" fails=0 f name num slug first other held_slug held_num
  local dir=docs/adr
  [ -d "$dir" ] || { echo "OK: no $dir"; return 0; }
  declare -A seen=()
  for f in "$dir"/[0-9][0-9][0-9][0-9]-*.md; do
    [ -e "$f" ] || continue
    name=$(basename "$f" .md)
    num=${name%%-*}
    slug=${name#*-}
    if [ -n "${seen[$num]:-}" ]; then
      echo "FAIL $f: number $num is also ${seen[$num]}"
      fails=$((fails + 1))
    else
      seen[$num]=$f
    fi
    first=$(head -n 1 "$f" | tr -d '\r')
    case "$first" in
      "# ADR $num "*) ;;
      *) echo "FAIL $f: first line does not begin '# ADR $num ': $first"; fails=$((fails + 1)) ;;
    esac
    held_slug=$(printf '%s\n' "$HELD" | awk -v n="$num" '$1 == n { print $2 }')
    if [ -n "$held_slug" ] && [ "$held_slug" != "$slug" ]; then
      echo "FAIL $f: $num is held for $held_slug"
      fails=$((fails + 1))
    fi
    held_num=$(printf '%s\n' "$HELD" | awk -v s="$slug" '$2 == s { print $1 }')
    if [ -n "$held_num" ] && [ "$held_num" != "$num" ]; then
      echo "FAIL $f: $slug is held at $held_num"
      fails=$((fails + 1))
    fi
  done
  if [ -n "$base" ]; then
    git rev-parse --verify --quiet "$base^{commit}" > /dev/null || { echo "no such base: $base"; exit 2; }
    while IFS= read -r f; do
      [ -n "$f" ] || continue
      name=$(basename "$f" .md)
      num=${name%%-*}
      other=$(git ls-tree --name-only "$base" -- "$dir/" | awk -F/ '{print $NF}' |
        grep -E "^$num-" | grep -vx "$name.md" | head -n 1)
      if [ -n "$other" ]; then
        echo "FAIL $f: $base already holds $num as $other"
        fails=$((fails + 1))
      fi
    done < <(git diff --name-only --diff-filter=A "$base"...HEAD -- "$dir/" | grep -E '/[0-9]{4}-[^/]*\.md$')
  fi
  if [ "$fails" -gt 0 ]; then
    return 1
  fi
  echo "OK: ADR numbers unique and held ($(printf '%s\n' "$HELD" | awk 'NF { printf "%s%s", sep, $1; sep=" " }') held)"
}

self_test() {
  local self fails=0 tmp
  self="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
  tmp="$(mktemp -d)"
  trap 'rm -rf "$tmp"' EXIT
  export GIT_CONFIG_NOSYSTEM=1 GIT_CONFIG_GLOBAL=/dev/null
  local g=(git -C "$tmp" -c user.name=t -c user.email=t@example.invalid -c commit.gpgsign=false)
  "${g[@]}" init -q -b main || { echo "FAIL: git init"; exit 1; }
  mkdir -p "$tmp/docs/adr"
  adr() { printf '# ADR %s - %s\n\nbody\n' "${1%%-*}" "${1#*-}" > "$tmp/docs/adr/$1.md"; }
  reset() { rm -f "$tmp"/docs/adr/*.md; }
  expect() { # <want-exit> <name> <want-text> [args]
    local want="$1" name="$2" text="$3" got
    shift 3
    (cd "$tmp" && "$self" "$@") > "$tmp/out" 2>&1
    got=$?
    if [ "$got" = "$want" ] && grep -q -- "$text" "$tmp/out"; then
      echo "ok   $name (exit $got)"
    else
      echo "FAIL $name: want exit $want and '$text', got $got:"
      sed 's/^/     /' "$tmp/out"
      fails=$((fails + 1))
    fi
  }

  reset; adr 0046-rollback-under-a-clock-that-steps-back; adr 0046-a-tauri-desktop-interface
  expect 1 "two files, one number" "is also"
  reset; adr 0045-work-units; printf '# ADR 0044 - wrong\n' > "$tmp/docs/adr/0045-work-units.md"
  expect 1 "first line names another number" "first line"
  reset; adr 0047-model-writes-stay-out-of-the-vault
  expect 1 "a held slug at another number" "is held at 0049"
  reset; adr 0046-a-tauri-desktop-interface
  expect 1 "a held number under another slug" "is held for rollback"
  reset; adr 0046-rollback-under-a-clock-that-steps-back; adr 0047-colibri-as-a-model-runtime
  adr 0048-a-tauri-desktop-interface; adr 0049-model-writes-stay-out-of-the-vault; adr 0031-x
  expect 0 "each held ADR at its number, and no 0030" "OK"

  reset; adr 0045-base; "${g[@]}" add -A && "${g[@]}" commit -qm base
  "${g[@]}" checkout -q -b branch
  adr 0050-a; "${g[@]}" add -A && "${g[@]}" commit -qm a
  "${g[@]}" checkout -q main
  adr 0050-b; "${g[@]}" add -A && "${g[@]}" commit -qm b
  "${g[@]}" checkout -q branch
  expect 1 "a branch adds a number its base gained after the branch point" "main already holds 0050" main
  expect 2 "an unknown base" "no such base" no-such-ref

  [ "$fails" -eq 0 ] && echo "OK: self-test" && exit 0
  echo "FAIL: $fails self-test case(s)"
  exit 1
}

case "${1:-}" in
  --self-test) self_test ;;
  -*) usage ;;
esac
[ $# -le 1 ] || usage
root=$(git rev-parse --show-toplevel 2> /dev/null) || { echo "not in a git repository"; exit 2; }
cd "$root" || exit 2
check "${1:-}"
