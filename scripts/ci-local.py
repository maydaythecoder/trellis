"""Run .github/workflows/ci.yml on this machine, without GitHub.

    py -3 scripts/ci-local.py [--ref <commit>] [--job <name>]... [--keep-going]

Every job's `run:` steps execute in a clean checkout of <ref> (default HEAD):
Linux legs in WSL (`wsl -d Ubuntu`), Windows legs natively under Git Bash.
Jobs run one at a time, because the host also runs a local model and a
second build at once has run it out of memory.

`uses:` steps are not run. Each one is either replaced or already satisfied:
checkout is the clean checkout itself; the Rust toolchain comes from
rust-toolchain.toml through the installed rustup; rust-cache is skipped (it
only saves time, and its cleanup deleted the owner's toolchain on a
self-hosted runner, #158); setup-python becomes a virtualenv whose `python`
and `python3` come first on PATH. A `uses:` this script does not know fails
the job rather than being skipped silently.

Jobs get the owner's toolchain but none of the owner's state: a throwaway
GIT_CONFIG_GLOBAL (ci.yml runs `git config --global`), CARGO_INSTALL_ROOT
under the run directory (`cargo install` in supply-chain), no provider-key
variables, and OPENAI_BASE_URL pointed at a closed port so the live-model
tests skip as they do on a GitHub runner.

Each step's environment is the workflow's `env:`, then the job's, then the
step's own, later ones winning, as on GitHub. Step output is decoded as UTF-8
with replacement: a step that printed a byte the Windows locale codec leaves
undefined (cp1252 has five) once left an empty log behind an exit code of 0
(#223).

On a Linux host a Linux leg runs natively in a clean clone, and a Windows leg
fails; it is not skipped. CI_LOCAL_DIR, if set, replaces the run directory
(clone, logs, throwaway home) of native legs; the self-test
(scripts/checks/ci-local-selftest.sh) sets it so that it never touches the
clone a real run reuses.

Exit 0 only if every step of every selected job exited 0.
"""

import argparse
import os
import re
import shutil
import subprocess
import sys
import time
from pathlib import Path

import yaml

ROOT = Path(__file__).resolve().parent.parent
ON_WINDOWS = os.name == "nt"
# Short on Windows: the tree has paths near MAX_PATH.
WIN_DIR = Path(os.environ.get("CI_LOCAL_DIR")
               or ("C:/ci-local" if ON_WINDOWS else Path.home() / "ci-local"))
WSL_DIR = "$HOME/ci-local"
DISTRO = "Ubuntu"
KEYS = ["OPENAI_API_KEY", "ANTHROPIC_API_KEY", "GROQ_API_KEY", "GEMINI_API_KEY"]
KNOWN_USES = ("actions/checkout@", "dtolnay/rust-toolchain@", "Swatinem/rust-cache@",
              "actions/setup-python@")


# Every child's output is read with these. The default is the locale codec,
# cp1252 on this host, and the reader thread decoding it died on a byte cp1252
# leaves undefined: the log came out empty and the exit code still said 0
# (#223). Replacement keeps every line and never raises.
UTF8 = dict(text=True, encoding="utf-8", errors="replace")


def sh(cmd, **kw):
    return subprocess.run(cmd, check=True, capture_output=True, **UTF8, **kw).stdout.strip()


def git_bash():
    if not ON_WINDOWS:
        return shutil.which("bash") or sys.exit("bash not found")
    # `bash` on PATH can be System32\bash.exe, which is WSL's.
    for c in [r"C:\Program Files\Git\bin\bash.exe", shutil.which("bash") or ""]:
        if c and "system32" not in c.lower() and Path(c).exists():
            return c
    sys.exit("Git Bash not found")


def wsl(script):
    # `--exec`, not `--`: after `--`, wsl.exe hands the joined command line to
    # the user's login shell, which expands every `$VAR` in `script` before the
    # inner bash runs. `W=...; git clone ... $W` then cloned into an empty `$W`,
    # i.e. into the current directory, which was the Windows checkout (#166).
    # `--cd ~` so that a relative path, if one slips in again, lands in the WSL
    # home and not in whichever Windows directory this script was started from.
    env = dict(os.environ, MSYS_NO_PATHCONV="1")
    return subprocess.run(["wsl.exe", "-d", DISTRO, "--cd", "~", "--exec", "bash", "-lc", script],
                          capture_output=True, env=env, **UTF8)


