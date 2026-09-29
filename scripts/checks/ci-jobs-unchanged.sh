#!/usr/bin/env bash
# Fails if .github/workflows/ci.yml lost a job relative to a base ref.
#
#   scripts/checks/ci-jobs-unchanged.sh [<base-ref>]   (default origin/v0.1-resolver)
#
# A change meant to make CI cheaper or faster must not get there by dropping
# a gate. Job ids are the two-space-indented keys under `jobs:`.
set -euo pipefail
base="${1:-origin/v0.1-resolver}"
cd "$(git rev-parse --show-toplevel)"
jobs_of() { awk '/^jobs:/{j=1; next} j && /^[^ #]/{j=0} j && /^  [A-Za-z0-9_-]+:[[:space:]]*$/{sub(/:.*/, ""); sub(/^  /, ""); print}' | sort; }
before=$(git show "$base:.github/workflows/ci.yml" | tr -d '\r' | jobs_of)
after=$(tr -d '\r' < .github/workflows/ci.yml | jobs_of)
[ -n "$before" ] || { echo "FAIL: no jobs found in $base's ci.yml"; exit 1; }
missing=$(comm -23 <(printf '%s\n' "$before") <(printf '%s\n' "$after"))
if [ -n "$missing" ]; then
  echo "FAIL: jobs in $base but not here:"; printf '  %s\n' $missing; exit 1
fi
echo "OK: every job in $base's ci.yml is still present ($(printf '%s\n' "$before" | wc -l | tr -d ' ') jobs)"
