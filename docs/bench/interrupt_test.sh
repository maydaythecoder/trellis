#!/usr/bin/env bash
# v1.0 gate condition 4, injection point 6: SIGINT mid-run.
#
# `04-v1.0-harness.md` calls Ctrl-C one of the two injection points usually
# missed, and it is the one that cannot be covered by a unit test: the
# question is whether a real signal, delivered to a real process mid-run,
# leaves the repository as it was.
#
# Runs on a POSIX host. It is deliberately NOT run on Windows: MSYS/Git Bash
# `kill -INT` does not raise a console Ctrl-C event for a native binary, so
# the signal never arrives and the test would pass by never testing anything.
#
# Determinism comes from the check command rather than from timing a real
# type checker: `.trellis/config.toml` configures `typecheck` as a 30-second
# sleep, so the run is reliably still going when the signal lands.
set -euo pipefail

TRELLIS="${1:?usage: interrupt_test.sh /path/to/trellis}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
# The log lives outside the repository: an untracked file inside it would
# make the worktree dirty, and TRELLIS refuses to start on a dirty worktree.
LOG="$WORK/run.log"
DIR="$WORK/repo"
mkdir -p "$DIR"

cd "$DIR"
git init -q .
git config user.email t@example.invalid
git config user.name t
git config core.autocrlf false
mkdir -p app
printf 'def helper():\n    return 1\n' > app/util.py

mkdir -p .trellis
cat > .trellis/config.toml <<'CFG'
[checks]
typecheck = ["python3", "-c", "import time; time.sleep(30)"]

[budget]
max_iterations = 50
max_seconds = 300
CFG

cat > script.json <<'JSON'
{"steps":[
 {"tool":"edit","path":"app/util.py","old":"return 1","new":"return 2"},
 {"tool":"run_checks","paths":["app/util.py"],"kinds":["typecheck"]},
 {"tool":"done","summary":"done"}
]}
JSON

git add -A
git commit -qm init

"$TRELLIS" run --model "scripted:$DIR/script.json" "interrupt me" > "$LOG" 2>&1 &
PID=$!

# Wait for the edit to reach disk, so the interrupt catches a run that has
# actually written something. Polled through `git status` rather than the log:
# the step list is printed when the run ends, so nothing appears in the log
# while the run is still going — which is the only moment this test cares
# about.
for _ in $(seq 1 100); do
  [ -n "$(git status --porcelain)" ] && break
  sleep 0.1
done
if [ -z "$(git status --porcelain)" ]; then
  echo "FAIL: nothing was written, so there is nothing to interrupt"
  cat "$LOG"
  exit 1
fi

kill -INT "$PID"
set +e
wait "$PID"
CODE=$?
set -e

echo "--- run output ---"
cat "$LOG"

if [ "$CODE" -ne 130 ]; then
  echo "FAIL: expected exit 130 (SIGINT), got $CODE"
  exit 1
fi
if [ -n "$(git status --porcelain)" ]; then
  echo "FAIL: the worktree still holds the run's edits:"
  git status --porcelain
  exit 1
fi
if [ "$(cat app/util.py)" != "$(printf 'def helper():\n    return 1\n')" ]; then
  echo "FAIL: the file was not restored"
  exit 1
fi
if [ -z "$(ls .trellis/runs/*.json 2>/dev/null)" ]; then
  echo "FAIL: an interrupted run must still be recorded"
  exit 1
fi

echo "PASS: SIGINT rolled back, left a clean worktree, and recorded the run."
