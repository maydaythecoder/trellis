#!/usr/bin/env python3
"""Measure answer-or-task classification against its expectations.

Classification is reachable only through `trellis tui`, so the prompts here are
not rebuilt — `qa/fixtures/classify-prompts.json` is written by `ask.rs`'s own
test from the function that ships, on every `cargo test`. Twice in this project
a prompt rebuilt by hand behaved differently from the real one.

The sampling settings match the client the interface uses: temperature 0.0 and
the same reply ceiling. Measuring at a different temperature would measure a
different thing.

Three outcomes per case, not two. A reply that is neither the marker nor a
usable answer — empty, because the model spent its budget reasoning — is its
own row, because its fix is the reply ceiling and the fix for a misclassified
answer is the prompt.

Usage:
    python qa/run-classify.py [--repeat 3] [--model qwen3:8b]
"""

import argparse
import json
import pathlib
import sys
import urllib.request

HERE = pathlib.Path(__file__).resolve().parent

# Matches `ask::MAX_REPLY_TOKENS`. A local copy, and the mismatch it would
# cause is worth naming: raising it there and not here would measure a
# classifier the interface does not run.
MAX_REPLY_TOKENS = 2048


def complete(base, model, prompt, timeout):
    body = json.dumps(
        {
            "model": model,
            "prompt": prompt,
            "max_tokens": MAX_REPLY_TOKENS,
            "temperature": 0.0,
        }
    ).encode()
    req = urllib.request.Request(
        base + "/v1/completions", data=body, headers={"Content-Type": "application/json"}
    )
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read().decode())["choices"][0].get("text") or ""


def interpret(text, marker):
    """The same rule `ask::interpret` applies."""
    t = text.strip()
    if marker in t:
        return "task"
    if not t:
        return "empty"
    return "answer"


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repeat", type=int, default=3)
    ap.add_argument("--model", default="qwen3:8b")
    ap.add_argument("--base", default="http://127.0.0.1:11434")
    ap.add_argument("--timeout", type=float, default=600.0)
    ap.add_argument("--fixture", default=str(HERE / "fixtures" / "classify-prompts.json"))
    args = ap.parse_args()

    spec = json.loads(pathlib.Path(args.fixture).read_text(encoding="utf-8"))
    marker, cases = spec["marker"], spec["cases"]

    print(f"model     {args.model}")
    print(f"cases     {len(cases)} x {args.repeat}, temperature 0.0, "
          f"max_tokens {MAX_REPLY_TOKENS}")
    print()

    rows = []
    for case in cases:
        for n in range(args.repeat):
            try:
                text = complete(args.base, args.model, case["prompt"], args.timeout)
                got = interpret(text, marker)
            except Exception as e:  # noqa: BLE001 - a measurement, not a dependency
                got, text = "ERROR", str(e)[:80]
            ok = got == case["expect"]
            rows.append({"id": case["id"], "run": n + 1, "expect": case["expect"],
                         "got": got, "match": ok})
            shown = text.strip().replace("\n", " ")[:58]
            print(f"  {'match   ' if ok else 'MISMATCH'} {case['id']:<20} "
                  f"expect={case['expect']:<7} got={got:<7} {shown!r}")

    print()
    matched = sum(1 for r in rows if r["match"])
    print(f"{matched}/{len(rows)} matched")
    empties = sum(1 for r in rows if r["got"] == "empty")
    if empties:
        print(f"{empties} empty repl{'y' if empties == 1 else 'ies'} — the reply "
              f"ceiling, not the prompt")

    by_case = {}
    for r in rows:
        by_case.setdefault(r["id"], []).append(r["match"])
    unstable = [k for k, v in by_case.items() if any(v) and not all(v)]
    if unstable:
        print(f"unstable across repeats: {', '.join(unstable)}")
        print("  at temperature 0 this means the runtime is not deterministic,")
        print("  which is worth knowing before any single run is quoted.")

    out = HERE / "classify-latest.json"
    out.write_text(json.dumps(rows, indent=2), encoding="utf-8")
    print(f"\nrows written to {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
