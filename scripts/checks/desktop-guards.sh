#!/usr/bin/env bash
# desktop-guards.sh: Tauri stays out of the root graph, and desktop/ stays
# inside ADR 0048's lines.
#
#   scripts/checks/desktop-guards.sh [--self-test]
#
# desktop/ is its own Cargo workspace (ADR 0048 section 1), so nothing the
# root workspace's gates run reads its code or its lock. These are the lines
# that can be checked by reading files, each a rule named in brackets on the
# line that reports it:
#
#   root-lock            no `tauri` or `trellis-desktop` package in the root
#                        Cargo.lock: the root Cargo.toml is not edited, so the
#                        lock is what shows Tauri reaching the root graph
#   desktop-plugin       no `tauri-plugin-*` package in desktop/Cargo.lock
#                        (section 5: the shell has no Tauri plugin)
#   desktop-trellis-dep  no `trellis-*` dependency in desktop/Cargo.toml
#                        (section 1), in any dependency table, as a key, a
#                        `[dependencies.trellis-*]` table or a `package =`
#   desktop-lock-current `cargo metadata --locked` in desktop/ accepts its
#                        lock, so the audits below read what a build uses
#   src-command          `Command::new` only in desktop/src/child.rs
#                        (section 2: one file starts trellis)
#   src-kill, src-envs, src-env-clear, src-env-var, src-api-key,
#   src-key-env          none of `.kill(`, `.envs(`, `env_clear`,
#                        `env::var` (and so `var_os`, `vars`, `vars_os`),
#                        `_API_KEY`, `key_env` under desktop/src/ (section 3:
#                        quit and wait, add nothing to a child's
#                        environment, read no key variable)
#   src-env              `.env(` under desktop/src/ only as
#                        `.env("TRELLIS_CONFIG_DIR",` after the file's first
#                        `#[cfg(test)]` line: test code may keep a test off
#                        the developer's own config, and nothing else may
#                        set a variable
#   ui-*                 under desktop/ui/, nothing that parses a string as
#                        HTML or script, opens a window or sets an inline
#                        handler (section 5; ADR 0007, S10)
#   deny-copy            desktop/deny.toml is the root deny.toml line for
#                        line, in order, once the lines between
#                        `# desktop-only: begin` and `# desktop-only: end`
#                        are removed; markers balanced (section 7)
#   audit-toml           no audit.toml under desktop/ or .cargo/: cargo audit
#                        ignores no `vulnerability` (section 7)
#
# The scans read text line by line, comments included, and do not see a
# name built at run time; review of the diff answers for that.
#
# --self-test copies the tracked files these rules read to a temporary
# directory, shows the copy passes, then plants each case on a fresh copy
# and shows the script refuses it naming the rule. Nothing is planted in the
# working tree.
#
# Exit 0: every rule holds (or every self-test case held). Exit 1: a rule
# failed, each failure printed, or a self-test case did not hold. Exit 2:
# wrong arguments, or not in a git repository.
set -uo pipefail

usage='usage: desktop-guards.sh [--self-test]'
# Git Bash rewrites a path-looking argument otherwise.
export MSYS_NO_PATHCONV=1

# What cargo prints when --locked stops it rewriting the lock, as in
# fuzz-lock-current.sh (cargo 1.93 and earlier wordings).
STALE_MSG='lock file .* --locked was passed to prevent this'

# rule|ERE, matched under desktop/src/.
SRC_RULES=(
  'src-kill|\.kill\('
  'src-envs|\.envs\('
  'src-env-clear|env_clear'
  'src-env-var|env::var'
  'src-api-key|_API_KEY'
  'src-key-env|key_env'
)

# rule|ERE, matched under desktop/ui/.
UI_RULES=(
  'ui-innerHTML|innerHTML'
  'ui-outerHTML|outerHTML'
  'ui-insertAdjacentHTML|insertAdjacentHTML'
  'ui-document-write|document\.write'
  'ui-eval|eval\('
  'ui-new-function|new Function'
  'ui-domparser|DOMParser'
  'ui-contextual-fragment|createContextualFragment'
  'ui-srcdoc|srcdoc'
  'ui-setHTMLUnsafe|setHTMLUnsafe'
  'ui-parseHTMLUnsafe|parseHTMLUnsafe'
  "ui-on-attribute|setAttribute\\([[:space:]]*[\"']on"
  'ui-window-open|window\.open'
  "ui-target-blank|target=[\"']_blank"
  "ui-string-timer|set(Timeout|Interval)[[:space:]]*\\([[:space:]]*[\"'\`]"
)

