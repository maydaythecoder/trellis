#!/usr/bin/env bash
# no-bare-git-in-cli-tests.sh: no test in trellis-cli starts `git` itself.
#
#   scripts/checks/no-bare-git-in-cli-tests.sh [--self-test]
#
# tests/hygiene.rs holds shipped code to trellis_vcs and exempts tests, so
# nothing held fixtures to it, and `testutil::git_commit_all` spawned bare
# `git` and dropped its exit status (#240): a fixture whose commit failed
# went on and failed later at an unrelated assertion. A fixture goes
# through `trellis_vcs::command()` (or `testutil::git_ok`), so it runs git
# as trellis does, with no hook or fsmonitor from the machine's
# configuration, and asserts the status.
#
# Scans crates/trellis-cli/src and crates/trellis-cli/tests for
# `Command::new("git")` in any spelling hygiene.rs treats as git (`GIT`,
# `git.exe`, `/usr/bin/git`), std or tokio. Not matched, because they are
# not spawns: `trellis_vcs::command()`, a `//` comment, a raw string
# holding the text (hygiene.rs's own test vectors), and `"git ..."` in a
# message or in an argv handed to the model's tools. Like hygiene.rs, it
# reads line by line and does not see a name built at run time.
#
# ALLOWED lists the spawns that are bare on purpose, counted exactly, so a
# new one in a listed file is an edit here rather than a pass.
#
# Exit 0: none found. Exit 1: found (each printed as file:line), or an
# allowed count changed. Exit 2: wrong arguments, or not in a git
# repository.
set -uo pipefail

BARE='Command::new\([[:space:]]*"([^"]*[/\\])?[Gg][Ii][Tt](\.([Ee][Xx][Ee]|[Cc][Mm][Dd]|[Bb][Aa][Tt]|[Cc][Oo][Mm]))?"'

# path|count|why
ALLOWED=(
  'crates/trellis-cli/tests/cli.rs|1|the control in a_core_fsmonitor_bash_sets_is_never_run_by_trellis_during_or_after_the_run: it shows plain git runs the planted fsmonitor, which trellis_vcs would switch off'
)

# Lines of `git grep -n` output (file:line:text) that are spawns: drops a
# line whose text starts with `//`, and one where the match sits inside a
# raw string.
spawns() {
  grep -E "$BARE" \
    | grep -vE '^[^:]+:[0-9]+:[[:space:]]*//' \
    | grep -vE "r#+\".*$BARE"
}

usage='usage: no-bare-git-in-cli-tests.sh [--self-test]'
if [ "${1:-}" = "--self-test" ]; then
  [ $# -eq 1 ] || { echo "$usage"; exit 2; }
  fail=0
  for bad in \
    'a.rs:1:        std::process::Command::new("git")' \
    'a.rs:2:    let mut c = tokio::process::Command::new("git");' \
    'a.rs:3:    let o = Command::new( "GIT" ).args(["status"]);' \
    'a.rs:4:    Command::new("git.exe")' \
    'a.rs:5:    Command::new("/usr/bin/git")' \
    'a.rs:6:    Command::new("C:\\Program Files\\Git\\cmd\\git.cmd")'; do
    [ -n "$(printf '%s\n' "$bad" | spawns)" ] \
      || { echo "FAIL: misses a bare spawn: $bad"; fail=1; }
  done
  for ok in \
    'a.rs:1:    let out = trellis_vcs::command()' \
    'a.rs:2:    crate::testutil::git_ok(dir, &["add", "-A"]);' \
    'a.rs:3:    // Command::new("git") in a comment' \
    'a.rs:4:    assert!(names_git(r#"    Command::new("git.exe")"#));' \
    'a.rs:5:        "git {args:?} failed: {}",' \
    'a.rs:6:const PASSES: &str = r#"["git", "--version"]"#;' \
    'a.rs:7:    Command::new(env!("CARGO_BIN_EXE_trellis"))' \
    'a.rs:8:    Command::new("gitleaks")' \
    'a.rs:9:    Command::new("legit")'; do
    if [ -n "$(printf '%s\n' "$ok" | spawns)" ]; then
      echo "FAIL: matches a line that is not a bare spawn: $ok"; fail=1
    fi
  done
  [ "$fail" -eq 0 ] || exit 1
  echo "OK: self-test (matches bare git spawns, not trellis_vcs, comments, raw strings or messages)"
  exit 0
fi
[ $# -eq 0 ] || { echo "$usage"; exit 2; }

git rev-parse --show-toplevel >/dev/null 2>&1 || { echo "FAIL: not in a git repository"; exit 2; }
cd "$(git rev-parse --show-toplevel)" || exit 2
# git grep exits 1 on no match and >1 on error; only the latter is a failure
# to run.
raw=$(git grep -nIE "$BARE" -- crates/trellis-cli/src crates/trellis-cli/tests)
rc=$?
if [ "$rc" -gt 1 ]; then
  echo "FAIL: git grep failed ($rc)"
  exit 2
fi
hits=$(printf '%s\n' "$raw" | spawns)

fail=0
for entry in "${ALLOWED[@]}"; do
  path=${entry%%|*}
  rest=${entry#*|}
  want=${rest%%|*}
  got=$(printf '%s\n' "$hits" | grep -c "^$path:" || true)
  if [ "$got" -ne "$want" ]; then
    echo "FAIL: $path starts bare git on $got line(s), $want allowed (${rest#*|})"
    fail=1
  fi
  hits=$(printf '%s\n' "$hits" | grep -v "^$path:" || true)
done
if [ -n "$hits" ]; then
  echo "FAIL: trellis-cli test code starts git without trellis_vcs (use trellis_vcs::command() or testutil::git_ok):"
  printf '%s\n' "$hits" | cut -d: -f1-2 | sed 's/^/  /'
  fail=1
fi
[ "$fail" -eq 0 ] || exit 1
echo "OK: trellis-cli tests start git only through trellis_vcs, apart from ${#ALLOWED[@]} allowed control(s)"
