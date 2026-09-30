#!/usr/bin/env bash
# ci-local-selftest.sh: scripts/ci-local.py keeps every byte of a step's
# output, gives each step its own `env:`, gives each leg its own emptied
# RUNNER_TEMP, and resolves the workflow expressions it knows and refuses the
# rest.
#
#   scripts/checks/ci-local-selftest.sh
#
# #223: ci-local read step output in the locale codec (cp1252 on the owner's
# host). A step that printed a byte cp1252 leaves undefined killed the reader
# thread, so the step's log came out empty while its exit code still said 0.
# It also ignored step-level `env:`, so such a step ran differently from CI.
#
# This builds a throwaway git repository in a temp dir holding a copy of
# ci-local.py and a fixture workflow (no real CI job runs), and runs it with
# CI_LOCAL_DIR in that temp dir, so the clone a real run reuses is not
# touched. The fixture has one leg, on this host's own platform:
#   - a step prints U+0101 and U+0150, whose UTF-8 holds 0x81 and 0x90 (two
#     of cp1252's undefined bytes), then a line after them; the log must hold
#     both, byte for byte.
#   - a step with `env: FOO: step` must see FOO=step, and the step after it,
#     with no env of its own, must see the job's FOO=job again.
#   - a second job prints the same bytes and exits 3: ci-local must report
#     that exit and print the tail without raising while writing to a pipe.
#
# #237: ci-local set no RUNNER_TEMP and passed `${{ github.base_ref }}` and
# `${{ github.event.before }}` through as literal text, so the gitleaks step
# of supply-chain failed locally for a reason CI does not have. The fixture
# history is A (branch v0.1-resolver) <- B <- C (HEAD), so the base and the
# first parent are different commits:
#   - job `exprs` must see BASE_REF=v0.1-resolver, origin/v0.1-resolver at A,
#     BEFORE=B (through `env:` and, unspaced, in `run:`), and a RUNNER_TEMP
#     that exists, is empty and is its own; job `temp` must get a different
#     one. Both leave a file behind, and a second run must find each
#     RUNNER_TEMP empty again.
#   - with CI_LOCAL_BASE set empty (a push), BASE_REF must be empty.
#   - job `unknown` names `${{ github.sha }}`, which ci-local does not
#     resolve: it must fail, naming the expression, before any step runs.
# The environment is passed through as it is: setting PYTHONUTF8 or
# PYTHONIOENCODING here would hide the defect this check exists for.
#
# Exit 0: all of the above held. Exit 1: otherwise, with each failed condition
# printed; also when no Python, or no PyYAML, is available.
set -uo pipefail

here="$(cd "$(dirname "$0")/../.." && pwd)" || exit 1

if command -v py >/dev/null 2>&1; then
  py=(py -3)
elif command -v python3 >/dev/null 2>&1; then
  py=(python3)
else
  echo "FAIL: no py or python3 on PATH"
  exit 1
fi

# A Windows Python reads /tmp/... as C:\tmp\...; hand it a mixed path.
native() { if command -v cygpath >/dev/null 2>&1; then cygpath -m "$1"; else printf '%s\n' "$1"; fi; }

case "$(uname -s)" in
  MINGW*|MSYS*|CYGWIN*) os=windows-latest; platform=windows ;;
  *) os=ubuntu-latest; platform=linux ;;
esac

tmp="$(mktemp -d)" || exit 1
trap 'rm -rf "$tmp"' EXIT
repo="$tmp/repo"
mkdir -p "$repo/scripts" "$repo/.github/workflows" || exit 1
cp "$here/scripts/ci-local.py" "$repo/scripts/" || exit 1

cat > "$repo/.github/workflows/ci.yml" <<EOF
name: ci-local-selftest
on: push
env:
  FOO: workflow