# check_tree <dir>: every rule against the tree at <dir>, laid out as the
# repository is. Prints one `FAIL [rule]: ...` line per finding (cargo's or
# diff's output indented under it) and returns 1 if there was any.
check_tree() {
  local d=$1 bad=0 hits rc entry rule re f out

  # root-lock. [[:space:]] takes a CR a Windows checkout may leave.
  hits=$(grep -nE '^name = "(tauri|trellis-desktop)"[[:space:]]*$' "$d/Cargo.lock")
  rc=$?
  if [ "$rc" -gt 1 ]; then
    echo "FAIL [root-lock]: cannot read Cargo.lock"; bad=1
  elif [ -n "$hits" ]; then
    printf '%s\n' "$hits" | sed 's/^/FAIL [root-lock]: Cargo.lock:/'; bad=1
  fi

  # desktop-plugin
  hits=$(grep -nE '^name = "tauri-plugin-' "$d/desktop/Cargo.lock")
  rc=$?
  if [ "$rc" -gt 1 ]; then
    echo "FAIL [desktop-plugin]: cannot read desktop/Cargo.lock"; bad=1
  elif [ -n "$hits" ]; then
    printf '%s\n' "$hits" | sed 's/^/FAIL [desktop-plugin]: desktop\/Cargo.lock:/'; bad=1
  fi

  # desktop-trellis-dep. A table whose header names `dependencies` is a
  # dependency table ([dependencies], [dev-dependencies],
  # [build-dependencies], [target.*.dependencies], [dependencies.<name>]).
  hits=$(tr -d '\r' < "$d/desktop/Cargo.toml" | awk '
    /^[[:space:]]*\[/ {
      h = $0; gsub(/[[:space:]"'\'']/, "", h)
      dep = (h ~ /dependencies/)
      if (h ~ /dependencies\.trellis[-_]/) print NR ": " $0
      next
    }
    dep && /^[[:space:]]*["'\'']?trellis[-_]/ { print NR ": " $0; next }
    dep && /package[[:space:]]*=[[:space:]]*["'\'']trellis[-_]/ { print NR ": " $0 }
  ')
  if [ -n "$hits" ]; then
    printf '%s\n' "$hits" | sed 's/^/FAIL [desktop-trellis-dep]: desktop\/Cargo.toml:/'; bad=1
  fi

  # desktop-lock-current. In desktop/ with no path argument, so Git Bash has
  # nothing to convert. The lock's checksum is compared before and after, so
  # a cargo that wrote anyway is caught rather than trusted.
  local before after
  before=$(cksum < "$d/desktop/Cargo.lock")
  (cd "$d/desktop" && cargo metadata --locked --format-version 1 >/dev/null 2>"$ERRF")
  rc=$?
  after=$(cksum < "$d/desktop/Cargo.lock")
  if [ "$before" != "$after" ]; then
    echo "FAIL [desktop-lock-current]: desktop/Cargo.lock changed during a --locked cargo metadata"; bad=1
  fi
  if [ "$rc" -ne 0 ]; then
    if grep -qE -- "$STALE_MSG" "$ERRF"; then
      echo "FAIL [desktop-lock-current]: desktop/Cargo.lock is stale: building desktop/ would rewrite it"
      echo "  fix: (cd desktop && cargo metadata --format-version 1 >/dev/null), then commit desktop/Cargo.lock on its own"
    else
      echo "FAIL [desktop-lock-current]: cargo metadata --locked in desktop/ failed ($rc) for a reason other than a stale lock"
    fi
    sed 's/^/  /' "$ERRF"
    bad=1
  fi
  : > "$ERRF"

  # src-command
  hits=$(cd "$d" && grep -rn -F 'Command::new' desktop/src | grep -v '^desktop/src/child\.rs:')
  if [ -n "$hits" ]; then
    printf '%s\n' "$hits" | sed 's/^/FAIL [src-command]: /'; bad=1
  fi

  # src-* and ui-*: grep exits 1 on no match and 2 on an error, which is a
  # failure to run, not a pass.
  for entry in "${SRC_RULES[@]/#/desktop/src|}" "${UI_RULES[@]/#/desktop/ui|}"; do
    f=${entry%%|*}
    entry=${entry#*|}
    rule=${entry%%|*}
    re=${entry#*|}
    hits=$(cd "$d" && grep -rnE -- "$re" "$f")
    rc=$?
    if [ "$rc" -gt 1 ]; then
      echo "FAIL [$rule]: grep could not read $f"; bad=1
    elif [ -n "$hits" ]; then
      printf '%s\n' "$hits" | sed "s/^/FAIL [$rule]: /"; bad=1
    fi
  done

  # src-env: every `.env(` left once each `.env("TRELLIS_CONFIG_DIR",` past
  # the file's first `#[cfg(test)]` line is taken out.
  out=$(cd "$d" && find desktop/src -type f | sort | while IFS= read -r f; do
    tr -d '\r' < "$f" | awk -v f="$f" '
      !t && /#\[cfg\(test\)\]/ { t = NR }
      /\.env\(/ {
        line = $0
        if (t && NR > t) gsub(/\.env\("TRELLIS_CONFIG_DIR",/, "", line)
        if (line ~ /\.env\(/) print f ":" NR ":" $0
      }'
  done)
  if [ -n "$out" ]; then
    printf '%s\n' "$out" | sed 's/^/FAIL [src-env]: /'; bad=1
  fi

  # deny-copy
  # Gated on the reads succeeding, not on content: an empty desktop/deny.toml
  # is a copy missing every root line, not one with nothing to check.
  local root_deny desk_deny read_ok=0
  root_deny=$(tr -d '\r' < "$d/deny.toml") \
    && desk_deny=$(tr -d '\r' < "$d/desktop/deny.toml") \
    && read_ok=1 \
    || { echo "FAIL [deny-copy]: cannot read deny.toml or desktop/deny.toml"; bad=1; }
  if [ "$read_ok" -eq 1 ]; then
    out=$(printf '%s\n' "$desk_deny" | awk '
      /^# desktop-only: begin$/ { if (open) print NR ": a second begin before an end"; open = 1; next }
      /^# desktop-only: end$/   { if (!open) print NR ": an end with no begin"; open = 0; next }
      END { if (open) print "end of file: a begin with no end" }')
    if [ -n "$out" ]; then
      printf '%s\n' "$out" | sed 's/^/FAIL [deny-copy]: desktop\/deny.toml unbalanced markers: /'; bad=1
    fi
    out=$(diff <(printf '%s\n' "$root_deny") <(printf '%s\n' "$desk_deny" | awk '
      /^# desktop-only: begin$/ { open = 1; next }
      /^# desktop-only: end$/   { open = 0; next }
      !open'))
    if [ -n "$out" ]; then
      echo "FAIL [deny-copy]: desktop/deny.toml outside its desktop-only markers is not the root deny.toml (< root only, > desktop only):"
      printf '%s\n' "$out" | sed 's/^/  /'
      bad=1
    fi
  fi

  # audit-toml. desktop/target is a build directory, not the tree.
  hits=$(cd "$d" && find desktop -path desktop/target -prune -o -name audit.toml -print; \
         [ -d .cargo ] && find .cargo -name audit.toml -print)
  if [ -n "$hits" ]; then
    printf '%s\n' "$hits" | sed 's/^/FAIL [audit-toml]: /'; bad=1
  fi

  return $bad
}

# plant <case> <dir>: writes one case into the copy at <dir>.
plant() {
  local d=$2
  case $1 in
    root-lock-tauri)
      printf '\n[[package]]\nname = "tauri"\nversion = "2.0.0"\n' >> "$d/Cargo.lock" ;;
    root-lock-desktop)
      printf '\n[[package]]\nname = "trellis-desktop"\nversion = "0.0.1"\n' >> "$d/Cargo.lock" ;;
    desktop-plugin)
      printf '\n[[package]]\nname = "tauri-plugin-shell"\nversion = "2.0.0"\n' >> "$d/desktop/Cargo.lock" ;;
    trellis-dep-key)
      printf '\n[target.'\''cfg(windows)'\''.dependencies]\ntrellis-core = { path = "../crates/trellis-core" }\n' >> "$d/desktop/Cargo.toml" ;;
    trellis-dep-table)
      printf '\n[dev-dependencies.trellis-vcs]\npath = "../crates/trellis-vcs"\n' >> "$d/desktop/Cargo.toml" ;;
    trellis-dep-package)
      printf '\n[build-dependencies]\ncore = { package = "trellis-core", path = "../crates/trellis-core" }\n' >> "$d/desktop/Cargo.toml" ;;
    lock-stale)
      # serde_json, a direct dependency, cut from the lock: a package the
      # manifest reaches and the lock does not record. Blocks are blank-line
      # separated, so paragraph mode drops exactly one.
      awk 'BEGIN { RS = ""; ORS = "\n\n" } !/\nname = "serde_json"\n/' \
        "$d/desktop/Cargo.lock" > "$d/stale.lock" && mv "$d/stale.lock" "$d/desktop/Cargo.lock" ;;
    src-command)
      printf 'fn f() { std::process::Command::new("trellis"); }\n' > "$d/desktop/src/planted.rs" ;;
    src-kill)      printf 'fn f(c: &mut std::process::Child) { let _ = c.kill(); }\n' > "$d/desktop/src/planted.rs" ;;
    src-envs)      printf 'fn f() { c.envs(vars); }\n' > "$d/desktop/src/planted.rs" ;;
    src-env-clear) printf 'fn f() { c.env_clear(); }\n' > "$d/desktop/src/planted.rs" ;;
    src-env-var)   printf 'fn f() { let _ = std::env::var_os("HOME"); }\n' > "$d/desktop/src/planted.rs" ;;
    src-api-key)   printf 'const K: &str = "OPENAI_API_KEY";\n' > "$d/desktop/src/planted.rs" ;;
    src-key-env)   printf 'fn f(key_env: &str) {}\n' > "$d/desktop/src/planted.rs" ;;
    src-env-plain)
      printf 'fn f() { c.env("HOME", "/tmp"); }\n' > "$d/desktop/src/planted.rs" ;;
    src-env-config-before-test)
      printf 'fn f() { c.env("TRELLIS_CONFIG_DIR", &d); }\n#[cfg(test)]\nmod tests {}\n' > "$d/desktop/src/planted.rs" ;;
    src-env-other-in-test)
      printf '#[cfg(test)]\nfn f() { c.env("TRELLIS_CONFIG_DIR", &d).env("HOME", &d); }\n' > "$d/desktop/src/planted.rs" ;;
    ui-innerHTML)          printf 'el.innerHTML = s;\n' > "$d/desktop/ui/planted.js" ;;
    ui-outerHTML)          printf 'el.outerHTML = s;\n' > "$d/desktop/ui/planted.js" ;;
    ui-insertAdjacentHTML) printf 'el.insertAdjacentHTML("beforeend", s);\n' > "$d/desktop/ui/planted.js" ;;
    ui-document-write)     printf 'document.write(s);\n' > "$d/desktop/ui/planted.js" ;;
    ui-eval)               printf 'eval(s);\n' > "$d/desktop/ui/planted.js" ;;
    ui-new-function)       printf 'const g = new Function(s);\n' > "$d/desktop/ui/planted.js" ;;
    ui-domparser)          printf 'new DOMParser().parseFromString(s, "text/html");\n' > "$d/desktop/ui/planted.js" ;;
    ui-contextual-fragment) printf 'range.createContextualFragment(s);\n' > "$d/desktop/ui/planted.js" ;;
    ui-srcdoc)             printf '<iframe srcdoc="x"></iframe>\n' > "$d/desktop/ui/planted.html" ;;
    ui-setHTMLUnsafe)      printf 'el.setHTMLUnsafe(s);\n' > "$d/desktop/ui/planted.js" ;;
    ui-parseHTMLUnsafe)    printf 'Document.parseHTMLUnsafe(s);\n' > "$d/desktop/ui/planted.js" ;;
    ui-on-attribute-dq)    printf 'el.setAttribute("onclick", s);\n' > "$d/desktop/ui/planted.js" ;;
    ui-on-attribute-sq)    printf 'el.setAttribute('\''onload'\'', s);\n' > "$d/desktop/ui/planted.js" ;;
    ui-window-open)        printf 'window.open(url);\n' > "$d/desktop/ui/planted.js" ;;
    ui-target-blank)       printf '<a href="x" target="_blank">x</a>\n' > "$d/desktop/ui/planted.html" ;;
    ui-string-timeout)     printf 'setTimeout("tick()", 10);\n' > "$d/desktop/ui/planted.js" ;;
    ui-string-interval)    printf 'setInterval('\''tick()'\'', 10);\n' > "$d/desktop/ui/planted.js" ;;
    deny-missing-line)
      sed -i '/^wildcards = "deny"$/d' "$d/desktop/deny.toml" ;;
    deny-extra-line)
      printf 'unused-allowed-license = "allow"\n' >> "$d/desktop/deny.toml" ;;
    deny-unclosed-marker)
      printf '# desktop-only: begin\n' >> "$d/desktop/deny.toml" ;;
    deny-empty-file)
      : > "$d/desktop/deny.toml" ;;
    audit-toml-desktop)
      printf '[advisories]\nignore = []\n' > "$d/desktop/audit.toml" ;;
    audit-toml-cargo)
      mkdir -p "$d/.cargo" && printf '[advisories]\nignore = []\n' > "$d/.cargo/audit.toml" ;;
    *) return 1 ;;
  esac
}

