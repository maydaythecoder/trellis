#!/usr/bin/env bash
# G5: licences and advisories clean, and no key-shaped literal added.
#
# The key scan is a fallback for a real secret scanner (none is installed
# here; GitGuardian runs on the PR). It looks only at lines this change
# added, for the shapes providers and platforms actually issue.
source "$(dirname "$0")/lib.sh"
cargo deny check
cargo audit
# fuzz/ is its own workspace with its own lock (issue #95), so the two lines
# above never see libfuzzer-sys or what it pulls in. deny.toml at the root
# applies to it: cargo deny looks upward from the manifest for its policy.
cargo deny --manifest-path fuzz/Cargo.toml check
cargo audit --file fuzz/Cargo.lock
keys='(sk-(ant-|proj-|live-)?[A-Za-z0-9_-]{20,})|(AKIA[0-9A-Z]{16})|(gh[pousr]_[A-Za-z0-9]{36})|(xox[baprs]-[A-Za-z0-9-]{10,})|(-----BEGIN [A-Z ]*PRIVATE KEY-----)|(AIza[0-9A-Za-z_-]{35})'
hits=$(git diff -U0 "$GATE_BASE"...HEAD | grep -E '^\+[^+]' | grep -E "$keys" || true)
if [ -n "$hits" ]; then
  echo "FAIL: key-shaped literals added (build test canaries at run time instead):"
  printf '%s\n' "$hits"
  exit 1
fi
echo "OK: cargo deny, cargo audit (root and fuzz/ locks), no key-shaped literal added"
