"""Builds the S7 model-floor task set, and verifies every task before it counts.

A floor task is a real defect injected into the benchmark repository, chosen so
that an executed check *catches it*. The model's job is to read the check
output and repair the file. That is the check-recovery rate — the primary
metric, because the whole architecture rests on the model acting sensibly on
check output.

**Every task is verified, not assumed.** A mutation only becomes a task when
the checker is clean before it and fails after it. An unverified task set would
measure the checker's blind spots and call the result a model floor.

Usage:
    python docs/bench/make_floor_tasks.py <repo-root> <out.json> [--limit N]

Writes a JSON task set. Nothing here runs a model; this produces the fixtures
the probe rig consumes.
"""

from __future__ import annotations

import argparse
import json
import re
import subprocess
import sys
import time
from dataclasses import dataclass, asdict
from pathlib import Path

# mypy runs against poetry's own source without poetry's dependencies
# installed. `--ignore-missing-imports` is what makes that possible and is the
# reason this is runnable offline; it costs some checks that depend on
# third-party stubs, which is acceptable because the defects below are all
# first-party and local.
MYPY_ARGS = [
    "--ignore-missing-imports",
    "--no-error-summary",
    "--no-color-output",
    "--hide-error-context",
]


@dataclass
class Task:
    id: str
    file: str
    """The exact text replaced to inject the defect."""
    defect_old: str
    defect_new: str
    kind: str
    """What a correct repair restores. The probe grades against behaviour, not
    against this string — a different correct fix is still correct — but it is
    recorded so a grader can tell a repair from a revert-by-luck."""
    gold_note: str
    detected_by: str
    """The checker's own message, so a task that stops being detected after a
    dependency bump is visible rather than silently inert."""
    evidence: str


class Mutation:
    """A realistic defect an agent might be asked to repair.

    Each is a class of mistake a person actually makes, not a random character
    swap: a wrong argument count, a wrong return type, a call to something that
    does not exist. Random mutation would measure the model's tolerance for
    nonsense; these measure its ability to act on a type error.
    """

    def __init__(self, kind: str, pattern: str, replace, note: str):
        self.kind = kind
        self.pattern = re.compile(pattern, re.MULTILINE)
        self.replace = replace
        self.note = note

    def find(self, src: str):
        for m in self.pattern.finditer(src):
            new = self.replace(m)
            if new is not None and new != m.group(0):
                yield m.group(0), new


MUTATIONS = [
    Mutation(
        "return-type-annotation",
        r"^(\s*def\s+\w+\([^)]*\))\s*->\s*(bool|int)\s*:",
        lambda m: f"{m.group(1)} -> str:",
        "return annotation contradicts what the body returns",
    ),
    Mutation(
        "undefined-method",
        r"\bself\.(_?[a-z_][a-z0-9_]*)\(\)",
        lambda m: f"self.{m.group(1)}_typo()",
        "calls a method that does not exist on the class",
    ),
    Mutation(
        "wrong-argument-type",
        r"^(\s*def\s+\w+\(self,\s*\w+)\s*:\s*str",
        lambda m: f"{m.group(1)}: int",
        "parameter annotation contradicts its callers",
    ),
    Mutation(
        "dropped-optional-guard",
        r"^(\s*)if\s+(\w+)\s+is\s+not\s+None\s*:",
        lambda m: f"{m.group(1)}if True:",
        "removes a None guard the type checker relies on",
    ),
]


def run_mypy(root: Path, target: Path) -> tuple[bool, str]:
    """Returns (clean, output). A crash counts as not clean and is reported."""
    try:
        p = subprocess.run(
            [sys.executable, "-m", "mypy", *MYPY_ARGS, str(target)],
            cwd=root,
            capture_output=True,
            text=True,
            timeout=300,
        )
    except subprocess.TimeoutExpired:
        return False, "mypy timed out"
    out = (p.stdout or "") + (p.stderr or "")
    return p.returncode == 0, out.strip()


def first_error_line(output: str) -> str:
    for line in output.splitlines():
        if ": error:" in line:
            return line.strip()
    return output.splitlines()[0].strip() if output.strip() else ""


def build(root: Path, limit: int) -> list[Task]:
    tasks: list[Task] = []
    # Only src/: the tests directory is poetry's own, and injecting defects
    # there would measure the model against test code rather than library code.
    files = sorted((root / "src").rglob("*.py"))
    print(f"scanning {len(files)} files under src/", flush=True)

    for path in files:
        if len(tasks) >= limit:
            break
        rel = path.relative_to(root).as_posix()
        original = path.read_text(encoding="utf-8")

        # The file must be clean first, or "the check fails" proves nothing.
        clean, before = run_mypy(root, path)
        if not clean:
            continue

        for mut in MUTATIONS:
            if len(tasks) >= limit:
                break
            for old, new in mut.find(original):
                mutated = original.replace(old, new, 1)
                path.write_text(mutated, encoding="utf-8")
                caught, after = run_mypy(root, path)
                path.write_text(original, encoding="utf-8")

                if caught:
                    # The checker did not notice. Not a task: it would measure
                    # the checker's blind spot, not the model.
                    continue

                tasks.append(
                    Task(
                        id=f"t{len(tasks):02d}",
                        file=rel,
                        defect_old=old,
                        defect_new=new,
                        kind=mut.kind,
                        gold_note=mut.note,
                        detected_by="mypy",
                        evidence=first_error_line(after),
                    )
                )
                print(f"  [{len(tasks):2}] {mut.kind:24} {rel}", flush=True)
                break  # one defect per file keeps tasks independent

    return tasks


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("root")
    ap.add_argument("out")
    ap.add_argument("--limit", type=int, default=20)
    a = ap.parse_args()

    root = Path(a.root).resolve()
    started = time.time()
    tasks = build(root, a.limit)

    if not tasks:
        print(
            "No verified tasks. Either mypy is unavailable or nothing it catches "
            "was injectable — do not write an empty task set and call S7 measured.",
            file=sys.stderr,
        )
        return 1

    Path(a.out).write_text(
        json.dumps(
            {
                "corpus": root.name,
                "checker": "mypy " + " ".join(MYPY_ARGS),
                "built_s": round(time.time() - started, 1),
                "count": len(tasks),
                "protocol": (
                    "Apply defect_old -> defect_new to file, then ask the agent to "
                    "make the checks pass. Every task is verified: the checker was "
                    "clean before the mutation and failed after it. Grade on "
                    "check-recovery (did failures fall), not on matching gold_note."
                ),
                "tasks": [asdict(t) for t in tasks],
            },
            indent=1,
        ),
        encoding="utf-8",
    )
    print(f"wrote {len(tasks)} verified tasks to {a.out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
