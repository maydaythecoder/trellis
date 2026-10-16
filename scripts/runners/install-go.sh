#!/usr/bin/env bash
# install-go.sh: install the pinned Go toolchain into a self-hosted runner's
# own directory, from a go.dev archive checked against its SHA-256 (#503,
# ADR 0052 decision 2.4).
#
#   scripts/runners/install-go.sh [--runner-dir <dir>] [--target <dir>]
#                                 [--archive <file>] [--dry-run]
#   scripts/runners/install-go.sh --self-test
#
# Run once by the owner as each runner account (`trellis-ci` in WSL,
# `trellis-ci-win` on Windows, from Git Bash), with the runner stopped, then
# restart the runner: the runner reads `.env` and `.path` when it starts.
#
# Why a pinned archive and not `actions/setup-go` or a distribution package:
# neither pins the toolchain to a hash this repository records, and
# `scripts/ci-local.py` refuses setup-go. Why under the runner directory: a
# job runs as the runner account and must not reach the owner's Go
# (`C:\Program Files\Go` is the owner's system install, not a pin) or write
# a system path.
#
# What it does, in order, and nothing else:
#   1. checks <dir> is a runner directory (it holds `.runner`) and not a
#      system path, and that the target resolves inside it;
#   2. downloads go<PIN> for this platform from go.dev, or takes --archive;
#   3. refuses, before unpacking anything, unless the archive's SHA-256 is
#      the one recorded below;
#   4. unpacks into <target> (default <dir>/toolchains/go<PIN>), so GOROOT is
#      <target>/go;
#   5. sets GOROOT, GOPATH, GOMODCACHE, GOCACHE (all under <dir>),
#      GOTOOLCHAIN=local and GOFLAGS=-mod=readonly in <dir>/.env;
#   6. puts <target>/go/bin first in <dir>/.path, dropping any other
#      toolchains/go*/go/bin entry an earlier pin left there.
# Run again with the same pin, it downloads nothing and changes nothing.
#
# GOTOOLCHAIN=local: a `toolchain` line in tui/go.mod must never make the go
# command download another toolchain at build time.
#
# Moving the pin is one commit that changes GO_VERSION and both hashes,
# copied from https://go.dev/dl/?mode=json (docs/BUILD_SYSTEM.md, "CI
# runners"), then the owner runs this again on both runners.
#
# Exit 0: installed, or already installed. Exit 1: a refusal or a failed
# step, named on stderr. Exit 2: wrong arguments.
set -euo pipefail

GO_VERSION="1.27.2"
SHA256_LINUX_AMD64="ecbadb99091a3f46e31f5f934b068b1864eafa7995211b39eaddf76996045fe5"
SHA256_WINDOWS_AMD64="1314008898bd40df77af4b014f777f08873dbdfbcd3d92308728ee03304fe04f"

usage='usage: install-go.sh [--runner-dir <dir>] [--target <dir>] [--archive <file>] [--dry-run] | --self-test'
# Git Bash rewrites a path-looking argument otherwise.
export MSYS_NO_PATHCONV=1

die() { echo "install-go: REFUSED: $*" >&2; exit 1; }
note() { echo "install-go: $*"; }

is_windows() {
	case "$(uname -s)" in MINGW* | MSYS* | CYGWIN*) return 0 ;; *) return 1 ;; esac
}

# The archive name and recorded hash for the platform this runs on. amd64
# only: both runners are X64, and an unknown platform is refused rather than
# guessed.
platform_archive() {
	local m
	m=$(uname -m)
	case "$m" in x86_64 | amd64) ;; *) die "machine $m is not amd64; no hash is recorded for it" ;; esac
	if is_windows; then
		ARCHIVE_NAME="go${GO_VERSION}.windows-amd64.zip"
		ARCHIVE_SHA="$SHA256_WINDOWS_AMD64"
	else
		case "$(uname -s)" in Linux) ;; *) die "OS $(uname -s) has no recorded hash (Linux and Windows only, ADR 0052 decision 2.5)" ;; esac
		ARCHIVE_NAME="go${GO_VERSION}.linux-amd64.tar.gz"
		ARCHIVE_SHA="$SHA256_LINUX_AMD64"
	fi
}

