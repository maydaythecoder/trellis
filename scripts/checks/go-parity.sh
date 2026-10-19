#!/usr/bin/env bash
# go-parity.sh: tui/PARITY.md maps each rule the ratatui TUI's tests hold to
# a Go test that exists, or says why it is not ported (ADR 0052 section 7,
# #508).
#
#   scripts/checks/go-parity.sh [--complete]
#   scripts/checks/go-parity.sh --self-test
#
# Reads the table rows of tui/PARITY.md, `| <file> | <rust test> | <rule> |
# <ticket> | <go test> |`, and fails:
#
#   - for a row whose last cell names a Go test (`TestName`, one or more)
#     that no `func TestName(` under tui/ defines: a row pointing at nothing
#     would read as parity that is not there;
#   - for a test in the five Rust files (while they exist) that has no row,
#     so a rule added to the ratatui TUI cannot be left out of the map;
#   - with --complete, for a row whose last cell is neither a Go test nor
#     "not ported" with a reason (#514 and #516 run it so).
#
# --self-test plants each case in a temporary tree and shows the script
# refuses it, and passes a well-formed one. Nothing is planted in the
# working tree.
#
# Exit 0: holds. Exit 1: a row or a test failed a rule above, each printed,
# or a self-test case did not hold. Exit 2: wrong arguments, or not in a
# git repository.
set -uo pipefail

usage='usage: go-parity.sh [--complete] | --self-test'
# Git Bash rewrites a path-looking argument otherwise.
export MSYS_NO_PATHCONV=1
self="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"

# The five Rust files ADR 0052 section 7 names.
RUST_FILES=(
	crates/tui-demo/src/tui/app.rs
	crates/tui-demo/src/tui/view.rs
	crates/tui-demo/src/tui/events.rs
	crates/trellis-cli/src/tui_bridge.rs
	crates/trellis-cli/src/tui_run.rs
)

