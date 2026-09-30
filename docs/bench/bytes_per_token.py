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

Usage:
  python docs/bench/bytes_per_token.py <floor-dir>

Prints one row per run and the pooled ratio (bytes over tokens, summed over
the runs used), with the per-run minimum, median and maximum beside it, the
same per request (each counted attempt paired with the runtime's count for
it), and the first prompts pooled. The pooled figure weights each request by
its size, which is what a byte budget converts; the lowest per-request figure
is printed beside it because a pooled ratio is not a bound on every prompt.
"""

from __future__ import annotations

import json
import re
import sys
from pathlib import Path

ANSI = re.compile(r"\x1b\[[0-9;]*m")
# The adapter's event is the only DEBUG line carrying both fields; the loop's
# INFO `turn` event repeats `prompt_bytes` without `turns`. The log does not
# print targets, so the pair is what identifies it.
BYTES = re.compile(r"\bDEBUG\b.*\bprompt_bytes=(\d+) turns=\d+")
RETRY = re.compile(r"(?:malformed|invalid) tool call; retrying")
# Both end the run inside a step whose last attempt's tokens were not counted.
UNCOUNTED_LAST = ("no valid tool call after", "runtime call failed")
REQUEST = re.compile(r"new prompt, n_ctx_slot = \d+, .*task\.n_tokens = (\d+)")
TRUNCATED = "truncating input prompt"
PROBE_TOKENS = 34


def requests(log: str) -> list[tuple[int, int]] | None:
    """(bytes, attempts counted) per `next_step`, or None when there are none."""
    steps: list[list[int]] = []
    for line in ANSI.sub("", log).splitlines():
        m = BYTES.search(line)
        if m:
            steps.append([int(m.group(1)), 1])
        elif RETRY.search(line) and steps:
            steps[-1][1] += 1
    if not steps:
        return None
    if any(s in log for s in UNCOUNTED_LAST):
        steps[-1][1] -= 1
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


def main() -> int:
    floor = Path(sys.argv[1])
    trails = sorted(floor.glob("t*-*.json"))
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
    each = sorted(b / t for b, t in pairs)
    first = sum(b for b, _ in firsts) / sum(t for _, t in firsts)
    print(f"per request ({len(each)}) min {each[0]:.3f}  median {each[len(each) // 2]:.3f}  "
          f"max {each[-1]:.3f}")
    print(f"first prompts ({len(firsts)}) pooled {first:.3f}")
    print(f"4,096 tokens x {pooled:.3f} = {int(4096 * pooled)} bytes (rounded down)")
    print(f"4,096 tokens x {each[0]:.3f} (lowest request) = {int(4096 * each[0])} bytes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