# case|rule the script must name for it.
CASES=(
  'root-lock-tauri|root-lock'
  'root-lock-desktop|root-lock'
  'desktop-plugin|desktop-plugin'
  'trellis-dep-key|desktop-trellis-dep'
  'trellis-dep-table|desktop-trellis-dep'
  'trellis-dep-package|desktop-trellis-dep'
  'lock-stale|desktop-lock-current'
  'src-command|src-command'
  'src-kill|src-kill'
  'src-envs|src-envs'
  'src-env-clear|src-env-clear'
  'src-env-var|src-env-var'
  'src-api-key|src-api-key'
  'src-key-env|src-key-env'
  'src-env-plain|src-env'
  'src-env-config-before-test|src-env'
  'src-env-other-in-test|src-env'
  'ui-innerHTML|ui-innerHTML'
  'ui-outerHTML|ui-outerHTML'
  'ui-insertAdjacentHTML|ui-insertAdjacentHTML'
  'ui-document-write|ui-document-write'
  'ui-eval|ui-eval'
  'ui-new-function|ui-new-function'
  'ui-domparser|ui-domparser'
  'ui-contextual-fragment|ui-contextual-fragment'
  'ui-srcdoc|ui-srcdoc'
  'ui-setHTMLUnsafe|ui-setHTMLUnsafe'
  'ui-parseHTMLUnsafe|ui-parseHTMLUnsafe'
  'ui-on-attribute-dq|ui-on-attribute'
  'ui-on-attribute-sq|ui-on-attribute'
  'ui-window-open|ui-window-open'
  'ui-target-blank|ui-target-blank'
  'ui-string-timeout|ui-string-timer'
  'ui-string-interval|ui-string-timer'
  'deny-missing-line|deny-copy'
  'deny-extra-line|deny-copy'
  'deny-unclosed-marker|deny-copy'
  'deny-empty-file|deny-copy'
  'audit-toml-desktop|audit-toml'
  'audit-toml-cargo|audit-toml'
)

ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || { echo "FAIL: not in a git repository"; exit 2; }
cd "$ROOT" || exit 2
tmp="$(mktemp -d)" || { echo "FAIL: mktemp -d failed"; exit 1; }
trap 'rm -rf "$tmp"' EXIT
# cargo metadata's stderr, outside every tree check_tree reads.
ERRF="$tmp/metadata-err"

