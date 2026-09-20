"""Where the `Attribute` deficit actually is.

`trellis index` on poetry reports 8,287 `MethodDispatchUntyped` misses — a
receiver whose type the resolver could not determine — against 21.0% binding on
`Attribute`-shaped call sites. That is the whole remaining gap to the 85% gate.

The roadmap pointed at pytest fixtures, on the evidence that 51% of first-party
misses cite them. `fixture_opportunity.py` sized that and found it already
closed: parameter annotations have been collected as type facts since ADR 0012,
poetry annotates its fixture parameters, and only 10 call sites in the corpus
have an *unannotated* test-function parameter as receiver.

So this asks the question the other way round. Rather than guessing at a rule
and sizing it, classify every `X.m()` call site by the **syntactic shape of its
receiver**, and let the distribution say which rule is worth writing. A rule
that would bind 40 sites is not worth the soundness risk; one that would bind
2,000 might be.

Counts are of call sites in the source text, so they will disagree with the
resolver at the margins. They are for choosing what to build. The figure that
decides whether a gate is met is a hand-audited sample (`BENCHMARK_AND_EVAL.md`
§2), never this.

Usage:  python docs/bench/receiver_shapes.py <corpus-root>
"""

from __future__ import annotations

import ast
import collections
import sys
from pathlib import Path


def head(n) -> str | None:
    """The normalised type name, as `resolve::type_head` computes it."""
    if n is None:
        return None
    if isinstance(n, ast.Name):
        return n.id
    if isinstance(n, ast.Attribute):
        return n.attr
    if isinstance(n, ast.Subscript):
        return head(n.value)
    if isinstance(n, ast.Constant) and isinstance(n.value, str):
        return n.value.split("[")[0].split(".")[-1].strip() or None
    return None


class Scope:
    """What a function body knows about its own names.

    Only what the resolver could also know from one file: parameter
    annotations, annotated assignments, and assignments from a constructor
    call. Anything requiring inference across files is deliberately out, since
    a measurement that assumes a smarter resolver would size a rule nobody is
    going to write.
    """

    def __init__(self, fn: ast.FunctionDef | ast.AsyncFunctionDef):
        self.params: dict[str, str | None] = {}
        for a in fn.args.args + fn.args.kwonlyargs + fn.args.posonlyargs:
            self.params[a.arg] = head(a.annotation)
        self.assigned: dict[str, str | None] = {}
        self.from_ctor: set[str] = set()
        for node in ast.walk(fn):
            if isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
                self.assigned[node.target.id] = head(node.annotation)
            elif isinstance(node, ast.Assign):
                for t in node.targets:
                    if not isinstance(t, ast.Name):
                        continue
                    if isinstance(node.value, ast.Call):
                        f = node.value.func
                        name = f.id if isinstance(f, ast.Name) else head(f)
                        # Capitalised callee: the convention the resolver uses
                        # to guess a constructor. Recorded as its own bucket so
                        # the guess is visible rather than folded in.
                        if name and name[:1].isupper():
                            self.assigned[t.id] = name
                            self.from_ctor.add(t.id)
                        else:
                            self.assigned.setdefault(t.id, None)
                    else:
                        self.assigned.setdefault(t.id, None)


def classify(recv, sc: Scope, classes: set[str]) -> str:
    if isinstance(recv, ast.Name):
        n = recv.id
        if n in ("self", "cls"):
            return "self / cls"
        if n in sc.params:
            t = sc.params[n]
            if t is None:
                return "parameter, unannotated"
            return (
                "parameter, annotated first-party"
                if t in classes
                else "parameter, annotated non-first-party"
            )
        if n in sc.assigned:
            t = sc.assigned[n]
            if t is None:
                return "local, type not derivable in-file"
            where = "first-party" if t in classes else "non-first-party"
            kind = "constructor call" if n in sc.from_ctor else "annotation"
            return f"local from {kind}, {where}"
        return "free name (module-level, import, or builtin)"
    if isinstance(recv, ast.Attribute):
        if isinstance(recv.value, ast.Name) and recv.value.id == "self":
            return "self.attr"
        return "attribute chain (a.b.m())"
    if isinstance(recv, ast.Call):
        return "call result (f().m())"
    if isinstance(recv, ast.Subscript):
        return "subscript (d[k].m())"
    if isinstance(recv, (ast.Str, ast.Constant)):
        return "literal"
    return type(recv).__name__


def main() -> int:
    root = Path(sys.argv[1]).resolve()
    classes: set[str] = set()
    trees: dict[Path, ast.Module] = {}
    for f in sorted(root.rglob("*.py")):
        try:
            t = ast.parse(f.read_text(encoding="utf-8", errors="replace"))
        except SyntaxError:
            continue
        trees[f] = t
        for n in ast.walk(t):
            if isinstance(n, ast.ClassDef):
                classes.add(n.name)

    tally: collections.Counter = collections.Counter()
    examples: dict[str, list[str]] = collections.defaultdict(list)

    for f, t in trees.items():
        rel = f.relative_to(root).as_posix()
        for fn in ast.walk(t):
            if not isinstance(fn, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            sc = Scope(fn)
            for call in ast.walk(fn):
                if not isinstance(call, ast.Call) or not isinstance(call.func, ast.Attribute):
                    continue
                k = classify(call.func.value, sc, classes)
                tally[k] += 1
                if len(examples[k]) < 3:
                    src = ast.unparse(call)[:72] if hasattr(ast, "unparse") else ""
                    examples[k].append(f"{rel}:{call.lineno}  {src}")

    total = sum(tally.values())
    print(f"corpus {root.name}: {total} attribute-shaped call sites, "
          f"{len(classes)} first-party classes\n")
    print(f"{'receiver shape':<44}{'sites':>7}{'share':>8}")
    for k, v in tally.most_common():
        print(f"  {k:<42}{v:>7}{v / total * 100:>7.1f}%")
    print()
    print("examples")
    for k, _ in tally.most_common(8):
        print(f"  {k}")
        for e in examples[k]:
            print(f"      {e}")
    print()
    print(
        "Counted from the source text, not from the resolver, and not a recall\n"
        "figure. It exists to choose which rule is worth the soundness risk."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