def to_wsl(path):
    """C:/x/y -> /mnt/c/x/y, the drvfs mount of a Windows path."""
    p = Path(path).resolve()
    return f"/mnt/{p.drive[0].lower()}" + p.as_posix()[len(p.drive):]


def legs(jobs, only):
    """(job, platform) pairs in file order; a matrix over os becomes one leg per os."""
    out = []
    for name, job in jobs.items():
        if only and name not in only:
            continue
        oses = job.get("strategy", {}).get("matrix", {}).get("os")
        for os_ in (oses or ["ubuntu-latest"]):
            out.append((name, "windows" if "windows" in str(os_) else "linux", os_))
    return out


def prelude(env, platform, venv):
    lines = ["set -eo pipefail"]
    for k in KEYS:
        lines.append(f"unset {k}")
    for k, v in env.items():
        lines.append(f"export {k}='{v}'")
    home = "$CI_HOME"
    lines += [f'export GIT_CONFIG_GLOBAL="{home}/gitconfig"',
              f'export CARGO_INSTALL_ROOT="{home}/cargo-install"',
              f'export PATH="{home}/cargo-install/bin:$PATH"',
              "export OPENAI_BASE_URL=http://127.0.0.1:9/v1"]
    if platform == "linux":
        lines.append('export PATH="$HOME/.cargo/bin:$PATH"')
        # WSL does not inherit the Windows environment, so a job cap the
        # caller set for the shared host has to be carried across by hand.
        if os.environ.get("CARGO_BUILD_JOBS", "").isdigit():
            lines.append(f"export CARGO_BUILD_JOBS={os.environ['CARGO_BUILD_JOBS']}")
    if venv:
        lines.append(f'export PATH="{home}/venv/bin:$PATH"')
    return "\n".join(lines) + "\n"


