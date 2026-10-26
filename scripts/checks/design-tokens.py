#!/usr/bin/env python3
"""design-tokens.py: every copy of the design tokens matches tokens.json.

    scripts/checks/design-tokens.py               check every copy
    scripts/checks/design-tokens.py --write       regenerate DESIGN.md's tables, tokens_gen.go and style.css's token block
    scripts/checks/design-tokens.py --self-test   prove each check bites

docs/design/system/tokens.json is the one source of every token value: the
owner's design system, vendored into the repository (DESIGN.md section 3).
Everything else is a copy that must agree with it:

- the three tables in docs/design/DESIGN.md (colour, base, terminal), which
  --write regenerates from tokens.json;
- tui/internal/theme/tokens_gen.go, the TUI's theme table, which --write
  also regenerates: each term-* token's colour on a dark and on a light
  terminal at truecolor, 256 and 16 colours (DESIGN.md section 3.4);
- the token block of desktop/ui/style.css, between its tokens:start and
  tokens:end markers, which --write also regenerates (#813): the colour and
  shadow tokens with Dark as the default and Daylight under
  [data-theme=light] and the light colour-scheme query, then the spacing,
  radius and font tokens. The desktop's own tokens, which the brand book has
  no name for, sit below the block and are the stylesheet's to keep;
- the token block at the top of docs/design/run-view-wireframe.html (the
  dark rule, the light rule and the theme-free rule of its first <style>),
  checked name by name and in order, and never rewritten by this script.

The theme table takes a term-* token's truecolor hex from the colour role of
the same name (term-ink is ink), its 256-colour index from its value, and its
16-colour index from its usage text: the index in parentheses beside its
colour's name ("bright black (8)"), the one marked "on dark terminals" for a
dark terminal where a usage gives two; no colour for "default background" or
"reverse video"; another token's for "Same as <role>". A usage it cannot read
fails the check. Below truecolor a light terminal gets no colour for the ink
roles (its own foreground) and every other role's 16-colour index at both
depths (owner decision, 2026-10-25, #840). An empty value is never painted.

Exit 0: every copy matches (or the copies were written). Exit 1: a copy
differs, each difference printed; a term-* usage gives no 16-colour value
the theme table can read; or the self-test failed. Exit 2: a file, a
selector or a marker is missing, or tokens.json lacks a section.
"""
import difflib
import io
import json
import os
import re
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
SOURCE = os.path.join(ROOT, "docs", "design", "system", "tokens.json")
WIREFRAME = os.path.join(ROOT, "docs", "design", "run-view-wireframe.html")
DESIGN = os.path.join(ROOT, "docs", "design", "DESIGN.md")
THEME = os.path.join(ROOT, "tui", "internal", "theme", "tokens_gen.go")
STYLE = os.path.join(ROOT, "desktop", "ui", "style.css")
# style.css's generated block starts at a line beginning with the first and
# ends with the line that is the second.
CSS_START, CSS_END = "/* tokens:start", "/* tokens:end */"

# The wireframe's three token rules, by the selector each is written under.
RULES = (("dark", ":root,[data-theme=dark]"), ("light", "[data-theme=light]"), ("base", ":root"))
TABLES = ("colour", "base", "terminal")

# The sixteen terminal colours, by the names the usage texts give them.
ANSI = ("black", "red", "green", "yellow", "blue", "magenta", "cyan", "white")
ANSI_INDEX = {name: i for i, name in enumerate(ANSI)}
ANSI_INDEX.update({"bright " + name: i + 8 for i, name in enumerate(ANSI)})
# "bright black (8)"; "bright blue (12) on dark terminals" where a usage gives
# a dark terminal an index of its own.
NAMED_INDEX = re.compile(r"((?:bright )?(?:%s)) \((\d+)\)( on dark terminals)?" % "|".join(ANSI))


class Missing(Exception):
    """A file, selector, marker or section this check needs is absent (exit 2)."""


class Unreadable(Exception):
    """A term-* token gives no value the TUI's theme table can be made from (exit 1)."""


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


