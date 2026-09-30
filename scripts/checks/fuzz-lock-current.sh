#!/usr/bin/env bash
# fuzz-lock-current.sh: fuzz/Cargo.lock records every package fuzz/ resolves
# to, so no build of fuzz/ rewrites it.
#
#   scripts/checks/fuzz-lock-current.sh
#
# fuzz/ is its own workspace (issue #95), so a dependency added to a crate
# the fuzz target reaches updates the root lock and not this one. #172 did
# that; the next gate run that built fuzz/ rewrote the lock, left the
# lander's checkout dirty, and every queued ticket after it was refused as
# a dirty worktree (#197).
#
# The check runs `cargo metadata --locked` in fuzz/, which fails instead of
# writing when the lock would change. The lock's hash is compared before and
# after as well, so a cargo that wrote anyway is caught rather than trusted.
#
# Then it proves itself on a copy, never on the working tree: the root
# manifest, rust-toolchain.toml, crates/ and fuzz/ go to a temporary
# directory (the path dependencies inherit from the root workspace, so fuzz/
# alone does not resolve). The untouched copy must pass, which shows the
# copy resolves at all; the copy with libfuzzer-sys cut from its lock must
# fail with cargo's lock message, not with some other error.
#
# Exit 0: the lock is current and the self-test held. Exit 1: the lock is
# stale, cargo could not run, or the self-test did not hold. Exit 2: wrong
# arguments, or not in a git repository.
set -uo pipefail

[ $# -eq 0 ] || { echo "usage: fuzz-lock-current.sh"; exit 2; }
ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || { echo "FAIL: not in a git repository"; exit 2; }
cd "$ROOT" || exit 2

# What cargo prints when --locked stops it rewriting the lock. Cargo 1.93
# says "cannot update the lock file <path> because --locked was passed to
# prevent this"; earlier releases said "the lock file <path> needs to be
# updated but --locked was passed to prevent this". The pattern takes both.
# Matching it is what separates "stale" from "cargo could not run" (no
# network, bad path).
STALE_MSG='lock file .* --locked was passed to prevent this'

tmp="$(mktemp -d)" || { echo "FAIL: mktemp -d failed"; exit 1; }
trap 'rm -rf "$tmp"' EXIT

# Runs `cargo metadata --locked` in the fuzz/ directory under $1, with no
# path arguments so Git Bash has nothing to convert. Leaves cargo's stderr
# in $tmp/err and returns cargo's exit code.
locked_metadata() {
  (cd "$1/fuzz" && cargo metadata --locked --format-version 1 >/dev/null 2>"$tmp/err")
}

# 1. The lock in this checkout.
[ -f fuzz/Cargo.lock ] || { echo "FAIL: fuzz/Cargo.lock does not exist"; exit 1; }
before="$(git hash-object fuzz/Cargo.lock)"
locked_metadata "$ROOT"
rc=$?
after="$(git hash-object fuzz/Cargo.lock)"
if [ "$before" != "$after" ]; then
  echo "FAIL: fuzz/Cargo.lock changed during a --locked cargo metadata ($before -> $after)"
  exit 1
fi
if [ "$rc" -ne 0 ]; then
  if grep -qE -- "$STALE_MSG" "$tmp/err"; then
    echo "FAIL: fuzz/Cargo.lock is stale: building fuzz/ would rewrite it"
    echo "  fix: (cd fuzz && cargo metadata --format-version 1 >/dev/null), then commit fuzz/Cargo.lock on its own"
  else
    echo "FAIL: cargo metadata --locked in fuzz/ failed ($rc) for a reason other than a stale lock"
  fi
  sed 's/^/  /' "$tmp/err"
  exit 1
fi

# 2. A copy laid out as the repository is, so the relative path dependencies
# and the workspace inheritance behind them resolve the same way.
copy="$tmp/repo"
mkdir -p "$copy/fuzz" || { echo "FAIL: could not create $copy"; exit 1; }
cp Cargo.toml rust-toolchain.toml "$copy/" \
  && cp -r crates "$copy/" \
  && cp -r fuzz/Cargo.toml fuzz/fuzz_targets "$copy/fuzz/" \
  && tr -d '\r' < fuzz/Cargo.lock > "$copy/fuzz/Cargo.lock" \
  || { echo "FAIL: could not copy the repository to $copy"; exit 1; }

if ! locked_metadata "$copy"; then
  echo "FAIL: self-test: the unmodified copy of fuzz/ does not resolve, so the stale case below would prove nothing"
  sed 's/^/  /' "$tmp/err"
  exit 1
fi

# 3. The copy with libfuzzer-sys, fuzz/'s one direct registry dependency,
# cut from its lock: what #172 left behind, a package the manifest reaches
# and the lock does not record. Blocks in Cargo.lock are blank-line
# separated, so paragraph mode drops exactly one.
awk 'BEGIN { RS = ""; ORS = "\n\n" } !/\nname = "libfuzzer-sys"\n/' \
  "$copy/fuzz/Cargo.lock" > "$tmp/stale.lock" \
  && mv "$tmp/stale.lock" "$copy/fuzz/Cargo.lock" \
  || { echo "FAIL: self-test: could not write the staled copy"; exit 1; }
if grep -qF 'name = "libfuzzer-sys"' "$copy/fuzz/Cargo.lock"; then
  echo "FAIL: self-test: libfuzzer-sys is still in the staled copy of the lock"
  exit 1
fi
if locked_metadata "$copy"; then
  echo "FAIL: self-test: a copy of fuzz/Cargo.lock without libfuzzer-sys passed --locked"
  exit 1
fi
if ! grep -qE -- "$STALE_MSG" "$tmp/err"; then
  echo "FAIL: self-test: the staled copy failed, but not with cargo's stale-lock message"
  sed 's/^/  /' "$tmp/err"
  exit 1
fi

echo "OK: fuzz/Cargo.lock is current (self-test: an unmodified copy passes, a copy without libfuzzer-sys fails as stale)"
