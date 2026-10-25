#!/usr/bin/env python3
"""design-tokens.py: docs/design/DESIGN.md's token tables match the wireframe.

    scripts/checks/design-tokens.py               check
    scripts/checks/design-tokens.py --write       regenerate the tables from the wireframe
    scripts/checks/design-tokens.py --self-test   prove the check bites on a drifted value

The source of truth for every design token is the first <style> block of
docs/design/run-view-wireframe.html (DESIGN.md section 3). DESIGN.md carries
a copy of the values for reading; a copy that drifts is worse than none, so
this check regenerates the two tables from the wireframe and fails when
either differs from what the document holds. Values are compared as text,
so a token added, removed, renamed or recoloured in either file fails here.

Exit 0: the tables match (or were written). Exit 1: they differ, with a
diff, or the self-test failed. Exit 2: a file or a marker is missing.
"""
import difflib
import os
import re
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
WIREFRAME = os.path.join(ROOT, "docs", "design", "run-view-wireframe.html")
DESIGN = os.path.join(ROOT, "docs", "design", "DESIGN.md")

# The three selectors the wireframe's token block declares, in order: the
# dark theme (also :root), the light theme, and the theme-free base tokens.
DARK = ":root,[data-theme=dark]"
LIGHT = "[data-theme=light]"
BASE = ":root"


def read(path):
    try:
        with open(path, encoding="utf-8") as f:
            return f.read()
    except OSError as e:
        print(f"FAIL: cannot read {path}: {e}")
        sys.exit(2)


def token_block(html):
    """The first <style> block, where the tokens live (docs/design/README.md)."""
    m = re.search(r"<style[^>]*>(.*?)</style>", html, re.S)
    if not m:
        print(f"FAIL: no <style> block in {WIREFRAME}")
        sys.exit(2)
    return m.group(1)


def tokens(block, selector):
    """{name: value} for the one rule whose selector is exactly `selector`."""
    m = re.search(re.escape(selector) + r"\{(.*?)\}", block, re.S)
    if not m:
        print(f"FAIL: no rule for {selector} in the wireframe's token block")
        sys.exit(2)
    pairs = re.findall(r"(--[\w\\.-]+)\s*:\s*([^;]+);", m.group(1))
    # CSS escapes the dot in --space-0.5; the document shows the plain name.
    return {k.replace("\\", ""): v.strip() for k, v in pairs}


def tables(html):
    """The two markdown tables DESIGN.md holds, generated from the wireframe."""
    block = token_block(html)
    dark, light, base = tokens(block, DARK), tokens(block, LIGHT), tokens(block, BASE)
    colour = ["| Token | Dark | Light |", "|---|---|---|"]
    for k, v in dark.items():
        colour.append(f"| `{k}` | `{v}` | `{light.get(k, '')}` |")
    for k in light:
        if k not in dark:
            colour.append(f"| `{k}` | `` | `{light[k]}` |")
    plain = ["| Token | Value |", "|---|---|"]
    for k, v in base.items():
        plain.append(f"| `{k}` | `{v}` |")
    return {"colour": "\n".join(colour), "base": "\n".join(plain)}


def section(doc, name):
    """The text between <!-- tokens:name --> and <!-- /tokens:name -->."""
    m = re.search(
        rf"<!-- tokens:{name} -->\n(.*?)\n<!-- /tokens:{name} -->", doc, re.S
    )
    if not m:
        print(f"FAIL: no <!-- tokens:{name} --> section in {DESIGN}")
        sys.exit(2)
    return m


def check(wireframe_path, design_path):
    doc = read(design_path)
    want = tables(read(wireframe_path))
    ok = True
    for name, table in want.items():
        have = section(doc, name).group(1)
        if have != table:
            ok = False
            print(f"FAIL: DESIGN.md tokens:{name} differs from the wireframe")
            sys.stdout.writelines(
                difflib.unified_diff(
                    have.splitlines(True), table.splitlines(True),
                    "DESIGN.md", "run-view-wireframe.html",
                )
            )
    return ok


def write(wireframe_path, design_path):
    doc = read(design_path)
    for name, table in tables(read(wireframe_path)).items():
        m = section(doc, name)
        doc = doc[: m.start(1)] + table + doc[m.end(1):]
    with open(design_path, "w", encoding="utf-8", newline="\n") as f:
        f.write(doc)
    print(f"wrote {design_path}")


def self_test():
    """A drifted value, a missing token and an extra token must each fail."""
    html = read(WIREFRAME)
    doc = read(DESIGN)
    if not check(WIREFRAME, DESIGN):
        print("FAIL: self-test needs the committed files to match first")
        return False
    cases = {
        "drifted value": html.replace("--ground:#0b0b0d", "--ground:#000000", 1),
        "missing token": html.replace("--scrim:#000000b3;", "", 1),
        "extra token": html.replace("--scrim:", "--extra:#ffffff;--scrim:", 1),
    }
    with tempfile.TemporaryDirectory() as d:
        dpath = os.path.join(d, "DESIGN.md")
        with open(dpath, "w", encoding="utf-8") as f:
            f.write(doc)
        for label, variant in cases.items():
            if variant == html:
                print(f"FAIL: self-test could not plant '{label}'")
                return False
            wpath = os.path.join(d, "wireframe.html")
            with open(wpath, "w", encoding="utf-8") as f:
                f.write(variant)
            # Quietly: the planted diff is expected output, not a finding.
            out, sys.stdout = sys.stdout, open(os.devnull, "w")
            try:
                passed = check(wpath, dpath)
            finally:
                sys.stdout.close()
                sys.stdout = out
            if passed:
                print(f"FAIL: self-test: '{label}' was not caught")
                return False
    print("self-test: ok")
    return True


def main(argv):
    if argv == ["--self-test"]:
        return 0 if self_test() else 1
    if argv == ["--write"]:
        write(WIREFRAME, DESIGN)
        return 0
    if argv:
        print(__doc__)
        return 2
    return 0 if check(WIREFRAME, DESIGN) else 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
