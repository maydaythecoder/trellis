"""How much recall is sitting in pytest fixtures?

51% of first-party misses cite pytest fixtures (`BENCHMARK_AND_EVAL.md` §2),
which is the evidence pointing at a fixture-aware backstop for the 85% gate.
Before building one, this measures what it could reach — because "51% cite
fixtures" is a statement about the *reason* recorded for a miss, not about how
many of those misses a fixture rule could actually bind.

The rule being sized: in a test function, a parameter whose name matches a
`@pytest.fixture` is bound to that fixture's return value, so a call on it can
be typed from the fixture's return annotation.

Three things have to hold for a site to be reachable, and each one is counted
separately so the shrinkage is visible rather than asserted:

  1. the receiver is a parameter of the enclosing test function
  2. a fixture of that name exists and is unambiguous
  3. that fixture's return type is known and first-party

This is a **magnitude estimate, not a recall figure.** It reads the source with
regexes rather than the resolver, so it will disagree with the resolver at the
margins; it is here to answer "is this worth building" and nothing else. The
figure that counts is a hand-audited sample, per §2.

Usage:  python docs/bench/fixture_opportunity.py <corpus-root>
"""

from __future__ import annotations

import ast
import collections
import sys
from pathlib import Path


def is_fixture(node: ast.FunctionDef | ast.AsyncFunctionDef) -> bool:
    """`@pytest.fixture`, `@fixture`, and the parametrized call forms."""
    for d in node.decorator_list:
        target = d.func if isinstance(d, ast.Call) else d
        name = (
            target.attr
            if isinstance(target, ast.Attribute)
            else target.id
            if isinstance(target, ast.Name)
            else None
        )
        if name == "fixture":
            return True
    return False


def annotation_head(node) -> str | None:
    """`Config` from `Config`, `Config` from `Config | None`, None otherwise.

    Deliberately the same normalisation the resolver applies: a subscripted
    `list[Env]` types `list`, not `Env`.
    """
    if node is None:
        return None
    if isinstance(node, ast.Name):
        return node.id
    if isinstance(node, ast.Attribute):
        return node.attr
    if isinstance(node, ast.Subscript):
        return annotation_head(node.value)
    if isinstance(node, ast.Constant) and isinstance(node.value, str):
        return node.value.split("[")[0].split(".")[-1].strip() or None
    return None


def main() -> int:
    root = Path(sys.argv[1]).resolve()
    files = sorted(root.rglob("*.py"))

    fixtures: dict[str, set[str]] = collections.defaultdict(set)  # name -> return heads
    fixture_files: dict[str, set[str]] = collections.defaultdict(set)
    classes: set[str] = set()
    trees: dict[Path, ast.Module] = {}

    for f in files:
        try:
            tree = ast.parse(f.read_text(encoding="utf-8", errors="replace"))
        except SyntaxError:
            continue
        trees[f] = tree
        for node in ast.walk(tree):
            if isinstance(node, ast.ClassDef):
                classes.add(node.name)
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)) and is_fixture(node):
                fixtures[node.name].add(annotation_head(node.returns) or "")
                fixture_files[node.name].add(str(f.relative_to(root)))

    # Counters, each a narrower population than the last.
    on_param = 0           # obj.m() where obj is a parameter of a test function
    named_fixture = 0      # ... and a fixture of that name exists
    unambiguous = 0        # ... and exactly one fixture defines it
    annotated = 0          # ... and it has a return annotation
    first_party = 0        # ... and that annotation names a class in this repo
    by_reason: collections.Counter = collections.Counter()

    for f, tree in trees.items():
        for fn in ast.walk(tree):
            if not isinstance(fn, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            if not fn.name.startswith("test_"):
                continue
            params = {a.arg for a in fn.args.args} | {a.arg for a in fn.args.kwonlyargs}
            # Parameters carrying their own annotation are already typable
            # without a fixture rule, so they are not this rule's to claim.
            annotated_params = {
                a.arg for a in fn.args.args + fn.args.kwonlyargs if a.annotation is not None
            }
            for call in ast.walk(fn):
                if not isinstance(call, ast.Call):
                    continue
                recv = call.func.value if isinstance(call.func, ast.Attribute) else None
                if not isinstance(recv, ast.Name) or recv.id not in params:
                    continue
                if recv.id in annotated_params:
                    continue
                on_param += 1
                if recv.id not in fixtures:
                    by_reason["no fixture of that name"] += 1
                    continue
                named_fixture += 1
                if len(fixture_files[recv.id]) > 1:
                    by_reason["fixture defined in several files"] += 1
                    continue
                unambiguous += 1
                heads = {h for h in fixtures[recv.id] if h}
                if not heads:
                    by_reason["fixture has no return annotation"] += 1
                    continue
                annotated += 1
                if not (heads & classes):
                    by_reason["return type is not a first-party class"] += 1
                    continue
                first_party += 1

    print(f"corpus                      {root.name}")
    print(f"fixtures defined            {len(fixtures)}")
    print()
    print("call sites on an unannotated parameter of a test function")
    print(f"  total                     {on_param:>6}")
    print(f"  name matches a fixture    {named_fixture:>6}")
    print(f"  fixture is unambiguous    {unambiguous:>6}")
    print(f"  fixture has a return type {annotated:>6}")
    print(f"  and it is first-party     {first_party:>6}   <- what the rule could bind")
    print()
    print("where the rest go")
    for reason, n in by_reason.most_common():
        print(f"  {reason:<38} {n:>6}")
    print()
    print(
        "Magnitude estimate from the source text, not from the resolver, and\n"
        "not a recall figure. It answers whether the rule is worth building."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
