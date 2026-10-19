#!/usr/bin/env bash
# go-guards.sh: Go code in tui/ is held to the rules Rust code is held to by
# tests/hygiene.rs and tests/child_env.rs, which cannot read Go (ADR 0052
# section 3, #504).
#
#   scripts/checks/go-guards.sh [--self-test]
#
# Each rule is named in brackets on the line that reports it:
#
#   exec-import      `"os/exec"` imported only under tui/internal/rpc/: one
#                    package starts a process (as desktop/src/child.rs is
#                    the desktop's one file), and an aliased import cannot
#                    hide a spawn from the rule below
#   exec-outside-rpc `exec.Command` or `exec.CommandContext` only under
#                    tui/internal/rpc/
#   raw-spawn        no `os.StartProcess`, `syscall.Exec`,
#                    `syscall.ForkExec` or `syscall.StartProcess` anywhere:
#                    the spawns the two rules above do not see
#   git-spawn        no spawn whose program is `git` (or `git.exe`), and no
#                    `exec.LookPath("git")`: every git spawn goes through
#                    trellis_vcs (ADR 0023), and Go asks trellis instead
#   setenv           no `os.Setenv`, `os.Unsetenv`, `os.Clearenv` or
#                    `syscall.Setenv`/`Unsetenv`/`Clearenv`: a client adds
#                    nothing to a child's environment (ADR 0048 section 3)
#   cmd-env          no assignment to a `Cmd`'s `Env` (`.Env =`, `.Env =
#                    append(`) and no `Env:` field in a composite literal
#   test-setenv      `t.Setenv` only in a `_test.go` file and only for
#                    `TRELLIS_CONFIG_DIR`, so a test keeps trellis off the
#                    developer's own config and nothing else is set (the
#                    desktop's rule, desktop-guards.sh `src-env`)
#   gomod-replace    no `replace` directive in tui/go.mod: go.sum is the
#                    pin, and a replace points a build at code it does not
#                    record
#   gomod-toolchain  tui/go.mod has a `toolchain` line (ADR 0052 section 3)
#
# The scans read text line by line, skip lines that are only a `//`
# comment, and do not see a name built at run time; review of the diff
# answers for that.
#
# With tui/ absent it says it skipped and exits 0, so the gates can carry
# this before tui/ exists (#504 lands before #505 creates it).
#
# --self-test builds a temporary tree with one allowed form of each rule,
# shows it passes, then plants each refused form in a fresh copy and shows
# the script refuses it naming the rule. Nothing is planted in the working
# tree.
#
# Exit 0: every rule holds, tui/ is absent, or every self-test case held.
# Exit 1: a rule failed, each failure printed as file:line, or a self-test
# case did not hold. Exit 2: wrong arguments, or not in a git repository.
set -uo pipefail

usage='usage: go-guards.sh [--self-test]'
self="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
# Git Bash rewrites a path-looking argument otherwise.
export MSYS_NO_PATHCONV=1

