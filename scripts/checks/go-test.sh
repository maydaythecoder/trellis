#!/usr/bin/env bash
# go-test.sh: the Go TUI's checks, and the Verify form for "these Go tests
# exist and pass" (ADR 0052 section 6, #505).
#
#   scripts/checks/go-test.sh                  # every check over tui/
#   scripts/checks/go-test.sh <prefix> <min>   # tests named ^<prefix>
#   scripts/checks/go-test.sh --self-test
#
# Verify lines reach Go through this script, so G6's allowed prefixes
# (cargo, git, scripts/, python3 qa/) are not widened.
#
# With no arguments: `go mod verify`, `gofmt -l` (must print nothing),
# `go vet ./...` and `go test ./...`, from tui/.
#
# With <prefix> <min>: only the tests whose names match ^<prefix>, failing
# unless at least <min> top-level tests ran and every one passed. `go test
# -run` exits 0 when the pattern matches nothing, so a ticket whose tests
# were never written would otherwise pass its own Verify; this is the Go
# form of scripts/tickets/verify-tests.sh.
#
# Every go command runs with GOFLAGS=-mod=readonly (go.sum is the pin) and
# GOTOOLCHAIN=local (a `toolchain` line never downloads another toolchain;
# ADR 0052 decision 2.4). tui/ or go missing fails; there is no skip.
#
# The rpc tests spawn target/debug/trellis; build it first with
# `cargo build -p trellis-cli`.
#
# --self-test builds a throwaway module with one passing and one failing
# test, and shows that a filter matching nothing fails, that too few
# matches fail, that a failing match fails, and that enough passing
# matches pass.
#
# Exit 0: holds. Exit 1: a check failed, too few tests matched, or the
# self-test did not hold. Exit 2: wrong arguments, or not in a git
# repository.
set -uo pipefail

usage='usage: go-test.sh [<prefix> <min>] | --self-test'
# Git Bash rewrites a path-looking argument otherwise.
export MSYS_NO_PATHCONV=1
export GOFLAGS=-mod=readonly GOTOOLCHAIN=local

need_go() {
	command -v go >/dev/null 2>&1 || {
		echo "FAIL: go is not on PATH (scripts/runners/install-go.sh installs the pinned one)"
		exit 1
	}
}

# Run the tests matching ^<prefix> in module dir <dir>; fail unless at
# least <min> top-level tests ran and all passed.
run_prefix() {
	local dir=$1 prefix=$2 min=$3 out code ran failed
	code=0
	out=$(cd "$dir" && go test -count=1 -v -run "^${prefix}" ./... 2>&1) || code=$?
	ran=$(printf '%s\n' "$out" | grep -cE "^--- PASS: ${prefix}[A-Za-z0-9_]* " || true)
	failed=$(printf '%s\n' "$out" | grep -E "^--- FAIL: " || true)
	printf '%s\n' "$out" | grep -E '^--- (PASS|FAIL|SKIP): ' | tail -40 || true
	if [ "$code" -ne 0 ] || [ -n "$failed" ]; then
		printf '%s\n' "$out" | tail -40
		echo "FAIL: go test -run '^${prefix}' failed in $dir"
		return 1
	fi
	if printf '%s\n' "$out" | grep -qE "^--- SKIP: ${prefix}"; then
		echo "FAIL: a test matching '^${prefix}' skipped; a skipped test proves nothing"
		return 1
	fi
	if [ "$ran" -lt "$min" ]; then
		echo "FAIL: $ran test(s) matched '^${prefix}' and passed in $dir; need at least $min"
		return 1
	fi
	echo "OK: $ran test(s) matched '^${prefix}' in $dir and passed"
}

self_test() {
	need_go
	local fail=0 out
	tmp=$(mktemp -d) || { echo "FAIL: mktemp"; exit 1; }
	trap 'rm -rf "$tmp"' EXIT
	mkdir -p "$tmp/m"
	printf 'module example.invalid/m\n\ngo 1.21\n' >"$tmp/m/go.mod"
	cat >"$tmp/m/m_test.go" <<-'EOF'
	package m

	import "testing"

	func TestAlphaOne(t *testing.T) {}

	func TestAlphaTwo(t *testing.T) { t.Run("sub", func(t *testing.T) {}) }

	func TestBetaFails(t *testing.T) { t.Fatal("planted failure") }

	func TestGammaSkips(t *testing.T) { t.Skip("planted skip") }
	EOF
	expect() {
		local want=$1 why=$2 got=0
		shift 2
		out=$(run_prefix "$tmp/m" "$@" 2>&1) || got=1
		if [ "$got" != "$want" ]; then
			echo "FAIL: self-test: $why: run_prefix $* returned $got, want $want"
			printf '%s\n' "$out" | tail -5 | sed 's/^/  /'
			fail=1
		fi
	}
	expect 1 "a filter matching nothing" TestNoSuchThing 1
	expect 1 "too few matches" TestAlpha 3
	expect 1 "a failing match" TestBeta 1
	expect 1 "a skipped match" TestGamma 1
	expect 0 "enough passing matches (a subtest is not counted)" TestAlpha 2
	[ "$fail" -eq 0 ] || exit 1
	echo "OK: self-test (a filter matching nothing fails; too few, failing or skipped matches fail; enough passing matches pass)"
}

if [ "${1:-}" = "--self-test" ]; then
	[ $# -eq 1 ] || { echo "$usage"; exit 2; }
	self_test
	exit 0
fi

root=$(git rev-parse --show-toplevel 2>/dev/null) || { echo "FAIL: not in a git repository"; exit 2; }
[ -f "$root/tui/go.mod" ] || { echo "FAIL: tui/go.mod not found under $root; nothing to test"; exit 1; }
need_go

if [ $# -eq 2 ]; then
	case "$1" in '' | *[!A-Za-z0-9_]*) echo "FAIL: prefix must be a Go identifier, not '$1'"; exit 2 ;; esac
	case "$2" in '' | *[!0-9]*) echo "FAIL: min must be a whole number, not '$2'"; exit 2 ;; esac
	run_prefix "$root/tui" "$1" "$2" || exit 1
	exit 0
fi
[ $# -eq 0 ] || { echo "$usage"; exit 2; }

cd "$root/tui" || exit 1
go version
go mod verify || { echo "FAIL: go mod verify: go.sum and the module cache disagree"; exit 1; }
unformatted=$(gofmt -l .)
if [ -n "$unformatted" ]; then
	printf '%s\n' "$unformatted"
	echo "FAIL: gofmt -l lists the files above"
	exit 1
fi
go vet ./... || { echo "FAIL: go vet"; exit 1; }
go test -count=1 ./... || { echo "FAIL: go test"; exit 1; }
echo "OK: go mod verify, gofmt, go vet and go test over tui/"
