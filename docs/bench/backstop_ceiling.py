"""How far each candidate backstop could move recall on the audited samples.

`receiver_shapes.py` classifies every `Attribute` call site in the corpus by
the syntactic shape of its receiver. That is the population a rule would
touch, not the population the gate figure is computed over. The gate figure
is recall on a hand-audited sample (`BENCHMARK_AND_EVAL.md` §2), so the only
place a backstop's effect on it can be read is the audited **misses**: sites
the auditor judged first-party that the resolver did not bind to the true
callee.

This takes those misses, classifies each with `receiver_shapes.classify`
(imported, not copied, so the two scripts cannot drift apart), and prints for
each shape the recall the sample would show if every miss of that shape were
bound. That is a **ceiling**: an upper bound on what a backstop reaching that
shape could do, assuming it binds every such site and binds each one
correctly. It is not a recall figure and must not be compared to §3's
thresholds as one.

The resolver's verdicts are read from a freshly regenerated `answers.json`,
never the committed one: the committed seed-20260919 verdicts are stale (the
`precision-gate` CI job's comment records how). The human judgements are read
from the committed `sample.json`, and the regenerated sample's ids must equal
them, or the join is between two different samples and the script refuses.

Usage:
  cargo run --release -p trellis-index --example audit_sample -- \\
      <corpus-root> <n> <seed> <regen-dir>
  py -3 docs/bench/backstop_ceiling.py <corpus-root> \\
      docs/bench/poetry-2.5.0-seed<seed> <regen-dir>
"""

from __future__ import annotations

import ast
import collections
import inspect
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import receiver_shapes  # noqa: E402  (the path insert above is what finds it)

# The two shapes backstop (a), cross-file propagation of local and attribute
# types, is defined against. Spelled exactly as `receiver_shapes.classify`
# returns them; `main` fails if either string stops being a shape it can
# return, rather than silently reporting a zero ceiling.
CROSS_FILE_SHAPES = (
    "attribute chain (a.b.m())",
    "local, type not derivable in-file",
)

# Two populations of misses fall outside `classify`'s domain. They get their
# own labels rather than a forced receiver shape, so that no backstop is
# credited with reaching a site whose shape was never measured.
BARE_NAME = "bare name call f() (not an Attribute site)"
NO_FUNCTION = "attribute call outside any function body"

# The verdict kinds a Pyright backstop would be invoked on (§4: "only for call
# sites the primary resolver left Unresolved or Candidates").
PYRIGHT_TRIGGER = ("Unresolved", "Candidates")


def matches(resolver: str, auditor: str) -> bool:
    """`audit_score.rs::matches`, ported literally.

    The baseline this script prints must equal `audit_score`'s, or every
    ceiling is computed against the wrong numerator. `main` prints the
    baseline so the two can be compared.
    """
    if resolver == auditor:
        return True
    if resolver.rsplit(".", 1)[-1] != auditor.rsplit(".", 1)[-1]:
        return False
    prefix = resolver.rsplit(".", 1)[0] if "." in resolver else ""
    return bool(prefix) and auditor.startswith(prefix)


def constructor_convention(auditor: str) -> bool:
    """`audit_score.rs::constructor_convention`, ported literally."""
    return auditor.endswith(".__init__")


def bound(verdict: dict, truth: str) -> bool:
    """Whether `audit_score` counts this site in the recall numerator."""
    if verdict.get("kind") != "Resolved":
        return False
    target = verdict.get("target") or ""
    return matches(target, truth) or constructor_convention(truth)


def miss_kind(verdict: dict) -> str:
    kind = verdict.get("kind", "Missing")
    if kind == "Resolved":
        # Only reachable if precision fell below 100%, which the precision
        # gate forbids; named so it is visible if it ever happens.
        return "Resolved-wrong"
    if kind == "ExternalExcluded":
        return "ExternalExcluded"
    return kind


class Corpus:
    """Parsed files and the first-party class set, built as
    `receiver_shapes.main` builds them."""

    def __init__(self, root: Path):
        self.root = root
        self.classes: set[str] = set()
        self.sources: dict[str, str] = {}
        self.trees: dict[str, ast.Module] = {}
        for f in sorted(root.rglob("*.py")):
            src = f.read_text(encoding="utf-8", errors="replace")
            try:
                t = ast.parse(src)
            except SyntaxError:
                continue
            rel = f.relative_to(root).as_posix()
            self.sources[rel] = src
            self.trees[rel] = t
            for n in ast.walk(t):
                if isinstance(n, ast.ClassDef):
                    self.classes.add(n.name)

    def locate(self, site: dict) -> tuple[ast.Call, ast.AST | None]:
        """The call node for a sampled site and its innermost enclosing def.

        Matched on start line and exact source text. Chained calls such as
        `a.b().c()` share a start position, so the line alone is ambiguous;
        the text is what tells them apart. Anything but exactly one match is
        an error, because a guessed node is a guessed shape.
        """
        rel = site["file"]
        if rel not in self.trees:
            raise SystemExit(f"site {site['id']}: {rel} is not a parseable file under {self.root}")
        src = self.sources[rel]
        want = site["call"].replace("\r\n", "\n")
        found: list[tuple[ast.Call, ast.AST | None]] = []

        def visit(node: ast.AST, fn: ast.AST | None) -> None:
            for child in ast.iter_child_nodes(node):
                inner = child if isinstance(child, (ast.FunctionDef, ast.AsyncFunctionDef)) else fn
                if (
                    isinstance(child, ast.Call)
                    and child.lineno == site["line"]
                    and ast.get_source_segment(src, child) == want
                ):
                    found.append((child, fn))
                visit(child, inner)

        visit(self.trees[rel], None)
        if len(found) != 1:
            raise SystemExit(
                f"site {site['id']} ({rel}:{site['line']}): {len(found)} call nodes "
                f"match the sampled text, expected exactly one"
            )
        return found[0]


