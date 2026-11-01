#!/usr/bin/env bash
# mutants-ledger.sh: every surviving mutant of a cargo-mutants shard has a
# disposition in CORRECTNESS_LEDGER.md.
#
#   scripts/checks/mutants-ledger.sh <shard-dir>
#   scripts/checks/mutants-ledger.sh --all <dir>
#   scripts/checks/mutants-ledger.sh --self-test
#
# A shard directory holds `COMMIT`, `missed.txt` and `timeout.txt` as
# `trellis-cli` sharded runs write them (ADR 0060 §5). Each line of missed.txt
# and timeout.txt must appear, after the last ` | `, on one line of the ledger's
# 6b804cb block (the `<details>` whose summary names that commit), and that
# line must start with a disposition: killed, equivalent, unexercisable,
# windows-only, unix-only or not applicable. A kill is only as good as the
# test it names, so "killed … by <path::to::test>" also needs `fn <test>`
# under crates/. Mutant lines are compared as fixed strings: they contain
# `&&`, `||`, `<`, `->` and `|`, and a pattern match on them would be wrong in
# ways a reader cannot see.
#
# `--all <dir>` checks `<dir>/shard-<k>-of-10` for k in 0..9 and fails unless
# all ten are present and pass, so a dropped shard cannot read as a clean run.
#
# Exit 0: every line is dispositioned. Exit 1: some are not (each is printed
# with why), COMMIT does not read 6b804cb, a shard file or the ledger block is
# missing, or a shard is missing under --all. Exit 2: wrong arguments, or not
# in a git repository.
set -uo pipefail

usage='usage: mutants-ledger.sh <shard-dir> | --all <dir> | --self-test'
want_commit=6b804cb
block_summary='<summary>trellis-cli at 6b804cb, workspace suite on Linux (#248)</summary>'
# Git Bash rewrites a path-looking argument otherwise.
export MSYS_NO_PATHCONV=1

