#!/usr/bin/env bash
# Shows the pre-commit hook refuses what it claims to and passes what it
# should (#206). Each case stages one change in a throwaway repository that
# carries this checkout's hook and the scripts it reads, then runs the hook.
# The planted literals are built here at run time, so this file holds none.
set -u
src=$(git rev-parse --show-toplevel)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
pass=0; bad=0

fresh() {
  rm -rf "$tmp/r"; mkdir -p "$tmp/r/scripts/checks" "$tmp/r/scripts/hooks"
  cp "$src/scripts/checks/key-pattern.sh" "$src/scripts/checks/no-auth-urls.sh" "$tmp/r/scripts/checks/"
  cp "$src/scripts/hooks/pre-commit" "$tmp/r/scripts/hooks/"
  (cd "$tmp/r" && git init -q && git config core.autocrlf false && git config user.email t@t && git config user.name t \
    && git add -A && git commit -qm base --no-verify)
}
# expect <refuse|pass> <label> <path> <content> [<message>]
# A refusal counts only with the hook's own <message>: actionlint, zizmor and
# gitleaks run here too, and would refuse some cases even if the hook's own
# check for them were gone.
expect() {
  fresh
  mkdir -p "$(dirname "$tmp/r/$3")"
  printf '%s\n' "$4" > "$tmp/r/$3"
  (cd "$tmp/r" && git add -A)
  if err=$(cd "$tmp/r" && bash scripts/hooks/pre-commit 2>&1 >/dev/null); then got=pass; else got=refuse; fi
  if [ "$got" = refuse ] && [ "$1" = refuse ] && ! printf '%s\n' "$err" | grep '^pre-commit: ' | grep -qF "${5:?refuse needs a message}"; then
    bad=$((bad+1)); echo "FAIL: $2: refused, but no 'pre-commit:' line says '$5'"; return
  fi
  if [ "$got" = "$1" ]; then pass=$((pass+1)); else bad=$((bad+1)); echo "FAIL: $2: expected $1, got $got"; fi
}

ant="sk-ant-$(printf 'a%.0s' $(seq 1 30))"
aws="AKIA$(printf 'A%.0s' $(seq 1 16))"
pem="-----BEGIN RSA PRIVATE $(echo KEY)-----"
pw=hunter2
url="https://user:${pw}@example.invalid/x"
expect refuse "provider key literal"  src/a.rs  "let k = \"$ant\";" "key-shaped literal staged"
expect refuse "aws key literal"       src/b.rs  "const K: &str = \"$aws\";" "key-shaped literal staged"
expect refuse "private key header"    src/c.rs  "$pem" "key-shaped literal staged"
expect refuse "basic-auth url"        src/d.rs  "let u = \"$url\";" "basic-auth URL staged"
expect refuse "dotenv file"           .env      "X=1" "secret-shaped file staged"
expect refuse "pem file"              k/server.pem "x" "secret-shaped file staged"
expect refuse "gitleaks ignore edit"  .gitleaksignore "abc" "allow/ignore files"
expect refuse "pull_request_target"   .github/workflows/w.yml "on: pull_request_target" "uses pull_request_target"
# A step's run: is either a key line under `- name:` or the list item itself
# (`- run: |`); the hook's scan missed the second until #423.
expect refuse "event in run block"    .github/workflows/x.yml "$(printf 'on: push\njobs:\n  a:\n    runs-on: x\n    steps:\n      - run: |\n          echo \"${{ github.event.issue.title }}\"\n')" "interpolated into a run: block"
expect refuse "event in run key line" .github/workflows/z.yml "$(printf 'on: push\njobs:\n  a:\n    runs-on: x\n    steps:\n      - name: x\n        run: |\n          echo \"${{ github.event.head_commit.message }}\"\n')" "interpolated into a run: block"
# A chomping indicator (`|-`, `>+`) still opens a block; until #429 the scan
# read such a line as a one-line run: and never looked at the body.
expect refuse "event in run |- block" .github/workflows/w.yml "$(printf 'on: push\njobs:\n  a:\n    runs-on: x\n    steps:\n      - run: |-\n          echo \"${{ github.event.issue.title }}\"\n')" "interpolated into a run: block"
expect pass   "clean change"          src/e.rs  "fn f() -> u32 { 1 }"
expect pass   "format! built canary"  src/f.rs  'let k = format!("sk-ant-{}", "a".repeat(30));'
expect pass   "event through env"     .github/workflows/y.yml "$(printf 'on: push\njobs:\n  a:\n    runs-on: x\n    steps:\n      - env:\n          M: ${{ github.event.head_commit.message }}\n        run: echo \"$M\"\n' | sed 's/^on: push$/on: push\npermissions: {}/; s/runs-on: x/runs-on: ubuntu-latest/')"

# #208: the hook must fail closed without its pattern scripts, and fall back
# to the copies install.sh puts beside it on a branch older than them.
# layout <none|installed> <refuse|pass> <label> <path> <content>
layout() {
  fresh
  (cd "$tmp/r" && git rm -rq --cached scripts/checks && rm -rf scripts/checks \
    && git commit -qm "older branch" --no-verify)
  if [ "$1" = installed ]; then
    mkdir -p "$tmp/r/scripts/hooks/trellis-security"
    cp "$src/scripts/checks/key-pattern.sh" "$src/scripts/checks/no-auth-urls.sh" \
      "$tmp/r/scripts/hooks/trellis-security/"
  fi
  mkdir -p "$(dirname "$tmp/r/$4")"
  printf '%s\n' "$5" > "$tmp/r/$4"
  (cd "$tmp/r" && git add "$4")
  if (cd "$tmp/r" && bash scripts/hooks/pre-commit >/dev/null 2>&1); then got=pass; else got=refuse; fi
  if [ "$got" = "$2" ]; then pass=$((pass+1)); else bad=$((bad+1)); echo "FAIL: $3: expected $2, got $got"; fi
}
layout none      refuse "no pattern scripts anywhere"      src/g.rs "fn g() {}"
layout installed refuse "installed copy still finds a key" src/h.rs "let k = \"$ant\";"
layout installed pass   "installed copy passes clean code" src/i.rs "fn i() {}"

echo "pre-commit self-test: $pass passed, $bad failed"
[ "$bad" -eq 0 ]
