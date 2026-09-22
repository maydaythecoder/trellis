---
command: qa-stuck
description: QA fixture — identical error every cycle, so the no-progress halt must fire
parameters:
  dir: "ignored"
---
1. Check the target
2. Run the project's check command:
   - `sh -c 'echo "E0308: mismatched types" >&2; exit 1'`
3. Halt on no progress
