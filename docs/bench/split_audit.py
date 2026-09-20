"""Splits a drawn audit sample into chunks an auditor can work through.

The counterpart to `merge_audit.py`, which until now had none: the 400-site
audits were chunked by hand. The pre-gate audit required by A-34 is roughly
1,000 draws, and doing that split by hand again is how a site gets duplicated
into two chunks or dropped from all of them — neither of which announces
itself, because both produce a sample.json that scores perfectly well and
reports a recall figure that is simply wrong.

So the same discipline `merge_audit.py` applies on the way back in is applied
here on the way out: every id lands in exactly one chunk, the counts add up,
and the script refuses rather than overwriting an auditor's work.

**The auditor never sees the resolver's verdicts.** `answers.json` stays where
`audit_sample` wrote it and is opened only by the scorer. Each chunk carries
the sample entries and the instructions, nothing else. That blindness is the
whole reason the audit is worth more than the resolver's own coverage report.

Usage:
    python docs/bench/split_audit.py <audit-dir> [chunk-size]

Reads  <audit-dir>/sample.json
Writes <audit-dir>/chunks/chunk-N.json
"""

from __future__ import annotations

import io
import json
import os
import sys


def main() -> int:
    if len(sys.argv) < 2:
        print(__doc__)
        return 2
    audit_dir = sys.argv[1]
    size = int(sys.argv[2]) if len(sys.argv) > 2 else 40

    sample_path = os.path.join(audit_dir, "sample.json")
    with io.open(sample_path, encoding="utf-8") as f:
        doc = json.load(f)

    entries = doc["sample"]
    instructions = doc["instructions"]
    chunk_dir = os.path.join(audit_dir, "chunks")
    os.makedirs(chunk_dir, exist_ok=True)

    # Refuse rather than overwrite. A filled chunk is hours of a person's
    # judgement and is not reproducible from anything: the seed reproduces
    # the draw, never the judgements.
    filled = [f for f in os.listdir(chunk_dir) if f.endswith("-filled.json")]
    if filled:
        print(
            f"refusing: {len(filled)} filled chunk(s) already in {chunk_dir}.\n"
            "Re-splitting would orphan them against new chunk boundaries. Move "
            "them aside deliberately if that is really what you want.",
            file=sys.stderr,
        )
        return 1

    written = []
    for i in range(0, len(entries), size):
        part = entries[i : i + size]
        n = i // size
        path = os.path.join(chunk_dir, f"chunk-{n}.json")
        with io.open(path, "w", encoding="utf-8", newline="\n") as f:
            json.dump(
                {"chunk": n, "instructions": instructions, "sample": part},
                f,
                indent=1,
            )
            f.write("\n")
        written.append((path, len(part)))

    # The checks that make the split safe to trust. Each of these would
    # otherwise produce a plausible-looking audit with a wrong denominator.
    ids: list[int] = []
    for path, _ in written:
        with io.open(path, encoding="utf-8") as f:
            ids.extend(e["id"] for e in json.load(f)["sample"])
    original = [e["id"] for e in entries]

    if len(ids) != len(original):
        print(f"BUG: split {len(original)} entries into {len(ids)}", file=sys.stderr)
        return 1
    if len(set(ids)) != len(ids):
        print("BUG: an id appears in more than one chunk", file=sys.stderr)
        return 1
    if sorted(ids) != sorted(original):
        print("BUG: the chunks do not carry the same ids as the sample", file=sys.stderr)
        return 1

    print(f"{len(original)} sites -> {len(written)} chunks of up to {size}")
    for path, n in written:
        print(f"  {path}  ({n})")
    print()
    print(
        "Each chunk is blind: it carries call sites and source, never the\n"
        "resolver's verdicts.\n\n"
        "Record `true_callee`, `is_first_party` and `reasoning` per site and\n"
        "write them to `chunk-N-filled.json`. A filled chunk carries `results`,\n"
        "NOT `sample`, and only these four fields per site:\n\n"
        '  {"chunk": 0, "results": [\n'
        '    {"id": 12, "true_callee": "external", "is_first_party": false,\n'
        '     "reasoning": "isinstance is a Python builtin."}\n'
        "  ]}\n\n"
        "Then:\n"
        f"  python docs/bench/merge_audit.py {audit_dir} {len(original)}\n"
        f"  cargo run --release -p trellis-index --example audit_score -- {audit_dir}\n"
    )
    print(
        "If the resolver changes before the audit is scored, regenerate\n"
        "answers.json with the same seed and size; the draw is reproducible,\n"
        "the verdicts are not."
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())
