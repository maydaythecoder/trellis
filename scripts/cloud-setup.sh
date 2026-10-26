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

# G5's tools, at the versions ci.yml pins. Each install is skipped when that
# version is already there, so a second run in one session is quick.
installed() { command -v "$1" >/dev/null 2>&1 && "$@" 2>/dev/null | grep -qF "$VERSION"; }
VERSION=0.20.2; installed cargo-deny deny --version \
  || cargo install -q --locked cargo-deny --version "$VERSION" || { echo "FAIL: cargo-deny $VERSION"; exit 1; }
VERSION=0.22.2; installed cargo-audit audit --version \
  || cargo install -q --locked cargo-audit --version "$VERSION" || { echo "FAIL: cargo-audit $VERSION"; exit 1; }
python3 -m pip install -q --root-user-action=ignore --require-hashes -r .github/requirements-zizmor.txt \
  || { echo "FAIL: could not install .github/requirements-zizmor.txt"; exit 1; }
# Outside tui/, so `go install` cannot touch tui/go.mod.
(cd / && go install golang.org/x/vuln/cmd/govulncheck@v1.8.0 \
  && go install github.com/google/go-licenses/v2@v2.0.1) \
  || { echo "FAIL: could not install govulncheck or go-licenses"; exit 1; }
# gitleaks, from the release ci.yml downloads, checked against the same hash.
VERSION=8.21.2
if ! installed gitleaks version; then
  dir="$HOME/.local/gitleaks"; mkdir -p "$dir"
  tgz="$dir/gitleaks_${VERSION}_linux_x64.tar.gz"
  curl -sSfL -o "$tgz" "https://github.com/gitleaks/gitleaks/releases/download/v$VERSION/gitleaks_${VERSION}_linux_x64.tar.gz" \
    && echo "5bc41815076e6ed6ef8fbecc9d9b75bcae31f39029ceb55da08086315316e3ba  $tgz" | sha256sum -c - >/dev/null \
    && tar -xzf "$tgz" -C "$dir" gitleaks \
    && ln -sf "$dir/gitleaks" "$HOME/.local/bin/gitleaks" \
    || { echo "FAIL: gitleaks $VERSION did not download, or its hash did not match"; exit 1; }
fi

# G2b's guard fixture runs `python -m mypy` and `python -m pytest`; the
# versions and hashes are the ones ci.yml's guard job installs (#215).
python3 -m pip install -q --root-user-action=ignore --require-hashes -r qa/requirements-ci.txt \
  || { echo "FAIL: could not install qa/requirements-ci.txt"; exit 1; }

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

echo "OK: go $got, G5's tools, git identity set, a listener on 127.0.0.1:11434, gh api reaches the repository"
