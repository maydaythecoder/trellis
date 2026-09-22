# Findings, written as they were found

## F1 — `/clear` cleared the display and not the model's history  **FIXED**
`/clear` was handled entirely in `AppState::update`: it emptied the pane, reset
`turns` and `context_used`, and emitted no effect. The runtime's `history`
vector — the thing actually sent — kept every dropped turn.

Evidence, through the pty: after `/clear` the pane read `ctx 0 / turns 0`; the
very next turn reported **166 tokens** where a fresh history costs ~21.

Worse than a silent bug: `/clear` is a *confirmed* action. The user is asked to
approve losing the history and it is not lost.

Fix: `update` now pushes `Effect::Run(Command::Clear)`, and the runtime resets
history to the system message and clears the injected context.
Re-verified: next turn after `/clear` is **79 tokens**, not 2.5K.

## F2 — `/add` counted a file and threw it away  **FIXED**
`read_into_context` returned `(files, bytes)` from `fs::metadata`. `/add` then
reported "added 1 file(s), ~7276 tokens" having never read a byte of content.
Context stayed at 98, the gauge stayed at 0%, and the model never saw the file.

Fix: it returns `Vec<(name, contents)>`, the runtime injects each as a system
message and records it. Re-verified: `/add` moved ctx **0 → 2.5K** and the
gauge filled to **8%** with a visible bar.

Also added: 2 MiB per-file cap and a NUL check, so `/add` on a binary does not
push megabytes of replacement characters into the window.

## F3 — `/context` was a placeholder  **FIXED**
Printed `· context: (the runtime tracks this per turn)`. §6 asks it to show what
is loaded with token cost per item. Now lists each injected file with its
estimate and a total. Re-verified through the pty.

## F4 — the conversation pane never followed the stream  **FIXED**
The scroll offset was pinned to 0. Once the transcript grew past the pane, every
new line arrived below the fold: the model answered off-screen while the user
watched the top of the conversation.

Two causes, and the first fix was not enough:
1. No follow behaviour at all. Added `AppState::follow`, default true, off on
   scroll-up, back on with `End` or on sending a turn.
2. **The surface was sized by logical lines, not wrapped rows.** Eight turns is
   ~17 logical lines, which fits a 28-row pane, so the offset stayed 0 while the
   `Paragraph` wrapped them into ~50 rows and drew the first 28. The rest was
   rendered into a surface too short to hold it and vanished, with no scrollbar
   movement to suggest there was more.

Evidence: after 8 turns the pane showed questions 1–6 and neither scroll-up nor
`End` changed anything. After the fix it shows **4–8**.

## F5 — the app writes `chat.log` into the current directory  **NOT YET FIXED**
`install_logging` opens `chat.log` relative to cwd, so running the TUI in a
repository dirties the worktree. `/git status` during this run showed
`M chat.log`. For TRELLIS specifically this matters: a dirty worktree blocks a
run (ADR 0009).

## F6 — the footer advertised a quit key that types a letter  **FIXED**
`q quit` in the key hints. This interface has a text input, so `q` types a `q`.
Pressed it through the pty: a `q` appeared in the line and the session stayed
up. Footer now reads `esc clear · esc esc quit · … · end follow · tab focus`.

## F7 — Ctrl-Z typed `z`; every Ctrl chord typed its letter  **FIXED**
In raw mode the tty does not generate SIGTSTP, so the chord arrives as an
ordinary key event — and the `Char(ch)` arm pushed the bare letter. `^W`, `^U`,
`^A` all inserted text. Pressing `C-z` through the pty rendered `> z`.

Fix: Ctrl chords are swallowed unless bound; `C-z` raises `Effect::Suspend`,
and `terminal::suspend()` restores the screen, stops the process, and
re-enters on SIGCONT. Verified: `[1]+ Stopped`, shell prompt back, `fg`
returns all seven panes, still interactive, still streams.

## F8 — `/eng` generated a loop that was decorative  **FIXED**
`/test-loop` printed the cap, the state file and "/stop halts it mid-cycle",
and executed nothing. The §C.3 contract ended at text.

Fix: verification commands are parsed back out of the generated `.md` (the file
is the source of truth, not a constant), run through `LoopRunner`, reported per
cycle. Verified through the pty with two fixtures: `/qa-cap` ran 5 cycles then
"halted at the 5-cycle cap"; `/qa-stuck` halted after 2 with "the same 1
error(s) twice".

