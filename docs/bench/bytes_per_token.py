"""Bytes per prompt token, from a floor run's trails and logs (#170, ADR 0033).

The trail cannot give this ratio by itself. `factual.prompt_tokens` is the
adapter's sum over every request whose reply it read, retried ones included,
while `factual.context` records only the first and the largest prompt in
bytes. Dividing the token sum by either byte figure divides a sum over N
requests by one request. So the byte sum comes from the run's log, which
carries one `prompt_bytes=` line per `next_step` (the runtime adapter's debug
event, enabled with `RUST_LOG=trellis=info,trellis_loop::adapter=debug`), and
the trail's byte fields are the check on it: the log's first figure must equal
`first_prompt_bytes`, `peak_prompt_bytes` must be one of its figures, and
there must be at least `turns_measured` of them.

A `next_step` sends the same prompt once per attempt. Each attempt whose reply
held no valid call logs one `... tool call; retrying` warning, and a step that
found a valid call also sent the attempt that succeeded. A run that ended on
"no valid tool call" spent its last step's attempts on warnings only, and one
that ended on "runtime call failed" sent a last attempt whose reply was never
read, so the adapter counted no tokens for it and neither does this.

The runtime's own request log is the second check (`ollama-requests.txt`
beside the trails: Ollama 0.34.2's server log, cut to its per-request lines).
Ollama truncates a prompt longer than its window and reports the tokens it
kept, so a run whose counted requests include a truncated one understates its
tokens. A run is used only when its trail's `prompt_tokens` equals the sum of
its untruncated requests in that log and the log's counted attempts equal
their number. Each run begins with the probe trellis sends before its first
step, which this template renders as 34 tokens; that is how the log is split
into runs, and the split must yield one group per trail or nothing is scored.

Ollama's `/v1/completions` also wraps the prompt in the model's chat template,
so its counts include the template's tokens (about 29 per request here). That
is what the runtime charges against its window, so it is not subtracted.

The hosted adapter (a colibri run) logs differently, and has no runtime log
to pair with. It logs `prompt_bytes=` once per *attempt*, not once per step,
so a retry warning there is followed by the retried attempt's own line rather
than standing in for it: a warning counts as an attempt only when no line
for the same `turns=` follows it. An attempt refused with `context overflow;
eliding and retrying` read no reply and counts no tokens, and `hosted call
failed` ends a run on an attempt whose reply was never read. colibri refuses
a prompt past its window rather than cutting it, so the truncation check has
nothing to catch there; `--no-runtime-log` skips the runtime log, keeps the
trail checks, and prints per-run and pooled figures only, since per-request
token counts come from that log alone.

Usage:
  python docs/bench/bytes_per_token.py <floor-dir>
  python docs/bench/bytes_per_token.py --no-runtime-log <floor-dir>
  py -3 docs/bench/bytes_per_token.py --self-test

Prints one row per run and the pooled ratio (bytes over tokens, summed over
the runs used), with the per-run minimum, median and maximum beside it, the
same per request (each counted attempt paired with the runtime's count for
it), and the first prompts pooled. The pooled figure weights each request by
its size, which is what a byte budget converts; the lowest per-request figure
is printed beside it because a pooled ratio is not a bound on every prompt.
"""

from __future__ import annotations

import argparse
import contextlib
import io
import json
import re
import sys
import tempfile
from pathlib import Path

ANSI = re.compile(r"\x1b\[[0-9;]*m")
# The adapter's event is the only DEBUG line carrying both fields; the loop's
# INFO `turn` event repeats `prompt_bytes` without `turns`. The log does not
# print targets, so the pair is what identifies it.
BYTES = re.compile(r"\bDEBUG\b.*\bprompt_bytes=(\d+) turns=(\d+)")
# The runtime adapter's two warnings, then the hosted adapter's own.
RETRY = re.compile(r"(?:malformed|invalid|no usable) tool call; retrying")
# The hosted adapter's: the attempt was sent and refused, no reply was read.
OVERFLOW = "context overflow; eliding and retrying"
# Each ends the run inside a step whose last attempt's tokens were not counted.
UNCOUNTED_LAST = ("no valid tool call after", "runtime call failed", "hosted call failed")
REQUEST = re.compile(r"new prompt, n_ctx_slot = \d+, .*task\.n_tokens = (\d+)")
TRUNCATED = "truncating input prompt"
PROBE_TOKENS = 34


def requests(log: str) -> list[tuple[int, int]] | None:
    """(bytes, attempts counted) per `prompt_bytes=` line, or None when there
    are none. One line per step from the runtime adapter, one per attempt
    from the hosted one; see the module docstring."""
    steps: list[list[int]] = []
    turns = None
    pending = 0
    for line in ANSI.sub("", log).splitlines():
        m = BYTES.search(line)
        if m:
            # A warning followed by a line for the same step was the hosted
            # adapter's, and this line is the attempt it announced.
            if steps and int(m.group(2)) != turns:
                steps[-1][1] += pending
            pending = 0
            turns = int(m.group(2))
            steps.append([int(m.group(1)), 1])
        elif RETRY.search(line) and steps:
            pending += 1
        elif OVERFLOW in line and steps:
            steps[-1][1] = 0
    if not steps:
        return None
    steps[-1][1] += pending
    if any(s in log for s in UNCOUNTED_LAST):
        # Not below zero: a hosted run that ends refusing a prompt it can
        # elide no further sends nothing after its last overflow.
        steps[-1][1] = max(0, steps[-1][1] - 1)
    return [(b, n) for b, n in steps]


