"""Runs the S7 model-floor measurement over the verified task set.

Each task injects a defect the checker catches, then asks the loop to repair
it. The primary metric is **check-recovery**: given a failing check, does the
run reach a passing one? Everything else about a harness rests on the model
acting sensibly on check output, so a model that cannot is unusable here
regardless of how it scores on anything else.

The floor is reported as a **configuration** — model x quantization x runtime —
never as a parameter count. "7B minimum" tells a user nothing: a well-quantized
14B on a good runtime beats a badly-quantized 32B.

Outcomes are tallied separately rather than reduced to one percentage, because
they fail for different reasons and need different fixes:

  repaired             converged, and the injected defect is genuinely gone
  converged-unrepaired converged, but the defect is still there — see below
  no-progress          the same call and result three times over; the loop
                       stopped rather than spend the rest of the budget
  budget-exhausted     ran out of iterations or wall-clock while still varying
  no-recovery          ended without a passing check for some other reason
  format-failure       no valid tool call survived the adapter's retries
  error                the harness itself failed

`no-progress` and `budget-exhausted` are kept apart deliberately. A model that
repeated itself three times and one that worked steadily until the ceiling are
both failures and they are not the same failure, and folding them together
would hide which one a configuration produces.

**Harness version matters for the cost columns.** The no-progress stop landed
after the first configuration was measured, so wall-clock and iteration counts
are comparable only within a single harness version. `repaired` is unaffected:
the stop changes how quickly a failing run gives up, never whether a task is
fixed.

A run that never produces a valid tool call yields 0/0 on check-recovery, not
0%. Reporting that as a rate would invent a denominator.

**`converged-unrepaired` is why this script re-checks rather than trusting the
run.** `trellis run` converges when the *most recent executed check* passed,
and the model chooses which paths that check covers. A model that edits
`a.py` and then checks `b.py` gets a truthful passing verdict about the wrong
file. That is a TRELLIS defect rather than a model one, and collapsing it into
"converged" would hide it — so every converged run is re-checked here with the
reference checker from `floor_tasks.json`, on the file the defect was injected
into, by this script rather than by anything under measurement.

Usage:
  python docs/bench/run_floor.py <corpus> <tasks.json> <model-spec> <out.json>
                                 [--limit N] [--max-iterations N]
"""

from __future__ import annotations

import argparse
import io
import json
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

TRELLIS = str(Path("target/release/trellis.exe").resolve())


def git(repo: Path, *args: str) -> subprocess.CompletedProcess:
    return subprocess.run(
        ["git", *args], cwd=repo, capture_output=True, text=True, timeout=300
    )


def ensure_repo(corpus: Path) -> None:
    """The corpus must be a git repository: `trellis run` refuses to start on a
    dirty worktree and snapshots HEAD before it writes, and that snapshot is
    what makes a failed task recoverable."""
    if not (corpus / ".gitignore").exists():
        (corpus / ".gitignore").write_text(
            # Caches must stay untracked or every reset churns thousands of
            # files; they must also stay out of `clean -fd`'s way, which is
            # what .gitignore buys (clean without -x leaves ignored files).
            ".mypy_cache/\n__pycache__/\n*.pyc\n.trellis/index.bin\n",
            encoding="utf-8",
        )
    if (corpus / ".git").exists():
        return
    git(corpus, "init", "-q")
    git(corpus, "config", "user.email", "floor@example.invalid")
    git(corpus, "config", "user.name", "floor")
    # Line endings must not change under us: a defect anchor is matched
    # byte-for-byte, and autocrlf would rewrite files on checkout.
    git(corpus, "config", "core.autocrlf", "false")
    git(corpus, "add", "-A")
    git(corpus, "commit", "-qm", "floor baseline")
    print(f"initialised {corpus} as a git repository")


def head(corpus: Path) -> str:
    return git(corpus, "rev-parse", "HEAD").stdout.strip()


def reset_to(corpus: Path, base: str) -> None:
    """Back to `base`, not to HEAD.

    Each task commits its defect, and a converged run commits its trail, so
    HEAD advances as the suite runs. Resetting to HEAD would leave task N's
    defect in the tree while task N+1's was applied on top, and by task 10 the
    checker would be reporting ten unrelated errors — every result after the
    first quietly measuring a different corpus.
    """
    git(corpus, "reset", "--hard", "-q", base)
    git(corpus, "clean", "-qfd")


def apply_defect(corpus: Path, task: dict) -> bool:
    """Applies the mutation exactly as `make_floor_tasks.py` verified it.

    The corpus is CRLF — poetry's sdist, unpacked on Windows — while every
    anchor in `floor_tasks.json` is LF, because the task set was built with
    `Path.read_text`, which translates on the way in. Matching the anchor
    against the raw bytes finds nothing, and every task silently skips: the
    suite reports twenty skips and zero failures, which reads like a clean
    run of nothing.

    So read and write through the same universal-newline translation the task
    set was built with. `write_text` converts back to the platform ending, so
    the file keeps its CRLF and the commit diff is the defect alone.
    """
    path = corpus / task["file"]
    src = path.read_text(encoding="utf-8")
    if task["defect_old"] not in src:
        return False
    path.write_text(src.replace(task["defect_old"], task["defect_new"], 1),
                    encoding="utf-8")
    return True


