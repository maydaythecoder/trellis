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

`--chat` sends what `trellis ask` sends to an endpoint identified as colibri:
one user message to the chat endpoint. On colibri `/v1/completions` applies no
chat template, so the default mode measures a prompt the interface never sends
there. `--key-env` names the variable holding the endpoint's key; the key is
sent as a bearer token and never printed or written.

Usage:
    python qa/run-classify.py [--repeat 3] [--model qwen3:8b] [--chat]
                              [--key-env NAME]
    python qa/run-classify.py --self-test
"""

import argparse
import contextlib
import http.server
import io
import json
import os
import pathlib
import secrets
import statistics
import sys
import tempfile
import threading
import time
import urllib.request

HERE = pathlib.Path(__file__).resolve().parent

# Matches `ask::MAX_REPLY_TOKENS`. A local copy, and the mismatch it would
# cause is worth naming: raising it there and not here would measure a
# classifier the interface does not run.
MAX_REPLY_TOKENS = 2048


def complete(base, model, prompt, timeout, chat=False, key=None):
    if chat:
        # The colibri request profile (ADR 0047 decision 5) as `ask_hosted`
        # sends it: with no `temperature` colibri samples at 0.7, and with no
        # thinking field it thinks at `xhigh`.
        path = "/v1/chat/completions"
        payload = {
            "model": model,
            "messages": [{"role": "user", "content": prompt}],
            "max_completion_tokens": MAX_REPLY_TOKENS,
            "temperature": 0,
            "enable_thinking": False,
        }
    else:
        path = "/v1/completions"
        payload = {
            "model": model,
            "prompt": prompt,
            "max_tokens": MAX_REPLY_TOKENS,
            "temperature": 0.0,
        }
    headers = {"Content-Type": "application/json"}
    if key is not None:
        headers["Authorization"] = "Bearer " + key
    req = urllib.request.Request(
        base + path, data=json.dumps(payload).encode(), headers=headers
    )
    with urllib.request.urlopen(req, timeout=timeout) as r:
        choice = json.loads(r.read().decode())["choices"][0]
    if chat:
        return choice["message"].get("content") or ""
    return choice.get("text") or ""


def interpret(text, marker):
    """The same rule `ask::interpret` applies."""
    t = text.strip()
    if marker in t:
        return "task"
    if not t:
        return "empty"
    return "answer"


def build_parser():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repeat", type=int, default=3)
    ap.add_argument("--model", default="qwen3:8b")
    ap.add_argument("--base", default="http://127.0.0.1:11434")
    ap.add_argument("--timeout", type=float, default=600.0)
    ap.add_argument("--fixture", default=str(HERE / "fixtures" / "classify-prompts.json"))
    ap.add_argument("--chat", action="store_true",
                    help="post one user message to /v1/chat/completions")
    ap.add_argument("--key-env", metavar="NAME",
                    help="environment variable holding the endpoint's key")
    ap.add_argument("--self-test", action="store_true")
    return ap


def run(args, key, out):
    """Classify every case, print the table and summary, write the rows to `out`.

    `key` is held here and passed to the request only: it is never on `args`,
    in a row or in anything printed.
    """
    spec = json.loads(pathlib.Path(args.fixture).read_text(encoding="utf-8"))
    marker, cases = spec["marker"], spec["cases"]

    print(f"model     {args.model}")
    print(f"mode      {'chat' if args.chat else 'completion'}")
    print(f"cases     {len(cases)} x {args.repeat}, temperature 0.0, "
          f"max_tokens {MAX_REPLY_TOKENS}")
    print()

    rows = []
    for case in cases:
        for n in range(args.repeat):
            # A monotonic clock: a wall-clock step mid-request is not latency.
            start = time.perf_counter()
            try:
                text = complete(args.base, args.model, case["prompt"], args.timeout,
                                chat=args.chat, key=key)
                got = interpret(text, marker)
            except Exception as e:  # noqa: BLE001 - a measurement, not a dependency
                got, text = "ERROR", str(e)[:80]
            seconds = round(time.perf_counter() - start, 3)
            ok = got == case["expect"]
            rows.append({"id": case["id"], "run": n + 1, "expect": case["expect"],
                         "got": got, "match": ok, "seconds": seconds})
            shown = text.strip().replace("\n", " ")[:58]
            print(f"  {'match   ' if ok else 'MISMATCH'} {case['id']:<20} "
                  f"expect={case['expect']:<7} got={got:<7} {seconds:7.2f}s {shown!r}")

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

    if rows:
        times = [r["seconds"] for r in rows]
        print(f"seconds per input: median {statistics.median(times):.2f}, "
              f"max {max(times):.2f}, over {len(rows)} inputs")

    out.write_text(json.dumps(rows, indent=2), encoding="utf-8")
    print(f"\nrows written to {out}")
    return rows


# --- self-test ---------------------------------------------------------------

SELFTEST_MARKER = "NEEDS_TASK"
SELFTEST_FIXTURE = {
    "marker": SELFTEST_MARKER,
    "cases": [
        {"id": "a-question", "prompt": "what does main do?", "expect": "answer"},
        {"id": "a-change", "prompt": "rename main to run", "expect": "task"},
    ],
}


class _Stub(http.server.BaseHTTPRequestHandler):
    """Answers both endpoints, recording what each request carried."""

    seen = []

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        _Stub.seen.append({"path": self.path,
                           "auth": self.headers.get("Authorization"),
                           "body": body})
        prompt = (body["messages"][0]["content"] if "messages" in body
                  else body.get("prompt", ""))
        text = SELFTEST_MARKER if "rename" in prompt else "It returns 0."
        choice = ({"message": {"role": "assistant", "content": text}}
                  if self.path == "/v1/chat/completions" else {"text": text})
        reply = json.dumps({"choices": [choice]}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(reply)))
        self.end_headers()
        self.wfile.write(reply)

    def log_message(self, *_):
        pass


def self_test():
    failed = 0

    def fail(label, msg):
        nonlocal failed
        print(f"FAIL ({label}): {msg}")
        failed = 1

    before = {p: p.stat().st_mtime_ns for p in HERE.rglob("*")}
    server = http.server.HTTPServer(("127.0.0.1", 0), _Stub)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{server.server_address[1]}"
    # Built at run time, never a literal: a key that leaked would be this one.
    key = "sk-selftest-" + secrets.token_hex(16)
    key_var = "TRELLIS_QA_SELFTEST_KEY_" + secrets.token_hex(4).upper()
    os.environ[key_var] = key
    scenarios = [
        ("chat with --key-env", ["--chat", "--key-env", key_var], "/v1/chat/completions", True),
        ("chat without a key", ["--chat"], "/v1/chat/completions", False),
        ("completion without a key", [], "/v1/completions", False),
    ]
    try:
        with tempfile.TemporaryDirectory() as tmp:
            fixture = pathlib.Path(tmp) / "fixture.json"
            fixture.write_text(json.dumps(SELFTEST_FIXTURE), encoding="utf-8")
            prompts = [c["prompt"] for c in SELFTEST_FIXTURE["cases"]]
            for label, extra, want_path, with_key in scenarios:
                args = build_parser().parse_args(
                    ["--base", base, "--model", "m", "--repeat", "1", "--timeout", "10",
                     "--fixture", str(fixture)] + extra)
                _Stub.seen = []
                out = pathlib.Path(tmp) / "rows.json"
                captured = io.StringIO()
                with contextlib.redirect_stdout(captured):
                    rows = run(args, key_from(args), out)
                printed, written = captured.getvalue(), out.read_text(encoding="utf-8")
                print(f"-- {label}: {len(_Stub.seen)} requests, rows {rows}")

                # A reply the parser misread raises, the run loop records it as
                # ERROR, and every other check here would still pass.
                if len(rows) != len(prompts) or any(
                        r["got"] == "ERROR" or not r["match"] for r in rows):
                    fail(label, f"expected every case to match, got {rows}")
                if any(not isinstance(r.get("seconds"), float) or r["seconds"] < 0
                       for r in rows):
                    fail(label, "a row has no `seconds`")
                if json.loads(written) != rows:
                    fail(label, "the rows written are not the rows returned")
                times = [r.get("seconds", -1.0) for r in rows] or [-1.0]
                summary = (f"seconds per input: median {statistics.median(times):.2f}, "
                           f"max {max(times):.2f}, over {len(prompts)} inputs")
                if summary not in printed:
                    fail(label, f"no timing summary in {printed!r}")
                if key in printed or key in written:
                    fail(label, "the key appears in the output")
                for req, prompt in zip(_Stub.seen, prompts):
                    if req["path"] != want_path:
                        fail(label, f"posted to {req['path']}, expected {want_path}")
                    want_auth = "Bearer " + key if with_key else None
                    if req["auth"] != want_auth:
                        fail(label, "Authorization header "
                             + ("missing or wrong" if with_key else "sent with no key"))
                    if want_path == "/v1/chat/completions":
                        want = {"model": "m",
                                "messages": [{"role": "user", "content": prompt}],
                                "max_completion_tokens": MAX_REPLY_TOKENS,
                                "temperature": 0, "enable_thinking": False}
                    else:
                        want = {"model": "m", "prompt": prompt,
                                "max_tokens": MAX_REPLY_TOKENS, "temperature": 0.0}
                    if req["body"] != want:
                        fail(label, f"body {req['body']}, expected {want}")
                if len(_Stub.seen) != len(prompts):
                    fail(label, f"{len(_Stub.seen)} requests, expected {len(prompts)}")

        # An unset variable is refused by name, before any request.
        err = io.StringIO()
        try:
            with contextlib.redirect_stderr(err):
                key_from(build_parser().parse_args(["--key-env", key_var + "_UNSET"]))
            refused = False
        except SystemExit:
            refused = True
        print(f"-- --key-env unset: refused={refused}")
        if not refused or key_var + "_UNSET" not in err.getvalue():
            fail("--key-env unset", f"expected a refusal naming the variable, "
                 f"got {err.getvalue()!r}")
    finally:
        os.environ.pop(key_var, None)
        server.shutdown()
        server.server_close()

    after = {p: p.stat().st_mtime_ns for p in HERE.rglob("*")}
    if after != before:
        fail("qa/ untouched", "the self-test wrote under qa/")
    print("OK: run-classify self-test" if not failed else "FAIL: run-classify self-test")
    return failed


def key_from(args):
    """The key named by `--key-env`, or None. An unset variable is an error."""
    if args.key_env is None:
        return None
    key = os.environ.get(args.key_env)
    if not key:
        build_parser().error(f"--key-env: {args.key_env} is not set")
    return key


def main():
    args = build_parser().parse_args()
    if args.self_test:
        return self_test()
    run(args, key_from(args), HERE / "classify-latest.json")
    return 0


if __name__ == "__main__":
    sys.exit(main())
