#!/usr/bin/env bash
# zizmor-outcome.sh: say why zizmor failed, from its own exit status and
# output.
#
#   scripts/checks/zizmor-outcome.sh <exit-status> < zizmor-log
#   scripts/checks/zizmor-outcome.sh --self-test
#
# G5 used to report "found a medium-or-higher finding" on any non-zero exit.
# Where the network refuses GitHub, an online audit (artipacked listing
# actions/checkout's tags) fails with `request error while accessing GitHub
# API`, zizmor ends with `fatal: no audit was performed` and exits 1, and the
# gate pointed its reader at a finding that does not exist (#985, the
# misreport #960 fixed for govulncheck). This tells them apart. Both still
# fail: a run that performed no audit has checked nothing, so it is never a
# pass, and the gate does not fall back to --offline, which would drop the
# online audits and call what is left clean.
#
# The shapes are zizmor v1.30.1's. Its --help lists no exit statuses and
# the installed package ships no docs, so these are what it did in this
# container, with findings planted in scratch workflows under
# --min-severity medium: 0 no finding (also when every finding is below the
# minimum: the filter governs the exit), 13 the worst finding is medium, 14
# it is high, 1 a fatal error, 2 a usage error. No other status was
# observed; any other falls to "did not complete", not to a finding. The two
# strings are from the installed binary and a run in this container that got
# HTTP 401 through the proxy. A version bump re-reads all of it. The exit
# status is judged first, so a log that merely contains the request-error
# text beside a finding is still a finding. `no audit was performed` alone is
# not enough to call GitHub unreachable: a path zizmor cannot read ends the
# same way, and that is reported as a run that did not complete.
#
# The classifier does not run zizmor: the self-test feeds it each shape,
# which a live run cannot plant (a finding in .github/workflows/, a refused
# network).
#
# Prints one line: OK or FAIL and why. Exit 0: zizmor exited 0. Exit 1:
# anything else, or the self-test did not hold. Exit 2: wrong arguments.
set -uo pipefail

found_msg="zizmor found a medium-or-higher finding in .github/workflows/ (listed above)"
unreach_msg="zizmor could not reach GitHub (\`request error while accessing GitHub API\` above), so .github/workflows/ was not audited; this is not a finding, and not a pass"

# $1 exit status, $2 the log. Prints the verdict line; returns 0 only for a
# clean run.
classify() {
  case "$1" in
    0) echo "OK: zizmor found no medium-or-higher finding in .github/workflows/"; return 0 ;;
    13|14) echo "FAIL: $found_msg"; return 1 ;;
  esac
  if [ "$1" -eq 1 ] \
    && printf '%s\n' "$2" | grep -qF 'no audit was performed' \
    && printf '%s\n' "$2" | grep -qF 'request error while accessing GitHub API'; then
    echo "FAIL: $unreach_msg"
  else
    echo "FAIL: zizmor did not complete its audit of .github/workflows/ (exit $1, above); no finding and no clean result"
  fi
  return 1
}

usage='usage: zizmor-outcome.sh <exit-status> < log | --self-test'
if [ "${1:-}" = "--self-test" ]; then
  [ $# -eq 1 ] || { echo "$usage"; exit 2; }
  fail=0
  # $1 name, $2 status, $3 log, $4 expected exit, $5 phrase that must
  # appear, $6.. phrases that must not.
  expect() {
    local name=$1 st=$2 log=$3 want=$4 has=$5 out got
    shift 5
    out=$(classify "$st" "$log"); got=$?
    [ "$got" -eq "$want" ] \
      || { echo "FAIL: self-test: $name: exit $got, want $want ($out)"; fail=1; }
    printf '%s\n' "$out" | grep -qF -- "$has" \
      || { echo "FAIL: self-test: $name: no \"$has\" in: $out"; fail=1; }
    local not
    for not in "$@"; do
      printf '%s\n' "$out" | grep -qF -- "$not" \
        && { echo "FAIL: self-test: $name: \"$not\" in: $out"; fail=1; }
    done
  }
  # The request error is built at run time from parts, as this container's
  # proxy printed it (#985).
  url="https://github.com/actions/checkout.git/git-upload-pack"
  unreach=" INFO zizmor: v1.30.1
fatal: no audit was performed
'artipacked' audit failed on file://.github/workflows/audit.yml

Caused by:
    0: error in 'artipacked' audit
    1: couldn't list tags for actions/checkout
    2: request error while accessing GitHub API
    3: HTTP status client error (401 Unauthorized) for url ($url)"
  medium="warning[artipacked]: credential persistence through GitHub Actions artifacts
 --> .github/workflows/planted.yml:7:9

4 findings (3 suppressed): 0 informational, 0 low, 1 medium, 0 high"
  high="error[template-injection]: code injection via template expansion
 --> .github/workflows/planted.yml:7:24

5 findings (3 suppressed): 0 informational, 0 low, 0 medium, 2 high"
  badpath="fatal: no audit was performed
error: can't fetch remote repository: /nonexistent"
  # An unreachable GitHub must not be read as a finding: that sent the
  # reader after a finding that does not exist.
  expect "unreachable GitHub" 1 "$unreach" 1 "could not reach GitHub" "found a medium-or-higher finding" "did not complete"
  # A real finding must stay a finding, and fail, at either severity.
  expect "planted medium finding" 13 "$medium" 1 "found a medium-or-higher finding" "could not reach" "did not complete"
  expect "planted high finding" 14 "$high" 1 "found a medium-or-higher finding" "could not reach" "did not complete"
  # Exit status first: a finding whose log happens to hold the request-error
  # text is still a finding, so a stray line cannot downgrade one.
  expect "finding beside request-error text" 13 "$medium
$unreach" 1 "found a medium-or-higher finding" "could not reach"
  # `no audit was performed` without the request error is some other fatal
  # error, not an unreachable GitHub, and neither is a usage error.
  expect "unreadable path" 1 "$badpath" 1 "did not complete" "found a medium-or-higher finding" "could not reach"
  expect "usage error" 2 "error: unexpected argument '--bogus' found" 1 "did not complete" "found a medium-or-higher finding" "could not reach"
  # A clean run passes.
  expect "clean run" 0 "No findings to report. Good job! (40 suppressed)" 0 "OK: " "FAIL"
  [ "$fail" -eq 0 ] || exit 1
  echo "OK: self-test (an unreachable GitHub, a planted medium and a planted high finding, a finding beside request-error text, two other failures and a clean run each classified, and only one way)"
  exit 0
fi
[ $# -eq 1 ] || { echo "$usage"; exit 2; }
case "$1" in ''|*[!0-9]*) echo "$usage (exit status not a number: $1)"; exit 2 ;; esac
classify "$1" "$(cat)"
