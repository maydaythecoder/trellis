#!/usr/bin/env python3
"""Run the guard cases: the behaviour that must not depend on a model.

Eleven guards: refusals, scripted loops and trail checks (see
qa/fixtures/behaviour.json). These rows are deterministic, so unlike the
model cases they can be quoted from a single run — and if one of them ever
changes, something structural has moved rather than a weight having a bad day.

The refusals are the ones worth having. A run that starts on top of uncommitted
work cannot later tell that work from its own, and a run that cannot reach its
runtime should cost nothing at all. Both are claims the design makes about what
it will never do, which is exactly the kind of claim that rots silently.

Usage:
    python qa/run-guards.py [--repo PATH]
"""

import argparse
import json
import pathlib
import subprocess
import sys

# qa/demo_fixture.py is imported from qa/; a __pycache__ left beside it would
# be an untracked directory in this repository after every run.
sys.dont_write_bytecode = True

from demo_fixture import FixtureLock, fixture_repo, reset, resolve_model

HERE = pathlib.Path(__file__).resolve().parent


def sh(args, cwd, timeout=300):
    return subprocess.run(
        args, cwd=cwd, capture_output=True, text=True, timeout=timeout,
        encoding="utf-8", errors="replace",
    )


def trails(repo):
    d = pathlib.Path(repo) / ".trellis" / "runs"
    return {p.name for p in d.glob("*.json")} if d.is_dir() else set()


def main():
    ap = argparse.ArgumentParser()
    # Default: a fresh clone of qa/fixtures/trellis-demo.bundle, removed at
    # exit (qa/demo_fixture.py). A path given here is used as it is.
    ap.add_argument("--repo", default=None)
    ap.add_argument("--fixture", default=str(HERE / "fixtures" / "behaviour.json"))
    args = ap.parse_args()

    spec = json.loads(pathlib.Path(args.fixture).read_text(encoding="utf-8"))
    commit = spec["_baseline_commit"]
    with fixture_repo(args.repo, commit) as repo:
        return run(spec, repo, commit)


def run(spec, repo, commit):
    print(f"fixture   {repo}")
    print(f"baseline  {commit}")
    print(f"guards    {len(spec['guards'])}\n")

    with FixtureLock(repo):
        rows = []
        for g in spec["guards"]:
            reset(repo, commit)
            exp = g["expect"]

            # A precondition the case needs in place before the run.
            if g.get("setup") == "dirty":
                p = repo / "app" / "calc.py"
                p.write_text(p.read_text(encoding="utf-8") + "\n# a person's work\n",
                             encoding="utf-8")

            # A trail that has been edited since it was written. The digest
            # covers the factual section (ADR 0007), and a trail is read back
            # as untrusted data (S10) — so the one thing `trellis log` must not
            # do is present an altered record as an intact one.
            if g.get("setup") == "tampered_trail":
                sh(["trellis", "run", "--model", resolve_model(g["seed_model"]),
                    "seed a trail"], repo)
                newest = max(
                    (repo / ".trellis" / "runs").glob("*.json"),
                    key=lambda f: f.stat().st_mtime,
                )
                d = json.loads(newest.read_text(encoding="utf-8"))
                # Change a fact, leave the digest alone. This is what an edit
                # by hand looks like, and what the seal exists to catch.
                d["factual"]["proposed"] = ["something/else.py"]
                newest.write_text(json.dumps(d, indent=2), encoding="utf-8")

            # A converged run's trail, with the worktree put back clean so the
            # next run may start. The trail commit survives; the edits do not.
            if g.get("setup") == "resumable":
                sh(["trellis", "run", "--model", resolve_model(g["seed_model"]),
                    "fix the annotation"], repo)
                sh(["git", "checkout", "--", "."], repo)

            before = trails(repo)
            cmd = ["trellis"] + g.get("command", ["run"])
            if g.get("model"):
                cmd += ["--model", resolve_model(g["model"])]
            if g.get("task"):
                cmd.append(g["task"])
            p = sh(cmd, repo)
            out = p.stdout + p.stderr
            after = trails(repo)

            checks = {}
            if exp.get("refused"):
                checks["refused"] = p.returncode != 0
            if "says" in exp:
                checks["says"] = exp["says"].lower() in out.lower()
            # `in exp` rather than a truthiness test: `converges: false` is the
            # expectation of half these cases, and a truthy check silently
            # skipped it — a guard that asserts nothing passes every time.
            if "converges" in exp:
                converged = "CONVERGED after" in out and "NOT CONVERGED" not in out
                checks["converged" if exp["converges"] else "did not converge"] = (
                    converged == exp["converges"]
                )
                if exp["converges"] and "iterations" in exp:
                    checks["iterations"] = f"CONVERGED after {exp['iterations']} iterations" in out
            if "writes" in exp and not exp["writes"]:
                checks["wrote nothing"] = "files written: none" in out
            if exp.get("leaves_dirty"):
                dirty = sh(["git", "status", "--porcelain"], repo).stdout
                # The point is not merely that it is dirty but that the person's
                # own line is still there, untouched.
                kept = "a person's work" in (repo / "app" / "calc.py").read_text(encoding="utf-8")
                checks["left the work alone"] = bool(dirty.strip()) and kept
            if exp.get("writes_trail") is False:
                checks["wrote no trail"] = before == after

            reset(repo, commit)
            ok = all(checks.values())
            rows.append({"id": g["id"], "match": ok, "checks": checks})
            print(f"  {'match   ' if ok else 'MISMATCH'} {g['id']:<16} "
                  + "  ".join(f"{k}={'y' if v else 'N'}" for k, v in checks.items()))

        print()
        matched = sum(1 for r in rows if r["match"])
        print(f"{matched}/{len(rows)} guards held")
        (HERE / "guards-latest.json").write_text(json.dumps(rows, indent=2), encoding="utf-8")
        return 0 if matched == len(rows) else 1


if __name__ == "__main__":
    sys.exit(main())
