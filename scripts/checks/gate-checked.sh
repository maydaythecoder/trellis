#!/usr/bin/env bash
# gate-checked.sh: a milestone's exit gate is ticked, and every tick names
# the test that proves it.
#
#   scripts/checks/gate-checked.sh <doc> <n>
#   scripts/checks/gate-checked.sh --self-test
#
# A ticked box (`- [x]`) is a claim, and a claim with nothing behind it is
# how a gate gets checked off by hand and then quietly stops being true. So
# each ticked line must name, in backticks, at least one test, and each test
# it names must exist: some tracked .rs file holds `fn <name>(` (git grep,
# over the whole workspace, wherever this is run from). Only a backtick span
# that is a bare identifier is read as a test name, so a ticked item may
# still quote a command such as `trellis doctor` or a flag; write anything
# else (an ADR number, an issue) in plain prose. At least <n> boxes must be
# ticked. <doc> is relative to the current directory.
#
# Exit 0: at least <n> ticked, and every ticked box names tests that exist.
# Exit 1: too few ticked, a ticked box names no test, or a named test does
# not exist; each is printed. Exit 2: wrong arguments, an unreadable <doc>,
# or not in a git repository.
set -uo pipefail

usage='usage: gate-checked.sh <doc> <n> | --self-test'
# Git Bash rewrites a path-looking argument otherwise.
export MSYS_NO_PATHCONV=1

if [ "${1:-}" = "--self-test" ]; then
  [ $# -eq 1 ] || { echo "$usage"; exit 2; }
  self="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
  # The planted docs live outside the repository, but their test names are
  # looked up in it, so every case runs from here.
  git rev-parse --show-toplevel >/dev/null 2>&1 || { echo "FAIL: not in a git repository"; exit 2; }
  tmp=$(mktemp -d) || { echo "FAIL: mktemp failed"; exit 2; }
  trap 'rm -rf "$tmp"' EXIT
  # One function that does exist (what the check looks for), found rather
  # than hard-coded so a rename elsewhere cannot break this. And one that
  # does not: built here, so no source file (this one included) defines it.
  real=$(git grep -h -o -E 'fn [a-z][a-z_0-9]*\(' -- '*.rs' | head -1)
  real=${real#fn }
  real=${real%(}
  [ -n "$real" ] || { echo "FAIL: found no test in the workspace to plant"; exit 2; }
  missing="planted_missing_test_$$_$RANDOM"
  bt='`'
  {
    printf '## Exit gate\n\n'
    printf -- '- [x] One item. %s%s%s\n' "$bt" "$real" "$bt"
    printf -- '- [x] A command, `trellis doctor`, beside its test %s%s%s.\n' "$bt" "$real" "$bt"
    printf -- '- [ ] Not yet; names nothing.\n'
  } > "$tmp/good.md"
  cp "$tmp/good.md" "$tmp/missing.md"
  printf -- '- [x] Planted. %s%s%s and %s%s%s\n' "$bt" "$real" "$bt" "$bt" "$missing" "$bt" >> "$tmp/missing.md"
  cp "$tmp/good.md" "$tmp/bare.md"
  printf -- '- [X] Ticked, but only a command: `trellis doctor`.\n' >> "$tmp/bare.md"
  # A CRLF checkout must read the same as an LF one.
  sed 's/$/\r/' "$tmp/good.md" > "$tmp/crlf.md"
  fail=0
  run() { bash "$self" "$@" >/dev/null 2>&1; echo $?; }
  expect() {
    local want=$1 got
    shift
    got=$(run "$@")
    [ "$got" = "$want" ] || { echo "FAIL: gate-checked.sh $* exited $got, want $want"; fail=1; }
  }
  expect 0 "$tmp/good.md" 2
  expect 0 "$tmp/crlf.md" 2
  expect 1 "$tmp/good.md" 3            # too few ticked
  expect 1 "$tmp/missing.md" 3         # a ticked box names a missing test
  expect 1 "$tmp/bare.md" 3            # a ticked box names no test
  expect 2 "$tmp/good.md"              # one argument
  expect 2 "$tmp/good.md" two          # <n> not a number
  expect 2 "$tmp/absent.md" 1          # no such doc
  out=$(bash "$self" "$tmp/missing.md" 3 2>&1)
  case "$out" in
    *"$missing"*) ;;
    *) echo "FAIL: the missing test is not named: $out"; fail=1 ;;
  esac
  [ "$fail" -eq 0 ] || exit 1
  echo "OK: self-test (real tests pass, CRLF too; a planted missing test fails and is named; too few ticks and a tick with no test fail; bad args exit 2)"
  exit 0
fi

[ $# -eq 2 ] || { echo "$usage"; exit 2; }
doc=$1
want=$2
case "$want" in
  ''|*[!0-9]*) echo "$usage"; exit 2 ;;
esac
[ -f "$doc" ] && [ -r "$doc" ] || { echo "FAIL: cannot read $doc"; exit 2; }
root=$(git rev-parse --show-toplevel 2>/dev/null) || { echo "FAIL: not in a git repository"; exit 2; }

span='`([^`]*)`'
ticked=0
bad=0
lineno=0
while IFS= read -r line || [ -n "$line" ]; do
  lineno=$((lineno + 1))
  line=${line%$'\r'}
  case "$line" in
    '- [x] '*|'- [X] '*) ;;
    *) continue ;;
  esac
  ticked=$((ticked + 1))
  named=0
  rest=$line
  while [[ $rest =~ $span ]]; do
    name=${BASH_REMATCH[1]}
    rest=${rest#*"${BASH_REMATCH[0]}"}
    [[ $name =~ ^[A-Za-z_][A-Za-z0-9_]*$ ]] || continue
    named=$((named + 1))
    # The `(` keeps a name from matching a longer test it is a prefix of.
    git -C "$root" grep -q -F -e "fn $name(" -- '*.rs'
    rc=$?
    if [ "$rc" -gt 1 ]; then
      echo "FAIL: git grep failed ($rc)"
      exit 2
    fi
    if [ "$rc" -ne 0 ]; then
      echo "FAIL: $doc:$lineno names \`$name\`, and no test of that name exists"
      bad=1
    fi
  done
  if [ "$named" -eq 0 ]; then
    echo "FAIL: $doc:$lineno is ticked but names no test in backticks"
    bad=1
  fi
done < "$doc"

if [ "$ticked" -lt "$want" ]; then
  echo "FAIL: $doc has $ticked ticked box(es), want at least $want"
  bad=1
fi
[ "$bad" -eq 0 ] || exit 1
echo "OK: $doc has $ticked ticked box(es), each naming tests that exist"