sha256_of() {
	if command -v sha256sum >/dev/null 2>&1; then
		sha256sum -- "$1" | cut -d' ' -f1
	elif command -v shasum >/dev/null 2>&1; then
		shasum -a 256 -- "$1" | cut -d' ' -f1
	else
		die "neither sha256sum nor shasum is installed; the archive cannot be checked"
	fi
}

# An absolute, symlink-resolved path, for a path that may not exist yet:
# its deepest existing ancestor is resolved and the rest appended. Without
# resolving, `<dir>/link-to-elsewhere/go` or `<dir>/../x` would pass a
# prefix check and unpack outside the runner directory.
resolve() {
	local p=$1 head tail=""
	case "$p" in /*) ;; *) p="$PWD/$p" ;; esac
	head=$p
	while [ ! -e "$head" ]; do
		tail="/$(basename -- "$head")$tail"
		head=$(dirname -- "$head")
	done
	head=$(cd -P -- "$head" && pwd -P) || return 1
	p="$head$tail"
	# Any `..` left is in the part that does not exist yet; refuse it rather
	# than normalise it.
	case "$p" in */../* | */.. | */./* | */.) return 1 ;; esac
	p="${p%/}"
	printf '%s\n' "${p:-/}"
}

# A directory a job's toolchain may never live in.
is_system_path() {
	case "$1" in
	/ | /bin | /bin/* | /boot | /boot/* | /etc | /etc/* | /lib | /lib/* | /lib64 | /lib64/* | /opt | /opt/* | /sbin | /sbin/* | /usr | /usr/* | /var | /var/* | /proc | /proc/* | /sys | /sys/*) return 0 ;;
	/c | /c/ | /c/Windows | /c/Windows/* | "/c/Program Files" | "/c/Program Files/"* | "/c/Program Files (x86)" | "/c/Program Files (x86)/"* | /c/ProgramData | /c/ProgramData/*) return 0 ;;
	esac
	return 1
}

# The form a path takes in `.env` and `.path`: the runner on Windows is a
# Windows process and reads Windows paths.
native_path() {
	if is_windows && command -v cygpath >/dev/null 2>&1; then
		cygpath -w -- "$1"
	else
		printf '%s\n' "$1"
	fi
}

path_sep() { if is_windows; then printf ';'; else printf ':'; fi; }

# Set KEY=VALUE in an env file: replace the key's line if present, append it
# if not, and leave the file untouched (same bytes, same mtime) when it
# already says so. Prints "set KEY" or nothing.
set_env_key() {
	local file=$1 key=$2 value=$3 tmp
	tmp=$(mktemp "${file}.XXXXXX")
	awk -v k="$key" -v v="$value" '
		BEGIN { done = 0 }
		{
			line = $0; sub(/\r$/, "", line)
			if (index(line, k "=") == 1) {
				if (!done) print k "=" v
				done = 1
				next
			}
			print line
		}
		END { if (!done) print k "=" v }
	' "$file" >"$tmp"
	if cmp -s "$tmp" "$file"; then
		rm -f -- "$tmp"
	else
		if ((DRY)); then
			rm -f -- "$tmp"
			note "would set $key=$value in $file"
		else
			mv -f -- "$tmp" "$file"
			note "set $key=$value in $file"
		fi
	fi
}

# Put BIN first in the one-line PATH file, once, dropping any other pinned
# toolchain's bin under the runner's toolchains/ (the native form of
# <runner>/toolchains/go*/go/bin), so moving the pin replaces the old entry.
put_path_first() {
	local file=$1 bin=$2 old_prefix=$3 sep cur want=() p
	sep=$(path_sep)
	cur=$(tr -d '\r\n' <"$file")
	want=("$bin")
	local IFS="$sep"
	for p in $cur; do
		[ -n "$p" ] || continue
		[ "$p" = "$bin" ] && continue
		case "$p" in "$old_prefix"*) continue ;; esac
		want+=("$p")
	done
	local new="${want[*]}"
	unset IFS
	if [ "$new" = "$cur" ]; then
		return 0
	fi
	if ((DRY)); then
		note "would write $file: $new"
	else
		printf '%s' "$new" >"$file"
		note "wrote $file: $bin first"
	fi
}

