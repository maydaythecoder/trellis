#!/usr/bin/env bash
# G5: licences and advisories clean, workflows pass zizmor and build --locked,
# gitleaks finds no secret in the change's commits, and no key-shaped
# literal added.
#
# gitleaks scans every commit in the range, so a secret added and removed
# again inside the change is still found (#236). The key-shape scan below it
# looks only at the net diff's added lines, for the shapes providers and
# platforms actually issue and for credentials written into a URL; it is
# kept because its patterns are the pre-commit hook's, and GitGuardian still
# runs on the PR.
source "$(dirname "$0")/lib.sh"
# --locked: without it cargo deny runs `cargo metadata` in a mode that
# rewrites a stale lock and then passes on the rewritten one, which leaves
# the checkout dirty and the lock unreviewed (#197; shown on a copy of
# fuzz/ with a package cut from its lock).
cargo deny --locked check
cargo audit
# fuzz/ is its own workspace with its own lock (issue #95), so the two lines
# above never see libfuzzer-sys or what it pulls in. deny.toml at the root
# applies to it: cargo deny looks upward from the manifest for its policy.
# Its lock is not updated when a crate it reaches gains a dependency (#172
# did that), so it is checked for currency first, by name, before the two
# audits of it.
scripts/checks/fuzz-lock-current.sh
cargo deny --locked --manifest-path fuzz/Cargo.toml check
cargo audit --file fuzz/Cargo.lock
# The workflows, as the pre-commit hook audits them but over the whole
# directory, so a workflow change that reached the branch without the hook
# (another clone, --no-verify) is still held to it (#215). zizmor at medium
# and above: unpinned actions, token scope, persisted credentials, cache
# poisoning, injection. It installs where git's PATH does not look (pipx, go
# install), as the hook notes; absent, the gate fails rather than skips.
for d in "$HOME/go/bin" "$HOME/.local/bin"; do
  [ -d "$d" ] && PATH="$PATH:$d"
done
command -v zizmor >/dev/null 2>&1 \
  || fail "zizmor not installed (pipx install zizmor); G5 cannot audit .github/workflows/ without it"
zizmor --min-severity medium .github/workflows/ \
  || fail "zizmor found a medium-or-higher finding in .github/workflows/ (listed above)"
# gitleaks over the commits this change adds, the scan the pre-commit hook
# runs only where gitleaks is installed and only on commits made through the
# hook (#236). Absent, it fails rather than skips. The self-test first: it
# plants a token built at run time on a branch of a throwaway repository and
# shows the scan refuses it, and that an unresolvable base fails rather than
# reporting no leaks, which is what gitleaks alone does.
scripts/checks/gitleaks-range.sh --self-test
scripts/checks/gitleaks-range.sh "$GATE_BASE"
# Every cargo build/test/clippy/run in a workflow passes --locked, so CI
# builds what Cargo.lock records. The self-test first: a check that cannot
# fail cannot pass.
scripts/checks/workflows-locked.sh --self-test
scripts/checks/workflows-locked.sh
# CI keeps every job its base had, and the hooks and ci-local still do what
# their self-tests say. Nothing ran these four before #382, and the ci-local
# self-test sat failing for a branch rename (#378) unnoticed. Each prints
# only on failure, so a passing G5 stays quiet.
out=$(bash scripts/checks/ci-jobs-unchanged.sh "$GATE_BASE" 2>&1) \
  || { printf '%s\n' "$out"; fail "ci-jobs-unchanged.sh failed against $GATE_BASE: a CI job was dropped or the base has no ci.yml (above)"; }
out=$(bash scripts/checks/pre-push-selftest.sh 2>&1) \
  || { printf '%s\n' "$out"; fail "pre-push hook self-test failed (pre-push-selftest.sh, above)"; }
out=$(bash scripts/checks/pre-commit-selftest.sh 2>&1) \
  || { printf '%s\n' "$out"; fail "pre-commit hook self-test failed (pre-commit-selftest.sh, above)"; }
out=$(bash scripts/checks/ci-local-selftest.sh 2>&1) \
  || { printf '%s\n' "$out"; fail "ci-local self-test failed (ci-local-selftest.sh, above)"; }
# trellis-cli's test fixtures spawn git only through trellis_vcs, with the
# exit status asserted; tests/hygiene.rs exempts test code, so this is the
# check that keeps a bare spawn from coming back (#240, #241).
scripts/checks/no-bare-git-in-cli-tests.sh --self-test
scripts/checks/no-bare-git-in-cli-tests.sh
# The pattern is shared with the pre-commit hook, so the two cannot drift
# (#206).
keys=$(scripts/checks/key-pattern.sh)
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
echo "OK: fuzz/Cargo.lock current; cargo deny, cargo audit (root and fuzz/ locks); zizmor clean and every workflow cargo command --locked; no CI job lost; pre-push, pre-commit and ci-local self-tests pass; gitleaks clean over $GATE_BASE..HEAD; no key-shaped literal or basic-auth URL added"