def sixteen(terminal):
    """{token: (dark, light)}: each term-* token's 16-colour index on a dark
    and on a light terminal, read from its usage text; "" is no colour."""
    found, same = {}, {}
    for name, _, usage in terminal:
        _, marker, text = usage.partition("16-colour:")
        if marker:
            found[name] = fallback(name, text.strip())
            continue
        m = re.match(r"Same as ([a-z0-9-]+)", usage)
        if not m:
            raise Unreadable(f"{name}: its usage names no 16-colour value: {usage!r}")
        same[name] = "term-" + m.group(1)
    for name, other in same.items():
        if other not in found:
            raise Unreadable(f"{name}: its usage says it is the same as {other}, "
                             "which names no 16-colour value of its own")
        found[name] = found[other]
    return found


def fallback(name, text):
    """(dark, light) from the text after "16-colour:" in name's usage."""
    named = NAMED_INDEX.findall(text)
    if not named:
        if text.startswith(("default background", "reverse video")):
            return "", ""
        raise Unreadable(f"{name}: its usage names no 16-colour value in {text!r} (an index "
                         "beside its colour's name, default background, or reverse video)")
    for colour, index, _ in named:
        if ANSI_INDEX[colour] != int(index):
            raise Unreadable(f"{name}: its usage gives {colour} as {index}, but {colour} "
                             f"is {ANSI_INDEX[colour]}")
    plain = [index for _, index, dark in named if not dark]
    marked = [index for _, index, dark in named if dark]
    if len(plain) != 1 or len(marked) > 1:
        raise Unreadable(f"{name}: {text!r} does not give one 16-colour index for each background")
    return (marked or plain)[0], plain[0]


def theme_table(tok):
    """The TUI's theme table: (name, usage, dark, light) for each term-*
    token, dark and light each a (truecolor, 256-colour, 16-colour) triple
    in which "" is never painted."""
    dark_hex, light_hex = dict(tok["dark"]), dict(tok["light"])
    indexes = sixteen(tok["terminal"])
    rows = []
    for name, value, usage in tok["terminal"]:
        if not name.startswith("term-"):
            raise Unreadable(f"{name}: a terminal token's name starts with term-")
        role = name[len("term-"):]
        if "--" + role not in dark_hex:
            raise Unreadable(f"{name}: tokens.json has no colour named {role} to take its hex from")
        dark16, light16 = indexes[name]
        # Below truecolor a light terminal keeps its own foreground for text
        # and takes the 16-colour index for every other role, at both depths.
        light_index = "" if role == "ink" or role.startswith("ink-") else light16
        rows.append((name, usage,
                     (dark_hex["--" + role], value, dark16),
                     (light_hex["--" + role], light_index, light_index)))
    return rows


def go_name(token):
    """term-panel-raised -> termPanelRaised, the Go name of a token's entry."""
    first, *rest = token.split("-")
    return first + "".join(part.capitalize() for part in rest)


def go_source(tok):
    """tokens_gen.go: the theme table as Go, laid out as gofmt leaves it."""
    def colour(triple):
        # A JSON string is a Go string literal for these values.
        return "lipgloss.CompleteColor{TrueColor: %s, ANSI256: %s, ANSI: %s}" % tuple(
            json.dumps(v) for v in triple)

    rows = theme_table(tok)
    out = [
        "// Code generated by scripts/checks/design-tokens.py. DO NOT EDIT.",
        "",
        "package theme",
        "",
        'import "github.com/charmbracelet/lipgloss"',
        "",
        "// The term-* tokens of docs/design/system/tokens.json, in its order: each",
        "// one's colour on a dark and on a light terminal at truecolor, 256 and 16",
        "// colours. An empty value is never painted. Edit tokens.json, not this",
        "// file, and run scripts/checks/design-tokens.py --write.",
        "var (",
    ]
    for name, usage, dark, light in rows:
        out += [
            f"\t// {name}: {' '.join(usage.split())}",
            f"\t{go_name(name)} = lipgloss.CompleteAdaptiveColor{{",
            f"\t\tDark:  {colour(dark)},",
            f"\t\tLight: {colour(light)},",
            "\t}",
        ]
    out += [
        ")",
        "",
        "// termTokens is every entry above by its token's name, for the tests that",
        "// read tokens.json.",
        "var termTokens = map[string]lipgloss.CompleteAdaptiveColor{",
    ]
    # gofmt aligns the values of a literal's one-line entries one space past
    # the longest key.
    width = max(len(json.dumps(name)) + 1 for name, *_ in rows)
    out += [f"\t{(json.dumps(name) + ':').ljust(width)} {go_name(name)}," for name, *_ in rows]
    out += ["}", ""]
    return "\n".join(out)


