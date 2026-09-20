# Degradation UX at the approval prompt — 2026-09-20

v1.0 task item 4, S6. `04-v1.0-harness.md` states the design rule plainly:

> Design for the tenth partial-coverage run in an hour, not the first.

Partial coverage is not an edge case — Constraint 1 guarantees it is most
runs. Whether it reads as *honest* or as *useless* is decided entirely by how
it is surfaced, and a user who sees the same unactionable warning ten times an
hour stops reading it. Then the honesty mechanism is off.

## What existed, and what did not

`trellis radius` already had the two-block layout ADR 0014 produced: reach and
doubt, separately denominated, never a ratio. That is the *detail* view and it
was fine.

**The approval moment had nothing.** `/accept` committed with no coverage
information at all, and a converged one-shot run ended with "changes are left
in the working tree for you to review" and no account of what the graph could
not see in them. The one moment a human is deciding whether to trust the work
was the one moment TRELLIS said nothing about its own limits.

## Severity: scoped to the files being accepted

The doc's rule is *"an unresolved edge on a symbol nobody is changing is noise.
One on the symbol being edited is the headline."*

Repository-wide doubt on poetry is **11,233 sites**. Printed at approval that
is not information, it is a number whose only effect is to teach a reader to
skip the line. `doubt_in_files` counts only the files being accepted:

```
coverage: 22 call sites in the 1 file bound to nothing
          (MethodDispatchUntyped 20, DynamicAttribute 1, and 1 more)
          Any of them may call what you changed. The checks ran on what the
          graph could see.
```

Twenty-two is a number someone can hold. Same population caveat as
`RadiusCoverage`: these are calls *in* those files that bound to nothing, not
calls that reach what changed. It over-approximates, which is the safe
direction, and it is never divided by anything.

## Three paths, and the prompt never blocks

```
  a  accept, with the gap recorded in the commit
  i  inspect the unresolved sites
  t  what to test to close them
  anything else cancels
```

- **`a`** commits, and writes the coverage line into the commit message. That
  is what "proceed with gaps recorded" has to mean for it to survive: the
  trail records the run, and the commit records what was unknown at the moment
  a human said yes. `git log` is where that question gets asked later.
- **`i`** lists the sites with line numbers and the function each is in, then
  returns to the prompt. Line numbers are computed at display time because the
  index does no I/O, and a byte offset is not something a user can act on.
- **`t`** ranks the enclosing functions by how many unresolved calls they
  contain:

  ```
  A test that executes these turns an unfollowable edge into an observed one:
    create                             5 unresolved call(s)
    get                                4 unresolved call(s)
    _get_environment_repositories      3 unresolved call(s)
  ```

  "Add a test" is not advice. A named function is. And TRELLIS does not write
  it: a check it generated and then ran would be the model grading its own
  work.

**A file with no doubt costs no keystroke.** The prompt appears only when
there is something to say, which is what keeps it worth reading on the tenth
run. `i` and `t` return to the prompt rather than deciding, because a user who
asked to see the sites has not yet said whether to accept them. Anything
unrecognised cancels, and EOF is not approval — `trellis < /dev/null` must not
commit unreviewed work.

Nothing here halts a run. The warning attaches to the approval, never to the
loop.

## Two walls found while building it

**`.trellis/` counted as an unexamined source file.** The index cache made the
prompt fire on runs with nothing to report. `doubt_in_files` now considers
Python sources only: a changed `README.md` is not a file the graph failed to
examine, it is a file the graph was never about.

**Nothing ever wrote the gitignore.** A-24 records that `index.bin` *is*
gitignored — an intention, with no code making it true. Untracked, the cache
dirties the worktree, and a dirty worktree blocks the next run (ADR 0009): a
tool that writes a cache and then refuses to start because of it has built its
own wall. This is the third instance of that exact shape, after `/accept`
history and the trail-ordering hazard.

`cache::save` now writes `.trellis/.gitignore`, and **the file ignores
itself** — TRELLIS writes it unasked, so it must not be the reason `git
status` is dirty either. Verified end to end: indexing a clean repository
leaves `git status` empty. It is never overwritten, so a user who deletes the
self-ignore line keeps that choice.

## Reproducing

```
cargo test -p trellis-cli  -- session::tests
cargo test -p trellis-index -- cache

# live, on the benchmark repo
cd .bench/wedge && printf '/accept\ni\nt\na\n' | trellis
```