def shape_of(site: dict, corpus: Corpus) -> str:
    # The sample's own `shape` field is the resolver's site kind. A `Name`
    # site is a bare call; `classify` only speaks about receivers.
    if site["shape"] != "Attribute":
        return BARE_NAME
    call, fn = corpus.locate(site)
    if not isinstance(call.func, ast.Attribute):
        raise SystemExit(f"site {site['id']}: sampled as Attribute but the call is not `x.m()`")
    if fn is None:
        return NO_FUNCTION
    # Innermost enclosing def: the scope whose names the receiver refers to.
    return receiver_shapes.classify(call.func.value, receiver_shapes.Scope(fn), corpus.classes)


def pct(k: int, n: int) -> str:
    return f"{k}/{n} = {k / n * 100:.1f}%"


def main() -> int:
    if len(sys.argv) != 4:
        print(__doc__, file=sys.stderr)
        return 2
    root = Path(sys.argv[1]).resolve()
    committed = Path(sys.argv[2])
    regen = Path(sys.argv[3])

    sample = json.loads((committed / "sample.json").read_text(encoding="utf-8"))
    answers = json.loads((regen / "answers.json").read_text(encoding="utf-8"))
    regen_sample = json.loads((regen / "sample.json").read_text(encoding="utf-8"))

    if answers.get("seed") != sample.get("seed"):
        raise SystemExit(f"seed mismatch: sample {sample.get('seed')}, answers {answers.get('seed')}")
    ids = [e["id"] for e in sample["sample"]]
    if ids != [e["id"] for e in regen_sample["sample"]]:
        raise SystemExit(
            "the regenerated sample's sites differ from the committed sample's; "
            "the verdicts would be joined to the wrong judgements"
        )

    corpus = Corpus(root)
    classifier_src = inspect.getsource(receiver_shapes.classify)
    for s in CROSS_FILE_SHAPES:
        # Guard against a rename in receiver_shapes.py turning backstop (a)'s
        # ceiling into a silent zero.
        if f'"{s}"' not in classifier_src:
            raise SystemExit(f"{s!r} is not a shape receiver_shapes.classify returns")

    verdicts = answers["verdicts"]
    n = 0
    k = 0
    unaudited = 0
    misses: collections.Counter = collections.Counter()
    kinds: dict[str, collections.Counter] = collections.defaultdict(collections.Counter)
    examples: dict[str, list[str]] = collections.defaultdict(list)
    for e in sample["sample"]:
        truth = e.get("true_callee")
        if truth is None:
            unaudited += 1
            continue
        if not e.get("is_first_party"):
            continue
        n += 1
        v = verdicts.get(str(e["id"]), {"kind": "Missing"})
        if bound(v, truth):
            k += 1
            continue
        s = shape_of(e, corpus)
        misses[s] += 1
        kinds[s][miss_kind(v)] += 1
        if len(examples[s]) < 2:
            examples[s].append(f"{e['file']}:{e['line']}  {e['call'].splitlines()[0][:60]}")

    if n == 0:
        raise SystemExit("no audited first-party sites")
    total_misses = sum(misses.values())
    assert total_misses == n - k

    print(f"BACKSTOP CEILING  (seed {sample['seed']}, drawn {sample['drawn']})")
    if unaudited:
        print(f"  UNAUDITED {unaudited}: excluded, as audit_score excludes them")
    print(f"  baseline recall  {pct(k, n)}   (must equal audit_score on the same regen dir)")
    print(f"  misses           {total_misses}   (first-party per the auditor, not bound)")
    print()
    print("A ceiling is the recall if EVERY miss of that shape were bound correctly.")
    print("It is an upper bound on a backstop, not a recall figure.")
    print()
    print(f"  {'receiver shape of the miss':<44}{'misses':>7}  {'ceiling':<20}verdict kinds")
    for s, m in misses.most_common():
        kd = ", ".join(f"{kk} {vv}" for kk, vv in sorted(kinds[s].items()))
        print(f"  {s:<44}{m:>7}  {pct(k + m, n):<20}{kd}")
    print()

    a = sum(misses[s] for s in CROSS_FILE_SHAPES)
    print("candidate backstops")
    print(f"  (a) cross-file propagation of local and attribute types")
    print(f"      reaches: {', '.join(CROSS_FILE_SHAPES)}")
    print(f"      misses {a}, ceiling {pct(k + a, n)}")
    trig = collections.Counter()
    for s in misses:
        for kk, vv in kinds[s].items():
            trig[kk] += vv
    b = sum(trig[kk] for kk in PYRIGHT_TRIGGER)
    print(f"  (b) Pyright over LSP, invoked only on {' / '.join(PYRIGHT_TRIGGER)} sites")
    print(f"      misses carrying a trigger verdict {b} of {total_misses}, ceiling {pct(k + b, n)}")
    outside = total_misses - b
    if outside:
        print(f"      {outside} misses are not {' / '.join(PYRIGHT_TRIGGER)} and the trigger never reaches them")
    print("      This is where the trigger fires, not a measurement of what Pyright binds.")
    print()
    print("examples")
    for s, _ in misses.most_common():
        print(f"  {s}")
        for x in examples[s]:
            print(f"      {x}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