def css_block(tok):
    """style.css's token block, markers included, as --write writes it."""
    def rule(selector, pairs, indent="  "):
        # CSS escapes the dot in --space-0.5.
        lines = [f"{indent}{selector} {{"]
        for n, v in pairs:
            lines.append(indent + "  " + n.replace(".", "\\.") + ": " + v + ";")
        return lines + [f"{indent}}}"]

    out = [
        CSS_START,
        "   Generated by scripts/checks/design-tokens.py --write from",
        "   docs/design/system/tokens.json: do not edit between these markers.",
        "   Dark is the default theme; Daylight is the light one, which the",
        "   system's setting chooses (the page has no toggle) or data-theme",
        "   names. */",
    ]
    out += rule(":root,\n[data-theme=dark]", tok["dark"], indent="")
    out += [""] + rule("[data-theme=light]", tok["light"], indent="")
    out += ["", "@media (prefers-color-scheme: light) {"]
    out += rule(":root:not([data-theme=dark])", tok["light"])
    out += ["}", ""] + rule(":root", tok["base"], indent="")
    out += [CSS_END]
    return "\n".join(out) + "\n"


def css_span(style):
    """(start, end) of style.css's token block, markers included."""
    start = style.find("\n" + CSS_START)
    end = style.find("\n" + CSS_END + "\n")
    if start < 0 or end < start:
        raise Missing(f"no {CSS_START} ... {CSS_END} block in style.css")
    return start + 1, end + 1 + len(CSS_END) + 1


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


def check(source_path, wireframe_path, design_path, theme_path, style_path, out=sys.stdout):
    """True when the wireframe, DESIGN.md, tokens_gen.go and style.css's
    token block all agree with tokens.json."""
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
    try:
        want = go_source(tok)
    except Unreadable as e:
        problems.append(f"the TUI's theme table: {e}")
    else:
        have = read(theme_path)
        if have != want:
            problems.append("tui/internal/theme/tokens_gen.go differs from tokens.json "
                            "(scripts/checks/design-tokens.py --write regenerates it)")
            out.writelines(difflib.unified_diff(
                have.splitlines(True), want.splitlines(True), "tokens_gen.go", "tokens.json"))
            out.write("\n")
    style = read(style_path)
    start, end = css_span(style)
    want = css_block(tok)
    if style[start:end] != want:
        problems.append("desktop/ui/style.css's token block differs from tokens.json "
                        "(scripts/checks/design-tokens.py --write regenerates it)")
        out.writelines(difflib.unified_diff(
            style[start:end].splitlines(True), want.splitlines(True), "style.css", "tokens.json"))
        out.write("\n")
    for p in problems:
        out.write(f"FAIL: {p}\n")
    return not problems


def write(source_path, design_path, theme_path, style_path):
    tok = load(source_path)
    # Every copy is made before one is written, so a usage the theme table
    # cannot read (Unreadable) leaves every file as it was.
    go = go_source(tok)
    doc = read(design_path)
    for name, table in tables(tok).items():
        m = section(doc, name)
        doc = doc[: m.start(1)] + table + "\n" + doc[m.end(1):]
    style = read(style_path)
    start, end = css_span(style)
    style = style[:start] + css_block(tok) + style[end:]
    for path, body in ((design_path, doc), (theme_path, go), (style_path, style)):
        with open(path, "w", encoding="utf-8", newline="\n") as f:
            f.write(body)
        print(f"wrote {path}")


def plant(paths, bodies):
    for path, body in zip(paths, bodies):
        with open(path, "w", encoding="utf-8", newline="\n") as f:
            f.write(body)


