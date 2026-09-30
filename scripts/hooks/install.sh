#!/usr/bin/env bash
# Installs the tracked hooks into this clone's shared git dir, so the main
# checkout and every worktree run them (#206). Existing hooks of other names
# (the local pre-push guard on v0.1-resolver) are left alone.
#
# The pattern scripts the pre-commit hook reads are installed beside it too,
# so a commit on a branch older than those scripts is still scanned, not
# waved through (#208).
set -eu
root=$(git rev-parse --show-toplevel)
hooks="$(git rev-parse --git-common-dir)/hooks"
mkdir -p "$hooks/trellis-security"
for h in pre-commit; do
  cp "$root/scripts/hooks/$h" "$hooks/$h"
  chmod +x "$hooks/$h"
  echo "installed $hooks/$h"
done
for s in key-pattern.sh no-auth-urls.sh; do
  cp "$root/scripts/checks/$s" "$hooks/trellis-security/$s"
  echo "installed $hooks/trellis-security/$s"
done
