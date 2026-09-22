---
command: qa-slow
description: QA fixture — each cycle sleeps, so /stop has a cycle to interrupt
parameters:
  dir: "ignored"
---
1. Check the target
2. Run the project's check command:
   - `sh -c 'sleep 4; echo "slow failure $(date +%s%N)" >&2; exit 1'`
3. Halt