## F9 — `/stop` could not interrupt a loop  **FIXED (two causes)**
1. The effect loop `.await`ed the running loop, so the `/stop` meant to
   interrupt it sat unread in the channel until the loop finished. Observed:
   `/stop` sent during cycle 1, loop ran on to cycle 3.
2. Even once delivered, the cycle body ran a child process to completion. The
   runner checks cancellation *between* steps, so a `/stop` during a 4-second
   check was noticed only after it finished — while `/stop` describes itself as
   "kill running child processes".

Fix: the loop is spawned rather than awaited; `/stop` is handled in the effect
loop and replaces the token (a cancelled token stays cancelled, so reusing it
made the *next* loop halt before its first cycle); `shell` polls `try_wait` and
kills the child on cancel. Verified: `/stop` 2s into a 4s cycle halted at ~4s,
before the sleep would have ended.

## F10 — `/delegate` was a stub  **FIXED**
Returned `format!("· delegated: {task}")`. Now spawns a sub-agent with its own
history — deliberately not a clone of the parent's — and returns one summary
line. Verified: `· sub-agent returned (277 chars, 0 kept in the main history)`,
with `turns` and `ctx` both unchanged. No history leak.

## F11 — the first `Up` jumped to the start of the session  **FIXED**
`scroll` was an absolute offset from the top, and while following the tail it
was still 0 — so one decrement saturated there and every further press did
nothing. Now counted back from the newest line. Verified: one `Up` moves one
line, three move three, `Down` back to the tail resumes following.

## F12 — the interrupt path, and a diagnosis I got wrong twice

**What I first claimed:** "one Ctrl-C bricks the session; every later turn is
silently cancelled." **That was wrong**, and two of my own greps produced it.

- A `grep -c CHARLIE` matched *my own prompt echo* in the transcript, so a turn
  with no reply looked like a turn with a reply, and later the reverse.
- A `grep -ac "O N V E R S"` was mis-quoted and silently matched nothing, so I
  read "app is up" as "app is gone" and vice versa. One whole test — where I
  concluded "BRAVO arrived" — was measuring the **shell**, because the app had
  already exited and my keystrokes were landing in bash.

