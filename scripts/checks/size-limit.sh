#!/usr/bin/env bash
# size-limit.sh: a file, or one function in it, is at most <max> lines.
#
#   scripts/checks/size-limit.sh <max> <file>...
#   scripts/checks/size-limit.sh --fn <name> <max> <file>
#   scripts/checks/size-limit.sh --self-test
#
# The Verify form for a refactor that splits a file or a function (#926):
# G6 runs each Verify line on its own and allows no shell composition, so
# "wc -l is under N" needs a script. A file's size is `wc -l`. A function's
# is counted from the line that declares `fn <name>(` (Rust) or
# `func <name>(` (Go) at column 0, after an optional `pub`/`pub(crate)`
# and `async`, to the first later line that is a lone `}` at column 0,
# which is where rustfmt and gofmt put a top-level function's closing brace.
# Methods inside an impl are not top-level and are not found.
#
# Exit 0: within the limit. Exit 1: over it, with the count printed.
# Exit 2: wrong arguments, a file that does not exist, or a function that
# is not found (a limit on something absent proves nothing).
set -uo pipefail

usage='usage: size-limit.sh <max> <file>... | --fn <name> <max> <file> | --self-test'

fn_lines() { # <name> <file>
  awk -v name="$1" '
    !start && $0 ~ "^(pub(\\([a-z]+\\))? )?(async )?(fn|func) " name "[(<]" { start = NR }
    start && NR > start && /^}$/ { print NR - start + 1; found = 1; exit }
    END { if (!found) exit 1 }' "$2"
}

if [ "${1:-}" = "--self-test" ]; then
  self="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
  tmp=$(mktemp -d) || exit 2
  trap 'rm -rf "$tmp"' EXIT
  printf 'a\nb\nc\n' > "$tmp/three.txt"
  printf 'fn small() {\n    1\n}\n\npub(crate) fn big(x: u8) {\n    let y = x;\n    if y > 0 {\n        y;\n    }\n}\n' > "$tmp/lib.rs"
  printf 'package p\n\nfunc Run(a int) {\n\t_ = a\n}\n' > "$tmp/p.go"
  fail=0
  expect() {
    local want=$1 got
    shift
    bash "$self" "$@" >/dev/null 2>&1
    got=$?
    [ "$got" = "$want" ] || { echo "FAIL: size-limit.sh $* exited $got, want $want"; fail=1; }
  }
  expect 0 3 "$tmp/three.txt"
  expect 1 2 "$tmp/three.txt"
  expect 1 3 "$tmp/three.txt" "$tmp/lib.rs"         # one file over fails the lot
  expect 0 --fn small 3 "$tmp/lib.rs"
  expect 1 --fn big 5 "$tmp/lib.rs"                  # 6 lines: the inner } is indented
  expect 0 --fn big 6 "$tmp/lib.rs"
  expect 0 --fn Run 3 "$tmp/p.go"
  expect 2 --fn absent 100 "$tmp/lib.rs"             # not found is not within limits
  expect 2 3 "$tmp/missing.txt"
  expect 2 x "$tmp/three.txt"
  expect 2
  [ "$fail" -eq 0 ] || exit 1
  echo "OK: self-test (file limits, Rust and Go top-level functions, absent function and bad arguments exit 2)"
  exit 0
fi

if [ "${1:-}" = "--fn" ]; then
  [ $# -eq 4 ] || { echo "$usage"; exit 2; }
  name=$2 max=$3 file=$4
  case "$max" in ''|*[!0-9]*) echo "$usage"; exit 2 ;; esac
  [ -f "$file" ] || { echo "FAIL: no file $file"; exit 2; }
  n=$(fn_lines "$name" "$file") || { echo "FAIL: no top-level fn $name in $file"; exit 2; }
  [ "$n" -le "$max" ] || { echo "FAIL: fn $name in $file is $n lines, over $max"; exit 1; }
  echo "OK: fn $name in $file is $n lines (limit $max)"
  exit 0
fi

[ $# -ge 2 ] || { echo "$usage"; exit 2; }
max=$1; shift
case "$max" in ''|*[!0-9]*) echo "$usage"; exit 2 ;; esac
over=0
for file in "$@"; do
  [ -f "$file" ] || { echo "FAIL: no file $file"; exit 2; }
  n=$(wc -l < "$file" | tr -d ' ')
  if [ "$n" -gt "$max" ]; then
    echo "FAIL: $file is $n lines, over $max"
    over=1
  fi
done
[ "$over" -eq 0 ] || exit 1
echo "OK: $# file(s) within $max lines"
