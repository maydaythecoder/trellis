# Cost by radius size

Measured 2026-10-02 at `a6463af` (branch `drain`), for #147. It replaces the
guesses in `docs/COST_MODEL.md` C-3 and C-4 with what these runs observed, and
leaves C-2 a guess because no run converged.

**What this measures is narrower than the ticket hoped.** Every run stopped on
the model's tool-call format before it reached the edit, so the table shows
what a run costs until it fails on format, not what a change costs. That
failure turned out to be the finding: see "Why nothing converged".

## Setup

| | |
|---|---|
| Machine | Windows 11 Home 10.0.26200, 16 logical cores, 15.2 GB RAM, NVIDIA RTX 5060 Ti 8 GB |
| Model × quantization × runtime | `qwen3:8b` (`500a1f067a9f`, Q4_K_M) × Ollama 0.34.2 at `127.0.0.1:11434`, `/v1/completions`; tier 4, no schema |
| Binary | `target/debug/trellis.exe` built from `a6463af` |
| Corpus | poetry 2.5.0 sdist (`.bench/poetry-2.5.0`), copied to `.bench/cost`, `git init` with `core.autocrlf=false`, one commit `e176ce8` |
| Budget | the default: 40 iterations, 20 minutes (`trellis-loop` `Budget::default`); no `[budget]` in the config |
| Checks config | `.bench/floor`'s `[checks]`: `mypy --ignore-missing-imports --no-error-summary --no-color-output --hide-error-context --follow-imports=silent --no-incremental`, and `python -m pytest -q -x -p no:cacheprovider` |

The work was held constant on purpose: every task is a docstring edit in the
same file, `src/poetry/utils/helpers.py`, so the radius is the variable that
moves. `remove_directory` already has a docstring, so its task extends it.

### Commands

```
# once
cd .bench/cost && trellis index

# radius, as printed
trellis radius poetry.utils.helpers.<symbol>

# each run, from a hard reset to e176ce8, timed from start to exit
trellis run "<task>" --model vllm:http://127.0.0.1:11434#qwen3:8b

# the scoped check, three repetitions, median
trellis check src/poetry/utils/helpers.py
```

The tasks, verbatim:

| symbol | task |
|---|---|
| `paths_csv` | add a docstring to the paths_csv function in src/poetry/utils/helpers.py saying what it returns |
| `get_real_windows_path` | add a docstring to the get_real_windows_path function in src/poetry/utils/helpers.py saying what it returns |
| `merge_dicts` | add a docstring to the merge_dicts function in src/poetry/utils/helpers.py saying what it does to its first argument |
| `remove_directory` | extend the docstring of the remove_directory function in src/poetry/utils/helpers.py to describe its force parameter |

## Radius

As `trellis radius` prints it. Reach and doubt are different populations and
are not divided (ADR 0014).

| symbol | resolved | ambiguous | reached transitively | unbound call sites in the files the radius touches |
|---|---|---|---|---|
| `paths_csv` | 9 | 4 | 8 | 57 across 3 files |
| `get_real_windows_path` | 57 | 0 | 50 | 564 across 10 files |
| `merge_dicts` | 120 | 14 | 119 | 1,361 across 29 files |
| `remove_directory` | 193 | 0 | 169 | 1,360 across 25 files |

Smallest to largest resolved reach: 9 to 193, 21×.

## Runs

Two rounds, n=2 per symbol. Wall-clock is the `trellis run` process from start
to exit, measured outside it, and includes loading the index. Prompt sizes are
the trail's `factual.context`, in bytes. Token counts are **the runtime's
count**, as the adapter reported it (`factual.tokens`, `factual.prompt_tokens`),
summed over every call in the run including retries.

| symbol | round | wall-clock | iterations | first prompt | peak prompt | converged | completion tokens | prompt tokens | stopped on |
|---|---|---|---|---|---|---|---|---|---|
| `paths_csv` | 1 | 71 s | 3 | 9,787 B | 13,409 B | no | 4,377 | 23,627 | `run_checks` args wrapped in `input` |
| `paths_csv` | 2 | 69 s | 3 | 9,787 B | 13,409 B | no | 4,377 | 23,627 | same |
| `get_real_windows_path` | 1 | 118 s | 2 | 9,799 B | 11,781 B | no | 7,799 | 23,589 | `run_checks` args wrapped in `params` |
| `get_real_windows_path` | 2 | 118 s | 2 | 9,799 B | 11,781 B | no | 7,799 | 23,589 | same |
| `merge_dicts` | 1 | 91 s | 2 | 9,808 B | 12,294 B | no | 5,965 | 19,748 | `run_checks` args wrapped in `args` |
| `merge_dicts` | 2 | 91 s | 2 | 9,808 B | 12,294 B | no | 5,965 | 19,748 | same |
| `remove_directory` | 1 | 145 s | 1 | 9,808 B | 9,808 B | no | 9,881 | 14,667 | `edit` with `file` for `path` |
| `remove_directory` | 2 | 144 s | 1 | 9,808 B | 9,808 B | no | 9,881 | 14,667 | same |

