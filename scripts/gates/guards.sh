#!/usr/bin/env bash
# G2b: the owner's deterministic guard suite (qa/run-guards.py), against the
# binary this change builds. Its two preconditions are checked, not assumed:
# the fresh debug binary first on PATH, and the demo's runtime reachable
# (dirty_worktree refuses for the wrong reason without it; CLAUDE.md).
source "$(dirname "$0")/lib.sh"
[ -n "$PY" ] || fail "no python3 or py on PATH; the guard suite cannot run"
cargo build -p trellis-cli
export PATH="$ROOT/target/debug:$PATH"
curl -fsS -o /dev/null --max-time 5 http://127.0.0.1:11434/api/tags \
  || fail "no model runtime at 127.0.0.1:11434 (start Ollama); dirty_worktree needs it"
out=$($PY qa/run-guards.py 2>&1) || true
git checkout -- qa/guards-latest.json 2>/dev/null || true
printf '%s\n' "$out" | tail -15
printf '%s' "$out" | grep -qE '^([0-9]+)/\1 guards held' || fail "a guard did not hold"
echo "OK: every guard held"
