#!/usr/bin/env bash
# cloud-setup.sh: make a Claude Code cloud session able to run every gate.
#
#   scripts/cloud-setup.sh
#
# Run once at the start of a cloud session, from the repository (docs/RUNNER.md).
# A cloud container ships a Go older than tui/go.mod's toolchain line, and
# lib.sh holds every gate to GOTOOLCHAIN=local, so G1 fails there. This
# fetches the pinned toolchain through the Go module proxy (which checks it
# against sum.golang.org) and links it ahead of the system Go. It does not
# touch the owner's host: on a machine whose go already matches, it changes
# nothing.
#
# Commits are the owner's: no agent identity and no co-author trailer (owner
# decision 2026-10-26). The session's git identity is set to the owner's.
#
# Exit 0: go matches the pin and `gh api` reaches the repository. Exit 1: one
# of them does not, and the reason is printed.
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
cd "$ROOT"

pin=$(sed -n 's/^toolchain \(go[0-9.]*\)$/\1/p' tui/go.mod)
[ -n "$pin" ] || { echo "FAIL: tui/go.mod has no toolchain line"; exit 1; }

if [ "$(go env GOVERSION 2>/dev/null || true)" != "$pin" ]; then
  goroot=$(cd / && GOTOOLCHAIN="$pin" go env GOROOT) \
    || { echo "FAIL: could not fetch $pin through the module proxy"; exit 1; }
  mkdir -p "$HOME/.local/bin"
  ln -sf "$goroot/bin/go" "$HOME/.local/bin/go"
  ln -sf "$goroot/bin/gofmt" "$HOME/.local/bin/gofmt"
  case ":$PATH:" in *":$HOME/.local/bin:"*) ;; *)
    echo "FAIL: $HOME/.local/bin is not on PATH, so $pin is not the go the gates run"; exit 1 ;;
  esac
fi
got=$(go env GOVERSION)
[ "$got" = "$pin" ] || { echo "FAIL: go is $got, tui/go.mod pins $pin"; exit 1; }

# The owner's GitHub noreply address, as on the merges GitHub makes for them.
git config user.name "muhyadin mohamed"
git config user.email "99102186+maydaythecoder@users.noreply.github.com"

# G2b's dirty_worktree guard needs 127.0.0.1:11434 to accept a connection
# (RuntimeClient::reachable is a TCP connect), and a cloud session has no
# model runtime. A plain listener serving an empty directory, as ci.yml's
# guard job uses; nothing answers as a model, so no guard can pass on one.
if ! python3 -c 'import socket; socket.create_connection(("127.0.0.1", 11434), timeout=1).close()' 2>/dev/null; then
  nohup python3 -m http.server 11434 --bind 127.0.0.1 --directory "$(mktemp -d)" >/dev/null 2>&1 &
  for _ in $(seq 50); do
    python3 -c 'import socket; socket.create_connection(("127.0.0.1", 11434), timeout=1).close()' 2>/dev/null && break
    sleep 0.2
  done
  python3 -c 'import socket; socket.create_connection(("127.0.0.1", 11434), timeout=1).close()' 2>/dev/null \
    || { echo "FAIL: no listener on 127.0.0.1:11434 after 10s"; exit 1; }
fi

# REST only: cloud sessions refuse GraphQL, so `gh issue view` fails there (#924).
gh api 'repos/{owner}/{repo}' --jq .full_name >/dev/null \
  || { echo "FAIL: gh api cannot reach the repository"; exit 1; }

echo "OK: go $got, git identity set, a listener on 127.0.0.1:11434, gh api reaches the repository"
