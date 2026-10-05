"""The trellis-demo fixture, from the bundle vendored in this repository.

Shared by qa/run-guards.py and qa/run-behaviour.py. Both used to default to
the fixture's path on the owner's machine, and trellis-demo has no remote, so
on any other machine the suites had nothing to run against. The fixture's
history through the baseline commit is vendored as a git bundle
(qa/fixtures/trellis-demo.bundle, ADR 0027 row #32) and cloned afresh for each
run.
"""

import contextlib
import hashlib
import os
import pathlib
import shutil
import stat
import subprocess
import sys
import tempfile

HERE = pathlib.Path(__file__).resolve().parent
FIXTURES = HERE / "fixtures"
BUNDLE = FIXTURES / "trellis-demo.bundle"


def resolve_model(spec):
    """A `scripted:` spec with a relative path, made absolute against qa/fixtures/.

    behaviour.json names each script by its path under qa/fixtures/ so that the
    file holds no machine's home directory. `trellis run` resolves a relative
    path against its own working directory, which is the fixture clone, where
    the scripts are not — so the runner makes it absolute here. Forward slashes,
    because that is the form every recorded run used on Windows.
    """
    if not spec or not spec.startswith("scripted:"):
        return spec
    path = pathlib.Path(spec[len("scripted:"):])
    if path.is_absolute():
        return spec
    return "scripted:" + (FIXTURES / path).resolve().as_posix()


def _git(args, cwd):
    p = subprocess.run(
        ["git", *args], cwd=cwd, capture_output=True, text=True,
        encoding="utf-8", errors="replace",
    )
    if p.returncode != 0:
        raise SystemExit(
            f"git {' '.join(args)} failed ({p.returncode}) preparing the fixture:\n"
            f"{p.stdout}{p.stderr}"
        )


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


def reset(repo, commit):
    """Back to the baseline, keeping the trails this suite is about to read."""
    for args in (["reset", "--hard", commit], ["clean", "-fd", "--", "app", "tests"]):
        subprocess.run(
            ["git", *args], cwd=repo, capture_output=True, text=True, timeout=300,
            encoding="utf-8", errors="replace",
        )


def _force_remove(func, path, _exc):
    # git writes its object files read-only; on Windows rmtree cannot delete a
    # read-only file until the bit is cleared.
    os.chmod(path, stat.S_IWRITE)
    func(path)


@contextlib.contextmanager
def fixture_repo(repo_arg, commit):
    """The fixture repository for one run.

    With `--repo PATH`, that directory, used as it is and never removed.
    Without, a fresh clone of the vendored bundle with `master` checked out at
    `commit`, deleted on exit — including an exit through SystemExit. The
    fixture lock only matters with a shared `--repo PATH`: a default fresh
    clone is private to its run, so no other run can reset it.
    """
    if repo_arg is not None:
        yield pathlib.Path(repo_arg)
        return
    if not BUNDLE.is_file():
        raise SystemExit(f"no fixture: {BUNDLE} is missing and --repo was not given")
    tmp = pathlib.Path(tempfile.mkdtemp(prefix="trellis-demo-"))
    try:
        repo = tmp / "trellis-demo"
        _git(["clone", "-q", str(BUNDLE), str(repo)], tmp)
        # On a branch, as the owner's fixture is: trail commits land on it and
        # `git reset --hard <commit>` between cases moves it back.
        _git(["checkout", "-q", "-B", "master", commit], repo)
        # Guards that seed a trail commit. A machine with no global identity
        # (a CI runner) would otherwise fail those setups for a reason that has
        # nothing to do with the harness. Local to the throwaway clone.
        _git(["config", "user.name", "demo"], repo)
        _git(["config", "user.email", "demo@example.invalid"], repo)
        # The owner's fixture has no remote; neither does this one.
        _git(["remote", "remove", "origin"], repo)
        yield repo
    finally:
        if sys.version_info >= (3, 12):
            shutil.rmtree(tmp, onexc=_force_remove)
        else:
            shutil.rmtree(tmp, onerror=_force_remove)
