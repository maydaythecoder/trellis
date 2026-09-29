"""S7 metric 3: the per-call argument audit (ADR 0028).

For each task in a floor run, counts the tool calls the model made, the calls
whose `path` names a file that exists in the task's repository, and the `edit`
calls whose `old` anchor occurs in that file as it stood when the call was
made. The counts are printed per task and never folded into a rate: a task
with forty calls would otherwise outweigh one with three (ADR 0028).

These are not argument correctness in the gold-trace sense. A failing call is
a lower bound on argument error; a passing call is an upper bound on argument
correctness, since an anchor that exists can still be the wrong line.

**Where the calls come from.** The per-task `.json` trails under
`docs/bench/floor-*/` carry no calls (`factual.proposed` is empty in every one
of them). The calls are in the per-task `tNN.log`, which is `trellis run`'s
stdout: one numbered line per executed turn, printed as the call's Rust
`{:?}`, untruncated. That is what this script reads.

**What is excluded.** A call the grammar or the adapter rejected never became
a turn: it shows up only as a `malformed tool call; retrying` warning, or as
the run ending on `no valid tool call`, and gets no numbered line. So format
failures (§5.2's column) are excluded by construction, not by a filter here.

**The task's repository**, as `run_floor.py` builds it: the pinned corpus
(poetry 2.5.0's sdist), plus the `.gitignore` and `.trellis/config.toml` the
harness writes, with the task's `defect_old -> defect_new` applied once to its
file. The corpus is not committed, so it is an argument; without it every
path and anchor check is `unknown`, never absent.

**"At that point".** A file listed on the log's `files written:` line changed
during the run, and the log does not say which edit wrote it, so every anchor
check on that file after the first `edit` to it is `unknown`. A file not
listed was never written, so its content is the same at every step. `edit`
never creates a file (it refuses a missing path), so in the committed runs,
which had no other writing tool, path existence does not change. Later
binaries have more: after a `WriteFile`, that path's existence and content
are `unknown`; after a `Bash` or `Custom` call, or a turn this script cannot
parse, every later check is `unknown`, since any of them can change the tree
without naming what they changed.

Matching rules, each mirroring what the tools did:
  - anchors are matched on LF-normalised text, file and anchor both, as the
    edit tool matches; presence means at least one occurrence;
  - a path is relative to the repository root; `/` and `\\` both separate
    (the runs were on Windows); an absolute path or a `..` component is not in
    the repository; case must match exactly;
  - a path must name a file, except for `Ls` and `Grep`, which take a
    directory too, and `WriteFile`, whose path exists when the file or its
    parent directory does (a new file in a real directory refers to something
    real).

Only a singular `path` argument is audited (`Grep`'s `None` is no path).
`run_checks`' `paths` list is not counted as a path argument (it was empty in
every committed run).

Usage:
  py -3 docs/bench/metric3_audit.py <trail-dir> --tasks <floor_tasks.json>
                                    [--corpus <dir | sdist .tar.gz>]
  py -3 docs/bench/metric3_audit.py --self-test
"""

from __future__ import annotations

import argparse
import hashlib
import json
import re
import sys
import tarfile
from dataclasses import dataclass, field
from pathlib import Path

HERE = Path(__file__).resolve().parent
SELFTEST = HERE / "metric3-selftest"

# Files `run_floor.py` adds to the corpus before the first task.
HARNESS_FILES = (".gitignore", ".trellis/config.toml")

TURN = re.compile(r"^\s*\d+\.\s+(\w+)(.*)$")
ITERATIONS = re.compile(r"(?:NOT )?CONVERGED after (\d+) iteration")


# --- the corpus --------------------------------------------------------------


