"""Compares S7 floor reports, and refuses when they did not measure the same thing.

Two floor reports are comparable only when they ran the same task set, scored
by the same checker, under the same budget and the same `--limit`. Those four
fields are the **comparability key** `run_floor.py` writes into every report as
`comparable`; when any of them differ this script exits 1 naming each one, and
prints no figure.

Two axes may differ, and are what a comparison is for:

  configuration   the model x quantization x runtime under test
  trellis_sha256  the `trellis` binary that ran (its `--version` is printed
                  beside it, but the hash is the axis: two builds of one
                  version differ)

A report with no `comparable` block (every report committed before #344) is
refused by name: nothing in it says which task set or binary produced it.

Figures are printed one column per report and never combined across reports:
no totals, deltas or rates. Wall-clock is printed only when every report ran
the same binary, because the cost columns are comparable only within one
harness version (`run_floor.py`'s docstring); `repaired` is not affected by
that and is always printed.

Usage:
  py -3 docs/bench/compare_floor.py <report.json> <report.json>...
  py -3 docs/bench/compare_floor.py --self-test
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import io
import json
import subprocess
import sys
from pathlib import Path

# Must be equal across reports, or the comparison is refused.
MUST_MATCH = ("tasks_sha256", "checker", "budget", "limit")
# Allowed to differ; shown as the axes of the comparison.
AXES = ("configuration", "trellis_sha256", "trellis_version")
KEY_FIELDS = MUST_MATCH + ("trellis_sha256", "trellis_version")


def sha256_file(path: Path) -> str:
    # Bytes, not text: newline translation would hash one file two ways.
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


def comparable_key(tasks_path, checker: str, budget: dict, limit: int, trellis_bin) -> dict:
    """The fields that must match for two floor reports to be compared, plus
    the binary that ran, which is allowed to differ but must be recorded."""
    p = subprocess.run(
        [str(trellis_bin), "--version"], capture_output=True, text=True, timeout=60
    )
    if p.returncode != 0:
        raise RuntimeError(f"{trellis_bin} --version exited {p.returncode}: {p.stderr.strip()}")
    return {
        "tasks_sha256": sha256_file(Path(tasks_path)),
        "checker": checker,
        "budget": budget,
        "limit": limit,
        "trellis_sha256": sha256_file(Path(trellis_bin)),
        "trellis_version": p.stdout.strip(),
    }


def compare(named: list[tuple[str, dict]]) -> int:
    """Prints `named` side by side, or refuses. Returns the exit code."""
    missing = [
        name for name, r in named
        if not isinstance(r.get("comparable"), dict)
        or any(f not in r["comparable"] for f in KEY_FIELDS)
    ]
    if missing:
        for name in missing:
            print(f"REFUSED: {name} has no comparable block "
                  f"(or one missing a field of {', '.join(KEY_FIELDS)})")
        return 1

    first = named[0][1]["comparable"]
    differ = [f for f in MUST_MATCH
              if any(r["comparable"][f] != first[f] for _, r in named[1:])]
    if differ:
        for f in differ:
            print(f"REFUSED: {f} differs: "
                  + "; ".join(f"{n}={json.dumps(r['comparable'][f])}" for n, r in named))
        return 1

    def row(label: str, cells: list) -> None:
        print("\t".join([label, *map(str, cells)]))

    reports = [r for _, r in named]
    key = [r["comparable"] for r in reports]
    row("report", [n for n, _ in named])
    print("-- axes (allowed to differ)")
    row("configuration", [r.get("configuration") for r in reports])
    row("trellis_sha256", [k["trellis_sha256"] for k in key])
    row("trellis_version", [k["trellis_version"] for k in key])
    print("-- key (equal in every report)")
    for f in MUST_MATCH:
        row(f, [json.dumps(k[f]) for k in key])
    print("-- figures (per report, never combined)")
    row("repaired", [f"{r['check_recovery']['repaired']} / {r['check_recovery']['attempted']}"
                     for r in reports])
    for outcome in sorted({o for r in reports for o in r.get("outcomes", {})}):
        row(outcome, [r.get("outcomes", {}).get(outcome, 0) for r in reports])
    if len({k["trellis_sha256"] for k in key}) == 1:
        row("median_s", [r["wall_clock_s"]["median"] for r in reports])
    else:
        print("median_s\tnot shown: the reports ran different binaries, and "
              "wall-clock compares only within one harness version")
    return 0


# --- self-test ---------------------------------------------------------------

def _report(configuration: str, **key_overrides) -> dict:
    key = {
        "tasks_sha256": "a" * 64,
        "checker": "mypy --no-incremental",
        "budget": {"max_iterations": 8, "max_wall_clock_s": 1800},
        "limit": 0,
        "trellis_sha256": "b" * 64,
        "trellis_version": "trellis 0.1.0",
    }
    key.update(key_overrides)
    return {
        "configuration": configuration,
        "comparable": key,
        "outcomes": {"no-progress": 20},
        "check_recovery": {"repaired": 0, "attempted": 20},
        "wall_clock_s": {"median": 15.1},
    }


def _run(named: list[tuple[str, dict]]) -> tuple[int, str]:
    buf = io.StringIO()
    with contextlib.redirect_stdout(buf):
        code = compare(named)
    return code, buf.getvalue()


def self_test() -> int:
    no_key = _report("cfg-old")
    del no_key["comparable"]
    cases = [
        ("equal keys", [("a.json", _report("cfg-a")), ("b.json", _report("cfg-b"))],
         0, ["cfg-a", "cfg-b"]),
        ("tasks_sha256 mismatch",
         [("a.json", _report("cfg-a")), ("b.json", _report("cfg-b", tasks_sha256="c" * 64))],
         1, ["tasks_sha256"]),
        ("budget mismatch",
         [("a.json", _report("cfg-a")),
          ("b.json", _report("cfg-b", budget={"max_iterations": 12, "max_wall_clock_s": 1800}))],
         1, ["budget"]),
        ("no comparable block", [("a.json", _report("cfg-a")), ("old.json", no_key)],
         1, ["old.json"]),
    ]
    failed = 0
    for label, named, want_code, want_text in cases:
        code, out = _run(named)
        print(f"-- {label}\n{out}", end="")
        if code != want_code or not all(t in out for t in want_text):
            print(f"FAIL ({label}): expected exit {want_code} naming {want_text}, got exit {code}")
            failed = 1
        # A refusal prints no figure: an incomparable pair is never shown as a comparison.
        if want_code == 1 and "repaired" in out:
            print(f"FAIL ({label}): a refused comparison printed figures")
            failed = 1
    print("OK: floor compare self-test" if not failed else "FAIL: floor compare self-test")
    return failed


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("reports", nargs="*")
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        return self_test()
    if len(a.reports) < 2:
        ap.error("at least two reports are required")
    named = []
    for path in a.reports:
        try:
            report = json.loads(Path(path).read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError) as e:
            print(f"REFUSED: {path} could not be read as a report: {e}")
            return 1
        if not isinstance(report, dict):
            print(f"REFUSED: {path} is not a report object")
            return 1
        named.append((path, report))
    return compare(named)


if __name__ == "__main__":
    sys.exit(main())
