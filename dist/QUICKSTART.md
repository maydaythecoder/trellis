# TRELLIS — quickstart

A Rust agent harness for Python repositories. It maps the blast radius of a
change, runs the repository's own checks, and commits a record of what
happened.

## Getting `trellis` on your PATH

```
cargo install --path crates/trellis-cli
```

That drops `trellis.exe` into `~/.cargo/bin`, which is already on your PATH if
you have Rust. Otherwise copy `dist/trellis.exe` into any directory that is.

Then `trellis` works from anywhere. **It finds the repository root by walking
up**, the way git does, so `trellis index` three directories deep indexes the
whole repository rather than the empty subdirectory you happen to be standing
in. It prints the root it chose whenever that is not where you are:

```
C:\repo\app\deep\nested> trellis index
root:     C:\repo (repository root)
indexed 4 files in 1.4ms
```

With no git repository anywhere above, it **refuses** and tells you how to say
what you meant. An earlier version fell back to the current directory, on the
reasoning that `index` and `radius` are useful on a plain source tree. True,
and still the wrong default — `trellis index` typed in a home folder walked
25,147 files over two minutes before anything could stop it. Git refuses here
too.

For a tree that is not a repository, name it:

```
trellis index --root .
```

`--root <path>` always wins, and is the escape hatch for every case the walk
gets wrong.

---

## What it needs

- **A git repository** with a clean worktree, for `run`. Runs snapshot `HEAD`
  before writing so a failure can be rolled back; a dirty tree is refused
  rather than half-restored.
- **Python, and your checker on PATH.** `mypy` and `pytest` by default,
  configurable. `trellis doctor` tells you what it found.
- **A model, only for `run`.** Everything else — `index`, `radius`, `check`,
  `doctor` — works with no model at all.

Vendored and generated trees are skipped: `.venv`, `venv`, `node_modules`,
`site-packages`, `__pycache__`, `build`, `dist` and the rest. Pointing it at a
project with a virtualenv in it will not index the virtualenv.

---

## Five minutes

```
cd your-repo
trellis doctor
```

Reports what it found and what it could not determine. It does not guess: with
no `--model` it says the adapter tier is **not probed** rather than printing a
default.

```
trellis index
```

Parses the repository and prints coverage. **Read the last paragraph it
prints.** The percentage is what the resolver *bound* — a lower bound on
recall, not recall, and not comparable to any threshold.

```
trellis radius SomeFunction
```

Who calls it, in two separately-counted blocks: **reach** (call sites that
reach the symbol) and **doubt** (call sites in the same files that bound to
nothing). They come from different populations and the tool will not divide
them — an earlier version did, and printed "3.7% resolved" for a resolver that
audits at 72%.

```
trellis check
```

Runs the configured checks and exits non-zero if any fail.

---

## Running the loop

```
trellis run --model scripted:fix.json "fix the type error in app/calc.py"
```

`scripted:` replays a fixed list of tool calls. Real files, real checks, real
rollback — only judgement is stubbed. Useful for seeing the machinery without a
model, and **never a measure of what a model can do**.

Against a real model, point at a serving runtime:

```
trellis run --model llamacpp:http://127.0.0.1:8080 "..."
trellis run --model vllm:http://127.0.0.1:8000     "..."
trellis run --model sglang:http://127.0.0.1:30000  "..."
```

Check the runtime honours guided decoding first — `trellis doctor --model ...`
reports `RuntimeConstrained` if it does. If it does not, add `--unconstrained`
to make that explicit; runs can then fail on output format alone.

`--resume` starts from the last recorded run, so the model is not re-told what
the previous one established. Afterwards:

```
trellis resume
```

---

## The one thing worth understanding

**The model never decides whether the work is correct.** It can say it is
finished; the loop then asks the checks, and if the most recent executed check
did not pass, the run does not converge and the tree is rolled back. That path
is enforced by the type system, not by a prompt: a passing verdict can only be
constructed inside the crate that runs the checker.

You can watch it happen. Point `run` at a script whose last step is `done`
without ever calling `run_checks`, and the run reports *"the model stopped
without a passing check … nothing was accepted, because only an executed check
can accept it"* and restores the tree byte for byte.

**Changes are never committed for you.** A converged run leaves its edits in
the working tree for you to review. Only the decision trail is committed, and
it records the run as unapproved until you say otherwise.

---

## Configuration — `.trellis/config.toml`

Optional. A malformed file is an error, not a silent fallback to defaults.

```toml
[checks]
typecheck = ["mypy", "--no-incremental"]
tests     = ["pytest", "-q"]
python    = "python"

[budget]
max_iterations   = 40
max_wall_clock_s = 1200
regime           = "local"     # or "metered" when tokens cost money

[governance]
deny_write = ["vendor/", "*.lock", "setup.py"]
```

`deny_write` is **enforced**, not suggested: an edit to a matching path is
refused by the orchestrator, which owns the only write path to the tree. Three
forms — a directory (`vendor/`), a suffix (`*.lock`), an exact path. It is not
a security boundary; the rules live in the repository being edited.

`--no-incremental` on mypy is a correctness flag, not a preference. mypy's
cache keys on size and mtime, so a rollback that restores a file of the same
length looks unchanged to it, and it will report a cached pass for code it
never read.

`.trellis/context.md`, if present, is read into the model's prompt as the
project's own conventions. Nothing in it can authorise skipping a check.

---

## What this does not do

- **It does not find every caller.** Roughly three call edges in ten are
  statically invisible in Python. Every blast radius ships with a count of what
  could not be resolved, and there is no API that returns reach without it.
- **It does not run agents concurrently.** `trellis partitions` exits non-zero
  and says so. The planner is built and property-tested; running partitions in
  parallel is not.
- **It has no model floor yet.** One configuration has been measured
  (Qwen2.5-0.5B-Instruct Q4_K_M) and it is *below* the floor — it produces
  schema-valid tool calls whose contents are nonsense. That bounds the floor
  from below without locating it. See `docs/BENCHMARK_AND_EVAL.md` §5.

Recall on the benchmark corpus is **72.0%** on a hand-audited sample
(n=100 first-party sites), precision 100%. The v1.0 gate wants 85% and a larger
audit; neither has happened. `docs/STATUS.md` is the honest state of every
claim.