class Corpus:
    """The pinned tree, read from a directory or straight from the sdist.

    Reading the tarball directly means the audit can run on the exact bytes
    whose hash is pinned, with nothing unpacked and nothing to drift.
    """

    def __init__(self, source: Path):
        self.source = source
        self.sha256: str | None = None
        self._files: dict[str, bytes | Path] = {}
        if source.is_dir():
            for p in source.rglob("*"):
                if p.is_file():
                    self._files[p.relative_to(source).as_posix()] = p
        else:
            self.sha256 = hashlib.sha256(source.read_bytes()).hexdigest()
            with tarfile.open(source) as tar:
                for m in tar.getmembers():
                    if not m.isfile():
                        continue
                    # An sdist unpacks under one top-level `name-version/`.
                    rel = m.name.split("/", 1)[1] if "/" in m.name else m.name
                    f = tar.extractfile(m)
                    if f is not None:
                        self._files[rel] = f.read()
        for h in HARNESS_FILES:
            self._files.setdefault(h, b"")
        self._dirs = {""} | {
            "/".join(parts[:i])
            for rel in self._files
            for parts in [rel.split("/")]
            for i in range(1, len(parts))
        }

    def has_file(self, rel: str) -> bool:
        return rel in self._files

    def has_dir(self, rel: str) -> bool:
        return rel in self._dirs

    def text(self, rel: str) -> str:
        v = self._files[rel]
        raw = v.read_bytes() if isinstance(v, Path) else v
        # Normalised by hand rather than through text-mode reads, so a
        # directory and a tarball give the same string.
        return raw.decode("utf-8", "replace").replace("\r\n", "\n")


def normalise(path: str) -> str | None:
    """The repository-relative form of a model-supplied path, or None when it
    cannot name a file inside the repository."""
    if not path or path.startswith(("/", "\\")) or re.match(r"^[A-Za-z]:", path):
        return None
    parts = [p for p in re.split(r"[/\\]", path) if p not in ("", ".")]
    if ".." in parts:
        return None
    # "" is the root itself, which `Ls` and `Grep` may name as ".".
    return "/".join(parts)


# --- parsing a turn line -----------------------------------------------------


def unescape(body: str) -> str:
    """Undoes Rust's `{:?}` escaping of a string literal's body."""
    out, i = [], 0
    simple = {"n": "\n", "t": "\t", "r": "\r", "0": "\0",
              "\\": "\\", '"': '"', "'": "'"}
    while i < len(body):
        c = body[i]
        if c != "\\":
            out.append(c)
            i += 1
            continue
        nxt = body[i + 1]
        if nxt in simple:
            out.append(simple[nxt])
            i += 2
        elif nxt == "u":
            end = body.index("}", i)
            out.append(chr(int(body[i + 3:end], 16)))
            i = end + 1
        else:
            raise ValueError(f"unknown escape \\{nxt} in {body!r}")
    return "".join(out)


def skip_braces(s: str, i: int) -> int:
    """The index after the `{...}` opening at `s[i]`, skipping string literals,
    so a `}` inside one does not close it."""
    depth, j = 0, i
    while True:
        if s[j] == '"':
            j += 1
            while s[j] != '"':
                j += 2 if s[j] == "\\" else 1
        elif s[j] == "{":
            depth += 1
        elif s[j] == "}":
            depth -= 1
            if depth == 0:
                return j + 1
        j += 1


class Opaque:
    """A value this audit does not read, such as `Custom`'s JSON arguments."""


def parse_value(s: str, i: int):
    """Parses one `{:?}` field value at `s[i]`: a string literal, a list,
    `Some(v)` / `None`, a bare identifier, or a struct or tuple value, which
    is skipped as `Opaque`. Returns (value, index after it)."""
    if s[i] == '"':
        j = i + 1
        while s[j] != '"':
            j += 2 if s[j] == "\\" else 1
        return unescape(s[i + 1:j]), j + 1
    if s.startswith("None", i) and not re.match(r"\w", s[i + 4:i + 5]):
        return None, i + 4
    if s.startswith("Some(", i):
        v, j = parse_value(s, i + 5)
        if s[j] != ")":
            raise ValueError(f"unclosed Some( at {s[i:i + 20]!r}")
        return v, j + 1
    if s[i] == "[":
        items, j = [], i + 1
        while s[j] == " ":
            j += 1
        while s[j] != "]":
            v, j = parse_value(s, j)
            items.append(v)
            while s[j] in ", ":
                j += 1
        return items, j + 1
    m = re.compile(r"[\w:.-]+").match(s, i)
    if not m:
        raise ValueError(f"cannot parse a value at {s[i:i + 20]!r}")
    j = m.end()
    if s.startswith(" {", j):
        return Opaque(), skip_braces(s, j + 1)
    if s.startswith("(", j):
        j += 1
        while True:
            while s[j] in ", ":
                j += 1
            if s[j] == ")":
                return Opaque(), j + 1
            _, j = parse_value(s, j)
    return m.group(0), j