unpack() {
	local archive=$1 into=$2
	mkdir -p -- "$into"
	case "$archive" in
	*.tar.gz) tar -xzf "$archive" -C "$into" ;;
	*.zip)
		if command -v unzip >/dev/null 2>&1; then
			unzip -q "$archive" -d "$into"
		elif [ -x /c/Windows/System32/tar.exe ]; then
			# Windows' own bsdtar reads zip; Git Bash's GNU tar does not.
			/c/Windows/System32/tar.exe -xf "$(native_path "$archive")" -C "$(native_path "$into")"
		else
			die "no unzip and no C:\\Windows\\System32\\tar.exe to unpack $archive"
		fi
		;;
	*) die "unknown archive type: $archive" ;;
	esac
}

install() {
	local runner_arg=$1 target_arg=$2 archive_arg=$3
	local runner target goroot gobin download
	platform_archive

	[ -d "$runner_arg" ] || die "runner directory $runner_arg does not exist"
	runner=$(resolve "$runner_arg") || die "runner directory $runner_arg does not resolve"
	is_system_path "$runner" && die "runner directory $runner is a system path"
	[ -f "$runner/.runner" ] || die "$runner holds no .runner file; it is not a configured runner directory"
	[ -f "$runner/.env" ] || die "$runner/.env is missing; the runner writes it at configuration (docs/BUILD_SYSTEM.md, CI runners)"
	# A missing .path would mean writing a PATH that holds only Go; refuse
	# rather than drop every other entry the jobs rely on.
	[ -f "$runner/.path" ] || die "$runner/.path is missing; the runner writes it at configuration (docs/BUILD_SYSTEM.md, CI runners)"

	[ -n "$target_arg" ] || target_arg="$runner/toolchains/go$GO_VERSION"
	target=$(resolve "$target_arg") || die "target $target_arg holds '..' or '.', or does not resolve"
	case "$target" in
	"$runner"/*) ;;
	*) die "target $target is outside the runner directory $runner" ;;
	esac
	is_system_path "$target" && die "target $target is a system path"

	goroot="$target/go"
	gobin="$goroot/bin"
	if [ -f "$goroot/VERSION" ] && [ "$(head -n1 "$goroot/VERSION" | tr -d '\r')" = "go$GO_VERSION" ] \
		&& { [ -x "$gobin/go" ] || [ -f "$gobin/go.exe" ]; }; then
		note "go$GO_VERSION already unpacked at $goroot"
	elif [ -e "$target" ]; then
		die "$target exists but does not hold go$GO_VERSION; remove it and run again"
	else
		download="$runner/toolchains/.download"
		local archive
		if [ -n "$archive_arg" ]; then
			archive=$archive_arg
			[ -f "$archive" ] || die "archive $archive does not exist"
		else
			archive="$download/$ARCHIVE_NAME"
			if ((DRY)); then
				note "would download https://go.dev/dl/$ARCHIVE_NAME to $archive"
			else
				mkdir -p -- "$download"
				note "downloading https://go.dev/dl/$ARCHIVE_NAME"
				curl --proto '=https' --tlsv1.2 -fsSL -o "$archive" "https://go.dev/dl/$ARCHIVE_NAME" \
					|| die "download of $ARCHIVE_NAME failed"
			fi
		fi
		if [ -f "$archive" ]; then
			local got
			got=$(sha256_of "$archive")
			if [ "$got" != "$ARCHIVE_SHA" ]; then
				[ -n "$archive_arg" ] || rm -f -- "$archive"
				die "SHA-256 of $archive is $got, not the recorded $ARCHIVE_SHA for $ARCHIVE_NAME; nothing was unpacked"
			fi
			note "SHA-256 matches the recorded hash for $ARCHIVE_NAME"
		fi
		if ((DRY)); then
			note "would unpack into $target"
		else
			# Into a sibling first and renamed after, so an interrupted unpack
			# never leaves a target that the check above takes as installed.
			local partial="$target.partial"
			rm -rf -- "$partial"
			unpack "$archive" "$partial"
			[ -f "$partial/go/VERSION" ] || die "the archive held no go/VERSION; $partial left for inspection"
			mv -- "$partial" "$target"
			[ -n "$archive_arg" ] || rm -f -- "$archive"
			note "unpacked go$GO_VERSION into $target"
		fi
	fi

	local n_goroot n_gopath n_modcache n_cache
	n_goroot=$(native_path "$goroot")
	n_gopath=$(native_path "$runner/go-path")
	n_modcache=$(native_path "$runner/go-path/pkg/mod")
	n_cache=$(native_path "$runner/go-cache")
	set_env_key "$runner/.env" GOROOT "$n_goroot"
	set_env_key "$runner/.env" GOPATH "$n_gopath"
	set_env_key "$runner/.env" GOMODCACHE "$n_modcache"
	set_env_key "$runner/.env" GOCACHE "$n_cache"
	set_env_key "$runner/.env" GOTOOLCHAIN local
	set_env_key "$runner/.env" GOFLAGS -mod=readonly
	put_path_first "$runner/.path" "$(native_path "$gobin")" "$(native_path "$runner/toolchains/go")"
	note "done: go$GO_VERSION for $(basename -- "$runner"); restart the runner so it reads .env and .path"
}

self_test() {
	# tmp is global: the EXIT trap runs after this function's locals are gone.
	local self fail=0 out code
	self="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
	tmp=$(mktemp -d) || { echo "FAIL: mktemp"; exit 1; }
	trap 'rm -rf "$tmp"' EXIT
	tmp=$(cd -P "$tmp" && pwd -P)

	mkrunner() {
		mkdir -p "$1"
		: >"$1/.runner"
		printf 'LANG=C.UTF-8\nCARGO_HOME=%s/cargo\n' "$1" >"$1/.env"
		printf '%s' "/usr/bin$(path_sep)/bin" >"$1/.path"
	}
	run() {
		code=0
		out=$(bash "$self" "$@" 2>&1) || code=$?
	}
	expect_refused() {
		local why=$1 want=$2
		shift 2
		run "$@"
		if [ "$code" -eq 0 ]; then
			echo "FAIL: self-test: $why: exited 0"; fail=1
		elif ! printf '%s\n' "$out" | grep -q -- "$want"; then
			echo "FAIL: self-test: $why: output does not say '$want': $out"; fail=1
		fi
	}

	# 1. A tampered archive is refused before anything is unpacked, and the
	#    runner's files are left as they were.
	local r="$tmp/runner"
	mkrunner "$r"
	platform_archive
	local fake="$tmp/$ARCHIVE_NAME"
	printf 'not the go.dev archive\n' >"$fake"
	cp "$r/.env" "$tmp/env.before"
	cp "$r/.path" "$tmp/path.before"
	expect_refused "tampered archive" "SHA-256" --runner-dir "$r" --archive "$fake"
	if [ -e "$r/toolchains/go$GO_VERSION" ] || [ -e "$r/toolchains/go$GO_VERSION.partial" ]; then
		echo "FAIL: self-test: tampered archive: something was unpacked under $r/toolchains"; fail=1
	fi
	cmp -s "$r/.env" "$tmp/env.before" || { echo "FAIL: self-test: tampered archive: .env changed"; fail=1; }
	cmp -s "$r/.path" "$tmp/path.before" || { echo "FAIL: self-test: tampered archive: .path changed"; fail=1; }

	# 2. A target outside the runner directory is refused, by absolute path,
	#    by `..`, and through a symlink that points out of it.
	mkdir -p "$tmp/elsewhere"
	expect_refused "target outside, absolute" "outside the runner directory" \
		--runner-dir "$r" --target "$tmp/elsewhere/go" --archive "$fake"
	# Refused as holding '..' while toolchains/ does not exist, and as outside
	# once it does (the path then resolves); either way, refused.
	expect_refused "target outside, by .." "REFUSED: target" \
		--runner-dir "$r" --target "$r/toolchains/../../elsewhere/go" --archive "$fake"
	if ln -s "$tmp/elsewhere" "$r/escape" 2>/dev/null && [ -L "$r/escape" ]; then
		expect_refused "target outside, by symlink" "outside the runner directory" \
			--runner-dir "$r" --target "$r/escape/go" --archive "$fake"
	fi
	[ -z "$(ls -A "$tmp/elsewhere")" ] || { echo "FAIL: self-test: something was written outside the runner directory"; fail=1; }

	# 3. A directory that is not a runner, and a system path, are refused.
	mkdir -p "$tmp/not-a-runner"
	expect_refused "no .runner" "not a configured runner directory" --runner-dir "$tmp/not-a-runner" --archive "$fake"
	expect_refused "system path" "system path" --runner-dir / --archive "$fake"

	# 4. .env and .path: the variables are set, Go's bin is first once, an
	#    older pin's bin is dropped, and a second run changes nothing.
	DRY=0
	local env="$tmp/w/.env" pth="$tmp/w/.path" sep bin old
	mkdir -p "$tmp/w"
	sep=$(path_sep)
	bin="$tmp/w/toolchains/go$GO_VERSION/go/bin"
	old="$tmp/w/toolchains/go1.0.0/go/bin"
	printf 'CARGO_HOME=x\nGOFLAGS=-mod=mod\n' >"$env"
	printf '%s' "$old$sep/usr/bin$sep$bin$sep/bin" >"$pth"
	for k in GOROOT GOPATH GOMODCACHE GOCACHE; do set_env_key "$env" "$k" "$tmp/w/$k" >/dev/null; done
	set_env_key "$env" GOTOOLCHAIN local >/dev/null
	set_env_key "$env" GOFLAGS -mod=readonly >/dev/null
	put_path_first "$pth" "$bin" "$tmp/w/toolchains/go" >/dev/null
	grep -qx 'GOTOOLCHAIN=local' "$env" || { echo "FAIL: self-test: GOTOOLCHAIN=local not in .env"; fail=1; }
	grep -qx 'GOFLAGS=-mod=readonly' "$env" || { echo "FAIL: self-test: GOFLAGS not replaced in .env"; fail=1; }
	[ "$(grep -c '^GOFLAGS=' "$env")" = 1 ] || { echo "FAIL: self-test: GOFLAGS set twice in .env"; fail=1; }
	grep -qx 'CARGO_HOME=x' "$env" || { echo "FAIL: self-test: an unrelated .env line was lost"; fail=1; }
	[ "$(cat "$pth")" = "$bin$sep/usr/bin$sep/bin" ] \
		|| { echo "FAIL: self-test: .path is '$(cat "$pth")', want Go's bin first, once, and the old pin's gone"; fail=1; }
	cp "$env" "$tmp/env.1"
	cp "$pth" "$tmp/path.1"
	out=$(
		for k in GOROOT GOPATH GOMODCACHE GOCACHE; do set_env_key "$env" "$k" "$tmp/w/$k"; done
		set_env_key "$env" GOTOOLCHAIN local
		set_env_key "$env" GOFLAGS -mod=readonly
		put_path_first "$pth" "$bin" "$tmp/w/toolchains/go"
	)
	[ -z "$out" ] || { echo "FAIL: self-test: a second run reported a change: $out"; fail=1; }
	cmp -s "$env" "$tmp/env.1" && cmp -s "$pth" "$tmp/path.1" \
		|| { echo "FAIL: self-test: a second run changed .env or .path"; fail=1; }

	[ "$fail" -eq 0 ] || exit 1
	echo "OK: self-test (tampered archive refused before unpacking; targets outside the runner directory refused by path, '..' and symlink; non-runner and system directories refused; .env and .path set once and unchanged on a second run)"
}

DRY=0
if [ "${1:-}" = "--self-test" ]; then
	[ $# -eq 1 ] || { echo "$usage"; exit 2; }
	self_test
	exit 0
fi

runner_dir=""
target_dir=""
archive_file=""
while [ $# -gt 0 ]; do
	case "$1" in
	--runner-dir) [ $# -ge 2 ] || { echo "$usage"; exit 2; }; runner_dir=$2; shift 2 ;;
	--target) [ $# -ge 2 ] || { echo "$usage"; exit 2; }; target_dir=$2; shift 2 ;;
	--archive) [ $# -ge 2 ] || { echo "$usage"; exit 2; }; archive_file=$2; shift 2 ;;
	--dry-run) DRY=1; shift ;;
	*) echo "$usage"; exit 2 ;;
	esac
done
if [ -z "$runner_dir" ]; then
	# Where docs/BUILD_SYSTEM.md's runner table puts each runner after #219.
	if is_windows; then runner_dir=/c/actions-runner/win; else runner_dir="$HOME/actions-runner"; fi
fi
install "$runner_dir" "$target_dir" "$archive_file"