def runtime_runs(text: str) -> list[list[tuple[int, bool]]]:
    """Per run, (prompt tokens, truncated) for each request after the probe."""
    runs: list[list[tuple[int, bool]]] = []
    truncated = False
    for line in text.splitlines():
        if TRUNCATED in line:
            truncated = True
            continue
        m = REQUEST.search(line)
        if not m:
            continue
        n = int(m.group(1))
        if n == PROBE_TOKENS and not truncated:
            runs.append([])
        elif runs:
            runs[-1].append((n, truncated))
        truncated = False
    return runs


def report(floor: Path, runtime_log: bool) -> int:
    trails = sorted(floor.glob("t*-*.json"))
    if not runtime_log:
        runtime: list = [None] * len(trails)
    else:
        runtime = runtime_runs((floor / "ollama-requests.txt").read_text(encoding="utf-8"))
    if len(runtime) != len(trails):
        print(f"runtime log splits into {len(runtime)} runs for {len(trails)} trails; "
              "refusing to pair them")
        return 1

    rows = []
    pairs: list[tuple[int, int]] = []
    firsts: list[tuple[int, int]] = []
    for trail_path, served in zip(trails, runtime):
        task = trail_path.name.split("-", 1)[0]
        f = json.loads(trail_path.read_text(encoding="utf-8")).get("factual", {})
        tokens, ctx = f.get("prompt_tokens"), f.get("context")
        log_path = floor / f"{task}.log"
        if not isinstance(tokens, int) or not ctx or not log_path.exists():
            rows.append((task, None, "no prompt_tokens, context or log"))
            continue
        steps = requests(log_path.read_text(encoding="utf-8"))
        if steps is None:
            rows.append((task, None, "no prompt_bytes lines in log"))
            continue
        sizes = [b for b, _ in steps]
        # One log line per `next_step`, one trail entry per turn. A final step
        # that returned `done` or failed has a line and no turn, and its
        # prompt is the longest, so the peak must be among the sizes rather
        # than their maximum.
        if (sizes[0] != ctx["first_prompt_bytes"] or ctx["peak_prompt_bytes"] not in sizes
                or ctx["turns_measured"] > len(sizes)):
            rows.append((task, None, f"log bytes disagree with trail context {ctx}"))
            continue
        counted = sum(n for _, n in steps)
        if served is None:
            rows.append((task, (sum(b * n for b, n in steps), tokens, counted, 0), ""))
            continue
        whole = [n for n, cut in served if not cut]
        cut = len(served) - len(whole)
        if tokens != sum(whole) or counted != len(whole):
            rows.append((task, None,
                         f"trail {tokens} tokens over {counted} requests; runtime log "
                         f"{sum(whole)} over {len(whole)} untruncated, {cut} truncated"))
            continue
        # The counted attempts and the untruncated requests are the same
        # requests in the same order (the one uncounted attempt, when there
        # is one, is the run's last and was truncated), so they pair.
        sent = [b for b, n in steps for _ in range(n)]
        pairs.extend(zip(sent, whole))
        firsts.append((sent[0], whole[0]))
        rows.append((task, (sum(b * n for b, n in steps), tokens, counted, cut), ""))

    used = [r for _, r, _ in rows if r]
    for task, r, why in rows:
        if r:
            b, t, n, cut = r
            note = f"  ({cut} truncated, uncounted)" if cut else ""
            print(f"{task}  requests {n:>3}  bytes {b:>9}  tokens {t:>8}  "
                  f"{b / t:.3f} bytes/token{note}")
        else:
            print(f"{task}  excluded: {why}")
    if not used:
        print("no run passes both checks")
        return 1
    per_run = sorted(r[0] / r[1] for r in used)
    b_sum, t_sum = sum(r[0] for r in used), sum(r[1] for r in used)
    pooled = b_sum / t_sum
    mid = len(per_run) // 2
    median = per_run[mid] if len(per_run) % 2 else (per_run[mid - 1] + per_run[mid]) / 2
    print()
    print(f"runs used {len(used)} of {len(rows)}; requests {sum(r[2] for r in used)}")
    print(f"pooled {b_sum} bytes / {t_sum} tokens = {pooled:.3f} bytes per token")
    print(f"per run min {per_run[0]:.3f}  median {median:.3f}  max {per_run[-1]:.3f}")
    if not runtime_log:
        print("no runtime log was checked: the request counts are the trail's and the "
              "adapter log's alone")
        print("per-request figures are not available: they pair each attempt with the "
              "runtime log's token count for it")
        return 0
    each = sorted(b / t for b, t in pairs)
    first = sum(b for b, _ in firsts) / sum(t for _, t in firsts)
    print(f"per request ({len(each)}) min {each[0]:.3f}  median {each[len(each) // 2]:.3f}  "
          f"max {each[-1]:.3f}")
    print(f"first prompts ({len(firsts)}) pooled {first:.3f}")
    print(f"4,096 tokens x {pooled:.3f} = {int(4096 * pooled)} bytes (rounded down)")
    print(f"4,096 tokens x {each[0]:.3f} (lowest request) = {int(4096 * each[0])} bytes")
    return 0


