#!/usr/bin/env bash
# install-local.sh: install trellis and trellis-desktop into one directory.
#
#   scripts/install-local.sh [--force] [--dry-run]
#   scripts/install-local.sh --self-test
#   scripts/install-local.sh --help
#
# Why both, and why together: `trellis` finds the desktop interface only as
# `trellis-desktop` beside its own current_exe() (#374, #561); it does not
# search PATH. release.yml builds only `-p trellis-cli`, so a plain
# `cargo install` of the CLI leaves `trellis desktop` with nothing to find.
# This installs both crates with cargo, into cargo's one bin directory, and
# then checks that the two executables really are side by side.
#
# Why the builds are capped and sequential: a cold build of either crate at
# full parallelism exhausts the Windows commit limit, and rustc then fails
# with os error 1455 or "invalid metadata" errors that look like a corrupt
# toolchain. CARGO_BUILD_JOBS defaults to 2 here unless already set, and the
# two installs run one after the other, never at once.
#
# --force is passed to both installs, so a reinstall replaces the old copies.
# --dry-run prints the exact commands, CARGO_BUILD_JOBS included, and runs
# nothing. On Linux the desktop needs WebKitGTK headers, so before building
# this runs `pkg-config --exists webkit2gtk-4.1` and names the packages when
# it fails.
#
# Exit 0: both installed and side by side (or the dry run / self-test held).
# Exit 1: a missing build dependency, a failed install, the executables not
# found together, or the self-test did not hold. Exit 2: wrong arguments.
set -euo pipefail

usage='usage: install-local.sh [--force] [--dry-run] | --self-test | --help'
deb_pkgs='libwebkit2gtk-4.1-dev libxdo-dev pkg-config'

if [ "${1:-}" = "--self-test" ]; then
  [ $# -eq 1 ] || { echo "$usage" >&2; exit 2; }
  self="$(cd "$(dirname "$0")" && pwd)/$(basename "$0")"
  tmp=$(mktemp -d) || { echo "FAIL: mktemp failed"; exit 1; }
  trap 'rm -rf "$tmp"' EXIT
  # A cargo and a pkg-config that record being called: the self-test must
  # never build, and a dry run must run nothing.
  for tool in cargo pkg-config; do
    printf '#!/bin/sh\necho "%s $*" >> "%s/called"\nexit 0\n' "$tool" "$tmp" > "$tmp/$tool"
    chmod +x "$tmp/$tool"
  done
  fail=0
  run() { PATH="$tmp:$PATH" bash "$self" "$@" 2>&1; }
  has() {
    printf '%s\n' "$1" | grep -qxF -- "$2" \
      || { echo "FAIL: self-test: expected line missing: $2"; echo "  got:"; printf '%s\n' "$1" | sed 's/^/    /'; fail=1; }
  }
  code() {
    local want=$1 got
    shift
    got=$( (PATH="$tmp:$PATH" bash "$self" "$@") >/dev/null 2>&1; echo $?)
    [ "$got" = "$want" ] || { echo "FAIL: self-test: install-local.sh $* exited $got, want $want"; fail=1; }
  }
  cli='cargo install --locked --path crates/trellis-cli'
  desk='cargo install --locked --path desktop'

  out=$(unset CARGO_BUILD_JOBS; run --dry-run) || { echo "FAIL: self-test: --dry-run exited non-zero"; fail=1; }
  has "$out" "CARGO_BUILD_JOBS=2 $cli"
  has "$out" "CARGO_BUILD_JOBS=2 $desk"
  # The CLI installs first, then the desktop.
  [ "$(printf '%s\n' "$out" | grep -n -F "$cli" | cut -d: -f1)" \
    -lt "$(printf '%s\n' "$out" | grep -n -F "$desk" | cut -d: -f1)" ] 2>/dev/null \
    || { echo "FAIL: self-test: the CLI is not installed before the desktop"; fail=1; }
  printf '%s\n' "$out" | grep -q -e '--force' \
    && { echo "FAIL: self-test: --force printed without being given"; fail=1; }

  out=$(unset CARGO_BUILD_JOBS; run --force --dry-run) || { echo "FAIL: self-test: --force --dry-run exited non-zero"; fail=1; }
  has "$out" "CARGO_BUILD_JOBS=2 $cli --force"
  has "$out" "CARGO_BUILD_JOBS=2 $desk --force"

  out=$(CARGO_BUILD_JOBS=4 run --dry-run --force) || { echo "FAIL: self-test: dry run with CARGO_BUILD_JOBS=4 exited non-zero"; fail=1; }
  has "$out" "CARGO_BUILD_JOBS=4 $cli --force"
  has "$out" "CARGO_BUILD_JOBS=4 $desk --force"

  code 2 --bogus
  code 2 --dry-run --bogus
  code 2 extra-argument
  code 2 --self-test --dry-run
  code 0 --help
  out=$(run --help)
  has "$out" "$usage"
  [ ! -e "$tmp/called" ] || { echo "FAIL: self-test: a tool was run:"; sed 's/^/  /' "$tmp/called"; fail=1; }
  [ "$fail" -eq 0 ] || exit 1
  echo "OK: self-test (dry run prints both installs in order with the CARGO_BUILD_JOBS cap, default 2 and an inherited value kept; --force reaches both; unknown flags exit 2; --help works; nothing was run)"
  exit 0
fi

force=()
dry=0
for arg in "$@"; do
  case "$arg" in
    --force) force=(--force) ;;
    --dry-run) dry=1 ;;
    -h|--help) echo "$usage"; exit 0 ;;
    *) echo "install-local.sh: unknown argument: $arg" >&2; echo "$usage" >&2; exit 2 ;;
  esac