def reference_check(corpus: Path, checker: list[str], rel_file: str) -> tuple[int, str]:
    """Runs the task set's own checker, from the corpus root.

    Both details matter. The checker string is the one `make_floor_tasks.py`
    used to verify that every defect is caught, so any other invocation would
    be scoring against a different oracle. And mypy's module resolution depends
    on the working directory: the same file checked from the workspace root
    reports two spurious `attr-defined` errors that vanish when checked from
    the corpus root.
    """
    p = subprocess.run(
        [*checker, rel_file], cwd=corpus, capture_output=True, text=True, timeout=300
    )
    errors = [l for l in p.stdout.splitlines() if ": error:" in l]
    return len(errors), "\n".join(errors[:3])


def classify(out: str, code: int, repaired: bool | None) -> str:
    converged = "CONVERGED after" in out and "NOT CONVERGED" not in out
    if converged:
        return "repaired" if repaired else "converged-unrepaired"
    if "no valid tool call after" in out:
        return "format-failure"
    # `NotConverged::explain()`'s wording, not the enum's name.
    #
    # `no-progress` is checked before `budget-exhausted` because it is the
    # more specific finding: the loop stopped because the model repeated
    # itself, rather than because it ran out of room to keep trying. Folding
    # the two together would report a model that gave up immediately and one
    # that worked steadily until the ceiling as the same outcome.
    if "no progress: the same call" in out:
        return "no-progress"
    if "ceiling reached" in out:
        return "budget-exhausted"
    if "NOT CONVERGED" in out:
        return "no-recovery"
    return "error"