if [ "${1:-}" = "--self-test" ]; then
  [ $# -eq 1 ] || { echo "$usage"; exit 2; }
  # The tracked files the rules read, as they are in the working tree, laid
  # out as the repository is. rust-toolchain.toml keeps cargo the pinned one.
  base="$tmp/base"
  mkdir -p "$base" || { echo "FAIL: could not create $base"; exit 1; }
  git ls-files -z -- Cargo.lock deny.toml rust-toolchain.toml desktop .cargo \
    | xargs -0 cp --parents -t "$base" \
    || { echo "FAIL: could not copy the tracked files to $base"; exit 1; }
  if ! out=$(check_tree "$base"); then
    echo "FAIL: self-test: the unmodified copy fails, so the cases below would prove nothing:"
    printf '%s\n' "$out" | sed 's/^/  /'
    exit 1
  fi
  fail=0
  for entry in "${CASES[@]}"; do
    name=${entry%%|*}
    rule=${entry#*|}
    c="$tmp/case"
    rm -rf "$c" && cp -r "$base" "$c" && plant "$name" "$c" \
      || { echo "FAIL: self-test: could not plant $name"; fail=1; continue; }
    out=$(check_tree "$c")
    rc=$?
    if [ "$rc" -ne 1 ] || ! printf '%s\n' "$out" | grep -qF "FAIL [$rule]:"; then
      echo "FAIL: self-test: $name exited $rc without naming [$rule]:"
      printf '%s\n' "$out" | sed 's/^/  /'
      fail=1
    fi
  done
  [ "$fail" -eq 0 ] || exit 1
  echo "OK: self-test (an unmodified copy passes; each of ${#CASES[@]} planted cases fails naming its rule)"
  exit 0
fi
[ $# -eq 0 ] || { echo "$usage"; exit 2; }

check_tree "$ROOT" || exit 1
echo "OK: no Tauri in the root lock; desktop/ has no plugin or trellis crate, a current lock, one spawning file, no kill, env or key read, no HTML or script parsed from a string in ui/, a deny.toml in step with the root's, and no audit.toml"