if [ "${1:-}" = "--self-test" ]; then
  [ $# -eq 1 ] || { echo "$usage"; exit 2; }
  self="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
  tmp=$(mktemp -d) || { echo "FAIL: mktemp failed"; exit 2; }
  trap 'rm -rf "$tmp"' EXIT
  fail=0
  # Mutant lines as cargo-mutants prints them, with the characters that would
  # break a pattern match. `c` is the one the fixed-string case is about.
  a='crates/x/src/lib.rs:10:5: replace kills_it -> bool with true'
  b='crates/x/src/lib.rs:20:9: replace == with != in other'
  c='crates/x/src/lib.rs:30:12: replace a && b || c -> Option<&str> with None'
  d='crates/x/src/lib.rs:40:1: delete ! in slow_one'
  e='crates/x/src/lib.rs:50:1: replace < with <= in absent_one'
  f='crates/x/src/lib.rs:60:1: replace || with && in bad_disposition'
  g='crates/x/src/lib.rs:70:1: replace > with >= in no_such_test'
  h='crates/x/src/lib.rs:80:1: replace -= with += in suite_only'
  o='crates/x/src/lib.rs:90:1: replace * with / in older_block_only'
  shard() {
    # shard <name> <missed lines...>; the last argument is timeout.txt's one line
    # (or empty). COMMIT is the real sha unless the caller overwrites it.
    local dir=$1 timeout=${*: -1}
    mkdir -p "$dir"
    printf '%s\n' "$want_commit" > "$dir/COMMIT"
    : > "$dir/missed.txt"
    local m
    for m in "${@:2:$#-2}"; do printf '%s\n' "$m" >> "$dir/missed.txt"; done
    : > "$dir/timeout.txt"
    [ -z "$timeout" ] || printf '%s\n' "$timeout" > "$dir/timeout.txt"
  }
  (
    cd "$tmp" || exit 2
    git init -q . && git config core.autocrlf false || exit 2
    mkdir -p crates/x/src
    printf 'fn kills_it() {}\n' > crates/x/src/lib.rs
    {
      echo '# Ledger'
      echo '<details>'
      echo '<summary>trellis-cli: 407 survivors</summary>'
      echo "equivalent (#248): older pass | $o"
      echo '</details>'
      echo '<details>'
      echo "$block_summary"
      echo "killed (#248) by \`x::tests::kills_it\`: covered | $a"
      echo "equivalent (#248): display text only | $b"
      echo "killed (#248) by x::tests::kills_it: no backticks, trailing colon | $c"
      echo "unexercisable (#248): nightly only | $d"
      echo "survived (#248): not a disposition | $f"
      echo "killed (#248) by \`x::tests::no_such_test\`: the fn is not under crates/ | $g"
      echo "killed (#248) by the workspace suite: names no test | $h"
      echo '</details>'
    } > CORRECTNESS_LEDGER.md
    git add crates/x/src/lib.rs CORRECTNESS_LEDGER.md || exit 2
    shard good "$a" "$b" "$c" "$d"
    shard absent "$e" ''
    shard baddisp "$f" ''
    shard nofn "$g" ''
    shard suite "$h" ''
    shard outside "$o" ''
    shard badcommit "$a" ''
    printf 'abc1234\n' > badcommit/COMMIT
    mkdir -p nofiles
    printf '%s\n' "$want_commit" > nofiles/COMMIT
    for k in 0 1 2 3 4 5 6 7 8 9; do
      shard "all-ok/shard-$k-of-10" "$a" "$c" "$d"
      [ "$k" = 3 ] || shard "all-missing/shard-$k-of-10" "$a" ''
    done
    # A ledger without the block: every line is unaccounted.
    mkdir -p noblock && cd noblock && git init -q . || exit 2
    printf '# Ledger\n' > CORRECTNESS_LEDGER.md
    shard shard "$a" ''
  ) || { echo "FAIL: could not build the temp repository"; exit 2; }
  run() { (cd "$tmp" && bash "$self" "$@") >/dev/null 2>&1; echo $?; }
  expect() {
    local want=$1 got
    shift
    got=$(run "$@")
    [ "$got" = "$want" ] || { echo "FAIL: mutants-ledger.sh $* exited $got, want $want"; fail=1; }
  }
  expect 0 good                 # killed with its fn present, fixed-string `&&`/`||`/`->` line, timeout line
  expect 1 absent               # a missed line nowhere in the ledger
  expect 1 baddisp              # present, but the line starts with an unknown word
  expect 1 nofn                 # killed by a test whose fn is not under crates/
  expect 1 suite                # killed by "the workspace suite": no test named
  expect 1 outside              # dispositioned only in the older 407-survivors block
  expect 1 badcommit            # COMMIT reads another sha
  expect 1 nofiles              # missed.txt and timeout.txt absent
  expect 0 --all all-ok
  expect 1 --all all-missing    # nine of ten shards
  expect 2
  expect 2 --all
  expect 2 good extra
  expect 2 does-not-exist
  got=$( (cd "$tmp/noblock" && bash "$self" shard) >/dev/null 2>&1; echo $?)
  [ "$got" = 1 ] || { echo "FAIL: a ledger without the block exited $got, want 1"; fail=1; }
  notrepo=$(mktemp -d) || exit 2
  mkdir -p "$notrepo/s"
  got=$( (cd "$notrepo" && GIT_CEILING_DIRECTORIES="$(dirname "$notrepo")" bash "$self" s) >/dev/null 2>&1; echo $?)
  rm -rf "$notrepo"
  [ "$got" = 2 ] || { echo "FAIL: outside a repository exited $got, want 2"; fail=1; }
  [ "$fail" -eq 0 ] || exit 1
  echo "OK: self-test (dispositioned shard passes, absent line, bad disposition, missing killing test, wrong COMMIT, line outside the block, missing block, --all with nine and ten shards, bad args and no repo)"
  exit 0
fi

root=$(git rev-parse --show-toplevel 2>/dev/null) || { echo "FAIL: not in a git repository"; exit 2; }
ledger="$root/CORRECTNESS_LEDGER.md"

# Read the block once into mutant -> ledger line. The mutant text is whatever
# follows the last ` | `; the reason before it may itself contain ` | `.
declare -A by_mutant=()
block_found=0
load_block() {
  [ -f "$ledger" ] || return 0
  local start line key
  # The ledger may be checked out with CRLF on the Git Bash host.
  start=$(tr -d '\r' < "$ledger" | grep -n -F -x -- "$block_summary" | head -1 | cut -d: -f1)
  [ -n "$start" ] || return 0
  block_found=1
  while IFS= read -r line || [ -n "$line" ]; do
    line=${line%$'\r'}
    [ "$line" = '</details>' ] && break
    case $line in
      *' | '*) key=${line##* | } ;;
      *) continue ;;
    esac
    [ -n "$key" ] || continue
    # First line wins, so a duplicate cannot change the answer silently.
    [ -n "${by_mutant[$key]+x}" ] || by_mutant[$key]=$line
  done < <(tail -n +"$((start + 1))" "$ledger")
}