def turn_sequence(out: str) -> list[str]:
    """The per-turn lines `trellis run` prints, condensed to tool names.

    Read from the harness's own output rather than from the model's account of
    what it did — the whole point of the trail split is that the second is
    speculation.
    """
    seq = []
    for line in out.splitlines():
        m = re.match(r"\s*\d+\.\s+(\w+)", line)
        if m:
            mark = "+" if "check passed" in line else ("-" if "check FAILED" in line else "")
            seq.append(m.group(1) + mark)
    return seq


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("corpus")
    ap.add_argument("tasks")
    ap.add_argument("model")
    ap.add_argument("out")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--max-iterations", type=int, default=8)
    ap.add_argument("--max-wall-clock-s", type=int, default=1800)
    a = ap.parse_args()

    corpus = Path(a.corpus).resolve()
    spec = json.loads(Path(a.tasks).read_text(encoding="utf-8"))
    tasks = spec["tasks"][: a.limit] if a.limit else spec["tasks"]
    checker = spec["checker"].split()
    # S20: the reference checker must defeat the incremental cache too. Several
    # of these defects are length-preserving, and a reset restores the original
    # with a fresh mtime — exactly the case where mypy reports a cached verdict
    # about a file it did not read.
    if "--no-incremental" not in checker:
        checker.append("--no-incremental")

    out_dir = Path(a.out).with_suffix("")
    out_dir.mkdir(parents=True, exist_ok=True)

    ensure_repo(corpus)
    cfg = corpus / ".trellis" / "config.toml"
    cfg.parent.mkdir(parents=True, exist_ok=True)
    cfg.write_text(
        # The checker is the task set's, verbatim, so the loop and the scoring
        # ask the same oracle the same question.
        "[checks]\ntypecheck = {}\n".format(json.dumps(checker))
        + 'tests = ["python", "-m", "pytest", "-q", "-x", "-p", "no:cacheprovider"]\n'
        + 'python = "python"\n\n'
        # A tight budget, because an exhausted budget is itself a result and a
        # slow model must not turn the measurement into an overnight job. The
        # ceiling is ~2x the shortest honest path (check, read, edit, check),
        # so a capable model has room and a looping one is cut off. The loop
        # grants exactly one further turn on exhaustion.
        + f"[budget]\nmax_iterations = {a.max_iterations}\n"
        + f"max_wall_clock_s = {a.max_wall_clock_s}\nregime = \"local\"\n",
        encoding="utf-8",
    )
    git(corpus, "add", "-A")
    git(corpus, "commit", "-qm", "floor config")
    base = head(corpus)
    print(f"base {base[:12]}  checker {' '.join(checker[:2])}...  "
          f"budget {a.max_iterations} iters / {a.max_wall_clock_s}s")

    results = []
    for i, task in enumerate(tasks, 1):
        reset_to(corpus, base)

        # The file's error count *before* the defect, which is not always
        # zero: t09 is a verified `1 -> 5`, so a perfect repair on that task
        # leaves one error standing. Scoring repair as `after == 0` would mark
        # it unrepaired however well the model did. Measured on this run's own
        # clean tree rather than parsed out of `detected_by`.
        baseline, _ = reference_check(corpus, checker, task["file"])

        if not apply_defect(corpus, task):
            print(f"[{i}/{len(tasks)}] {task['id']} SKIPPED (anchor absent)")
            results.append({"id": task["id"], "outcome": "skipped"})
            continue

        # And the count with the defect in, so a task scoring as repaired
        # cannot be one that was never broken.
        before, _ = reference_check(corpus, checker, task["file"])
        if before <= baseline:
            print(f"[{i}/{len(tasks)}] {task['id']} SKIPPED (defect not detected: "
                  f"{baseline} -> {before})")
            results.append({"id": task["id"], "outcome": "skipped"})
            continue

        git(corpus, "add", "-A")
        git(corpus, "commit", "-qm", f"defect {task['id']}")

        started = time.time()
        try:
            p = subprocess.run(
                [TRELLIS, "run", "--root", str(corpus), "--model", a.model,
                 f"a type check is failing in {task['file']}; fix it"],
                capture_output=True, text=True, timeout=a.max_wall_clock_s + 900,
            )
            out, code = p.stdout + p.stderr, p.returncode
        except subprocess.TimeoutExpired as e:
            out = (e.stdout or b"").decode("utf-8", "replace") + "\n[harness timeout]"
            code = -1
        elapsed = time.time() - started

        converged = "CONVERGED after" in out and "NOT CONVERGED" not in out
        after, detail = reference_check(corpus, checker, task["file"])
        # Back to where the file started, not to zero.
        repaired = converged and after <= baseline
        outcome = classify(out, code, repaired)

        # The trail is the run's own record and it is committed, so it survives
        # the reset only if copied out first.
        runs = corpus / ".trellis" / "runs"
        trail = None
        if runs.is_dir():
            files = sorted(runs.glob("*.json"))
            if files:
                dst = out_dir / f"{task['id']}-{files[-1].name}"
                shutil.copy2(files[-1], dst)
                trail = dst.name

        m = re.search(r"(?:NOT )?CONVERGED after (\d+)", out)
        results.append({
            "id": task["id"],
            "file": task["file"],
            "kind": task["kind"],
            "outcome": outcome,
            "seconds": round(elapsed, 1),
            "iterations": int(m.group(1)) if m else 0,
            "errors_baseline": baseline,
            "errors_before": before,
            "errors_after": after,
            "turns": turn_sequence(out),
            "trail": trail,
            "residual": detail if after else "",
        })
        (out_dir / f"{task['id']}.log").write_text(out, encoding="utf-8")
        print(f"[{i}/{len(tasks)}] {task['id']:>4} {outcome:<21} {elapsed:6.1f}s  "
              f"errors {baseline}|{before}->{after}  "
              f"{' '.join(results[-1]['turns'][:6])}")

    reset_to(corpus, base)
    tally: dict[str, int] = {}
    for r in results:
        tally[r["outcome"]] = tally.get(r["outcome"], 0) + 1

    ran = [r for r in results if r["outcome"] != "skipped"]
    secs = sorted(r.get("seconds", 0) for r in ran)
    report = {
        "configuration": a.model,
        "corpus": corpus.name,
        "checker": " ".join(checker),
        "budget": {"max_iterations": a.max_iterations,
                   "max_wall_clock_s": a.max_wall_clock_s},
        "tasks_attempted": len(ran),
        "outcomes": tally,
        "check_recovery": {
            "repaired": tally.get("repaired", 0),
            "attempted": len(ran),
            "note": (
                "A task is repaired when its file's error count returns to the "
                "baseline measured on the clean tree immediately before the "
                "defect was applied — not to zero, since some files carry a "
                "pre-existing error (t09 is a verified 1->5). Each defect "
                "raises the count by a verified amount, between 1 and 5. "
                "Scored by re-running the reference checker here, never by "
                "reading the run's own verdict."
            ),
        },
        "wall_clock_s": {
            "total": round(sum(secs), 1),
            "median": secs[len(secs) // 2] if secs else 0,
            "note": (
                "This rig re-prefills the whole transcript each turn, so "
                "per-turn cost grows with the transcript. That is a property "
                "of the probe server, not of TRELLIS."
            ),
        },
        "floor_note": (
            "A floor is a configuration — model, quantization, runtime — not a "
            "parameter count. Outcomes are tallied separately because they "
            "fail for different reasons; a run with no valid tool call yields "
            "0/0 on check-recovery, not 0%."
        ),
        "results": results,
    }
    Path(a.out).write_text(json.dumps(report, indent=1), encoding="utf-8")
    print()
    print(json.dumps({k: report[k] for k in
                      ("configuration", "tasks_attempted", "outcomes",
                       "check_recovery", "wall_clock_s")}, indent=1))
    print(f"\nreport {a.out}   logs and trails {out_dir}/")
    return 0


if __name__ == "__main__":
    sys.exit(main())