def self_test():
    """Plant each kind of drift in a copy of the five files; each must fail."""
    with open(os.devnull, "w") as quiet:
        if not check(SOURCE, WIREFRAME, DESIGN, THEME, STYLE, out=quiet):
            print("FAIL: self-test needs the committed files to agree first")
            return False
    source, html, doc, go, css = read(SOURCE), read(WIREFRAME), read(DESIGN), read(THEME), read(STYLE)
    drifted = source.replace('"#0b0b0d"', '"#000001"', 1)
    no_index = source.replace("16-colour: bright white (15)", "16-colour: the brightest", 1)
    # Each case: its label, the four files, and what the failure must say
    # where another check failing too could hide it.
    cases = [
        ("a value drifted in tokens.json", (drifted, html, doc, go, css), None),
        ("a value drifted in the wireframe", (source, html.replace("--ground:#0b0b0d", "--ground:#000001", 1), doc, go, css), None),
        ("a token missing from the wireframe", (source, html.replace("--scrim:#000000b3;", "", 1), doc, go, css), None),
        ("a token only the wireframe has", (source, html.replace("--scrim:", "--extra:#ffffff;--scrim:", 1), doc, go, css), None),
        ("a DESIGN.md row edited by hand", (source, html, doc.replace("| `--ground` | `#0b0b0d`", "| `--ground` | `#000001`", 1), go, css), None),
        ("a stale tokens_gen.go", (source, html, doc, go.replace('ANSI256: "214"', 'ANSI256: "220"', 1), css),
         "tokens_gen.go differs"),
        ("a usage that names no 16-colour index", (no_index, html, doc, go, css), "names no 16-colour value"),
        ("a usage that gives a colour the wrong index",
         (source.replace("16-colour: yellow (3)", "16-colour: yellow (4)", 1), html, doc, go, css), "but yellow is 3"),
        ("a stale style.css token block",
         (source, html, doc, go, css.replace("  --ground: #0b0b0d;", "  --ground: #000001;", 1)),
         "style.css's token block differs"),
        ("a style.css token edited outside tokens.json",
         (source, html, doc, go, css.replace("  --ink: #ececf1;", "  --ink: #ececf1;\n  --extra: #ffffff;", 1)),
         "style.css's token block differs"),
    ]
    with tempfile.TemporaryDirectory() as d:
        paths = [os.path.join(d, n)
                 for n in ("tokens.json", "wireframe.html", "DESIGN.md", "tokens_gen.go", "style.css")]
        for label, files, says in cases:
            if files == (source, html, doc, go, css):
                print(f"FAIL: self-test could not plant '{label}'")
                return False
            plant(paths, files)
            said = io.StringIO()
            if check(*paths, out=said) or (says and says not in said.getvalue()):
                print(f"FAIL: self-test: '{label}' was not caught")
                return False
        # --write repairs DESIGN.md, tokens_gen.go and style.css's block
        # after tokens.json moves, and touches nothing else: the wireframe
        # still fails until a person edits it, and style.css outside its
        # markers is kept as it was.
        plant(paths, (drifted, html, doc, go, css))
        with open(os.devnull, "w") as quiet:
            stdout, sys.stdout = sys.stdout, quiet
            try:
                write(paths[0], paths[2], paths[3], paths[4])
            finally:
                sys.stdout = stdout
            tok = load(paths[0])
            regenerated = read(paths[2])
            if any(section(regenerated, n).group(1) != t + "\n" for n, t in tables(tok).items()):
                print("FAIL: self-test: --write did not regenerate the tables")
                return False
            if read(paths[3]) != go_source(tok):
                print("FAIL: self-test: --write did not regenerate tokens_gen.go")
                return False
            written = read(paths[4])
            start, end = css_span(written)
            if written[start:end] != css_block(tok):
                print("FAIL: self-test: --write did not regenerate style.css's token block")
                return False
            if written[:start] + written[end:] != css[:css_span(css)[0]] + css[css_span(css)[1]:]:
                print("FAIL: self-test: --write changed style.css outside its token block")
                return False
            if check(*paths, out=quiet):
                print("FAIL: self-test: the drifted wireframe passed after --write")
                return False
        # A usage the theme table cannot read stops --write before it writes
        # either file.
        plant(paths, (no_index, html, doc, go, css))
        try:
            write(paths[0], paths[2], paths[3], paths[4])
        except Unreadable:
            pass
        else:
            print("FAIL: self-test: --write wrote past a usage it cannot read")
            return False
        if read(paths[2]) != doc or read(paths[3]) != go or read(paths[4]) != css:
            print("FAIL: self-test: --write changed a file before refusing")
            return False
    print("self-test: ok")
    return True


def main(argv):
    try:
        if argv == ["--self-test"]:
            return 0 if self_test() else 1
        if argv == ["--write"]:
            write(SOURCE, DESIGN, THEME, STYLE)
            return 0
        if argv:
            print(__doc__)
            return 2
        return 0 if check(SOURCE, WIREFRAME, DESIGN, THEME, STYLE) else 1
    except Missing as e:
        print(f"FAIL: {e}")
        return 2
    except Unreadable as e:
        print(f"FAIL: {e}")
        return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