jobs:
  selftest:
    runs-on: \${{ matrix.os }}
    strategy:
      matrix:
        os: [$os]
    env:
      FOO: job
    steps:
      - uses: actions/checkout@v4
      - name: utf8
        run: |
          printf 'before-marker\n'
          printf 'marker \304\201 \305\220 end\n'
          printf 'after-marker\n'
      - name: step-env
        env:
          FOO: step
        run: echo "FOO=\$FOO"
      - name: job-env
        run: echo "FOO=\$FOO"
  fails:
    runs-on: \${{ matrix.os }}
    strategy:
      matrix:
        os: [$os]
    steps:
      - name: utf8-then-fail
        run: |
          printf 'marker \304\201 \305\220 end\n'
          exit 3
  exprs:
    runs-on: \${{ matrix.os }}
    strategy:
      matrix:
        os: [$os]
    steps:
      - uses: actions/checkout@v4
      - name: exprs
        env:
          BASE_REF: \${{ github.base_ref }}
          BEFORE: \${{github.event.before}}
        run: |
          echo "base_ref=\$BASE_REF"
          if [ -n "\$BASE_REF" ]; then echo "origin=\$(git rev-parse "origin/\$BASE_REF")"; fi
          echo "before=\$BEFORE"
          echo "run_before=\${{ github.event.before }}"
          test -d "\$RUNNER_TEMP"
          test -z "\$(ls -A "\$RUNNER_TEMP")"
          echo left > "\$RUNNER_TEMP/left"
          echo "runner_temp=\$RUNNER_TEMP"
  temp:
    runs-on: \${{ matrix.os }}
    strategy:
      matrix:
        os: [$os]
    steps:
      - name: temp
        run: |
          test -d "\$RUNNER_TEMP"
          test -z "\$(ls -A "\$RUNNER_TEMP")"
          echo left > "\$RUNNER_TEMP/left"
          echo "runner_temp=\$RUNNER_TEMP"
  unknown:
    runs-on: \${{ matrix.os }}
    strategy:
      matrix:
        os: [$os]
    env:
      SHA: \${{ github.sha }}
    steps:
      - name: never-runs
        run: echo "SHA=\$SHA"
EOF

# A scratch global config, so the owner's hooks and signing do not run on
# the fixture commit or inside ci-local's git calls.
export GIT_CONFIG_GLOBAL="$tmp/gitconfig"
: > "$GIT_CONFIG_GLOBAL"
g() { git -C "$repo" -c user.name=selftest -c user.email=selftest@invalid -c core.autocrlf=false "$@"; }
g init -q -b work || exit 1
g commit -q --allow-empty -m A || exit 1
g branch v0.1-resolver || exit 1
g commit -q --allow-empty -m B || exit 1
g add -A || exit 1
g commit -q -m fixture || exit 1
sha="$(g rev-parse HEAD)" || exit 1
base_sha="$(g rev-parse v0.1-resolver)" || exit 1
parent_sha="$(g rev-parse HEAD^)" || exit 1
# The caller's choice of base would change what `exprs` must see.
unset CI_LOCAL_BASE

# ci-local writes logs in text mode, so on Windows each line ends in CRLF.
lines() { tr -d '\r' < "$1" 2>/dev/null; }

run="$tmp/run"
script="$(native "$repo/scripts/ci-local.py")"
marker="$(printf 'marker \304\201 \305\220 end')"
fail=0

bad_output() {
  # A decode or encode error in ci-local itself, even one in a reader thread
  # that leaves the exit code alone.
  if printf '%s\n' "$1" | grep -Eq 'UnicodeDecodeError|UnicodeEncodeError|Exception in thread|Traceback'; then
    echo "FAIL: $2: ci-local raised:"
    printf '%s\n' "$1" | grep -E -A3 'Unicode|Exception in thread|Traceback'
    fail=1
  fi
}

echo "== ci-local --job selftest ($os)"
out="$(CI_LOCAL_DIR="$(native "$run")" "${py[@]}" "$script" --job selftest 2>&1)"
code=$?
printf '%s\n' "$out"
[ "$code" -eq 0 ] || { echo "FAIL: selftest job: ci-local exited $code"; fail=1; }
printf '%s\n' "$out" | grep -Eq "^selftest \| $os \| pass \|" \
  || { echo "FAIL: selftest job: no passing 'selftest | $os' row"; fail=1; }
bad_output "$out" "selftest job"

logs="$run/logs/${sha:0:12}"
log="$logs/selftest-$platform-01.log"
if [ ! -f "$log" ]; then
  echo "FAIL: no log at $log"
  fail=1
else
  for want in before-marker "$marker" after-marker; do
    lines "$log" | grep -qxF -- "$want" || { echo "FAIL: $log lacks the line '$want'"; fail=1; }
  done
fi
for pair in "02:FOO=step" "03:FOO=job"; do
  n="${pair%%:*}" want="${pair#*:}"
  f="$logs/selftest-$platform-$n.log"
  if [ "$(lines "$f")" != "$want" ]; then
    echo "FAIL: $f is '$(lines "$f")', want '$want'"
    fail=1
  fi
done

