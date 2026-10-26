#!/usr/bin/env python3
"""design-events.py: docs/design/events.md has a row for every record the contract lists.

    scripts/checks/design-events.py               check the rows
    scripts/checks/design-events.py --self-test   prove each check bites

crates/trellis-cli/tests/client_contract.json lists what `trellis rpc` sends
a client. docs/design/events.md says, one row each, the mark each of those
makes in the desktop and in the TUI (#804). This holds the page to the
contract's names:

- every framing record (rpc.framing and rpc.not_every_client.framing) has a
  `frame` row, every event (rpc.events and rpc.not_every_client.events) an
  `event` row, rpc.not_every_client.event_line a `line` row, and every
  command answer (rpc.responses, and framing.response's no_data commands)
  an `answer` row;
- no row names something the contract does not list, so a rename leaves no
  stale row behind;
- no name has two rows, and no row leaves the desktop or the TUI cell empty.

A row is a markdown table row `| <kind> | `<name>` | <desktop> | <tui> |`.
Whether a mark is right is not checked here: the row cites the code that
makes it.

Exit 0: every name has one row. Exit 1: a row is missing, extra, doubled or
empty, each one printed; or the self-test failed. Exit 2: a file is missing,
the contract is not JSON, or it lacks a section this check reads.
"""
import io
import json
import os
import re
import sys
import tempfile

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
CONTRACT = os.path.join(ROOT, "crates", "trellis-cli", "tests", "client_contract.json")
PAGE = os.path.join(ROOT, "docs", "design", "events.md")

KINDS = ("frame", "event", "line", "answer")
# | kind | `name` | desktop | tui |, the cells kept whole for the empty test.
# Cells are split on "|", so a cell may not hold one, escaped or not.
ROW = re.compile(r"^\|\s*(%s)\s*\|\s*`([^`]+)`\s*\|(.*)\|\s*$" % "|".join(KINDS))


class Missing(Exception):
    """A file or a contract section this check reads is absent (exit 2)."""


def read(path):
    try:
        with open(path, encoding="utf-8") as f:
            return f.read()
    except OSError as e:
        raise Missing(f"cannot read {path}: {e}") from e


def listed(contract_path):
    """{(kind, name)}: every record the contract lists."""
    try:
        rpc = json.loads(read(contract_path))["rpc"]
        extra = rpc["not_every_client"]
        names = {("frame", n) for n in list(rpc["framing"]) + list(extra["framing"])}
        names |= {("event", n) for n in list(rpc["events"]) + list(extra["events"])}
        if "event_line" in extra:
            names.add(("line", "event_line"))
        answers = list(rpc["responses"]) + list(rpc["framing"]["response"]["no_data"])
        names |= {("answer", n) for n in answers}
    except json.JSONDecodeError as e:
        raise Missing(f"{contract_path} is not JSON: {e}") from e
    except (KeyError, TypeError) as e:
        raise Missing(f"{contract_path} lacks a section this check reads: {e}") from e
    return names


def rows(page):
    """[(kind, name, [cells])] for each row of the page's tables."""
    out = []
    for line in page.splitlines():
        m = ROW.match(line.strip())
        if m:
            out.append((m.group(1), m.group(2), [c.strip() for c in m.group(3).split("|")]))
    return out


def check(contract_path, page_path, out=sys.stdout):
    want = listed(contract_path)
    have = rows(read(page_path))
    problems = []
    seen = set()
    for kind, name, cells in have:
        key = (kind, name)
        if key in seen:
            problems.append(f"{kind} `{name}` has more than one row")
        seen.add(key)
        if key not in want:
            problems.append(f"{kind} `{name}` has a row, and the contract lists no such {kind}")
        if len(cells) != 2 or not all(cells):
            problems.append(f"{kind} `{name}`'s row needs a desktop and a TUI cell, each saying "
                            "its mark or that there is none")
    for kind, name in sorted(want - seen):
        problems.append(f"{kind} `{name}` is in the contract and has no row in events.md")
    for p in problems:
        out.write(f"FAIL: {p}\n")
    return not problems


def self_test():
    """Plant each kind of drift in a copy of the two files; each must fail."""
    with open(os.devnull, "w") as quiet:
        if not check(CONTRACT, PAGE, out=quiet):
            print("FAIL: self-test needs the committed files to agree first")
            return False
    contract, page = read(CONTRACT), read(PAGE)
    lines = page.splitlines(True)

    def without(prefix):
        return "".join(l for l in lines if not l.startswith(prefix))

    def doubled(prefix):
        return "".join(l + l if l.startswith(prefix) else l for l in lines)

    data = json.loads(contract)
    data["rpc"]["events"]["planted event"] = {"always": []}
    new_event = json.dumps(data)
    data = json.loads(contract)
    data["rpc"]["responses"]["planted_command"] = {"always": []}
    new_answer = json.dumps(data)
    data = json.loads(contract)
    data["rpc"]["not_every_client"]["framing"]["planted_frame"] = {"always": []}
    new_frame = json.dumps(data)
    # Each case: its label, the two files, and what the failure must say.
    cases = [
        ("a frame row removed", (contract, without("| frame | `run_end`")), "frame `run_end` is in the contract"),
        ("an event row removed", (contract, without("| event | `run budget`")), "event `run budget` is in the contract"),
        ("a not_every_client event row removed", (contract, without("| event | `compacted` ")),
         "event `compacted` is in the contract"),
        ("the event_line row removed", (contract, without("| line | `event_line`")), "line `event_line`"),
        ("an answer row removed", (contract, without("| answer | `set_budget`")), "answer `set_budget`"),
        ("a no_data answer row removed", (contract, without("| answer | `quit`")), "answer `quit`"),
        ("an event the contract gained", (new_event, page), "event `planted event`"),
        ("a command answer the contract gained", (new_answer, page), "answer `planted_command`"),
        ("a not_every_client frame the contract gained", (new_frame, page), "frame `planted_frame`"),
        ("a row for a name the contract dropped",
         (contract, page + "| event | `tool.call` | x | y |\n"),
         "event `tool.call` has a row"),
        ("a row doubled", (contract, doubled("| answer | `abort`")), "answer `abort` has more than one row"),
        ("an empty TUI cell", (contract, re.sub(r"(\| event \| `paused` \|[^|]*\|)[^|]*\|", r"\1 |", page, count=1)),
         "event `paused`'s row needs"),
    ]
    with tempfile.TemporaryDirectory() as d:
        paths = [os.path.join(d, "client_contract.json"), os.path.join(d, "events.md")]
        for label, files, says in cases:
            if files == (contract, page):
                print(f"FAIL: self-test could not plant '{label}'")
                return False
            for path, body in zip(paths, files):
                with open(path, "w", encoding="utf-8", newline="\n") as f:
                    f.write(body)
            said = io.StringIO()
            if check(*paths, out=said) or says not in said.getvalue():
                print(f"FAIL: self-test: '{label}' was not caught")
                return False
    print("self-test: ok")
    return True


def main(argv):
    try:
        if argv == ["--self-test"]:
            return 0 if self_test() else 1
        if argv:
            print(__doc__)
            return 2
        if not check(CONTRACT, PAGE):
            return 1
        print(f"OK: events.md has one row for each of the {len(listed(CONTRACT))} records the contract lists")
        return 0
    except Missing as e:
        print(f"FAIL: {e}")
        return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