# Why a ledger line does not account for its mutant, or empty if it does.
# A kill is checked against the code: the last `::` segment of the named test
# must be a `fn` under crates/, found through git so an untracked scratch
# test does not count.
fault_in() {
  local line=$1 token seg
  case $line in
    killed*) ;;
    equivalent* | unexercisable* | windows-only* | unix-only* | 'not applicable'*) return 0 ;;
    *) echo "bad disposition"; return 0 ;;
  esac
  case $line in
    *' by '*) token=${line#* by } ;;
    *) echo "killing test not found (no 'by <test>')"; return 0 ;;
  esac
  token=${token%% *}
  token=${token%%|*}
  token=${token//\`/}
  # A single trailing colon ends the token; `::` is part of the path.
  token=${token%:}
  case $token in
    *::*) seg=${token##*::} ;;
    *) echo "killing test not found (no path in 'by $token')"; return 0 ;;
  esac
  # -w: `fn kills_it` must not be satisfied by `fn kills_it_twice`.
  if ! git -C "$root" grep -q -F -w -e "fn $seg" -- crates/ 2>/dev/null; then
    echo "killing test not found (no 'fn $seg' under crates/)"
  fi
}

# check_shard <dir>: prints one OK or FAIL report; returns 0, 1 or 2.
check_shard() {
  local dir=$1 commit n=0 line why fault f
  local -a faults=()
  [ -d "$dir" ] || { echo "FAIL: $dir is not a directory"; return 2; }
  if [ ! -f "$dir/COMMIT" ]; then
    echo "FAIL: $dir/COMMIT is missing, want $want_commit"
    return 1
  fi
  commit=$(tr -d ' \t\r\n' < "$dir/COMMIT")
  if [ "$commit" != "$want_commit" ]; then
    echo "FAIL: $dir/COMMIT reads '$commit', want $want_commit"
    return 1
  fi
  for f in missed.txt timeout.txt; do
    [ -f "$dir/$f" ] || { echo "FAIL: $dir/$f is missing (an empty file is fine)"; return 1; }
  done
  # With no block every line falls out as "not in block" below; the header
  # says why so the reader does not go looking for 13 individual omissions.
  [ "$block_found" -eq 1 ] || echo "FAIL: $ledger has no block '$block_summary'"
  while IFS= read -r line || [ -n "$line" ]; do
    line=${line%$'\r'}
    [ -n "$line" ] || continue
    n=$((n + 1))
    if [ -z "${by_mutant[$line]+x}" ]; then
      faults+=("not in block: $line")
      continue
    fi
    fault=$(fault_in "${by_mutant[$line]}")
    [ -z "$fault" ] || faults+=("$fault: $line")
  # awk, not cat: a missed.txt without a final newline would otherwise
  # join its last line to timeout.txt's first.
  done < <(awk 1 "$dir/missed.txt" "$dir/timeout.txt")
  if [ "${#faults[@]}" -gt 0 ]; then
    echo "FAIL: ${#faults[@]} of $n missed/timed-out line(s) in $dir lack a disposition in the $want_commit block:"
    for why in "${faults[@]}"; do echo "  $why"; done
    return 1
  fi
  # An empty shard against a missing block is still not a pass: the block is
  # the record that the shard was looked at.
  [ "$block_found" -eq 1 ] || return 1
  echo "OK: $n missed/timed-out line(s) in $dir each have a disposition in the $want_commit block"
  return 0
}

if [ "${1:-}" = "--all" ]; then
  [ $# -eq 2 ] && [ -n "$2" ] || { echo "$usage"; exit 2; }
  base=$2
  [ -d "$base" ] || { echo "FAIL: $base is not a directory"; exit 2; }
  missing=()
  for k in 0 1 2 3 4 5 6 7 8 9; do
    [ -d "$base/shard-$k-of-10" ] || missing+=("shard-$k-of-10")
  done
  if [ "${#missing[@]}" -gt 0 ]; then
    echo "FAIL: $base lacks ${#missing[@]} of the ten shards: ${missing[*]}"
    exit 1
  fi
  load_block
  rc=0
  for k in 0 1 2 3 4 5 6 7 8 9; do
    check_shard "$base/shard-$k-of-10" || rc=1
  done
  [ "$rc" -eq 0 ] || { echo "FAIL: not every shard under $base is dispositioned"; exit 1; }
  echo "OK: all ten shards under $base are dispositioned in the $want_commit block"
  exit 0
fi

[ $# -eq 1 ] && [ -n "$1" ] || { echo "$usage"; exit 2; }
load_block
check_shard "$1"
