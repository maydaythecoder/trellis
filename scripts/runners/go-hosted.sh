#!/usr/bin/env bash
# go-hosted.sh: put the pinned Go toolchain on PATH for one job on a
# GitHub-hosted runner, from the same go.dev archive and SHA-256 that
# install-go.sh records for the self-hosted runners (#503, ADR 0052
# decision 2.4). One pin, one set of hashes, read from install-go.sh.
#
#   scripts/runners/go-hosted.sh          # in a workflow step, bash shell
#   scripts/runners/go-hosted.sh --self-test
#
# Why not actions/setup-go: it does not pin the toolchain to a hash this
# repository records, and scripts/ci-local.py refuses it. Why not
# install-go.sh itself: it installs into a runner directory that holds
# `.runner`, which a hosted runner's job does not have.
#
# What it does: downloads go<PIN> for this platform into $RUNNER_TEMP,
# refuses before unpacking unless the archive's SHA-256 is the recorded
# one, unpacks it there, then appends <target>/go/bin to $GITHUB_PATH and
# GOROOT, GOTOOLCHAIN=local and GOFLAGS=-mod=readonly to $GITHUB_ENV, so
# every later step in the job sees the pin. Nothing lands outside
# $RUNNER_TEMP. Exit 0: installed. Exit 1: a refusal or a failed step.
set -euo pipefail

here=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
pin_file="$here/install-go.sh"

die() { echo "go-hosted: REFUSED: $*" >&2; exit 1; }

# The three values, read from install-go.sh so the two scripts cannot drift.
read_pin() {
	local name
	name=$1
	grep -E "^${name}=\"[^\"]+\"$" "$pin_file" | head -n1 | sed -E 's/^[^=]+="([^"]+)"$/\1/'
}

# Run from the file's own directory on a bare name: given a path with a
# backslash in it, sha256sum escapes the name and prefixes the whole line
# with `\`, which read as a wrong hash on the first Windows run.
sha256_of() {
	local dir name
	dir=$(dirname "$1")
	name=$(basename "$1")
	if command -v sha256sum >/dev/null 2>&1; then
		(cd "$dir" && sha256sum -- "$name") | cut -d' ' -f1
	elif command -v shasum >/dev/null 2>&1; then
		(cd "$dir" && shasum -a 256 -- "$name") | cut -d' ' -f1
	else
		die "no sha256sum or shasum on this runner"
	fi
}

is_windows() {
	case "$(uname -s)" in MINGW* | MSYS* | CYGWIN*) return 0 ;; *) return 1 ;; esac
}

if [ "${1:-}" = "--self-test" ]; then
	v=$(read_pin GO_VERSION)
	l=$(read_pin SHA256_LINUX_AMD64)
	w=$(read_pin SHA256_WINDOWS_AMD64)
	[ -n "$v" ] || die "self-test: GO_VERSION not found in install-go.sh"
	[ "${#l}" = 64 ] && [ "${#w}" = 64 ] || die "self-test: a hash in install-go.sh is not 64 hex characters"
	echo "go-hosted: self-test ok (pin go$v)"
	exit 0
fi

[ -n "${RUNNER_TEMP:-}" ] || die "RUNNER_TEMP is unset; this runs inside a GitHub Actions job"
# Git Bash hands RUNNER_TEMP over as D:\a\_temp; work with the POSIX form.
if is_windows; then RUNNER_TEMP=$(cygpath -u "$RUNNER_TEMP"); fi
[ -n "${GITHUB_PATH:-}" ] && [ -n "${GITHUB_ENV:-}" ] || die "GITHUB_PATH or GITHUB_ENV is unset; this runs inside a GitHub Actions job"

version=$(read_pin GO_VERSION)
[ -n "$version" ] || die "GO_VERSION not found in install-go.sh"

case "$(uname -m)" in x86_64 | amd64) ;; *) die "machine $(uname -m) is not amd64; no hash is recorded for it" ;; esac
if is_windows; then
	archive="go${version}.windows-amd64.zip"
	want=$(read_pin SHA256_WINDOWS_AMD64)
else
	case "$(uname -s)" in Linux) ;; *) die "OS $(uname -s) has no recorded hash (Linux and Windows only)" ;; esac
	archive="go${version}.linux-amd64.tar.gz"
	want=$(read_pin SHA256_LINUX_AMD64)
fi

target="$RUNNER_TEMP/go${version}"
mkdir -p "$target"
file="$target/$archive"
curl -sSfL -o "$file" "https://go.dev/dl/$archive"
got=$(sha256_of "$file")
[ "$got" = "$want" ] || die "$archive: SHA-256 $got is not the recorded $want; nothing unpacked"

if is_windows; then
	# bsdtar, which ships with Windows and unpacks zip; Git Bash's own tar
	# does not.
	/c/Windows/System32/tar.exe -xf "$(cygpath -w "$file")" -C "$(cygpath -w "$target")"
	goroot=$(cygpath -w "$target/go")
	gobin=$(cygpath -w "$target/go/bin")
else
	tar -xzf "$file" -C "$target"
	goroot="$target/go"
	gobin="$target/go/bin"
fi

echo "$gobin" >>"$GITHUB_PATH"
{
	echo "GOROOT=$goroot"
	echo "GOTOOLCHAIN=local"
	echo "GOFLAGS=-mod=readonly"
} >>"$GITHUB_ENV"
echo "go-hosted: go$version unpacked under RUNNER_TEMP (sha256 verified); on PATH for the rest of the job"
