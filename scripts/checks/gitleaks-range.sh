#!/usr/bin/env bash
# gitleaks-range.sh: gitleaks finds no secret in the commits a change adds.
#
#   scripts/checks/gitleaks-range.sh <base>        scan <base>..HEAD
#   scripts/checks/gitleaks-range.sh --self-test   show the scan can fail
#
# The pre-commit hook runs gitleaks only where it is installed and only on
# commits made through the hook, so a commit made with --no-verify or in
# another clone was never scanned by it (#233, T8). G5 and CI's supply-chain
# job both call this script, so the two run the same scan.
#
# gitleaks itself cannot be trusted to fail when it did not scan: given a
# range whose base does not resolve it logs an error, prints "no leaks
# found" and exits 0 (seen with 8.21.2). So the base is resolved here first,
# and the scan counts only if gitleaks reports no error and a commit count
# that is not zero when the range has commits in it.
#
# Exit 0: scanned, nothing found. Exit 1: a finding, or the scan did not run
# (gitleaks absent, base unresolvable, gitleaks errored). Exit 2: wrong
# arguments.
set -uo pipefail

usage='usage: gitleaks-range.sh <base> | --self-test'

# gitleaks installs where git's PATH does not look (go install, a release
# tarball unpacked by hand), as the pre-commit hook notes. Appended, so a
# pinned binary the caller put first (CI) still wins.
for d in "$HOME/go/bin" "$HOME/.local/bin"; do
  [ -d "$d" ] && PATH="$PATH:$d"
done
# A host-level config can widen gitleaks' allowlist without touching the
# repository's .gitleaks files, which the pre-commit hook guards. The scan
# uses the repository's config or gitleaks' defaults, nothing else.
unset GITLEAKS_CONFIG

# scan <base>: run in the repository that is the current directory.
scan() {
  local base="$1" out rc n commits
  command -v gitleaks >/dev/null 2>&1 \
    || { echo "FAIL: gitleaks not installed (go install github.com/zricethezav/gitleaks/v8@v8.21.2, or a release from github.com/gitleaks/gitleaks); cannot scan $base..HEAD"; return 1; }
  git rev-parse --verify --quiet "$base^{commit}" >/dev/null \
    || { echo "FAIL: base '$base' does not resolve to a commit; gitleaks would scan nothing and report no leaks"; return 1; }
  commits=$(git rev-list --count --no-merges "$base..HEAD") \
    || { echo "FAIL: cannot count the commits in $base..HEAD"; return 1; }
  # `.` rather than an absolute path: gitleaks may be a native Windows
  # binary, and a POSIX path as its argument depends on MSYS conversion.
  out=$(gitleaks detect --source . --log-opts="$base..HEAD" --redact --no-banner --no-color 2>&1)
  rc=$?
  printf '%s\n' "$out"
  if [ "$rc" -ne 0 ]; then
    echo "FAIL: gitleaks found a secret in $base..HEAD (exit $rc; findings above, redacted)"
    return 1
  fi
  if printf '%s\n' "$out" | grep -qE '(^| )ERR( |$)'; then
    echo "FAIL: gitleaks reported an error scanning $base..HEAD (above); its 'no leaks found' does not count"
    return 1
  fi
  n=$(printf '%s\n' "$out" | sed -nE 's/.* ([0-9]+) commits scanned.*/\1/p' | tail -1)
  [ -n "$n" ] \
    || { echo "FAIL: gitleaks printed no commit count for $base..HEAD; cannot tell that it scanned"; return 1; }
  # gitleaks counts commits with a diff, which is the non-merge count; an
  # empty commit can make its count lower, so only zero is refused.
  if [ "$n" -eq 0 ] && [ "$commits" -gt 0 ]; then
    echo "FAIL: gitleaks scanned 0 commits of the $commits in $base..HEAD"
    return 1
  fi
  echo "OK: gitleaks scanned $n commit(s) in $base..HEAD; no secret found"
}

if [ "${1:-}" = "--self-test" ]; then
  [ $# -eq 1 ] || { echo "$usage"; exit 2; }
  tmp=$(mktemp -d) || { echo "FAIL: mktemp"; exit 1; }
  trap 'rm -rf "$tmp"' EXIT
  mkdir -p "$tmp/repo" "$tmp/nohooks"
  # A throwaway repository, isolated from the host's git config: a global
  # hooks path pointing at this repository's pre-commit hook would refuse
  # the planted commit and fail the self-test for the wrong reason.
  g() {
    git -C "$tmp/repo" -c user.name=selftest -c user.email=selftest@example.invalid \
      -c commit.gpgsign=false -c core.autocrlf=false -c core.hooksPath="$tmp/nohooks" "$@"
  }
  g init -q -b base || { echo "FAIL: git init"; exit 1; }
  echo "base" > "$tmp/repo/README"
  g add README && g commit -q -m base || { echo "FAIL: base commit"; exit 1; }
  g checkout -q -b clean && echo "no secret here" >> "$tmp/repo/README" \
    && g commit -q -am clean || { echo "FAIL: clean commit"; exit 1; }
  # The planted literal is built here, at run time, from random bytes, so
  # neither this file nor any log holds one. A GitHub personal access token
  # shape: a prefix and 36 alphanumerics. The read from urandom is bounded
  # first, so no pipe stage dies of SIGPIPE under pipefail.
  body=$(head -c 96 /dev/urandom | base64 | tr -dc 'A-Za-z0-9' | head -c 36)
  [ "${#body}" -eq 36 ] || { echo "FAIL: could not build a 36-character token body"; exit 1; }
  prefix="gh""p_"
  g checkout -q -b planted base \
    && printf 'token = "%s%s"\n' "$prefix" "$body" > "$tmp/repo/config.py" \
    && g add config.py && g commit -q -m planted || { echo "FAIL: planted commit"; exit 1; }
  unset body

  fail=0
  g checkout -q planted || exit 1
  out=$(cd "$tmp/repo" && scan base 2>&1)
  if [ $? -eq 0 ]; then
    echo "FAIL: self-test: a planted key-shaped literal on a branch passed the scan:"
    printf '%s\n' "$out" | sed 's/^/  /'; fail=1
  elif ! printf '%s\n' "$out" | grep -q 'found a secret'; then
    echo "FAIL: self-test: the planted branch failed, but not on a finding:"
    printf '%s\n' "$out" | sed 's/^/  /'; fail=1
  fi
  g checkout -q clean || exit 1
  out=$(cd "$tmp/repo" && scan base 2>&1) || {
    echo "FAIL: self-test: a branch with no secret did not pass:"
    printf '%s\n' "$out" | sed 's/^/  /'; fail=1
  }
  if out=$(cd "$tmp/repo" && scan no-such-base 2>&1); then
    echo "FAIL: self-test: an unresolvable base passed (gitleaks reports no leaks when it scans nothing):"
    printf '%s\n' "$out" | sed 's/^/  /'; fail=1
  fi
  [ "$fail" -eq 0 ] || exit 1
  echo "OK: self-test (a planted token on a throwaway branch is refused; a clean branch passes; an unresolvable base fails)"
  exit 0
fi

[ $# -eq 1 ] && [ -n "$1" ] && [ "${1#-}" = "$1" ] || { echo "$usage"; exit 2; }
git rev-parse --show-toplevel >/dev/null 2>&1 || { echo "FAIL: not in a git repository"; exit 2; }
cd "$(git rev-parse --show-toplevel)" || exit 2
scan "$1"
