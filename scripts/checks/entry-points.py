#!/usr/bin/env python3
"""entry-points.py: every file and symbol docs/ENTRY_POINTS.md names exists.

    scripts/checks/entry-points.py               check the page
    scripts/checks/entry-points.py --self-test   prove each check bites

docs/ENTRY_POINTS.md tells a developer where each program starts and where
the flow goes next. It names each step as a pair of code spans, a path and
then a symbol with only whitespace between them:

    `desktop/src/state.rs` `pub fn update`

A pair is two code spans the first of which holds a `/` and no space. The
check fails when the path is not a file in the repository, or when the
symbol does not occur in it. A path named alone (a code span with a `/` and
no space, not followed by a symbol) must be a file or a directory; one that
holds a `<placeholder>` is an example and is skipped. A symbol that begins or ends with a letter,
digit or underscore must stand as a whole word there, so `fn view` is not
satisfied by `fn view_reads` after `fn view` was renamed. Symbols and not
line numbers, because a line number moves with every edit above it; a
symbol only moves when it is renamed, and then this check names it.

Zero pairs found also fails: a scan that matched nothing proves nothing,
and a broken pattern would otherwise pass.

Exit 0: every pair holds. Exit 1: a pair does not hold (each printed), the
page names none, or the self-test failed. Exit 2: the page is missing.
"""
import os
import re
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
PAGE = os.path.join("docs", "ENTRY_POINTS.md")

# A path span (a `/`, no whitespace, no backtick), whitespace that may cross
# a line break, then a symbol span (anything but a backtick or a newline).
PAIR = re.compile(r"`([^`\s]*/[^`\s]*)`\s+`([^`\n]+)`")
# Any path span, paired or not.
PATH = re.compile(r"`([^`\s]*/[^`\s]*)`")


def is_word(c):
    return c.isalnum() or c == "_"


def pairs(text):
    return [(m.group(1), m.group(2)) for m in PAIR.finditer(text)]


def outside(path):
    return os.path.isabs(path) or ".." in path.split("/")


def problems(root, page):
    """Every way the page at root/page fails, as lines to print; [] when
    every pair holds. Raises FileNotFoundError when the page is missing."""
    with open(os.path.join(root, page), encoding="utf-8") as f:
        text = f.read()
    found = pairs(text)
    if not found:
        return [f"{page}: no `path` `symbol` pair found; a check that matches nothing proves nothing"]
    out = []
    paired = {path for path, _ in found}
    for path in dict.fromkeys(m.group(1) for m in PATH.finditer(text)):
        if path in paired or "<" in path:
            continue
        if outside(path):
            out.append(f"{page}: `{path}` is not a path inside the repository")
        elif not os.path.exists(os.path.join(root, *path.rstrip("/").split("/"))):
            out.append(f"{page}: `{path}` is neither a file nor a directory")
    for path, symbol in found:
        if outside(path):
            out.append(f"{page}: `{path}` is not a path inside the repository")
            continue
        full = os.path.join(root, *path.split("/"))
        if not os.path.isfile(full):
            out.append(f"{page}: `{path}` is not a file (named with `{symbol}`)")
            continue
        with open(full, encoding="utf-8", errors="replace") as f:
            body = f.read()
        pattern = re.escape(symbol)
        if is_word(symbol[0]):
            pattern = r"\b" + pattern
        if is_word(symbol[-1]):
            pattern = pattern + r"\b"
        if not re.search(pattern, body):
            out.append(f"{page}: `{symbol}` does not occur in `{path}`")
    return out


def self_test():
    """Plants each failure in a scratch repository and checks it is caught,
    and that a page whose pairs all hold passes."""
    failures = []
    with tempfile.TemporaryDirectory() as root:
        os.makedirs(os.path.join(root, "docs"))
        os.makedirs(os.path.join(root, "src"))
        with open(os.path.join(root, "src", "lib.rs"), "w", encoding="utf-8") as f:
            f.write("pub fn view_reads() {}\npub fn update(x: u8) {}\n")

        def run(page_text):
            with open(os.path.join(root, PAGE), "w", encoding="utf-8") as f:
                f.write(page_text)
            return problems(root, PAGE)

        cases = [
            # (what, page, a fragment the report must hold, or None to pass)
            ("every pair holds", "`src/lib.rs` `pub fn update`, and across a line:\n`src/lib.rs`\n  `fn view_reads`;"
             " alone, `src/` and `src/lib.rs`; a placeholder, `src/<name>.rs`\n", None),
            ("a missing path named alone", "`src/lib.rs` `pub fn update`, then `src/gone/` alone\n",
             "`src/gone/` is neither a file nor a directory"),
            ("a missing file", "`src/gone.rs` `pub fn update`\n", "`src/gone.rs` is not a file"),
            ("a missing symbol", "`src/lib.rs` `pub fn render`\n", "`pub fn render` does not occur"),
            ("a symbol that is only a prefix of another", "`src/lib.rs` `pub fn view`\n", "`pub fn view` does not occur"),
            ("a path outside the repository", "`../src/lib.rs` `pub fn update`\n", "not a path inside the repository"),
            ("a page that names no pair", "`src/lib.rs` and `pub fn update`, apart\n", "no `path` `symbol` pair found"),
        ]
        for what, page, want in cases:
            got = run(page)
            if want is None and got:
                failures.append(f"{what}: expected a pass, got {got}")
            elif want is not None and not any(want in g for g in got):
                failures.append(f"{what}: expected a report holding {want!r}, got {got}")
    try:
        problems(os.path.join(ROOT, "no-such-directory"), PAGE)
        failures.append("a missing page: expected FileNotFoundError, got none")
    except FileNotFoundError:
        pass
    if failures:
        for f in failures:
            print(f"FAIL: self-test: {f}")
        return 1
    print("OK: self-test (a missing file, a missing symbol, a renamed prefix, a missing path "
          "named alone, a path outside the repository and a page with no pair each fail; "
          "a page whose pairs and paths hold passes)")
    return 0


def main(argv):
    if argv == ["--self-test"]:
        return self_test()
    if argv:
        print(__doc__.strip().splitlines()[2].strip())
        print(__doc__.strip().splitlines()[3].strip())
        return 1
    try:
        found = problems(ROOT, PAGE)
    except FileNotFoundError:
        print(f"FAIL: {PAGE} is missing")
        return 2
    if found:
        for p in found:
            print(f"FAIL: {p}")
        return 1
    with open(os.path.join(ROOT, PAGE), encoding="utf-8") as f:
        n = len(pairs(f.read()))
    print(f"OK: {PAGE} names {n} file and symbol pairs, and each symbol occurs in its file")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