def parse_call(line: str) -> tuple[str, dict] | None:
    """`   3. Edit { path: "a.py", old: "x", new: "y" } check FAILED` ->
    ("Edit", {"path": "a.py", ...}). None for a line that is not a turn."""
    m = TURN.match(line)
    if not m:
        return None
    name, rest = m.group(1), m.group(2)
    args: dict = {}
    s = rest.lstrip()
    if s.startswith("{"):
        i = 1
        while True:
            while s[i] in ", ":
                i += 1
            if s[i] == "}":
                break
            k = re.compile(r"\w+").match(s, i)
            key = k.group(0)
            i = k.end()
            assert s[i:i + 2] == ": ", f"expected ': ' after {key} in {line!r}"
            args[key], i = parse_value(s, i + 2)
    return name, args


# --- the audit ---------------------------------------------------------------


@dataclass
class Row:
    task: str
    calls: int = 0
    with_path: int = 0
    path_exists: int = 0
    path_unknown: int = 0
    edits: int = 0
    anchor_present: int = 0
    anchor_unknown: int = 0
    notes: list[str] = field(default_factory=list)

    def counts(self) -> tuple[int, ...]:
        return (self.calls, self.with_path, self.path_exists, self.path_unknown,
                self.edits, self.anchor_present, self.anchor_unknown)


def files_written(log: str) -> set[str] | None:
    """The `files written:` line as a set; None when the line is missing, in
    which case nothing about the file's content after an edit is known."""
    for line in log.splitlines():
        if line.startswith("files written:"):
            rest = line[len("files written:"):].strip()
            if rest in ("", "none"):
                return set()
            return {p for p in (normalise(x.strip()) for x in rest.split(",")) if p}
    return None


def audit_log(task_id: str, log: str, task: dict | None, corpus: Corpus | None) -> Row:
    row = Row(task_id)
    written = files_written(log)
    if written is None:
        row.notes.append("no `files written:` line")
    edited: set[str] = set()
    # Paths a `WriteFile` may have created or replaced, and whether a call
    # that can change any file without naming it has run.
    rewritten: set[str] = set()
    tree_unknown = False

    for line in log.splitlines():
        # The turn list ends where the run's summary begins; a checker's
        # output quoted below it could otherwise read as a numbered turn.
        if line.startswith(("files written:", "why the check failed:")):
            break
        if not TURN.match(line):
            continue
        row.calls += 1
        try:
            name, args = parse_call(line)
        except (ValueError, IndexError, AssertionError, AttributeError):
            # Counted as a call, audited as nothing; and since it may have
            # been one that writes, nothing after it is known either.
            row.notes.append(f"turn {row.calls} not parsed")
            tree_unknown = True
            continue
        if name in ("Bash", "Custom"):
            tree_unknown = True
        path = args.get("path")
        if not isinstance(path, str):
            continue
        row.with_path += 1
        rel = normalise(path)
        is_edit = name == "Edit"
        if is_edit:
            row.edits += 1

        if corpus is None or tree_unknown or rel in rewritten:
            row.path_unknown += 1
            if is_edit:
                row.anchor_unknown += 1
            continue
        if rel is None:
            exists = False
        elif name in ("Ls", "Grep"):
            exists = corpus.has_file(rel) or corpus.has_dir(rel)
        elif name == "WriteFile":
            parent = rel.rsplit("/", 1)[0] if "/" in rel else ""
            exists = bool(rel) and (corpus.has_file(rel) or corpus.has_dir(parent))
            rewritten.add(rel)
        else:
            exists = corpus.has_file(rel)
        if exists:
            row.path_exists += 1
        if not is_edit:
            continue
        if not exists:
            # No file, so no anchor in it: absent, not unknown.
            continue
        if rel in edited and (written is None or rel in written):
            row.anchor_unknown += 1
        else:
            src = corpus.text(rel)
            if task is not None and normalise(task["file"]) == rel:
                src = src.replace(task["defect_old"], task["defect_new"], 1)
            if args["old"].replace("\r\n", "\n") in src:
                row.anchor_present += 1
        edited.add(rel)

    m = ITERATIONS.search(log)
    if m and int(m.group(1)) != row.calls:
        row.notes.append(f"{row.calls} turn lines, run reports {m.group(1)} iterations")
    if task is None:
        row.notes.append("task not in the task set; defect not applied")
    return row


