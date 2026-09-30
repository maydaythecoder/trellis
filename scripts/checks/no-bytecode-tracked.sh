#!/usr/bin/env bash
# No Python bytecode is tracked, and git ignores it, so running or importing a
# script never dirties the checkout the lander needs clean (#224).
set -u
root=$(git rev-parse --show-toplevel); cd "$root" || exit 2
tracked=$(git ls-files | grep -E '(^|/)__pycache__/|\.py[cod]$' || true)
if [ -n "$tracked" ]; then echo "FAIL: Python bytecode is tracked:"; printf '  %s\n' $tracked; exit 1; fi
for p in scripts/__pycache__/x.cpython-312.pyc qa/y.pyc; do
  git check-ignore -q "$p" || { echo "FAIL: $p is not ignored"; exit 1; }
done
echo "OK: no Python bytecode tracked, and __pycache__/ and *.pyc are ignored"
