#!/usr/bin/env python3
"""Run the guard cases: the behaviour that must not depend on a model.

Two refusals and one scripted loop. These rows are deterministic, so unlike the
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
import hashlib
import os
import pathlib
import subprocess
import sys
import tempfile

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


def sh(args, cwd, timeout=300):
    return subprocess.run(
        args, cwd=cwd, capture_output=True, text=True, timeout=timeout,
        encoding="utf-8", errors="replace",
    )


def trails(repo):
    d = pathlib.Path(repo) / ".trellis" / "runs"
    return {p.name for p in d.glob("*.json")} if d.is_dir() else set()


def reset(repo, commit):
    sh(["git", "reset", "--hard", commit], repo)
    sh(["git", "clean", "-fd", "--", "app", "tests"], repo)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--repo", default=r"C:\Users\mayda\documents\dev\trellis-demo")
    ap.add_argument("--fixture", default=str(HERE / "fixtures" / "behaviour.json"))
    args = ap.parse_args()

    spec = json.loads(pathlib.Path(args.fixture).read_text(encoding="utf-8"))
    repo, commit = pathlib.Path(args.repo), spec["_baseline_commit"]

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
                sh(["trellis", "run", "--model", g["seed_model"], "seed a trail"], repo)
                newest = max(
                    (repo / ".trellis" / "runs").glob("*.json"),
                    key=lambda f: f.stat().st_mtime,
                )
                d = json.loads(newest.read_text(encoding="utf-8"))
                # Change a fact, leave the digest alone. This is what an edit
                # by hand looks like, and what the seal exists to catch.
                d["factual"]["proposed"] = ["something/else.py"]
                newest.write_text(json.dumps(d, indent=2), encoding="utf-8")

            before = trails(repo)
            cmd = ["trellis"] + g.get("command", ["run"])
            if g.get("model"):
                cmd += ["--model", g["model"]]
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