# --- self-test ---------------------------------------------------------------

# Two runs, counted by hand. t00 is the hosted adapter's shape: one
# `prompt_bytes=` line per attempt. Step 0 is retried once (1000 + 1000);
# step 1's first attempt is refused as too long and uncounted, its elided
# retry counts (1200); step 2's only attempt ends `hosted call failed`,
# uncounted. 3200 bytes over 3 attempts. The INFO `turn` line repeats
# `prompt_bytes` and must not be read as a request.
SELFTEST_HOSTED = """\
2026-10-06T10:00:00Z DEBUG run_seeded: prompt_bytes=1000 turns=0
2026-10-06T10:00:01Z  WARN run_seeded: attempt=1 error=no call no usable tool call; retrying
2026-10-06T10:00:01Z DEBUG run_seeded: prompt_bytes=1000 turns=0
2026-10-06T10:00:02Z  INFO run_seeded: turn turn=1 prompt_bytes=1000 model_ms=900
2026-10-06T10:00:02Z DEBUG run_seeded: prompt_bytes=1500 turns=1
2026-10-06T10:00:03Z  WARN run_seeded: bytes=1500 budget=1125 context overflow; eliding and retrying
2026-10-06T10:00:03Z DEBUG run_seeded: prompt_bytes=1200 turns=1
2026-10-06T10:00:04Z  INFO run_seeded: turn turn=2 prompt_bytes=1200 model_ms=800
2026-10-06T10:00:04Z DEBUG run_seeded: prompt_bytes=1400 turns=2
NOT CONVERGED after 2 iterations: hosted call failed: connection reset
"""
# t01 is the runtime adapter's: one line per step, each retry warning an
# attempt more on the same prompt (900 x 2), and `runtime call failed` on
# step 1's second attempt, uncounted (1100 x 1). 2900 bytes over 3 attempts.
SELFTEST_RUNTIME = """\
2026-10-06T10:01:00Z DEBUG run_seeded: prompt_bytes=900 turns=0
2026-10-06T10:01:01Z  WARN run_seeded: attempt=1 invalid tool call; retrying
2026-10-06T10:01:02Z  INFO run_seeded: turn turn=1 prompt_bytes=900
2026-10-06T10:01:02Z DEBUG run_seeded: prompt_bytes=1100 turns=1
2026-10-06T10:01:03Z  WARN run_seeded: attempt=1 malformed tool call; retrying
NOT CONVERGED after 1 iterations: runtime call failed: connection refused
"""
# (6100 bytes) / (1000 + 700 tokens), from the counts above.
SELFTEST_POOLED = "pooled 6100 bytes / 1700 tokens = 3.588 bytes per token"


def self_test() -> int:
    runs = {
        "t00": (SELFTEST_HOSTED, 1000, {"first_prompt_bytes": 1000,
                                         "peak_prompt_bytes": 1200, "turns_measured": 2}),
        "t01": (SELFTEST_RUNTIME, 700, {"first_prompt_bytes": 900,
                                         "peak_prompt_bytes": 1100, "turns_measured": 1}),
    }
    with tempfile.TemporaryDirectory() as tmp:
        floor = Path(tmp)
        for task, (log, tokens, ctx) in runs.items():
            (floor / f"{task}.log").write_text(log, encoding="utf-8")
            (floor / f"{task}-20261006-100000-000-selftest.json").write_text(
                json.dumps({"factual": {"prompt_tokens": tokens, "context": ctx}}),
                encoding="utf-8")
        # No `ollama-requests.txt` is written: reading one would fail the run.
        buf = io.StringIO()
        with contextlib.redirect_stdout(buf):
            code = report(floor, runtime_log=False)
    out = buf.getvalue()
    print(out, end="")
    want = [
        "t00  requests   3  bytes      3200  tokens     1000  3.200 bytes/token",
        "t01  requests   3  bytes      2900  tokens      700  4.143 bytes/token",
        SELFTEST_POOLED,
        "no runtime log was checked",
        "per-request figures are not available",
    ]
    missing = [w for w in want if w not in out]
    failed = int(code != 0 or bool(missing))
    if failed:
        print(f"FAIL: exit {code}, missing {missing}")
    print("OK: bytes per token self-test" if not failed else "FAIL: bytes per token self-test")
    return failed


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("floor", nargs="?")
    ap.add_argument("--no-runtime-log", action="store_true")
    ap.add_argument("--self-test", action="store_true")
    a = ap.parse_args()
    if a.self_test:
        return self_test()
    if not a.floor:
        ap.error("a floor directory is required")
    return report(Path(a.floor), runtime_log=not a.no_runtime_log)


if __name__ == "__main__":
    sys.exit(main())