done

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
[ -f crates/trellis-cli/Cargo.toml ] && [ -f desktop/Cargo.toml ] \
  || { echo "FAIL: $root is not the trellis repository root (no crates/trellis-cli or desktop)"; exit 1; }

jobs="${CARGO_BUILD_JOBS:-2}"
export CARGO_BUILD_JOBS="$jobs"
cmds=(crates/trellis-cli desktop)
os=$(uname -s)

if [ "$dry" -eq 1 ]; then
  [ "$os" = Linux ] && echo "pkg-config --exists webkit2gtk-4.1"
  for path in "${cmds[@]}"; do
    echo "CARGO_BUILD_JOBS=$jobs cargo install --locked --path $path${force[*]:+ ${force[*]}}"
  done
  exit 0
fi

if [ "$os" = Linux ]; then
  if ! pkg-config --exists webkit2gtk-4.1; then
    echo "FAIL: webkit2gtk-4.1 not found by pkg-config; the desktop cannot build without it."
    echo "  On Debian or Ubuntu: sudo apt-get install $deb_pkgs"
    exit 1
  fi
fi

for path in "${cmds[@]}"; do
  echo "+ CARGO_BUILD_JOBS=$jobs cargo install --locked --path $path${force[*]:+ ${force[*]}}"
  cargo install --locked --path "$path" ${force[@]+"${force[@]}"} \
    || { echo "FAIL: cargo install --path $path failed"; exit 1; }
done

# cargo installs to $CARGO_INSTALL_ROOT/bin, else $CARGO_HOME/bin, else
# ~/.cargo/bin.
if [ -n "${CARGO_INSTALL_ROOT:-}" ]; then
  bin="$CARGO_INSTALL_ROOT/bin"
elif [ -n "${CARGO_HOME:-}" ]; then
  bin="$CARGO_HOME/bin"
else
  bin="$HOME/.cargo/bin"
fi
exe=
case "$os" in
  MINGW*|MSYS*|CYGWIN*)
    exe=.exe
    command -v cygpath >/dev/null 2>&1 && bin=$(cygpath -u "$bin")
    ;;
esac
missing=
for name in trellis trellis-desktop; do
  [ -f "$bin/$name$exe" ] || missing="$missing $name$exe"
done
if [ -n "$missing" ]; then
  echo "FAIL: not found in $bin:$missing"
  echo "  trellis finds trellis-desktop only in its own directory; both must be in one bin directory."
  exit 1
fi
echo "OK: trellis$exe and trellis-desktop$exe are side by side in $bin"