Every run ended `the model could not continue: no valid tool call after 4
attempts`, was rolled back to `e176ce8` with the worktree restored, and wrote
no file. The second round reproduced the first exactly, to the byte and the
token, so at temperature 0 on this runtime these are one observation each,
not two.

### The scoped check

`trellis check src/poetry/utils/helpers.py`, three repetitions, run separately
after the runs. No run touched a file, so the ticket's "files the run touched"
is empty; this times the file every task targeted instead.

| repetition | wall-clock | typecheck | tests |
|---|---|---|---|
| 1 | 1.55 s | passed, 1,137 ms | FAILED, 398 ms |
| 2 | 1.09 s | passed, 778 ms | FAILED, 290 ms |
| 3 | 1.09 s | passed, 780 ms | FAILED, 296 ms |

Median **1.09 s**. The tests check cannot pass in this environment, for a reason
outside the harness: poetry's `pyproject.toml` adds `-n` (pytest-xdist) to every
invocation and its `conftest.py` imports `findpython`, and neither is installed.
Installing poetry's development dependencies into this machine's Python was out
of scope. So 0.3 s is the cost of pytest failing to start, not of running
tests. A converging poetry run on this setup could pass only with a model that
ran the typecheck alone, as the `.bench/gate` runs did.

The whole-repository check, timed once while choosing the setup: 11.7 s, failing
on the same missing dependency and on mypy errors under `tests/`.

## What the table shows

**The first prompt does not depend on the radius.** 9,787 B for a symbol with
9 resolved callers, 9,808 B for one with 193. The radius enters the context
only if the model calls `blast_radius`, and a large one is shortened when it
does: the `get_real_windows_path` trail records `summarised: 41`, 41 call sites
shown as per-file counts rather than one location each (ADR 0033). The 21 B spread is the
task text's own length. COST_MODEL C-3 assumed context grows with radius
size; in this design it is set by the task, the file inventory and the
conversation so far, and the radius is one bounded observation inside it.

**Peak prompt grew 1.0× to 1.4× over 1 to 3 turns**, with one compaction in
two of the runs (budget 14,336 B). Too few turns to say how it grows over a
long run.

**Wall-clock tracks completion tokens, not radius.** `remove_directory` ran one
iteration and took the longest, 145 s, because its four attempts produced the
most output, 9,881 completion tokens.

## Why nothing converged

The model put its arguments inside a wrapper field the protocol does not have:
`{"tool": "run_checks", "input": {...}}`, then `params`, `args`, and for the
edit tool `file` in place of `path`. The adapter rejected each with the field
it expected, retried, and stopped after four attempts, as tier 4 is specified
to (ADR 0013: malformed calls are retried, not prevented).

Two runs also anchored edits on code that does not exist in poetry, `def
merge_dicts(*dicts):` and a `get_real_windows_path` that calls
`path.replace('/', '\\')`. Both were invented from the function name.

The same model converges on the demo fixture (qa/BASELINE.md), where its first
prompt is about 2.7 KB (2,681 B in the 2026-10-02 `vague` run). Here it is
9.8 KB, mostly the file inventory: the prompt lists at most 200 paths
(`MAX_FILES_LISTED` in `trellis-loop`'s adapter), poetry has 451 Python files,
and the first 200 of `git ls-files '*.py'` come to 7,454 B. The prompt's own
selection and order may differ, so that figure is an estimate of the
inventory's size, not a measurement of it. Whether prompt size is the cause is **not established**:
these runs change the repository, the prompt and the task together, and nothing
here isolates one of them.

## What this does not measure

- The cost of a converging change. None converged.
- Iterations to converge (C-2). Eight runs, zero converged, so C-2 stays a
  guess.
- Any model other than qwen3:8b, or any runtime that enforces a schema.
- Tests on poetry. The tests check could not run here (above).
