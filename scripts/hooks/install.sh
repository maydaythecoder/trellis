#!/usr/bin/env bash
# Installs the tracked hooks into this clone's shared git dir, so the main
# checkout and every worktree run them (#206). Existing hooks of other names
# (the local pre-push guard on v0.1-resolver) are left alone.
set -eu
root=$(git rev-parse --show-toplevel)
hooks="$(git rev-parse --git-common-dir)/hooks"
mkdir -p "$hooks"
for h in pre-commit; do
  cp "$root/scripts/hooks/$h" "$hooks/$h"
  chmod +x "$hooks/$h"
  echo "installed $hooks/$h"
done