# check <root> <complete 0|1>: prints each failure, returns 1 if any.
check() {
	local root=$1 complete=$2 parity="$1/tui/PARITY.md" bad=0
	[ -f "$parity" ] || { echo "FAIL: $parity not found"; return 1; }

	# Every Go test defined under tui/.
	local defined
	defined=$(find "$root/tui" -type f -name '*_test.go' -print0 2>/dev/null \
		| xargs -0 -r sed -n 's/^func \(Test[A-Za-z0-9_]*\)(.*/\1/p' | sort -u)

	# Rows: `| file | test | rule | ticket | go |`, header and divider out.
	local rows
	rows=$(awk -F'|' '
		/^\|/ && NF >= 7 {
			file = $2; test = $3; go = $6
			gsub(/^[ \t`]+|[ \t`]+$/, "", file)
			gsub(/^[ \t`]+|[ \t`]+$/, "", test)
			gsub(/^[ \t]+|[ \t]+$/, "", go)
			if (test == "Rust test" || test ~ /^-+$/) next
			print NR "\t" file "\t" test "\t" go
		}
	' "$parity")
	[ -n "$rows" ] || { echo "FAIL: no table rows in $parity"; return 1; }

	local n file test go names name
	while IFS=$'\t' read -r n file test go; do
		case "$go" in
		"not ported"*)
			if [ "$go" = "not ported" ] || [ "$go" = "not ported:" ]; then
				echo "FAIL: tui/PARITY.md:$n: $test is not ported and gives no reason"
				bad=1
			fi
			continue
			;;
		esac
		names=$(printf '%s\n' "$go" | grep -oE 'Test[A-Za-z0-9_]+' || true)
		if [ -z "$names" ]; then
			if [ "$complete" = 1 ]; then
				echo "FAIL: tui/PARITY.md:$n: $test names no Go test and is not marked not ported (--complete)"
				bad=1
			fi
			continue
		fi
		for name in $names; do
			if ! printf '%s\n' "$defined" | grep -qx "$name"; then
				echo "FAIL: tui/PARITY.md:$n: $test names $name, which no tui/**/*_test.go defines"
				bad=1
			fi
		done
	done <<<"$rows"

	# Every test in the Rust files has a row, while the files exist.
	local f base t
	for f in "${RUST_FILES[@]}"; do
		[ -f "$root/$f" ] || continue
		base=$(basename "$f")
		for t in $(awk '
			/^[ \t]*#\[(tokio::)?test/ { want = 1; next }
			want && match($0, /fn [A-Za-z0-9_]+/) { print substr($0, RSTART + 3, RLENGTH - 3); want = 0 }
		' "$root/$f"); do
			if ! printf '%s\n' "$rows" | awk -F'\t' -v f="$base" -v t="$t" '$2 == f && $3 == t { found = 1 } END { exit !found }'; then
				echo "FAIL: $f: $t has no row in tui/PARITY.md"
				bad=1
			fi
		done
	done
	return "$bad"
}

if [ "${1:-}" = "--self-test" ]; then
	[ $# -eq 1 ] || { echo "$usage"; exit 2; }
	tmp=$(mktemp -d) || { echo "FAIL: mktemp"; exit 1; }
	trap 'rm -rf "$tmp"' EXIT
	fail=0
	mk() { # <dir>: a tree with one Rust test, one Go test and a full row
		mkdir -p "$1/tui/internal/m" "$1/crates/tui-demo/src/tui"
		printf 'package m\n\nimport "testing"\n\nfunc TestGoOne(t *testing.T) {}\n' >"$1/tui/internal/m/m_test.go"
		printf '#[cfg(test)]\nmod tests {\n    #[test]\n    fn rust_one() {}\n}\n' >"$1/crates/tui-demo/src/tui/app.rs"
		printf '| Rust file | Rust test | Rule | Ticket | Go test |\n| --- | --- | --- | --- | --- |\n| `app.rs` | `rust_one` | rust one | T2 | `TestGoOne` |\n' >"$1/tui/PARITY.md"
	}
	expect() { # <want 0|1> <why> <dir> <complete>
		local got=0 out
		out=$(check "$3" "$4" 2>&1) || got=1
		if [ "$got" != "$1" ]; then
			echo "FAIL: self-test: $2: returned $got, want $1"
			printf '%s\n' "$out" | sed 's/^/  /'
			fail=1
		fi
	}
	d="$tmp/ok"; mk "$d"
	expect 0 "a well-formed table" "$d" 1
	d="$tmp/missing-go"; mk "$d"
	sed -i.bak 's/TestGoOne` |$/TestGoMissing` |/' "$d/tui/PARITY.md"
	expect 1 "a row naming a Go test that does not exist" "$d" 0
	d="$tmp/no-row"; mk "$d"
	printf '    #[test]\n    fn rust_two() {}\n' >>"$d/crates/tui-demo/src/tui/app.rs"
	expect 1 "a Rust test with no row" "$d" 0
	d="$tmp/blank"; mk "$d"
	printf '| `app.rs` | `rust_two` | rust two | T3 | |\n' >>"$d/tui/PARITY.md"
	expect 0 "a blank Go cell without --complete" "$d" 0
	expect 1 "a blank Go cell with --complete" "$d" 1
	d="$tmp/reason"; mk "$d"
	printf '| `app.rs` | `rust_two` | rust two | T3 | not ported: the rpc server holds it |\n' >>"$d/tui/PARITY.md"
	expect 0 "not ported with a reason, --complete" "$d" 1
	d="$tmp/noreason"; mk "$d"
	printf '| `app.rs` | `rust_two` | rust two | T3 | not ported |\n' >>"$d/tui/PARITY.md"
	expect 1 "not ported with no reason" "$d" 0
	[ "$fail" -eq 0 ] || exit 1
	echo "OK: self-test (a well-formed table passes; a missing Go test, an unmapped Rust test and a reasonless not-ported row fail; a blank row fails only with --complete)"
	exit 0
fi

complete=0
case "${1:-}" in
"") ;;
--complete) complete=1 ;;
*) echo "$usage"; exit 2 ;;
esac
[ $# -le 1 ] || { echo "$usage"; exit 2; }
root=$(git rev-parse --show-toplevel 2>/dev/null) || { echo "FAIL: not in a git repository"; exit 2; }
if ! check "$root" "$complete"; then
	echo "FAIL: tui/PARITY.md does not hold (above)"
	exit 1
fi
rows=$(grep -cE '^\| `' "$root/tui/PARITY.md")
mapped=$(grep -E '^\| `' "$root/tui/PARITY.md" | grep -cE '\| (`Test|not ported)[^|]*\|$' || true)
echo "OK: tui/PARITY.md: $rows rows, $mapped mapped to a Go test or not ported, every named Go test exists$([ "$complete" = 1 ] && echo ', and every row is mapped')"
