#!/usr/bin/env bash
# govulncheck-outcome.sh: say why govulncheck failed, from its own exit
# status and output.
#
#   scripts/checks/govulncheck-outcome.sh <exit-status> < govulncheck-log
#   scripts/checks/govulncheck-outcome.sh --self-test
#
# G5 used to report "found a vulnerability" on any non-zero exit. Where the
# network denies vuln.go.dev, govulncheck exits 1 with `fetching
# vulnerabilities: ... Forbidden`, and the gate pointed its reader at a
# vulnerability that does not exist (#960). This tells the two apart. Both
# still fail: a scan that could not read the database has checked nothing,
# so it is never a pass and never a skip.
#
# The shapes are govulncheck v1.8.0's, in text mode (the mode G5 runs; under
# -json it does not exit 3): exit 3 is errVulnerabilitiesFound
# (internal/scan/errors.go); a database that could not be fetched is
# `fetching vulnerabilities: ` (internal/vulncheck/fetch.go), exit 1. A
# version bump re-reads both. The exit status is judged first, so a log that
# merely contains the fetch text beside a finding is still a finding. Any
# other failure is reported as a scan that did not complete, not as either.
#
# The classifier does not run govulncheck: the self-test feeds it each shape,
# which a live run cannot plant (a finding in tui/, a denied network).
#
# Prints one line: OK or FAIL and why. Exit 0: govulncheck exited 0.
# Exit 1: anything else, or the self-test did not hold. Exit 2: wrong
# arguments.
set -uo pipefail

found_msg="govulncheck found a vulnerability in tui/ (above); none is ignored (tui/VULN-EXCEPTIONS.md)"
unreach_msg="govulncheck could not reach the vulnerability database (\`fetching vulnerabilities\` above), so tui/ was not audited; this is not a finding, and not a pass"

# $1 exit status, $2 the log. Prints the verdict line; returns 0 only for a
# clean scan.
classify() {
  case "$1" in
    0) echo "OK: govulncheck found no known vulnerability reachable from tui/"; return 0 ;;
    3) echo "FAIL: $found_msg"; return 1 ;;
  esac
  if printf '%s\n' "$2" | grep -q 'fetching vulnerabilities: '; then
    echo "FAIL: $unreach_msg"
  else
    echo "FAIL: govulncheck did not complete its scan of tui/ (exit $1, above); no finding and no clean result"
  fi
  return 1
}

usage='usage: govulncheck-outcome.sh <exit-status> < log | --self-test'
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
  # The fetch error is built at run time from parts, as this container's
  # proxy printed it (#960).
  db="https://vuln.go.dev/index/modules.json.gz"
  unreach="Scanner: govulncheck@v1.8.0
DB: https://vuln.go.dev

govulncheck: fetching vulnerabilities: Get \"$db\": Forbidden"
  finding="=== Symbol Results ===

Vulnerability #1: GO-2099-0001
    A planted finding for the self-test
  More info: https://pkg.go.dev/vuln/GO-2099-0001
  Module: example.com/planted
    Found in: example.com/planted@v1.0.0
    Fixed in: example.com/planted@v1.0.1

Your code is affected by 1 vulnerability from 1 module."
  # An unreachable database must not be read as a finding: that sent the
  # reader after a vulnerability that does not exist.
  expect "unreachable database" 1 "$unreach" 1 "could not reach the vulnerability database" "found a vulnerability" "did not complete"
  # A real finding must stay a finding, and fail.
  expect "planted finding" 3 "$finding" 1 "found a vulnerability" "could not reach" "did not complete"
  # Exit status first: a finding whose log happens to hold the fetch text is
  # still a finding, so a stray line cannot downgrade one.
  expect "finding beside fetch text" 3 "$finding
$unreach" 1 "found a vulnerability" "could not reach"
  # Some other failure is neither a finding nor an unreachable database.
  expect "other failure" 1 "govulncheck: loading packages: no go.mod file" 1 "did not complete" "found a vulnerability" "could not reach"
  expect "usage error" 2 "invalid usage" 1 "did not complete" "found a vulnerability" "could not reach"
  # A clean scan passes.
  expect "clean scan" 0 "No vulnerabilities found." 0 "OK: " "FAIL"
  [ "$fail" -eq 0 ] || exit 1
  echo "OK: self-test (an unreachable database, a planted finding, a finding beside fetch text, two other failures and a clean scan each classified, and only one way)"
  exit 0
fi
[ $# -eq 1 ] || { echo "$usage"; exit 2; }
case "$1" in ''|*[!0-9]*) echo "$usage (exit status not a number: $1)"; exit 2 ;; esac
classify "$1" "$(cat)"
