---
command: qa-cap
description: QA fixture — fails with a fresh error each cycle, so only the cap can stop it
parameters:
  dir: "ignored"
---
1. Check the target
2. Run the project's check command:
   - `sh -c 'echo "unique failure $(date +%s%N)" >&2; exit 1'`
3. Halt at the cap
