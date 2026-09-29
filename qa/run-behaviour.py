#!/usr/bin/env python3
"""Run the behaviour cases and report matches against their expectations.

Each case is run from the same commit in the fixture repository, so runs do not
inherit each other's edits. The expectation was written before the run; a case
that disagrees with it is reported as a mismatch and **not** corrected in the
fixture, because an expectation that turned out wrong is a finding about what
this harness was believed to do.

Four facts are recorded per run and never collapsed into one score:

  converged   the loop reached a passing check and stopped on purpose
  writes      which files were left changed
  checks      which checks ran and whether each passed
  actions     what the agent did, in order, from the trail

`converged` is not a quality judgement. A run can converge on a change a person
would reject, and a run that correctly refuses an impossible task does not
converge. The table says which happened; it does not grade.

Usage:
    python qa/run-behaviour.py [--repeat 1] [--only id,id] [--repo PATH]
"""

import argparse
import json
import hashlib
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import time

# qa/demo_fixture.py is imported from qa/; a __pycache__ left beside it would
# be an untracked directory in this repository after every run.
sys.dont_write_bytecode = True

from demo_fixture import fixture_repo

HERE = pathlib.Path(__file__).resolve().parent


class FixtureLock:
    """One harness at a time against one fixture repository.

    Both runners reset the fixture with `git reset --hard` between cases. Run
    two at once and each silently pulls the ground from under the other: a
    guard case that sets up a dirty worktree finds it clean, refuses for the
    wrong reason, and reports a mismatch that says nothing about the harness
    under test.

    That happened, and it produced a false MISMATCH that looked exactly like a
    real defect. A baseline whose rows can be corrupted by another process is
    not a baseline, so this is a lock rather than a note.
    """

    def __init__(self, repo):
        # Outside the fixture, deliberately. The first version put the lock in
        # the repository's own `.trellis/`, where it was an untracked file —
        # so the worktree was never clean and every run refused to start. The
        # refusal cases still "passed", because they were being refused
        # anyway, for the wrong reason. A measurement harness must not be
        # visible to what it measures.
        key = hashlib.sha256(str(pathlib.Path(repo).resolve()).encode()).hexdigest()[:16]
        self.path = pathlib.Path(tempfile.gettempdir()) / f"trellis-qa-{key}.lock"

    def __enter__(self):
        self.path.parent.mkdir(parents=True, exist_ok=True)
        try:
            # Exclusive create: succeeds only if no one else holds it.
            fd = os.open(self.path, os.O_CREAT | os.O_EXCL | os.O_WRONLY)
        except FileExistsError:
            raise SystemExit(
                f"another qa run holds {self.path}.\n"
                "Two harnesses resetting one fixture corrupt each other's rows.\n"
                "Wait for it, or delete the file if it was left by a killed run."
            )
        os.write(fd, str(os.getpid()).encode())
        os.close(fd)
        return self

    def __exit__(self, *_):
        self.path.unlink(missing_ok=True)
        return False


def sh(args, cwd, timeout=600):
    return subprocess.run(
        args, cwd=cwd, capture_output=True, text=True, timeout=timeout, encoding="utf-8",
        errors="replace",
    )


def reset(repo, commit):
    """Back to the baseline, keeping the trails this suite is about to read."""
    sh(["git", "reset", "--hard", commit], repo)
    sh(["git", "clean", "-fd", "--", "app", "tests"], repo)


def parse_run(out):
    """What the run said about itself."""
    converged = bool(re.search(r"^CONVERGED after", out, re.M))
    m = re.search(r"^(?:NOT )?CONVERGED after (\d+) iterations", out, re.M)
    iterations = int(m.group(1)) if m else None
    m = re.search(r"^files written: (.+)$", out, re.M)
    writes = []
    if m and m.group(1).strip() != "none":
        writes = [w.strip() for w in m.group(1).split(",") if w.strip()]
    m = re.search(r"^trail:\s+(\S.*?)(?: \(|$)", out, re.M)
    trail = m.group(1).strip() if m else None
    return converged, iterations, writes, trail


