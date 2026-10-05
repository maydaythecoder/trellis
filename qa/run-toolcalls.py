#!/usr/bin/env python3
"""Measure tool calling against an OpenAI-compatible endpoint.

Four independent outcomes per run, never collapsed into one score. A
well-formed call to the wrong function and a malformed call with the right
intent are different failures with different fixes:

  valid       a populated `tool_calls` array came back
  correct     the tool named is the one the case requires
  args        the arguments match what the case expects
  prose       the reply was JSON embedded in the content field (plain text
              is counted on its own, as `text`)

`prose` is counted separately and is *not* the complement of `valid`: a
reply can be neither a tool call nor recognisable JSON, and lumping those
together would hide a model that simply refuses.

Usage:
    python qa/run-toolcalls.py [--model qwen3:8b] [--base http://127.0.0.1:11434/v1]
"""

import argparse
import json
import pathlib
import re
import sys
import time
import urllib.error
import urllib.request

HERE = pathlib.Path(__file__).resolve().parent


def post(url, payload, timeout):
    body = json.dumps(payload).encode()
    req = urllib.request.Request(
        url, data=body, headers={"Content-Type": "application/json"}
    )
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read().decode())


def ttft(base, model, timeout):
    """Seconds to the first content or tool-call chunk of a streamed reply."""
    payload = {
        "model": model,
        "messages": [{"role": "user", "content": "Count to three."}],
        "stream": True,
        "max_tokens": 2048,
        "temperature": 0,
    }
    body = json.dumps(payload).encode()
    req = urllib.request.Request(
        base + "/chat/completions", data=body, headers={"Content-Type": "application/json"}
    )
    start = time.monotonic()
    with urllib.request.urlopen(req, timeout=timeout) as r:
        for raw in r:
            line = raw.decode(errors="replace").strip()
            if not line.startswith("data:"):
                continue
            data = line[5:].strip()
            if data == "[DONE]":
                break
            try:
                d = json.loads(data)
            except json.JSONDecodeError:
                continue
            delta = (d.get("choices") or [{}])[0].get("delta") or {}
            if delta.get("content") or delta.get("tool_calls"):
                return time.monotonic() - start
    return None


# A brace-and-quoted-key shape. Deliberately loose: the point is to catch a
# model *describing* a call in prose, which is the failure mode, so a false
# positive here is better than missing one.
JSONISH = re.compile(r'\{[^{}]*"[A-Za-z_]+"\s*:')


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--model", default="qwen3:8b")
    ap.add_argument("--base", default="http://127.0.0.1:11434/v1")
    ap.add_argument("--timeout", type=float, default=300.0)
    ap.add_argument("--fixture", default=str(HERE / "fixtures" / "tool-calls.json"))
    ap.add_argument("--max-tokens", type=int, default=2048)
    args = ap.parse_args()

    fixture = json.loads(pathlib.Path(args.fixture).read_text(encoding="utf-8"))
    tools, system, cases = fixture["tools"], fixture["system"], fixture["cases"]

    print(f"endpoint  {args.base}")
    print(f"model     {args.model}")
    print(f"cases     {len(cases)}")
    print(f"max_tokens {args.max_tokens}")
    print()

    valid = correct = args_ok = prose = text = errors = 0
    rows = []
    t0 = time.monotonic()

    for case in cases:
        payload = {
            "model": args.model,
            "messages": [
                {"role": "system", "content": system},
                {"role": "user", "content": case["prompt"]},
            ],
            "tools": tools,
            "max_tokens": args.max_tokens,
            "temperature": 0,
        }
        try:
            d = post(args.base + "/chat/completions", payload, args.timeout)
        except (urllib.error.URLError, TimeoutError, OSError) as e:
            errors += 1
            rows.append((case["id"], "ERROR", str(e)[:60], "", ""))
            continue

        msg = (d.get("choices") or [{}])[0].get("message") or {}
        calls = msg.get("tool_calls") or []
        content = msg.get("content") or ""

        if calls:
            valid += 1
            fn = (calls[0].get("function") or {})
            name = fn.get("name", "")
            try:
                got = json.loads(fn.get("arguments") or "{}")
            except json.JSONDecodeError:
                got = {"_unparseable": fn.get("arguments")}
            name_ok = name == case["tool"]
            if name_ok:
                correct += 1
            # Expected keys must match in value; extra keys are tolerated,
            # because a model adding an optional field it was offered is not
            # the failure this measures.
            a_ok = name_ok and all(got.get(k) == v for k, v in case["args"].items())
            # A case expecting no arguments is satisfied by no arguments or
            # by an explicitly empty filter.
            if a_ok:
                args_ok += 1
            rows.append((case["id"], "call", name, json.dumps(got)[:48], "ok" if a_ok else "args"))
        else:
            if JSONISH.search(content):
                prose += 1
                rows.append((case["id"], "PROSE-JSON", content[:56].replace("\n", " "), "", ""))
            else:
                text += 1
                rows.append((case["id"], "TEXT", content[:56].replace("\n", " "), "", ""))

    elapsed = time.monotonic() - t0
    for r in rows:
        print(f"{r[0]:>3}  {r[1]:<11} {r[2]:<34} {r[3]:<50} {r[4]}")

    n = len(cases)
    pct = lambda x: f"{x}/{n} ({100*x/n:.0f}%)"
    print()
    print(f"valid tool_calls   {pct(valid)}")
    print(f"correct tool       {pct(correct)}")
    print(f"correct arguments  {pct(args_ok)}")
    print(f"text               {pct(text)}")
    print(f"JSON-in-prose      {pct(prose)}")
    if errors:
        print(f"transport errors   {errors}/{n}")
    print(f"wall clock         {elapsed:.1f}s for {n} requests")

    try:
        t = ttft(args.base, args.model, args.timeout)
        print(f"time to first token {t:.2f}s" if t else "time to first token  (no chunk seen)")
    except Exception as e:  # noqa: BLE001 - a measurement, not a dependency
        print(f"time to first token  unavailable: {e}")

    return 0


if __name__ == "__main__":
    sys.exit(main())