def audit_dir(trails: Path, tasks: dict, corpus: Corpus | None) -> list[Row]:
    by_id = {t["id"]: t for t in tasks["tasks"]}
    rows = []
    for log in sorted(trails.glob("t*.log")):
        tid = log.stem
        rows.append(audit_log(tid, log.read_text(encoding="utf-8", errors="replace"),
                              by_id.get(tid), corpus))
    return rows


HEADER = ("task", "calls", "with_path", "path_exists", "path_unknown",
          "edits", "anchor_present", "anchor_unknown")


def render(rows: list[Row]) -> str:
    lines = ["\t".join(HEADER) + "\tnotes"]
    for r in rows:
        lines.append("\t".join([r.task, *map(str, r.counts())]) + "\t" + "; ".join(r.notes))
    return "\n".join(lines)


# --- self-test ---------------------------------------------------------------

# Hand-counted from `metric3-selftest/trails/`; see the comments in each log.
EXPECTED_WITH_CORPUS = {
    #        calls path exist unk edits present unk
    "t00": (7, 6, 4, 0, 5, 2, 0),
    "t01": (13, 9, 6, 2, 5, 2, 2),
}
EXPECTED_WITHOUT_CORPUS = {
    "t00": (7, 6, 0, 6, 5, 0, 5),
    "t01": (13, 9, 0, 9, 5, 0, 5),
}


def self_test() -> int:
    tasks = json.loads((SELFTEST / "tasks.json").read_text(encoding="utf-8"))
    trails = SELFTEST / "trails"
    failed = 0
    for label, corpus, expected in (
        ("with corpus", Corpus(SELFTEST / "corpus"), EXPECTED_WITH_CORPUS),
        ("without corpus", None, EXPECTED_WITHOUT_CORPUS),
    ):
        rows = audit_dir(trails, tasks, corpus)
        got = {r.task: r.counts() for r in rows}
        print(f"-- {label}\n{render(rows)}")
        if got != expected:
            print(f"FAIL ({label}): expected {expected}, got {got}")
            failed = 1
    # The escapes the committed logs actually contain, including a tab that
    # reads like a Windows separator.
    probe = r'   1. Edit { path: "src\tests\u{11}x\0.py", old: "a\"b\\c", new: "\n" } '
    name, args = parse_call(probe)
    if (name, args) != ("Edit", {"path": "src\tests\x11x\x00.py",
                                 "old": 'a"b\\c', "new": "\n"}):
        print(f"FAIL: parse_call gave {name} {args}")
        failed = 1
    # A turn in a shape this parser does not know is counted, audited as
    # nothing, and makes what follows unknown rather than crashing the run.
    odd = ('   1. Edit { path: "pkg/a.py", old: <x>, new: "" } \n'
           '   2. ReadFile { path: "pkg/a.py" } \n')
    r = audit_log("odd", odd, None, Corpus(SELFTEST / "corpus"))
    if r.counts() != (2, 1, 0, 1, 0, 0, 0) or "turn 1 not parsed" not in r.notes:
        print(f"FAIL: an unparsed turn gave {r.counts()} {r.notes}")
        failed = 1
    print("OK: metric 3 audit self-test" if not failed else "FAIL: metric 3 audit self-test")
    return failed


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("trails", nargs="?")
    ap.add_argument("--tasks")
    ap.add_argument("--corpus")
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        return self_test()
    if not a.trails or not a.tasks:
        ap.error("a trail directory and --tasks are required")

    corpus = Corpus(Path(a.corpus)) if a.corpus else None
    tasks = json.loads(Path(a.tasks).read_text(encoding="utf-8"))
    if corpus is None:
        print("corpus: none given; every path and anchor check is unknown")
    elif corpus.sha256:
        print(f"corpus: {corpus.source.name} sha256 {corpus.sha256}")
    else:
        print(f"corpus: directory {corpus.source} (hash not verified)")
    print(render(audit_dir(Path(a.trails), tasks, corpus)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
