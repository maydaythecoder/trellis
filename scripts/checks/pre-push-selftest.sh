#!/usr/bin/env bash
# Shows the tracked pre-push guard, as scripts/hooks/install.sh installs it,
# refuses every push to main and to v0.1-resolver, and lets a push to dev,
# audit or a ticket branch through (#217, ADR 0036, ADR 0051).
#
# Every push goes to a bare repository created under a temp dir, added as the
# remote `throwaway`. The work repository has no other remote, so nothing
# here can reach origin. A refusal counts only when the push fails with the
# guard's own message and the bare repository's ref did not move: a push that
# failed for some other reason is a failure of this check, not a pass.
set -u
src=$(git rev-parse --show-toplevel)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
# A caller's git environment (this script run from inside a hook) would point
# every git command below at the real repository.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_COMMON_DIR
pass=0; bad=0
bare="$tmp/bare.git"
work="$tmp/work"

die() { echo "pre-push self-test: cannot run: $*" >&2; exit 1; }
ok() { pass=$((pass+1)); }
fail() { bad=$((bad+1)); echo "FAIL: $*"; }
w() { git -C "$work" "$@"; }
remote_ref() { git --git-dir="$bare" rev-parse -q --verify "$1" 2>/dev/null || echo absent; }

git init -q --bare "$bare" || die "git init --bare"
mkdir -p "$work/scripts/hooks" "$work/scripts/checks"
cp "$src/scripts/hooks/install.sh" "$src/scripts/hooks/pre-commit" "$src/scripts/hooks/pre-push" \
  "$work/scripts/hooks/" || die "copy hooks"
cp "$src/scripts/checks/key-pattern.sh" "$src/scripts/checks/no-auth-urls.sh" \
  "$work/scripts/checks/" || die "copy pattern scripts"
git init -q -b dev "$work" || die "git init"
w config core.autocrlf false
w config user.email t@t
w config user.name t
# The commits are made before install.sh runs, so the installed pre-commit
# (which wants gitleaks and friends) never runs here and no bypass is needed.
w add -A && w commit -qm base || die "base commit"
w remote add throwaway "$bare" || die "remote add"

# Install exactly as a contributor would, from inside the clone.
(cd "$work" && bash scripts/hooks/install.sh >/dev/null) || die "install.sh failed"
hooks="$(w rev-parse --absolute-git-dir)/hooks"
# A global core.hooksPath would route git around the installed hooks, and the
# refusal cases below would then fail for a reason unrelated to the guard.
w config core.hooksPath "$hooks"
if cmp -s "$hooks/pre-push" "$src/scripts/hooks/pre-push"; then ok
else fail "install.sh did not install scripts/hooks/pre-push byte for byte"; fi
if cmp -s "$hooks/pre-commit" "$src/scripts/hooks/pre-commit"; then ok
else fail "install.sh no longer installs scripts/hooks/pre-commit"; fi
if [ -x "$hooks/pre-push" ]; then ok; else fail "installed pre-push is not executable"; fi

# allow <label> <ref> <expected sha> <push args...>
allow() {
  label=$1 ref=$2 want=$3; shift 3
  if out=$(w push -q throwaway "$@" 2>&1); then
    got=$(remote_ref "$ref")
    if [ "$got" = "$want" ]; then ok; else fail "$label: pushed, but $ref is $got, expected $want"; fi
  else
    fail "$label: push refused: $out"
  fi
}
# refuse <label> <push args...>: the guard fires with $protected's refusal and
# no remote ref moves.
refuse() {
  label=$1; shift
  refusal="pre-push: refusing to push to ${protected#refs/heads/}; merge through a pull request on GitHub"
  before_p=$(remote_ref "$protected"); before_d=$(remote_ref refs/heads/dev)
  if out=$(w push -q throwaway "$@" 2>&1); then
    fail "$label: push went through"
  elif ! printf '%s\n' "$out" | grep -qF "$refusal"; then
    fail "$label: push failed, but not with the guard's refusal: $out"
  elif [ "$(remote_ref "$protected")" != "$before_p" ] || [ "$(remote_ref refs/heads/dev)" != "$before_d" ]; then
    fail "$label: refused, but a remote ref moved"
  else
    ok
  fi
}

base=$(w rev-parse HEAD)
allow  "push to dev" refs/heads/dev "$base" dev
allow  "push to audit" refs/heads/audit "$base" HEAD:refs/heads/audit
allow  "push to a ticket branch" refs/heads/t1-x "$base" HEAD:refs/heads/t1-x
w commit -q --allow-empty -m second || die "second commit"
second=$(w rev-parse HEAD)
w checkout -q -b other "$base" || die "checkout other"
w commit -q --allow-empty -m diverged || die "diverged commit"

for protected in refs/heads/main refs/heads/v0.1-resolver; do
  name=${protected#refs/heads/}
  refuse "create $name" "$base:$protected"
  [ "$(remote_ref "$protected")" = absent ] || fail "$name exists on the remote after a refused create"
  # Seed it directly in the bare repository, so update, force-push and
  # deletion each have a ref to act on.
  git --git-dir="$bare" update-ref "$protected" "$base" || die "seed $protected"
  refuse "fast-forward $name" "$second:$protected"
  refuse "force-push $name" --force "HEAD:$protected"
  refuse "delete $name" ":$protected"
  # git aborts the whole push when the hook exits non-zero, so a dev update
  # riding along with a protected one must not land either.
  refuse "dev alongside $name" "$second:refs/heads/dev" "$second:$protected"
  [ "$(remote_ref refs/heads/dev)" = "$base" ] || fail "dev moved in a push the guard refused"
done
# The check is on the remote ref alone: a local branch named main may still
# update dev.
w branch -q main "$second" || die "local main branch"
allow  "local main to dev" refs/heads/dev "$second" "main:refs/heads/dev"

echo "pre-push self-test: $pass passed, $bad failed"
[ "$bad" -eq 0 ]
