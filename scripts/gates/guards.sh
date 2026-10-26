#!/usr/bin/env bash
# G2b: the owner's deterministic guard suite (qa/run-guards.py), against the
# binary this change builds. Its two preconditions are checked, not assumed:
# the fresh debug binary first on PATH, and the demo's runtime reachable
# (dirty_worktree refuses for the wrong reason without it; CLAUDE.md).
source "$(dirname "$0")/lib.sh"
[ -n "$PY" ] || fail "no python3 or py on PATH; the guard suite cannot run"
cargo build -p trellis-cli
# The shell's own form of the root (lib.sh has cd'd there): in Git Bash
# `$ROOT` is `C:/...`, whose colon splits a PATH entry, and the suite then
# ran whichever `trellis` was installed instead of this build (#158).
debug="$(pwd)/target/debug"
export PATH="$debug:$PATH"
found=$(command -v trellis || true)
case "$found" in
  "$debug"/trellis|"$debug"/trellis.exe) ;;
  *) fail "trellis resolves to '${found:-nothing}', not this build in $debug" ;;
esac
# A TCP connect, as RuntimeClient::reachable checks and as ci.yml's guard job
# relies on: dirty_worktree needs the port to answer, not a model behind it.
# So Ollama on the owner's host and scripts/cloud-setup.sh's plain listener
# in a cloud session both satisfy it (#924).
$PY -c 'import socket; socket.create_connection(("127.0.0.1", 11434), timeout=5).close()' 2>/dev/null \
  || fail "nothing listening on 127.0.0.1:11434 (start Ollama, or scripts/cloud-setup.sh in a cloud session); dirty_worktree needs it"
out=$($PY qa/run-guards.py 2>&1) || true
git checkout -- qa/guards-latest.json 2>/dev/null || true
printf '%s\n' "$out" | tail -15
printf '%s' "$out" | grep -qE '^([0-9]+)/\1 guards held' || fail "a guard did not hold"
echo "OK: every guard held"