def parse_trail(path):
    """Actions and check outcomes, from the run's own record."""
    try:
        d = json.loads(pathlib.Path(path).read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError):
        return [], []
    f = d.get("factual", {})
    return f.get("actions") or [], f.get("check_outcomes") or []


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repeat", type=int, default=1)
    ap.add_argument("--only", default="")
    # Default: a fresh clone of qa/fixtures/trellis-demo.bundle, removed at
    # exit (qa/demo_fixture.py). A path given here is used as it is.
    ap.add_argument("--repo", default=None)
    ap.add_argument("--fixture", default=str(HERE / "fixtures" / "behaviour.json"))
    args = ap.parse_args()

    spec = json.loads(pathlib.Path(args.fixture).read_text(encoding="utf-8"))
    commit = spec["_baseline_commit"]
    cases = spec["cases"]
    if args.only:
        wanted = {c.strip() for c in args.only.split(",")}
        cases = [c for c in cases if c["id"] in wanted]

    with fixture_repo(args.repo, commit) as repo:
        return run(args, cases, repo, commit)


def run(args, cases, repo, commit):
    print(f"fixture   {repo}")
    print(f"baseline  {commit}")
    print(f"cases     {len(cases)} x {args.repeat}")
    print()

    with FixtureLock(repo):
        rows = []
        for case in cases:
            for run_n in range(args.repeat):
                reset(repo, commit)
                t0 = time.monotonic()
                try:
                    p = sh(["trellis", "run", case["task"]], repo)
                    out = p.stdout + p.stderr
                except subprocess.TimeoutExpired:
                    out = "TIMEOUT"
                elapsed = time.monotonic() - t0

                converged, iterations, writes, trail = parse_run(out)
                actions, checks = parse_trail(trail) if trail else ([], [])

                # Every run's own words, kept. The first suite produced a row
                # reading `iters=0 actions=0` after four minutes, and there was
                # nothing left to say why — the output had gone to a pipe and
                # the trail recorded a run that never took a turn. A row you
                # cannot explain is not yet a measurement.
                logs = HERE / "logs" / "behaviour"
                logs.mkdir(parents=True, exist_ok=True)
                (logs / f"{case['id']}-{run_n + 1}.log").write_text(out, encoding="utf-8")

                reset(repo, commit)

                exp = case["expect"]
                checks_pass = bool(checks) and all(c.get("passed") for c in checks)
                match_conv = converged == exp["converges"]
                # Expected writes are a subset check: a run that also touched a
                # file the expectation did not name is reported, not failed, since
                # "which files" is what this is measuring.
                match_writes = set(exp["writes"]).issubset(set(writes))
                rows.append(
                    {
                        "id": case["id"],
                        "run": run_n + 1,
                        "converged": converged,
                        "expected": exp["converges"],
                        "match": match_conv and match_writes,
                        "iterations": iterations,
                        "writes": writes,
                        "checks": [f"{c['kind']}{'+' if c['passed'] else '-'}" for c in checks],
                        "checks_pass": checks_pass,
                        "actions": len(actions),
                        "refused": sum(
                            1 for a in actions if a["tool"] == "edit" and not a["wrote"]
                        ),
                        "secs": round(elapsed, 1),
                    }
                )
                r = rows[-1]
                mark = "match   " if r["match"] else "MISMATCH"
                print(
                    f"  {mark} {r['id']:<12} conv={str(converged):<5} "
                    f"(expected {str(exp['converges']):<5}) iters={str(iterations):<4} "
                    f"actions={r['actions']:<2} refused={r['refused']:<2} "
                    f"writes={writes} checks={r['checks']} {r['secs']}s"
                )

        print()
        matched = sum(1 for r in rows if r["match"])
        print(f"{matched}/{len(rows)} matched their expectation")
        print()
        print("Mismatches are not failures of the harness or of the expectation")
        print("until one of the two is shown wrong. Each is a row to explain.")

        out_path = HERE / "behaviour-latest.json"
        out_path.write_text(json.dumps(rows, indent=2), encoding="utf-8")
        print(f"\nrows written to {out_path}")
        return 0


if __name__ == "__main__":
    sys.exit(main())
