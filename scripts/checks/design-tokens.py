#!/usr/bin/env python3
"""design-tokens.py: every copy of the design tokens matches tokens.json.

    scripts/checks/design-tokens.py               check every copy
    scripts/checks/design-tokens.py --write       regenerate DESIGN.md's tables
    scripts/checks/design-tokens.py --self-test   prove each check bites

docs/design/system/tokens.json is the one source of every token value: the
owner's design system, vendored into the repository (DESIGN.md section 3).
Everything else is a copy that must agree with it:

- the three tables in docs/design/DESIGN.md (colour, base, terminal), which
  --write regenerates from tokens.json;
- the token block at the top of docs/design/run-view-wireframe.html (the
  dark rule, the light rule and the theme-free rule of its first <style>),
  checked name by name and in order, and never rewritten by this script.

A client that keeps its own copy (desktop/ui/style.css, the TUI's theme
table) is added here by the ticket that creates that copy, so a value has one
place to change and one check that names where it drifted.

Exit 0: every copy matches (or the tables were written). Exit 1: a copy
differs, each difference printed, or the self-test failed. Exit 2: a file, a
selector or a marker is missing, or tokens.json lacks a section.
"""
import difflib
import json
import os
import re
import shutil
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
SOURCE = os.path.join(ROOT, "docs", "design", "system", "tokens.json")
WIREFRAME = os.path.join(ROOT, "docs", "design", "run-view-wireframe.html")
DESIGN = os.path.join(ROOT, "docs", "design", "DESIGN.md")

# The wireframe's three token rules, by the selector each is written under.
RULES = (("dark", ":root,[data-theme=dark]"), ("light", "[data-theme=light]"), ("base", ":root"))
TABLES = ("colour", "base", "terminal")


class Missing(Exception):
    """A file, selector, marker or section this check needs is absent (exit 2)."""


def read(path):
    try:
        with open(path, encoding="utf-8") as f:
            return f.read()
    except OSError as e:
        raise Missing(f"cannot read {path}: {e}") from e


def load(source_path):
    """tokens.json as ordered (name, value) lists: dark, light, base, terminal."""
    try:
        data = json.loads(read(source_path))
        dark, light = [], []
        for t in data["color"]["tokens"] + data["shadow"]["tokens"]:
            v = t["value"]
            # A value with no per-theme split (cool-fill) is the same in both.
            d, l = (v["dark"], v["light"]) if isinstance(v, dict) else (v, v)
            dark.append(("--" + t["name"], d))
            light.append(("--" + t["name"], l))
        base = [("--" + t["name"], t["value"])
                for t in data["spacing"]["tokens"] + data["radius"]["tokens"]]
        base += [("--font-" + fam, stack) for fam, stack in data["type"]["families"].items()]
        terminal = [(t["name"], t["value"], t["usage"]) for t in data["terminal"]["tokens"]]
    except json.JSONDecodeError as e:
        raise Missing(f"{source_path} is not JSON: {e}") from e
    except (KeyError, TypeError) as e:
        raise Missing(f"{source_path} lacks a section this check reads: {e}") from e
    return {"dark": dark, "light": light, "base": base, "terminal": terminal}


def tables(tok):
    """The markdown tables DESIGN.md holds, generated from the tokens."""
    light = dict(tok["light"])
    colour = ["| Token | Dark | Light |", "|---|---|---|"]
    colour += [f"| `{n}` | `{d}` | `{light[n]}` |" for n, d in tok["dark"]]
    base = ["| Token | Value |", "|---|---|"]
    base += [f"| `{n}` | `{v}` |" for n, v in tok["base"]]
    terminal = ["| Token | 256-colour index | Usage, with the 16-colour fallback |", "|---|---|---|"]
    for n, v, usage in tok["terminal"]:
        cell = usage.replace("|", "\\|")
        terminal.append(f"| `{n}` | `{v}` | {cell} |")
    return {"colour": "\n".join(colour), "base": "\n".join(base), "terminal": "\n".join(terminal)}


def section(doc, name):
    """The text between <!-- tokens:name --> and <!-- /tokens:name -->."""
    # The body is empty (a new section) or whole lines ending in a newline.
    m = re.search(rf"<!-- tokens:{name} -->\n((?:.*?\n)?)<!-- /tokens:{name} -->", doc, re.S)
    if not m:
        raise Missing(f"no <!-- tokens:{name} --> section in DESIGN.md")
    return m


def wireframe_rules(html):
    """{dark, light, base}: the wireframe's token rules as (name, value) lists."""
    m = re.search(r"<style[^>]*>(.*?)</style>", html, re.S)
    if not m:
        raise Missing("no <style> block in the wireframe")
    out = {}
    for key, selector in RULES:
        r = re.search(re.escape(selector) + r"\{(.*?)\}", m.group(1), re.S)
        if not r:
            raise Missing(f"no rule for {selector} in the wireframe's token block")
        # CSS escapes the dot in --space-0.5; tokens.json spells it plainly.
        out[key] = [(k.replace("\\", ""), v.strip())
                    for k, v in re.findall(r"(--[\w\\.-]+)\s*:\s*([^;]+);", r.group(1))]
    return out


