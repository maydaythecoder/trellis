# Model ladder — tool-calling measurement

Fixture: `qa/fixtures/tool-calls.json` (frozen, 20 cases, 10 per tool,
two tools: `read_file`, `run_tests`).
Harness: `qa/run-toolcalls.py`. Four independent rates, never collapsed.

Run started 2026-09-21. Status: IN PROGRESS.

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
17x faster to first token** (0.25 s vs qwen3:8b's 4.34 s), at 2.3 s per request
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
