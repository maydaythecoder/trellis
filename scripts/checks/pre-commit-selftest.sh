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
# expect <refuse|pass> <label> <path> <content>
expect() {
  fresh
  mkdir -p "$(dirname "$tmp/r/$3")"
  printf '%s\n' "$4" > "$tmp/r/$3"
  (cd "$tmp/r" && git add -A)
  if (cd "$tmp/r" && PATH="$PATH" bash scripts/hooks/pre-commit >/dev/null 2>&1); then got=pass; else got=refuse; fi
  if [ "$got" = "$1" ]; then pass=$((pass+1)); else bad=$((bad+1)); echo "FAIL: $2: expected $1, got $got"; fi
}

ant="sk-ant-$(printf 'a%.0s' $(seq 1 30))"
aws="AKIA$(printf 'A%.0s' $(seq 1 16))"
pem="-----BEGIN RSA PRIVATE $(echo KEY)-----"
pw=hunter2
url="https://user:${pw}@example.invalid/x"
expect refuse "provider key literal"  src/a.rs  "let k = \"$ant\";"
expect refuse "aws key literal"       src/b.rs  "const K: &str = \"$aws\";"
expect refuse "private key header"    src/c.rs  "$pem"
expect refuse "basic-auth url"        src/d.rs  "let u = \"$url\";"
expect refuse "dotenv file"           .env      "X=1"
expect refuse "pem file"              k/server.pem "x"
expect refuse "gitleaks ignore edit"  .gitleaksignore "abc"
expect refuse "pull_request_target"   .github/workflows/w.yml "on: pull_request_target"
expect refuse "event in run block"    .github/workflows/x.yml "$(printf 'on: push\njobs:\n  a:\n    runs-on: x\n    steps:\n      - run: |\n          echo \"${{ github.event.head_commit.message }}\"\n')"
expect pass   "clean change"          src/e.rs  "fn f() -> u32 { 1 }"
expect pass   "format! built canary"  src/f.rs  'let k = format!("sk-ant-{}", "a".repeat(30));'
expect pass   "event through env"     .github/workflows/y.yml "$(printf 'on: push\njobs:\n  a:\n    runs-on: x\n    steps:\n      - env:\n          M: ${{ github.event.head_commit.message }}\n        run: echo \"$M\"\n' | sed 's/^on: push$/on: push\npermissions: {}/; s/runs-on: x/runs-on: ubuntu-latest/')"

echo "pre-commit self-test: $pass passed, $bad failed"
[ "$bad" -eq 0 ]
