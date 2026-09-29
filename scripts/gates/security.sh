#!/usr/bin/env bash
# G5: licences and advisories clean, and no key-shaped literal added.
#
# The key scan is a fallback for a real secret scanner (none is installed
# here; GitGuardian runs on the PR). It looks only at lines this change
# added, for the shapes providers and platforms actually issue, and for
# credentials written into a URL.
source "$(dirname "$0")/lib.sh"
cargo deny check
cargo audit
# fuzz/ is its own workspace with its own lock (issue #95), so the two lines
# above never see libfuzzer-sys or what it pulls in. deny.toml at the root
# applies to it: cargo deny looks upward from the manifest for its policy.
cargo deny --manifest-path fuzz/Cargo.toml check
cargo audit --file fuzz/Cargo.lock
keys='(sk-(ant-|proj-|live-)?[A-Za-z0-9_-]{20,})|(AKIA[0-9A-Z]{16})|(gh[pousr]_[A-Za-z0-9]{36})|(xox[baprs]-[A-Za-z0-9-]{10,})|(-----BEGIN [A-Z ]*PRIVATE KEY-----)|(AIza[0-9A-Za-z_-]{35})'
# A URL literal with `user:pass@` before its host is flagged by the PR's
# scanner as a basic-auth string even when it is a fake in a test (#185). The
# pattern lives in the check script, so the two cannot drift; its self-test
# shows it matches such a literal and not the `format!("http://u:{pw}@host")`
# form tests use instead.
scripts/checks/no-auth-urls.sh --self-test
auth=$(scripts/checks/no-auth-urls.sh --pattern)
added=$(git diff -U0 "$GATE_BASE"...HEAD | grep -E '^\+[^+]' || true)
hits=$(printf '%s\n' "$added" | grep -E "$keys" || true)
if [ -n "$hits" ]; then
  echo "FAIL: key-shaped literals added (build test canaries at run time instead):"
  printf '%s\n' "$hits"
  exit 1
fi
hits=$(printf '%s\n' "$added" | grep -E "$auth" || true)
if [ -n "$hits" ]; then
  echo "FAIL: basic-auth URL literals added (build them at run time instead):"
  printf '%s\n' "$hits"
  exit 1
fi
echo "OK: cargo deny, cargo audit (root and fuzz/ locks), no key-shaped literal or basic-auth URL added"
