# Model ladder — tool-calling measurement

Fixture: `qa/fixtures/tool-calls.json` (frozen, 20 cases, 10 per tool,
two tools: `read_file`, `run_tests`).
Harness: `qa/run-toolcalls.py`. Four independent rates, never collapsed.

Run started 2026-09-21. Status: COMPLETE.

## Two endpoints, deliberately not merged

| | version | reachable from | models |
|---|---|---|---|
| Windows ollama | 0.34.2 | git-bash `127.0.0.1:11434` | qwen3:8b |
| WSL ollama | 0.15.6 | `wsl.exe -e bash -lc` `127.0.0.1:11434` | llama3:latest |

They are different daemons on the same port number in different network
namespaces. Every row below records which one produced it, because an
ollama four minor-versions apart is a different tool-call implementation.

## Memory budget, measured not assumed

(filled in below)

## Results

| rung | model | endpoint | valid | correct tool | correct args | prose/text | TTFT | wall | loaded |
|---|---|---|---|---|---|---|---|---|---|
| 1 | qwen3:8b | win | 20/20 | 20/20 | 20/20 | 0/20 | 4.34s | — | yes (baseline, not re-run) |

### Measured memory budgets (not the brief's estimates)

Windows host, before any ladder work:
```
TotalVisibleMemory_GB 15.16
FreePhysicalMemory_GB  4.64
Pagefile C:\pagefile.sys  AllocGB 17.30  CurrentUsageGB 2.36  PeakGB 4.12
```
WSL VM, before any ladder work:
```
               total        used        free      shared  buff/cache   available
Mem:            7514         685        5844           3        1155        6828
Swap:           2048           0        2048
```

Two separate budgets. The WSL VM has a **7.5 GB hard ceiling** carved out of the
host's 15.16 GB; a model too big for 7.5 GB cannot run in WSL at any amount of
host free memory. Windows free was **4.64 GB measured**, not the ~3 GB assumed.

Model weights, read from the registry manifest without downloading:

| model | weights |
|---|---|
| qwen2.5:3b | 1.93 GB |
| llama3:latest (local) | 4.66 GB |
| gemma2:9b | 5.44 GB |
| gemma3:12b | 8.15 GB |
| gpt-oss:20b | **13.79 GB** |

This corrects the brief on two points: **gemma2:9b is 5.44 GB, not ~9.6 GB**, so
it fits the WSL VM; and gpt-oss:20b at 13.79 GB exceeds the WSL ceiling outright,
so Windows is its only possible host.

**Order changed to 6 → 5 → 3 → 2.** gpt-oss:20b is the only rung that can
destabilise the host, and taking the WSL VM down with it would cost the other
rungs. It goes last, after every cheap rung is banked.

---

## Rung 5 — llama3:latest (WSL, ollama 0.15.6) — REFUSED, model never ran

The daemon rejects the request before the model is invoked:

```
HTTPError 400
BODY: {"error":{"message":"registry.ollama.ai/library/llama3:latest does not
support tools","type":"api_error","param":null,"code":null}}
```

Cause, from the model metadata itself:
```
capabilities: ['completion']          <- no 'tools'
template mentions Tools: False
```
llama3's ollama template has no `.Tools` block, so the tool definitions are
never rendered into the prompt. ollama checks the capability and refuses.

Full harness run (`qa/logs/raw-llama3.txt`):
```
valid tool_calls   0/20 (0%)
correct tool       0/20 (0%)
correct arguments  0/20 (0%)
text / JSON-in-prose 0/20 (0%)
transport errors   20/20
wall clock         0.8s for 20 requests
time to first token 17.35s
```

**All four rates are 0/20 and that is not four failures — it is one refusal.**
The 0.8 s wall clock for 20 requests is the tell: 40 ms per request is an HTTP
error, not inference. The model was never asked the question. Recording this as
"0% valid tool calls" alongside a model that was actually measured would be a
category error.

The model itself is alive and not the problem: the TTFT probe sends **no tools**,
and it answered in **17.35 s** to first token (cold). llama3 works as a chat
model on this daemon; it cannot be given tools by it.

---

## Rung 6 — qwen2.5:3b (WSL, ollama 0.15.6) — CLEAN SWEEP, and fast

Pulled into WSL ollama, 1.93 GB, exit 0. Capability check first:
```
capabilities: ['completion', 'tools']
template mentions .Tools: True
```

