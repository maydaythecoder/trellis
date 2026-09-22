# QA run — baseline
date: 2026-09-21T23:24:03Z

## Decisions made without asking (§0)
- **No tmux/script on Windows git-bash.** WSL2 Ubuntu 24.04 has tmux 3.4, cargo, cc, 16 cores.
  TUI QA runs there. §2 satisfied without a hard stop.
- **Model work stays on Windows**, against ollama 0.34.2 which already has qwen3:8b.
  WSL has a separate, older ollama (0.15.6, llama3:latest only). Pulling qwen3 into WSL
  would be a second 5GB download; exposing Windows ollama to WSL needs OLLAMA_HOST=0.0.0.0
  plus a firewall hole. Neither is needed: §6 runs against the mock stream, §7 runs
  `toolcall_smoke` which is not a TUI and needs no pty.
- **WSL builds use CARGO_TARGET_DIR on WSL-native fs.** /mnt/c is 9p and slow; source stays
  in the repo so fixes land in the real tree.

## Host
```
MINGW64_NT-10.0-26200 Muhyadins_PC 3.6.6-1cdd4371.x86_64 2026-01-15 22:20 UTC x86_64 Msys
cores: 16
```

## WSL
```
Ubuntu 24.04.3 LTS
tmux 3.4
rustc 1.93.0 (254b59607 2026-01-19)
16
               total        used        free      shared  buff/cache   available
Mem:           7.3Gi       622Mi       6.1Gi       3.5Mi       817Mi       6.7Gi
/dev/sdd       1007G   32G  924G   4% /
```
