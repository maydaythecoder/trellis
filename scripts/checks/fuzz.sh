#!/usr/bin/env bash
# fuzz.sh: run one cargo-fuzz target for a bounded time.
#
#   scripts/checks/fuzz.sh <target> <seconds>
#
# Runs `cargo +nightly fuzz run <target> -- -max_total_time=<seconds>` in
# fuzz/ and prints libFuzzer's `Done N runs` line, which is the iteration
# count CORRECTNESS_LEDGER.md records. A crash leaves its input under
# fuzz/artifacts/<target>/; commit it as a regression test in the crate that
# owns the parser.
#
# Windows: the ASan build links clang_rt.asan_dynamic-x86_64.dll, which ships
# in MSVC's bin\HostX64\x64 and is not on PATH by default; without it the
# fuzzer exits STATUS_DLL_NOT_FOUND before running once. That directory is
# found through vswhere.exe and prepended. The libFuzzer C++ build also fails
# under a long target path, so unless CARGO_TARGET_DIR is set the build goes
# to a short one at the drive root.
#
# Exit 0: the run finished with no crash. Exit 1: tooling missing, the build
# failed, or the fuzzer found a crash. Exit 2: wrong arguments.
set -uo pipefail

usage() {
  echo "usage: fuzz.sh <target> <seconds>"
  exit 2
}
[ $# -eq 2 ] || usage
target="$1" secs="$2"
case "$secs" in '' | *[!0-9]*) usage ;; esac
[ -n "$target" ] || usage

ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || { echo "not inside a git repository"; exit 2; }

if ! cargo fuzz --version >/dev/null 2>&1; then
  echo "FAIL: cargo fuzz is not installed."
  echo "  install: cargo install cargo-fuzz --locked"
  echo "  it also needs a nightly toolchain: rustup toolchain install nightly"
  exit 1
fi

case "$(uname -s)" in
  MINGW* | MSYS* | CYGWIN*)
    vswhere="${ProgramFiles:-C:\Program Files} (x86)"
    vswhere="$(cygpath -u "$vswhere")/Microsoft Visual Studio/Installer/vswhere.exe"
    if [ ! -x "$vswhere" ]; then
      echo "FAIL: vswhere.exe not found at $vswhere; the ASan runtime cannot be located."
      echo "  install the Visual Studio Build Tools with the C++ workload."
      exit 1
    fi
    # The highest MSVC version that ships the runtime; vswhere lists in
    # directory order, so the last line is the newest.
    dll=$("$vswhere" -latest -products '*' \
      -find 'VC\Tools\MSVC\**\bin\HostX64\x64\clang_rt.asan_dynamic-x86_64.dll' | tr -d '\r' | tail -n 1)
    if [ -z "$dll" ]; then
      echo "FAIL: no MSVC bin\HostX64\x64 directory with clang_rt.asan_dynamic-x86_64.dll."
      echo "  install the MSVC x64 build tools (they include AddressSanitizer)."
      exit 1
    fi
    asan_dir="$(dirname "$(cygpath -u "$dll")")"
    export PATH="$asan_dir:$PATH"
    echo "ASan runtime: $asan_dir"
    export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-${SystemDrive:-C:}/trellis-fuzz-target}"
    ;;
esac

log="$(mktemp)"
trap 'rm -f "$log"' EXIT
cd "$ROOT/fuzz" || exit 1
# libFuzzer reports on stderr; the log is kept so the summary line can be
# printed again at the end, after the build output that would bury it.
cargo +nightly fuzz run "$target" -- -max_total_time="$secs" 2>&1 | tee "$log"
status=${PIPESTATUS[0]}

done_line=$(grep -E '^Done [0-9]+ runs' "$log" | tail -n 1)
echo
if [ "$status" -ne 0 ]; then
  echo "FAIL: cargo fuzz run $target exited $status"
  [ -n "$done_line" ] && echo "$done_line"
  exit 1
fi
if [ -z "$done_line" ]; then
  echo "FAIL: libFuzzer printed no 'Done N runs' line"
  exit 1
fi
echo "$done_line"
echo "OK: $target ran ${secs}s with no crash"
