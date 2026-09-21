#!/usr/bin/env bash
# Mutation testing one crate at a time, saving after each.
#
# Two whole-workspace runs were stopped by the system for memory pressure,
# at 842 and 295 of ~1253 mutants, and halving concurrency made the second
# one worse rather than better. A per-crate run is short enough to finish,
# and the saving step means a kill costs at most the crate in flight
# instead of everything since the start.
#
# `cargo mutants -p X` runs only X's tests, so a mutant that a dependent's
# tests would catch is reported as surviving. Set WORKSPACE_TESTS=1 to mutate
# one crate and run the whole suite instead — the correct measurement, at the
# cost of the full suite per mutant. Results land in a separate directory so
# the two are never mixed.
#
# Usage:  scripts/mutate-by-crate.sh [crate ...]
#         WORKSPACE_TESTS=1 scripts/mutate-by-crate.sh [crate ...]
# With no arguments, runs every crate that has never been measured.

set -uo pipefail
cd "$(dirname "$0")/.."

COMMIT=$(git rev-parse --short HEAD)
if [ "${WORKSPACE_TESTS:-0}" = "1" ]; then
    OUT="docs/bench/mutants-by-crate-ws"
    SCOPE=(--test-workspace true)
else
    OUT="docs/bench/mutants-by-crate"
    SCOPE=()
fi
mkdir -p "$OUT"

CRATES=("$@")
if [ ${#CRATES[@]} -eq 0 ]; then
    CRATES=(trellis-model trellis-trail trellis-vcs trellis-core trellis-checks)
fi

for crate in "${CRATES[@]}"; do
    dest="$OUT/$crate"
    if [ -f "$dest/caught.txt" ]; then
        echo "== $crate: already recorded, skipping"
        continue
    fi

    echo "== $crate: starting at $(date +%H:%M:%S)"
    rm -rf mutants.out mutants.out.old
    # Not `set -e`: a crate that fails to build should not take the rest of
    # the run with it. The saved files say what happened either way.
    cargo mutants -p "$crate" "${SCOPE[@]}" --no-shuffle -j 4 2>&1 | tail -5

    if [ ! -f mutants.out/caught.txt ]; then
        echo "== $crate: no results — build failure or killed before the baseline"
        continue
    fi

    mkdir -p "$dest"
    for f in caught missed unviable timeout; do
        cp -f "mutants.out/$f.txt" "$dest/$f.txt" 2>/dev/null || : > "$dest/$f.txt"
    done
    echo "$COMMIT" > "$dest/COMMIT"

    c=$(wc -l < "$dest/caught.txt")
    m=$(wc -l < "$dest/missed.txt")
    echo "== $crate: caught $c, missed $m, at $COMMIT"
done

echo
echo "== summary =="
printf '%-18s %7s %7s %9s\n' crate caught missed score
for d in "$OUT"/*/; do
    crate=$(basename "$d")
    [ -f "$d/caught.txt" ] || continue
    c=$(wc -l < "$d/caught.txt"); m=$(wc -l < "$d/missed.txt")
    total=$((c + m))
    if [ "$total" -gt 0 ]; then
        printf '%-18s %7s %7s %8.1f%%\n' "$crate" "$c" "$m" \
            "$(awk "BEGIN{print 100*$c/$total}")"
    fi
done