# Scan the tree under $1 (a directory holding tui/). Prints one
# `FAIL [rule] file:line: text` per violation; returns 1 if any.
scan() {
	local root=$1 bad=0 out
	if [ ! -f "$root/tui/go.mod" ]; then
		echo "FAIL [gomod-toolchain] tui/go.mod: missing (tui/ exists without a module file)"
		return 1
	fi
	# go.mod: a `replace` directive, single or block form; a toolchain line.
	out=$(awk '
		{ sub(/\r$/, "") }
		/^[ \t]*\/\// { next }
		/^[ \t]*replace([ \t(]|$)/ { print "FAIL [gomod-replace] tui/go.mod:" FNR ": " $0 }
	' "$root/tui/go.mod")
	[ -z "$out" ] || { printf '%s\n' "$out"; bad=1; }
	if ! grep -Eq '^[[:space:]]*toolchain[[:space:]]+go[0-9]' "$root/tui/go.mod"; then
		echo "FAIL [gomod-toolchain] tui/go.mod: no toolchain line (ADR 0052 section 3)"
		bad=1
	fi

	local f rel
	while IFS= read -r -d '' f; do
		rel=${f#"$root"/}
		out=$(awk -v f="$rel" '
			BEGIN {
				in_rpc = (index(f, "tui/internal/rpc/") == 1)
				is_test = (f ~ /_test\.go$/)
			}
			function report(rule) { print "FAIL [" rule "] " f ":" FNR ": " line }
			{
				line = $0; sub(/\r$/, "", line)
				if (line ~ /^[ \t]*\/\//) next
				if (!in_rpc && line ~ /"os\/exec"/) report("exec-import")
				if (!in_rpc && line ~ /exec\.Command(Context)?[ \t]*\(/) report("exec-outside-rpc")
				if (line ~ /os\.StartProcess[ \t]*\(|syscall\.(Exec|ForkExec|StartProcess)[ \t]*\(/) report("raw-spawn")
				if (line ~ /exec\.Command[ \t]*\([ \t]*"git(\.exe)?"/ \
				    || line ~ /exec\.CommandContext[ \t]*\([^,]*,[ \t]*"git(\.exe)?"/ \
				    || line ~ /exec\.LookPath[ \t]*\([ \t]*"git(\.exe)?"/ \
				    || line ~ /StartProcess[ \t]*\([ \t]*"git(\.exe)?"/ \
				    || line ~ /Path:[ \t]*"git(\.exe)?"/) report("git-spawn")
				if (line ~ /(os|syscall)\.(Setenv|Unsetenv|Clearenv)[ \t]*\(/) report("setenv")
				if (line ~ /\.Env[ \t]*(=|\+=)[^=]/ || line ~ /(^|[{, \t])Env:[ \t]*/) report("cmd-env")
				if (line ~ /\.Setenv[ \t]*\(/ && line !~ /(os|syscall)\.Setenv/) {
					if (!is_test || line !~ /\.Setenv[ \t]*\([ \t]*"TRELLIS_CONFIG_DIR"[ \t]*,/) report("test-setenv")
				}
			}
		' "$f")
		[ -z "$out" ] || { printf '%s\n' "$out"; bad=1; }
	done < <(find "$root/tui" -type f -name '*.go' -print0 | sort -z)
	return "$bad"
}

if [ "${1:-}" = "--self-test" ]; then
	[ $# -eq 1 ] || { echo "$usage"; exit 2; }
	tmp=$(mktemp -d) || { echo "FAIL: mktemp"; exit 1; }
	trap 'rm -rf "$tmp"' EXIT

	# The allowed forms: a toolchain line, exec.Command in internal/rpc, and
	# t.Setenv("TRELLIS_CONFIG_DIR", ...) in a test.
	base="$tmp/base"
	mkdir -p "$base/tui/internal/rpc" "$base/tui/internal/model"
	printf 'module example/tui\n\ngo 1.24.2\n\ntoolchain go1.27.2\n' >"$base/tui/go.mod"
	cat >"$base/tui/internal/rpc/rpc.go" <<-'EOF'
	package rpc

	import "os/exec"

	func start(trellis, root string) *exec.Cmd {
		return exec.Command(trellis, "--root", root, "rpc")
	}
	EOF
	cat >"$base/tui/internal/rpc/rpc_test.go" <<-'EOF'
	package rpc

	import "testing"

	func TestX(t *testing.T) {
		t.Setenv("TRELLIS_CONFIG_DIR", t.TempDir())
		// os.Setenv in a comment is not code.
	}
	EOF
	printf 'package model\n\nfunc Env() string { return "" }\n' >"$base/tui/internal/model/model.go"

	if ! out=$(scan "$base"); then
		echo "FAIL: self-test: the allowed tree fails, so the cases below would prove nothing:"
		printf '%s\n' "$out"
		exit 1
	fi

	# name|rule|file (under tui/)|line to append
	CASES=(
		'exec in model|exec-outside-rpc|internal/model/model.go|func a() { _ = exec.Command("ls") }'
		'exec context in model|exec-outside-rpc|internal/model/model.go|func a() { _ = exec.CommandContext(ctx, "ls") }'
		'os/exec aliased outside rpc|exec-import|internal/model/model.go|import x "os/exec"'
		'git through exec|git-spawn|internal/rpc/rpc.go|func g() { _ = exec.Command("git", "status") }'
		'git through exec context|git-spawn|internal/rpc/rpc.go|func g() { _ = exec.CommandContext(ctx, "git.exe", "log") }'
		'git looked up|git-spawn|internal/rpc/rpc.go|func g() { _, _ = exec.LookPath("git") }'
		'os.StartProcess|raw-spawn|internal/rpc/rpc.go|func g() { os.StartProcess(p, nil, nil) }'
		'syscall.Exec|raw-spawn|internal/model/model.go|func g() { syscall.Exec(p, nil, nil) }'
		'os.Setenv|setenv|internal/model/model.go|func s() { os.Setenv("A", "b") }'
		'os.Unsetenv|setenv|internal/model/model.go|func s() { os.Unsetenv("OPENAI_API_KEY") }'
		'os.Clearenv|setenv|internal/rpc/rpc.go|func s() { os.Clearenv() }'
		'Cmd.Env assigned|cmd-env|internal/rpc/rpc.go|func e(c *exec.Cmd) { c.Env = append(c.Environ(), "A=b") }'
		'Env in a literal|cmd-env|internal/rpc/rpc.go|var c = exec.Cmd{Path: p, Env: nil}'
		't.Setenv of a key|test-setenv|internal/rpc/rpc_test.go|func TestY(t *testing.T) { t.Setenv("OPENAI_API_KEY", "x") }'
		't.Setenv outside a test|test-setenv|internal/model/model.go|func h(t *testing.T) { t.Setenv("TRELLIS_CONFIG_DIR", "x") }'
		'replace directive|gomod-replace|go.mod|replace github.com/a/b => ../b'
		'replace block|gomod-replace|go.mod|replace ('
	)
	fail=0
	for c in "${CASES[@]}"; do
		IFS='|' read -r name rule file text <<<"$c"
		d="$tmp/case"
		rm -rf "$d"
		cp -R "$base" "$d"
		printf '%s\n' "$text" >>"$d/tui/$file"
		out=$(scan "$d")
		rc=$?
		if [ "$rc" -eq 0 ] || ! printf '%s\n' "$out" | grep -qF "[$rule]"; then
			echo "FAIL: self-test: $name exited $rc without naming [$rule]:"
			printf '%s\n' "$out" | sed 's/^/  /'
			fail=1
		fi
	done
	# A missing toolchain line.
	d="$tmp/case"
	rm -rf "$d"
	cp -R "$base" "$d"
	printf 'module example/tui\n\ngo 1.24.2\n' >"$d/tui/go.mod"
	out=$(scan "$d")
	rc=$?
	if [ "$rc" -eq 0 ] || ! printf '%s\n' "$out" | grep -qF "[gomod-toolchain]"; then
		echo "FAIL: self-test: a go.mod with no toolchain line exited $rc without naming [gomod-toolchain]"
		fail=1
	fi
	# tui/ absent: skipped, exit 0.
	mkdir -p "$tmp/empty"
	out=$(cd "$tmp/empty" && git init -q . && bash "$self" 2>&1)
	rc=$?
	if [ "$rc" -ne 0 ] || ! printf '%s\n' "$out" | grep -q 'skipped'; then
		echo "FAIL: self-test: with tui/ absent it exited $rc, want 0 and 'skipped': $out"
		fail=1
	fi
	[ "$fail" -eq 0 ] || exit 1
	echo "OK: self-test (the allowed forms pass; each of $((${#CASES[@]} + 1)) planted cases fails naming its rule; tui/ absent is skipped)"
	exit 0
fi
[ $# -eq 0 ] || { echo "$usage"; exit 2; }

root=$(git rev-parse --show-toplevel 2>/dev/null) || { echo "FAIL: not in a git repository"; exit 2; }
if [ ! -d "$root/tui" ]; then
	echo "OK: go-guards skipped: tui/ is absent"
	exit 0
fi
if ! scan "$root"; then
	echo "FAIL: Go code under tui/ breaks a rule above (scripts/checks/go-guards.sh names each)"
	exit 1
fi
n=$(find "$root/tui" -type f -name '*.go' | wc -l | tr -d ' ')
echo "OK: go-guards: $n Go file(s) under tui/ hold every rule; tui/go.mod has a toolchain line and no replace"