echo "== ci-local --job fails ($os)"
out="$(CI_LOCAL_DIR="$(native "$run")" "${py[@]}" "$script" --job fails 2>&1)"
code=$?
printf '%s\n' "$out"
[ "$code" -eq 1 ] || { echo "FAIL: fails job: ci-local exited $code, want 1"; fail=1; }
printf '%s\n' "$out" | grep -Eq "^fails \| $os \| FAIL \|" \
  || { echo "FAIL: fails job: no failing 'fails | $os' row"; fail=1; }
printf '%s\n' "$out" | grep -q "step 'utf8-then-fail' exited 3" \
  || { echo "FAIL: fails job: the step's exit code 3 was not reported"; fail=1; }
bad_output "$out" "fails job"
lines "$logs/fails-$platform-00.log" | grep -qxF -- "$marker" \
  || { echo "FAIL: $logs/fails-$platform-00.log lacks the line '$marker'"; fail=1; }

# The value of `key=` in a log, CR stripped.
val() { lines "$1" | sed -n "s/^$2=//p"; }
want_eq() { [ "$2" = "$3" ] || { echo "FAIL: $1 is '$2', want '$3'"; fail=1; }; }

for n in 1 2; do
  echo "== ci-local --job exprs --job temp ($os), run $n"
  out="$(CI_LOCAL_DIR="$(native "$run")" "${py[@]}" "$script" --job exprs --job temp 2>&1)"
  code=$?
  printf '%s\n' "$out"
  [ "$code" -eq 0 ] || { echo "FAIL: exprs run $n: ci-local exited $code"; fail=1; }
  bad_output "$out" "exprs run $n"
  f="$logs/exprs-$platform-01.log"
  want_eq "run $n: BASE_REF" "$(val "$f" base_ref)" v0.1-resolver
  want_eq "run $n: origin/v0.1-resolver" "$(val "$f" origin)" "$base_sha"
  want_eq "run $n: BEFORE from env" "$(val "$f" before)" "$parent_sha"
  want_eq "run $n: github.event.before in run" "$(val "$f" run_before)" "$parent_sha"
  t1="$(val "$f" runner_temp)" t2="$(val "$logs/temp-$platform-00.log" runner_temp)"
  if [ "$platform" = windows ]; then
    want_eq "run $n: exprs RUNNER_TEMP" "$t1" "$(native "$run")/tmp-exprs-$platform"
  fi
  [ -n "$t1" ] && [ -n "$t2" ] && [ "$t1" != "$t2" ] \
    || { echo "FAIL: run $n: RUNNER_TEMP '$t1' and '$t2' are not two directories"; fail=1; }
done

echo "== CI_LOCAL_BASE= ci-local --job exprs ($os)"
out="$(CI_LOCAL_BASE= CI_LOCAL_DIR="$(native "$run")" "${py[@]}" "$script" --job exprs 2>&1)"
code=$?
printf '%s\n' "$out"
[ "$code" -eq 0 ] || { echo "FAIL: push exprs: ci-local exited $code"; fail=1; }
bad_output "$out" "push exprs"
want_eq "push: BASE_REF" "$(val "$logs/exprs-$platform-01.log" base_ref)" ""
want_eq "push: BEFORE" "$(val "$logs/exprs-$platform-01.log" before)" "$parent_sha"

echo "== ci-local --job unknown ($os)"
out="$(CI_LOCAL_DIR="$(native "$run")" "${py[@]}" "$script" --job unknown 2>&1)"
code=$?
printf '%s\n' "$out"
[ "$code" -eq 1 ] || { echo "FAIL: unknown job: ci-local exited $code, want 1"; fail=1; }
printf '%s\n' "$out" | grep -Eq "^unknown \| $os \| FAIL \|" \
  || { echo "FAIL: unknown job: no failing 'unknown | $os' row"; fail=1; }
printf '%s\n' "$out" | grep -qF 'env SHA: ${{ github.sha }}' \
  || { echo "FAIL: unknown job: the unresolved expression was not named"; fail=1; }
[ ! -e "$logs/unknown-$platform-00.log" ] \
  || { echo "FAIL: unknown job: a step ran with an unresolved expression"; fail=1; }
bad_output "$out" "unknown job"

if [ "$fail" -eq 0 ]; then
  echo "ci-local-selftest: ok (UTF-8 step output kept whole; step env merged over job env;" \
    "github.base_ref and github.event.before resolved, others refused; RUNNER_TEMP per leg, emptied)"
fi
exit "$fail"