Full harness run (`qa/logs/raw-qwen25-3b-wsl.txt`), all 20 rows `ok`:
```
valid tool_calls   20/20 (100%)
correct tool       20/20 (100%)
correct arguments  20/20 (100%)
text / JSON-in-prose 0/20 (0%)
wall clock         45.5s for 20 requests
time to first token 0.25s
```
It got both tools right, including the 10 `run_tests` cases where the optional
`filter` argument had to be present or absent depending on the prompt — 4 of the
10 correctly passed no filter, 6 correctly extracted one.

Resident footprint while loaded (`/api/ps`):
```
size 2,645,553,152 (2.65 GB)   size_vram 1,932,398,592 (1.93 GB)   context_length 4096
```

**A 3.1B model at 1.93 GB matches the 8.2B baseline on all four rates and is
17x faster to first token** (0.25 s vs qwen3:8b's 4.34 s — but see the thinking
section below: most of qwen3's 4.34 s is reasoning, not latency), at 2.3 s per request
against qwen3's unmeasured-but-slower. The plumbing is not the bottleneck and the
fixture is not hard enough to separate these two models.

WSL memory before / during: `available` 6828 MB -> 6156 MB. Swap touched 12 MB
of 2048. Nothing close to pressure.

---

## The constraint is VRAM, and the two daemons are fighting over it

The brief framed this as a system-RAM problem. It is not. There is a GPU:

```
NVIDIA GeForce RTX 5060 Ti, driver 610.74, 8151 MiB total, 7532 MiB USED
```

7.5 of 8.1 GiB were already occupied. The occupant is the **Windows** ollama,
holding the qwen3:8b baseline resident with a full offload:
```
qwen3:8b   size 5,578,204,118   size_vram 5,578,204,118   <- 100% in VRAM
```

So when the WSL daemon loaded qwen2.5:3b it found only 2.38 GiB free and did a
**partial** offload:
```
msg="gpu memory" library=CUDA available="1.9 GiB" free="2.3 GiB"
llama_model_load_from_file_impl: using device CUDA0 (RTX 5060 Ti) - 2382 MiB free
load_tensors: offloaded 27/37 layers to GPU
```

Two consequences that have to be stated, not averaged away:

1. **Rung 6's 0.25 s TTFT was achieved on a handicap** — 27 of 37 layers, with
   the other daemon holding 5.6 GB of the card. With the GPU to itself it would
   be faster still. The number is a floor, not a ceiling.
2. **The real budget for a third model is ~2.3 GiB of VRAM, 4.64 GB of host RAM,
   and a 7.5 GB WSL ceiling** — whichever binds first. For gpt-oss:20b at
   13.79 GB, all three bind at once.

`GPULayers:27 ... NumThreads:8 KvSize:4096` — note context was capped at 4096 by
both daemons, well under qwen3's advertised 40960. Not varied here; noted so a
later run does not read a context difference as a model difference.

---

## qwen3:8b is not slow — it is thinking. Measured, not inferred.

The baseline's 4.34 s TTFT reads as "an 8B is too slow to drive a TUI". That
reading is wrong, and the harness's own definition of TTFT is why.

`ttft()` counts the first chunk where `delta.content` or `delta.tool_calls` is
truthy. qwen3 is a thinking model: it streams its reasoning in a separate
`reasoning` field with `content` set to the empty string, which is falsy. So the
harness skips every reasoning chunk and reports time-to-first-*answer*.

Streamed one request and looked at the actual deltas:
```
FIRST DELTA at 0.42s keys=['role', 'content', 'reasoning']
  chunk 1 {"role": "assistant", "content": "", "reasoning": "Okay"}
  chunk 2 {"content": "", "reasoning": ","}
  chunk 3 {"content": "", "reasoning": " the"}
FIRST CONTENT at 2.59s
all delta keys seen: ['content', 'reasoning', 'role']
```
**Tokens start at 0.42 s. Answer tokens start at 2.59 s.** The 2.2 s between is
thinking, and the harness attributes all of it to latency.

Turning thinking off (native `/api/chat`, `think: false`, same warm model):
```
think=True   first content 2.86s   full reply 3.22s
think=False  first content 0.22s   full reply 0.30s
```
**13x on time to first content, 11x on the complete reply.**

Tool calling survives it — 3-case probe, not the full 20, and labelled as such:
```
case 1   0.69s  tool=read_file  args={"path": "src/main.rs"}   (expected, match)
case 2   0.45s  tool=read_file  args={"path": "Cargo.toml"}    (expected, match)
case 11  0.28s  tool=run_tests  args={}                        (expected, match)
```

Caveat kept attached to the number: qwen3 held a **full** 5.58 GB VRAM offload
for these, while rung 6's qwen2.5:3b had 27/37 layers. Not yet apples to apples;
the re-run below fixes that.

---

## Rung 3 — gemma2:9b (WSL, ollama 0.15.6) — REFUSED, same mechanism as rung 5

Before downloading, I fetched each candidate's **template layer** from the
registry (a few KB, not the weights) and grepped it for `.Tools`:

```
gemma2:9b:   template has NO .Tools -> ollama will refuse tools
gemma3:12b:  template has NO .Tools -> ollama will refuse tools
gemma3:4b:   template has NO .Tools -> ollama will refuse tools
mistral:7b:  TEMPLATE HAS .Tools  -> tool calling possible
llama3.1:8b: TEMPLATE HAS .Tools  -> tool calling possible
```

**No gemma in the ollama library has a tools template** — not the 9B, not either
gemma3. The rung was unmeasurable before it was downloaded.

I pulled gemma2:9b anyway (5.44 GB, exit 0) and confirmed it on the real tag
rather than resting on the registry grep:
```
capabilities: ['completion']
template mentions .Tools: False
params: 9.2B quant: Q4_0

HTTPError 400
BODY: {"error":{"message":"registry.ollama.ai/library/gemma2:9b does not
support tools","type":"api_error","param":null,"code":null}}
```
Harness (`qa/logs/raw-gemma2-9b.txt`):
```
valid tool_calls   0/20 (0%)
correct tool       0/20 (0%)
correct arguments  0/20 (0%)
text / JSON-in-prose 0/20 (0%)
transport errors   20/20
wall clock         0.8s for 20 requests
time to first token 63.99s
```
Again 0.8 s for 20 requests — refusals, not inference. The model loads and
generates fine without tools: 63.99 s to first token on a cold load while
competing for VRAM.

### Substitution, declared

A second refusal teaches nothing the first did not. To put an actual measurement
in this size slot I substituted **llama3.1:8b** (4.92 GB, `.Tools` present). It is
the highest-information choice available: same family and size as rung 5's
llama3, differing in exactly the property under test, so it also answers whether
rung 5's failure was llama-the-model or llama3-the-packaging. Recorded as a
substitution, not as gemma.

### Rung 3 substitute — llama3.1:8b (WSL, ollama 0.15.6) — CLEAN SWEEP on 7/33 layers

```
llama3.1:8b capabilities: ['completion', 'tools']
template mentions .Tools: True
params: 8.0B quant: Q4_K_M
```
Harness (`qa/logs/raw-llama31-8b.txt`):
```
valid tool_calls   20/20 (100%)
correct tool       20/20 (100%)
correct arguments  20/20 (100%)
text / JSON-in-prose 0/20 (0%)
wall clock         57.5s for 20 requests
time to first token 0.28s
```

It ran this **almost entirely on CPU**. Only 2383 MiB of VRAM was free (a process
outside this session — presumably the concurrent TUI half — had reloaded
qwen3:8b on the Windows daemon), so:
```
load_tensors: offloaded 8/33 layers to GPU
load_tensors: offloaded 7/33 layers to GPU
```
**7 of 33 layers on the GPU and it still answered in 2.9 s per request.** This is
the cleanest disproof of the brief's premise that this hardware is too small: the
bottleneck was never the model size.

It also settles rung 5. Same family, same 8B size, same daemon — the only
difference is a tools template. llama3's 0/20 was **packaging, not the model**.

One quality difference the four rates do not capture: on the no-filter cases
llama3.1 emitted `{"filter": ""}` (cases 17, 19) where qwen2.5:3b omitted the key
entirely. The harness counts both correct — a case with `args: {}` has nothing to
compare, so the check passes vacuously. Worth knowing before a tool treats an
empty filter as a filter that matches nothing.

---

## Rung 2 — gpt-oss:20b (Windows, ollama 0.34.2) — LOADED, AND ABANDONED

Pulled 13.79 GB, exit 0. It is genuinely tool-capable, unlike gemma/llama3:
```
capabilities: ['completion', 'tools', 'thinking']
template mentions .Tools: True
params: 20.9B quant: MXFP4 family: gptoss   ctx 131072
```

Before the attempt I unloaded every other model to give it the best chance:
`FreePhysGB 1.76  PagefileGB 3.05  vram_free 7882 MiB`.

Then **one** probe — "Say OK.", 16 tokens — not the 20-case harness, with the
abort command ready and memory polled every 25 s:

```
poll 1  freeGB/pagefileGB=0.73 2.94  vram_free=2203 MiB
poll 2  freeGB/pagefileGB=0.02 7.76  vram_free=1871 MiB
poll 3  freeGB/pagefileGB=0.99 9.63  vram_free=1827 MiB
!! PAGEFILE >= 8GB - ABORTING
```

**Free physical memory reached 0.02 GB. The pagefile went 2.94 -> 9.63 GB, peak
10.94 GB.** That is the machine swapping, which is the abandon condition.

The probe did complete, and its own timings are the verdict:
```
total_duration       81.32 s
load_duration        48.65 s
prompt_eval_count    65 tokens
prompt_eval_duration 31.41 s      <- 0.48 s per prompt token
eval_count           16 tokens
eval_duration         0.87 s      <- 18 tok/s once resident
done_reason          "length"
```

From the server log, the surprise: it is a **sparse MoE** (`expert_count 32`,
`expert_used_count 4`, `block_count 24`) at MXFP4, and every layer nominally
offloaded:
```
load_tensors: offloaded 25/25 layers to GPU
```
So the arithmetic that says "13.79 GB will not fit in 8 GB" is not what broke it.
What broke it is that 13.79 GB of weights still had to stream through a host with
1.76 GB free, and the paging that caused is the 31 s spent on a 65-token prompt.
Generation itself was fine at 18 tok/s.

**Verdict: loaded, answered, unusable.** Per-request prompt evaluation of ~31 s
puts the 20-case harness at 10+ minutes while pinning the host at zero free RAM,
with another QA run in progress on the same machine. Abandoned deliberately and
not retried. **The four rates for this rung are not measured, and are recorded as
unmeasured rather than as zeros** — the single probe emitted `thinking` and hit
the 16-token cap before any content, so it says nothing about tool-call validity.

Also noted: `think: false` was **ignored** — gpt-oss uses the harmony template
(`selected=harmony`) and reasons regardless. The qwen3 latency fix below does not
transfer to it.

Machine recovered fully after unload: `FreePhysGB 7.21`, no models resident.

---

## Rung 6 re-run — qwen2.5:3b with the GPU to itself

The first rung-6 run shared the card with qwen3:8b and got 27/37 layers. With the
other daemon unloaded (`raw-qwen25-3b-rerun.txt`):
```
load_tensors: offloaded 37/37 layers to GPU

valid tool_calls   20/20 (100%)
correct tool       20/20 (100%)
correct arguments  20/20 (100%)
text / JSON-in-prose 0/20 (0%)
wall clock         19.2s for 20 requests
time to first token 0.07s
```
Same four rates, **2.4x the speed**: 0.96 s per request vs 2.3 s, and 0.07 s TTFT
vs 0.25 s. The rates are a property of the model; the timings are a property of
who else is on the GPU. Both runs are kept, neither is averaged.

On the no-filter cases it emitted `{}` — cleanly omitting the optional key, where
llama3.1 emitted `{"filter": ""}`.

---

## Summary

| rung | model | endpoint | valid | correct tool | correct args | prose/text | TTFT | wall (20 req) | loaded |
|---|---|---|---|---|---|---|---|---|---|
| 1 | qwen3:8b | win 0.34.2 | 20/20 | 20/20 | 20/20 | 0/20 | 4.34 s | — | yes, full VRAM (baseline, not re-run) |
| 2 | gpt-oss:20b | win 0.34.2 | — | — | — | — | — | — | **loaded, then abandoned — swapped the host** |
| 3 | gemma2:9b | wsl 0.15.6 | n/a | n/a | n/a | n/a | 63.99 s* | 0.8 s* | loads, but **daemon refuses tools** |
| 3s | llama3.1:8b *(substitute)* | wsl 0.15.6 | 20/20 | 20/20 | 20/20 | 0/20 | 0.28 s | 57.5 s | yes, 7/33 layers on GPU |
| 4 | llama.cpp + GBNF | — | — | — | — | — | — | — | **not run — trigger never fired, see below** |
| 5 | llama3:latest | wsl 0.15.6 | n/a | n/a | n/a | n/a | 17.35 s* | 0.8 s* | loads, but **daemon refuses tools** |
| 6 | qwen2.5:3b | wsl 0.15.6 | 20/20 | 20/20 | 20/20 | 0/20 | 0.25 s | 45.5 s | yes, 27/37 layers |
| 6b | qwen2.5:3b *(GPU free)* | wsl 0.15.6 | 20/20 | 20/20 | 20/20 | 0/20 | **0.07 s** | **19.2 s** | yes, 37/37 layers |

\* For rungs 3 and 5 the wall clock is 20 HTTP 400s, not inference, and the TTFT
is the harness's no-tools probe, which the model did answer. `n/a` rather than
`0/20`: the model was never asked the question. Writing 0/20 would rank a model
that was refused below one that answered badly.

**Two stated gaps, so they are not read as oversights.** RAM/pagefile before and
after were captured for rungs 6, 3s and 2; they were **not** captured for rung 6b
or for the gemma2 TTFT probe, and that probe did load a 5.4 GB model, so there was
a real memory event with no snapshot beside it.

**Every wall clock above includes a cold model load on case 1** — qwen2.5's first
run and llama3.1 had just been pulled, and the 6b re-run followed a full unload.
The per-request averages (0.96 s, 2.3 s, 2.9 s) are inflated by that load, which
matters because the recommendation leans on them.

Supplementary (native `/api/chat`, not the frozen `/v1` protocol — labelled, not
mixed into the table): **qwen3:8b with `think: false` reaches 0.22 s to first
content**, 3/3 correct tool calls on a 3-case probe.

## Rung 4: not warranted, by the brief's own criterion

The trigger was "if ANY rung shows malformed output — text or JSON-in-prose
instead of a `tool_calls` array". **It never fired.** Every model that actually
ran returned a well-formed `tool_calls` array on all 20 cases:

```
qwen3:8b     prose/text 0/20
qwen2.5:3b   prose/text 0/20   (both runs)
llama3.1:8b  prose/text 0/20
```
Zero TEXT rows, zero PROSE-JSON rows, across 80 measured cases. There is no
malformed output for a GBNF grammar to constrain. **Rung 4 is skipped because the
diagnostic it performs has nothing to diagnose**, not because it was inconvenient.

A separate point, deliberately not smuggled in as the one above: llama.cpp would
be the only way to measure **gemma2 or llama3** here at all, since ollama's
refusal is missing template metadata rather than model inability, and llama.cpp
does not consult an ollama template. That is an argument about *coverage of
models ollama won't serve*, not about *malformed output*, and it is not the
argument rung 4 was specified to test. If someone wants gemma measured, that is
the route — but it is a new question, not this one.

## Recommendation for driving the TUI interactively

**Use `qwen2.5:3b` as the default. Keep `qwen3:8b` with `think: false` as the
step up.**

Reasoning, with the weakness stated first: **this fixture cannot separate these
models on accuracy.** Three models scored 20/20/20/0. A 3.1B and an 8.2B tie, and
a model running 7 of 33 layers on the GPU ties too. So the recommendation rests
on latency and footprint, which the fixture *does* measure — and any claim that
one of these is "more accurate" needs a harder fixture first.

On that basis:

- **qwen2.5:3b** — 0.96 s per request, 0.07 s TTFT, 1.93 GB. It is the only
  candidate that leaves the 8 GB card with room for anything else, which matters
  because the GPU is contended on this box. Below the ~100 ms threshold where a
  TUI feels like it is responding rather than fetching.
- **qwen3:8b + `think: false`** — 0.22 s to first content and 8B capability for
  5.23 GB. Worth it when the work is harder than this fixture and the card is
  free. Leaving thinking **on** is what makes it feel slow: 2.86 s to first
  content, and the brief's 4.34 s figure is that, not the model's speed.
- **llama3.1:8b** — a fine fallback, 2.9 s per request, but it is qwen2.5's
  accuracy at 3x the latency and 2.5x the size. Notable mainly as proof the
  hardware is not the limit: it scored 20/20 nearly entirely on CPU.
- **Not gpt-oss:20b.** It works and then takes the machine down with it: 31 s to
  evaluate a 65-token prompt, host free RAM at 0.02 GB, pagefile peak 10.94 GB.
- **Not gemma2 or llama3** via ollama at all — they cannot be given tools.

One operational note for whoever wires this up: the two ollama daemons share one
8 GB GPU and evict each other's models. Pick one daemon for the TUI rather than
letting both hold weights. During this run qwen3:8b was reloaded on the Windows
side by a process outside this session (presumably the concurrent TUI half),
which cut the WSL daemon to 2.38 GB of VRAM and tripled llama3.1's per-request
time. That is a configuration problem, not a
model problem, and it will silently look like a model problem.

Run complete.
