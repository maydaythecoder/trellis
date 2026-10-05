#!/usr/bin/env bash
# Fails if .github/workflows/ci.yml lost a job relative to a base ref.
#
#   scripts/checks/ci-jobs-unchanged.sh [<base-ref>]   (default: the gates' base)
#
# A change meant to make CI cheaper or faster must not get there by dropping
# a gate. Job ids are the two-space-indented keys under `jobs:`.
# The gates' base (scripts/gates/lib.sh): origin/dev on a ticket, feature or
# audit branch, origin/main otherwise, so jobs added on dev count.
source "$(dirname "$0")/../gates/lib.sh"
# Git Bash rewrites a `ref:path` argument into a Windows path without this.
export MSYS_NO_PATHCONV=1
base="${1:-$GATE_BASE}"
jobs_of() { awk '/^jobs:/{j=1; next} j && /^[^ #]/{j=0} j && /^  [A-Za-z0-9_-]+:[[:space:]]*$/{sub(/:.*/, ""); sub(/^  /, ""); print}' | sort; }
before=$(git show "$base:.github/workflows/ci.yml" | tr -d '\r' | jobs_of)
after=$(tr -d '\r' < .github/workflows/ci.yml | jobs_of)
[ -n "$before" ] || { echo "FAIL: no jobs found in $base's ci.yml"; exit 1; }
missing=$(comm -23 <(printf '%s\n' "$before") <(printf '%s\n' "$after"))
if [ -n "$missing" ]; then
  echo "FAIL: jobs in $base but not here:"; printf '  %s\n' $missing; exit 1
fi
echo "OK: every job in $base's ci.yml is still present ($(printf '%s\n' "$before" | wc -l | tr -d ' ') jobs)"
