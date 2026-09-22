---
command: test-loop
description: Autonomous self-verifying loop over the target directory (5 cycles max)
parameters:
  dir: "target path to check"
---
1. Read the file layout in {{dir}}
2. Run the project's check command:
   - `cargo fmt --all -- --check`
   - `cargo clippy --all-targets -- -D warnings`
   - `cargo test --workspace`
3. For each error: generate a fix, re-verify with local tests
4. Append the cycle's error set to `/mnt/c/Users/mayda/Documents/dev/trellis/.harness/loops/test-loop.json` before starting the next
5. Halt at zero errors, at 5 cycles, or when two cycles produce an
   identical error set
6. Stop at the human gate and report; nothing lands without it