def compare(label, want, have):
    """Each way two ordered (name, value) lists differ, as printable lines."""
    wd, hd = dict(want), dict(have)
    found = [f"{label}: {n} is missing" for n in wd if n not in hd]
    found += [f"{label}: {n} is not in tokens.json" for n in hd if n not in wd]
    found += [f"{label}: {n} is {hd[n]}, tokens.json says {wd[n]}"
              for n in wd if n in hd and hd[n] != wd[n]]
    if not found and [n for n, _ in want] != [n for n, _ in have]:
        found.append(f"{label}: the same tokens in a different order")
    return found


def check(source_path, wireframe_path, design_path, out=sys.stdout):
    """True when the wireframe and DESIGN.md both agree with tokens.json."""
    tok = load(source_path)
    problems = []
    rules = wireframe_rules(read(wireframe_path))
    for key, _ in RULES:
        problems += compare(f"wireframe {key} rule", tok[key], rules[key])
    doc = read(design_path)
    for name, table in tables(tok).items():
        have = section(doc, name).group(1)
        if have != table + "\n":
            problems.append(f"DESIGN.md tokens:{name} differs from tokens.json "
                            "(scripts/checks/design-tokens.py --write regenerates it)")
            out.writelines(difflib.unified_diff(
                have.splitlines(True), (table + "\n").splitlines(True), "DESIGN.md", "tokens.json"))
            out.write("\n")
    for p in problems:
        out.write(f"FAIL: {p}\n")
    return not problems


def write(source_path, design_path):
    doc = read(design_path)
    for name, table in tables(load(source_path)).items():
        m = section(doc, name)
        doc = doc[: m.start(1)] + table + "\n" + doc[m.end(1):]
    with open(design_path, "w", encoding="utf-8", newline="\n") as f:
        f.write(doc)
    print(f"wrote {design_path}")


def self_test():
    """Plant each kind of drift in a copy of the three files; each must fail."""
    with open(os.devnull, "w") as quiet:
        if not check(SOURCE, WIREFRAME, DESIGN, out=quiet):
            print("FAIL: self-test needs the committed files to agree first")
            return False
    source, html, doc = read(SOURCE), read(WIREFRAME), read(DESIGN)
    cases = {
        "a value drifted in tokens.json": (source.replace('"#0b0b0d"', '"#000001"', 1), html, doc),
        "a value drifted in the wireframe": (source, html.replace("--ground:#0b0b0d", "--ground:#000001", 1), doc),
        "a token missing from the wireframe": (source, html.replace("--scrim:#000000b3;", "", 1), doc),
        "a token only the wireframe has": (source, html.replace("--scrim:", "--extra:#ffffff;--scrim:", 1), doc),
        "a DESIGN.md row edited by hand": (source, html, doc.replace("| `--ground` | `#0b0b0d`", "| `--ground` | `#000001`", 1)),
    }
    with tempfile.TemporaryDirectory() as d:
        paths = [os.path.join(d, n) for n in ("tokens.json", "wireframe.html", "DESIGN.md")]
        for label, files in cases.items():
            if files == (source, html, doc):
                print(f"FAIL: self-test could not plant '{label}'")
                return False
            for path, body in zip(paths, files):
                with open(path, "w", encoding="utf-8", newline="\n") as f:
                    f.write(body)
            with open(os.devnull, "w") as quiet:
                if check(*paths, out=quiet):
                    print(f"FAIL: self-test: '{label}' was not caught")
                    return False
        # --write repairs DESIGN.md after tokens.json moves, and touches
        # nothing else: the wireframe still fails until a person edits it.
        shutil.copyfile(DESIGN, paths[2])
        with open(paths[0], "w", encoding="utf-8", newline="\n") as f:
            f.write(cases["a value drifted in tokens.json"][0])
        with open(paths[1], "w", encoding="utf-8", newline="\n") as f:
            f.write(html)
        with open(os.devnull, "w") as quiet:
            stdout, sys.stdout = sys.stdout, quiet
            try:
                write(paths[0], paths[2])
            finally:
                sys.stdout = stdout
            tok = load(paths[0])
            regenerated = read(paths[2])
            if any(section(regenerated, n).group(1) != t + "\n" for n, t in tables(tok).items()):
                print("FAIL: self-test: --write did not regenerate the tables")
                return False
            if check(*paths, out=quiet):
                print("FAIL: self-test: the drifted wireframe passed after --write")
                return False
    print("self-test: ok")
    return True


def main(argv):
    try:
        if argv == ["--self-test"]:
            return 0 if self_test() else 1
        if argv == ["--write"]:
            write(SOURCE, DESIGN)
            return 0
        if argv:
            print(__doc__)
            return 2
        return 0 if check(SOURCE, WIREFRAME, DESIGN) else 1
    except Missing as e:
        print(f"FAIL: {e}")
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
