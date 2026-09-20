"""Merges the filled audit chunks back into one sample.json, and refuses to
produce a partial result.

The 400-site sample was split into chunks so several auditors could work in
parallel. That is a throughput device and nothing more — each auditor sees
only call sites and source, never the resolver's verdicts, and no auditor
reviews another's work.

Every check here exists because scoring a partial or malformed audit would
produce a recall figure that looks exactly like a real one. A missing chunk
silently shrinks the denominator; a null `true_callee` silently drops a site;
a duplicated id silently double-counts. None of those announce themselves in
the output, so they are made fatal here instead.

Usage:
    python docs/bench/merge_audit.py <audit-dir> [expected-count]

Reads  <audit-dir>/sample.json and <audit-dir>/chunks/*-filled.json
Writes <audit-dir>/sample.json in place, with judgements filled in.
"""

import glob
import io
import json
import os
import sys


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    audit_dir = sys.argv[1]
    expected = int(sys.argv[2]) if len(sys.argv) > 2 else None

    base_path = os.path.join(audit_dir, "sample.json")
    with io.open(base_path, encoding="utf-8") as f:
        base = json.load(f)

    filled = {}
    duplicates = []
    for path in sorted(glob.glob(os.path.join(audit_dir, "chunks", "*-filled.json"))):
        with io.open(path, encoding="utf-8") as f:
            chunk = json.load(f)
        name = chunk.get("chunk", os.path.basename(path))
        for r in chunk.get("results", []):
            rid = r["id"]
            if rid in filled:
                duplicates.append(rid)
            # Which auditor judged this site, for the agreement diagnostic.
            r["_chunk"] = name
            filled[rid] = r

    problems = []
    if duplicates:
        problems.append(f"{len(duplicates)} ids appear in more than one chunk: {duplicates[:10]}")

    sample_ids = {e["id"] for e in base["sample"]}
    missing = sorted(sample_ids - set(filled))
    if missing:
        problems.append(
            f"{len(missing)} of {len(sample_ids)} sampled sites were never audited. "
            f"Re-dispatch those chunks; do not score a partial audit. First few: {missing[:10]}"
        )

    stray = sorted(set(filled) - sample_ids)
    if stray:
        problems.append(f"{len(stray)} audited ids are not in the sample: {stray[:10]}")

    for rid, r in sorted(filled.items()):
        if not isinstance(r.get("true_callee"), str) or not r["true_callee"].strip():
            problems.append(f"id {rid}: true_callee is missing or empty")
        if not isinstance(r.get("is_first_party"), bool):
            problems.append(f"id {rid}: is_first_party is not a boolean")

    if expected is not None and len(sample_ids) != expected:
        problems.append(f"expected {expected} sampled sites, found {len(sample_ids)}")

    if problems:
        print("MERGE REFUSED. A partial or malformed audit scores like a real one.")
        for p in problems[:40]:
            print(f"  - {p}")
        if len(problems) > 40:
            print(f"  ... and {len(problems) - 40} more")
        return 1

    for e in base["sample"]:
        r = filled[e["id"]]
        e["true_callee"] = r["true_callee"]
        e["is_first_party"] = bool(r["is_first_party"])
        e["reasoning"] = r.get("reasoning", "")

    with io.open(base_path, "w", encoding="utf-8") as f:
        json.dump(base, f, indent=1, ensure_ascii=False)

    first_party = sum(1 for e in base["sample"] if e["is_first_party"])
    print(f"merged {len(base['sample'])} audited sites into {base_path}")
    print(f"  first-party (the recall denominator): {first_party}")
    print(f"  excluded as not first-party:          {len(base['sample']) - first_party}")

    report_auditor_agreement(filled, base)
    return 0


def report_auditor_agreement(filled, base):
    """Reports how differently the auditors drew the first-party line.

    Splitting the sample across auditors buys throughput and costs
    consistency. `is_first_party` sets the recall denominator directly, so an
    auditor applying a systematically looser or tighter rule moves the headline
    figure by more than sampling error does — and unlike sampling error, that
    movement is invisible in the confidence interval.

    This is a diagnostic, not a gate: some spread is ordinary binomial noise at
    these chunk sizes. It is printed so the spread is on the record next to the
    number it affects, rather than discovered later.
    """
    chunks = {}
    for r in filled.values():
        chunks.setdefault(r.get("_chunk", "?"), []).append(r)

    if len(chunks) <= 1:
        return

    print()
    print("  auditor agreement on where the first-party line falls:")
    for name, rows in sorted(chunks.items(), key=lambda kv: str(kv[0])):
        fp = sum(1 for r in rows if r["is_first_party"])
        pct = fp / len(rows) * 100 if rows else 0.0
        print(f"    chunk {name:>3}: {fp:3}/{len(rows):3} first-party  ({pct:5.1f}%)")

    fixture = [r for r in filled.values() if "fixture" in (r.get("reasoning") or "").lower()]
    if fixture:
        fp = sum(1 for r in fixture if r["is_first_party"])
        print(
            f"    of {len(fixture)} sites whose reasoning cites a pytest fixture, "
            f"{fp} were judged first-party and {len(fixture) - fp} were not"
        )
        print(
            "    (dependency injection through a fixture is the single most common\n"
            "     pattern in this corpus's misses; if auditors split on it, the\n"
            "     denominator moves and so does recall)"
        )


if __name__ == "__main__":
    sys.exit(main())