**What settled it:** instrumenting the runtime (§5's "add tracing and run
again") rather than inferring from the screen:

```
QA-INSTR turn start: cancelled=false
QA-INSTR turn end:   chunks=387  interrupted=true   cancelled=true
QA-INSTR turn start: cancelled=false
QA-INSTR turn end:   chunks=922  interrupted=false  cancelled=false
```

Turn 2 received **922 chunks and completed normally**. The pipeline is not
bricked. What actually happens is that llama3 stays on the previous topic, and
the reply is slow enough on this box that my 20–30 s waits kept ending before it
landed. Instrumentation removed afterwards; 0 occurrences remain.

**Three real fixes came out of the chase anyway, and all are kept:**

1. `CancelToken::reset()`, called at the start of each turn and each loop. A
   cancelled token stays cancelled, so without this the *first* `/stop` really
   would have ended the session — the loop version of exactly this bug was
   observed and fixed earlier (F9).
2. `events.rs` cancels on seeing `Effect::Run(Command::Stop)` rather than only
   forwarding it. The runtime processes effects serially and a streaming turn
   holds that loop, so a `/stop` sent mid-stream was unread until the stream it
   was meant to interrupt had ended. Verified: `interrupted=true` at 387 chunks
   of a twelve-paragraph answer.
3. An interrupted answer is no longer stored verbatim. Three shapes were tried
   against llama3: storing the truncated text made it **continue** the essay (a
   request for one word returned paragraph five); an inline
   `[interrupted; do not continue]` marker was too weak a steer; a *system*
   note mid-conversation was worse — the model echoed the original system
   prompt back as its answer. A short `(interrupted)` **assistant** turn keeps
   the roles alternating, which is what the chat template expects.

**Still not established:** that a short question immediately after an interrupt
gets answered on its own terms. llama3's topic-stickiness and its latency on
this machine made it unreliable to observe either way.

## F13 — the interface offered the model no tools  **FIXED, and goal 1 closed**

`c.stream(&msgs, None, ...)`. No definitions ever reached the endpoint, so no
model could answer with a `tool_calls` array, so the `Chunk::ToolCall` arm was
unreachable against anything real. The tool counts and the graph pane could
only ever have been filled by the mock, which synthesises a call it was never
offered.

Fixing it immediately broke every model that cannot take tools — ollama refuses
the whole request ("llama3:latest does not support tools") and returns nothing.
One retry without them, and the user is told:

    > Say NOVEMBER and nothing else.
      ! this model does not support tools; answering without them
      November

**Goal 1 is now closed.** Driven through a pty against `qwen2.5:3b`:

    BEFORE   read_file    ○─┐          (ring, no count)
    AFTER    read_file    ●─┐ 1        (filled, count 1)
    turns 0 -> 1

A real tool call travelled HTTP -> SSE parse -> `Chunk::ToolCall` ->
`Message::ToolCalled` -> `AppState.tool_calls` -> the graph pane. The
conversation shows the prompt echo and no assistant text, which is correct: the
model answered with a call and no content.

## F14 — two ollama daemons were fighting over one GPU  *(environment, not code)*

Found by the ladder subagent, and it reframes the run's earlier conclusions.
This machine has an **RTX 5060 Ti with 8 GB**, and the binding constraint is
VRAM contention between the Windows daemon and the WSL one — not system RAM as
the QA brief and my own report assumed. With the Windows daemon holding
qwen3:8b, the WSL daemon was cut to 2.38 GB of VRAM and ran at a third speed.

That is very likely why llama3 replies took 20–70 s during the §6 work, which
is what made those observations unreliable and produced two wrong diagnoses.

Operational rule: **pick one daemon for the TUI.** Both holding weights looks
like a model problem and is a configuration problem.

## F15 — `llama3` and every `gemma` refuse tools for a packaging reason

Neither has a `.Tools` block in its ollama template. The refusal is missing
metadata, not model inability — `llama3.1:8b`, same family and size, scores
20/20/20/0. Recorded because "the model cannot do tools" and "this packaging of
the model was not given the template" lead to different fixes, and only the
second is worth routing around with llama.cpp.

## F8 — `tier 2, schema enforced by the runtime` is false against ollama

**FIXED.** And narrower than first recorded: `doctor` was already correct.

Every run prints, before its first turn:

```
model:    VllmOpenAi, tier 2, schema enforced by the runtime — degradation:
          none for correctness; grammar compilation happens in the serving runtime
```

Against ollama this is not true. `RuntimeClient::body` sends the schema as
`guided_json`, which is **vLLM's** parameter name. Ollama does not implement
it: structured output there is `format` on the native API, or
`response_format` on the OpenAI-compatible one. The field is accepted and
ignored.

Measured rather than argued. The same prompt to `qwen3:8b` at
`http://127.0.0.1:11434/v1/completions`, temperature 0, three times each with
and without `guided_json`:

```
temp=0.0 no schema     ctok=987  {"tool": "edit", "path": "app/calc.py", ...
temp=0.0 guided_json   ctok=987  {"tool": "edit", "path": "app/calc.py", ...
```

Byte-identical text and an identical completion-token count. A schema that
changed sampling could not produce that.

**Why it matters beyond tidiness.** The line states a degradation profile — 
"none for correctness" — that the harness has not established for this
runtime. Malformed calls are then read as model weakness when they may be an
unenforced grammar. It is also the claim CLAUDE.md's "state what is true and
what is not known" exists to prevent.

**Correction to the first version of this entry.** It said the fix belonged to
`doctor`. It did not: `doctor` has probed this all along via
`RuntimeClient::probe_constraint`, and against this runtime it already reported

```
tier:       ValidateAndRetry — malformed tool calls are retried, not prevented
The runtime ignored the schema, so output is NOT constrained.
```

The false claim was only ever in `trellis run`, which took its tier from the
`--unconstrained` flag and never asked the runtime anything. That is also where
it mattered most, because the run banner is the line a user actually reads.

**Fixed** by probing once at run start, between the reachability check and the
banner, reusing the runtime that check already builds. One request against an
already-loaded model, ahead of a run measured in minutes. Three outcomes, all
reported:

- honoured — the tier stands, and the banner says it was probed;
- ignored — the driver is rebuilt unconstrained and the banner says so, which
  is what now prints against ollama;
- probe failed — the tier is left as assumed and the banner says *that*, which
  is the difference between an unverified claim and a false one.

Behaviour is unchanged: the loop already validated and retried, because the
schema was being ignored regardless. What changed is that the report is true.