def run_leg(name, platform, job, top_env, sha, logdir):
    steps = job["steps"]
    unknown = [s["uses"] for s in steps if "uses" in s and not s["uses"].startswith(KNOWN_USES)]
    if unknown:
        return False, f"unknown action(s): {unknown}"
    venv = any(s.get("uses", "").startswith("actions/setup-python@") for s in steps)
    env = dict(top_env or {}, **(job.get("env") or {}))
    tag = f"{name}-{platform}"
    native = (platform == "windows") == ON_WINDOWS
    if platform == "windows" and not native:
        return False, "a windows leg cannot run on this host"
    if native:
        work = WIN_DIR / "win"
        if not (work / ".git").exists():
            shutil.rmtree(work, ignore_errors=True)
            WIN_DIR.mkdir(exist_ok=True)
            sh(["git", "clone", "-q", "--no-checkout", str(ROOT), str(work)])
        sh(["git", "-C", str(work), "fetch", "-q", str(ROOT), sha])
        sh(["git", "-C", str(work), "-c", "core.autocrlf=false", "checkout", "-q", "-f", "--detach", sha])
        sh(["git", "-C", str(work), "clean", "-q", "-fdx", "-e", "target"])
        ci_home = (WIN_DIR / "home-win").as_posix()
        Path(ci_home).mkdir(parents=True, exist_ok=True)
        Path(ci_home, "gitconfig").write_text("[core]\n\tlongpaths = true\n")
        # As the WSL checkout does. A Windows leg has never had one made (its
        # PATH entry names no directory, so the host Python runs); unchanged.
        if venv and not ON_WINDOWS and not Path(ci_home, "venv/bin/python").exists():
            sh([sys.executable, "-m", "venv", f"{ci_home}/venv"])
    else:
        # The common git dir, not ROOT: in a linked worktree ROOT/.git is a
        # file naming a `C:/...` gitdir, which git inside WSL cannot follow.
        common = sh(["git", "-C", str(ROOT), "rev-parse", "--path-format=absolute",
                     "--git-common-dir"])
        win_root = to_wsl(Path(common))
        r = wsl(f"set -e; mkdir -p {WSL_DIR}/home; W=\"{WSL_DIR}/trellis\"; echo \"clone: $W\"; "
                f"[ -d \"$W/.git\" ] || git clone -q --no-checkout {win_root} \"$W\"; "
                f"git -C \"$W\" fetch -q {win_root} {sha}; git -C \"$W\" checkout -q -f --detach {sha}; "
                f"git -C \"$W\" clean -q -fdx -e target -e .bench; "
                f"printf '' > {WSL_DIR}/home/gitconfig; "
                + (f"[ -x {WSL_DIR}/home/venv/bin/python ] || python3 -m venv {WSL_DIR}/home/venv" if venv else "true"))
        # A checkout failure used to leave no log at all, which is why the
        # first run's `test (ubuntu-latest)` failure had nothing to read.
        log = logdir / f"{tag}-checkout.log"
        log.write_text((r.stdout or "") + (r.stderr or ""), encoding="utf-8")
        if r.returncode:
            return False, f"checkout failed (log {log}): " + (r.stderr or r.stdout)[-400:]
    for i, st in enumerate(steps):
        if "run" not in st:
            continue
        label = st.get("name") or st["run"].strip().splitlines()[0][:60]
        # Step env over job env, per step, so one step's `env:` never
        # reaches the next.
        body = prelude(dict(env, **(st.get("env") or {})), platform, venv) + st["run"]
        script = logdir / f"{tag}-{i:02d}.sh"
        script.write_text(body, encoding="utf-8", newline="\n")
        log = logdir / f"{tag}-{i:02d}.log"
        t = time.time()
        if native:
            p = subprocess.run([git_bash(), script.as_posix()], cwd=WIN_DIR / "win",
                               capture_output=True, env=dict(os.environ, CI_HOME=ci_home), **UTF8)
        else:
            spath = to_wsl(script)
            p = wsl(f"export CI_HOME={WSL_DIR}/home; cd {WSL_DIR}/trellis && bash {spath}")
        log.write_text((p.stdout or "") + (p.stderr or ""), encoding="utf-8")
        mark = "ok  " if p.returncode == 0 else "FAIL"
        print(f"    {mark} {label} ({time.time() - t:.0f}s)", flush=True)
        if p.returncode:
            tail = "\n".join(((p.stdout or "") + (p.stderr or "")).splitlines()[-25:])
            return False, f"step '{label}' exited {p.returncode} (log {log})\n{tail}"
    return True, ""


def main():
    # A failing step's tail is printed, and stdout may be a pipe in the
    # locale codec; a character that codec cannot encode must not raise.
    sys.stdout.reconfigure(errors="replace")
    ap = argparse.ArgumentParser()
    ap.add_argument("--ref", default="HEAD")
    ap.add_argument("--job", action="append", default=[])
    ap.add_argument("--keep-going", action="store_true")
    a = ap.parse_args()
    wf = yaml.safe_load((ROOT / ".github/workflows/ci.yml").read_text(encoding="utf-8"))
    sha = sh(["git", "-C", str(ROOT), "rev-parse", a.ref])
    logdir = WIN_DIR / "logs" / sha[:12]
    logdir.mkdir(parents=True, exist_ok=True)
    print(f"ci-local: {sha[:12]} ({a.ref}); logs in {logdir}", flush=True)
    results = []
    for name, platform, os_ in legs(wf["jobs"], set(a.job)):
        print(f"  {name} ({os_})", flush=True)
        t = time.time()
        ok, why = run_leg(name, platform, wf["jobs"][name], wf.get("env"), sha, logdir)
        results.append((name, os_, ok, time.time() - t))
        if not ok:
            print(f"    -> {why}", flush=True)
            if not a.keep_going:
                break
    print("\nJOB | OS | RESULT | SECONDS")
    for name, os_, ok, secs in results:
        print(f"{name} | {os_} | {'pass' if ok else 'FAIL'} | {secs:.0f}")
    want = len(legs(wf["jobs"], set(a.job)))
    good = sum(1 for r in results if r[2])
    print(f"\n{good}/{want} job legs passed at {sha[:12]}")
    sys.exit(0 if good == want else 1)


if __name__ == "__main__":
    main()
