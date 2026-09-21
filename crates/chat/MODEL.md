# The local model

The RAM budget, the picks, and the measurement — with what has and has not
been run on this machine stated plainly.

## The budget

16GB total is not 16GB of model. Subtract the OS (~4GB), an editor, a
browser and the harness itself: **9–10GB, covering weights *and* KV cache.**
Context is not free — a long agent conversation with a large window can cost
as much as the weights.

That rules out more than people expect. Qwen2.5-Coder-14B at Q4_K_M is ~8GB
of weights and is commonly called the ceiling for a 16GB machine, leaving
roughly 8K–16K of context. It is **not** the pick for multi-file agentic
work: it runs, and then the context budget fights you every turn. A 7–8B
model buys that headroom back.

## The picks

| | size | why |
|---|---|---|
| **`qwen3:8b`** — the default | ~5GB Q4_K_M | Most stable series for local tool calling as of 2026: lowest rate of dropped calls. ~85% valid tool calls with two tools in a hands-on M1 16GB test, 1.5–2.5s to first token. Leaves real room for context. |
| **Gemma 4 (~9B)** — benchmark it | ~9.6GB | Highest measured reliability in the 8–10B class, ~90% valid `tool_calls` across 20 requests with two tools, and the most stable multi-tool behaviour tested. Tight on 16GB; it will squeeze the window. |
| **`gpt-oss-20b`** — benchmark it seriously | 21B total, ~3.6B active (MoE) | Apache 2.0, 128K context, and OpenAI states it runs in 16GB. Several 2026 roundups name it the best 16GB pick, with adjustable reasoning effort. MoE punches above its active-parameter weight at ~3B speed, which is exactly the right shape for a memory-constrained machine. |

**Do not use.** Dense 14B+ for agentic work — it fits and then starves the
context. `mistral-nemo` — ~30% reliability, ~70% of requests answered as
plain text with the JSON inside the content block rather than in
`tool_calls`, and never two tools in parallel. Anything without documented
tool-calling support: Ollama's endpoint takes tool definitions in OpenAI
format, and `qwen3` and `granite4` are trained for it, `gemma` supports it,
`llama3.3` handles it. Check before adopting.

## The thing that will mislead you

**Format reliability and judgment are different axes, and small models fail
on the first in a way that looks like the second.** One published comparison
had Llama-3.2-3B beat Qwen3-14B on JSON validity while the 14B won on
argument accuracy and multi-turn handling: the 3B emitted well-formed calls
to the wrong function, the 14B malformed calls with the right intent.

Grammar-constrained decoding fixes the format half outright — the schema is
enforced at the sampler, so malformed output becomes impossible rather than
unlikely. **Select for judgment, and let the grammar handle format.** A poor
smoke-test score on a model you otherwise like is an argument for llama.cpp,
not against the model.

This is why the interface shows the tier in its session pane:
`ollama (no grammar)` versus an endpoint that has one. A user who cannot see
which tier they are on cannot read a malformed-output failure correctly.

## Runtime

- **Ollama** to start. One command, OpenAI-compatible at
  `http://localhost:11434/v1`.
- **llama.cpp server** when you need grammar constraint:
  `llama-server --grammar-file tools.gbnf`. Ollama does not expose it.

`model/client.rs` is written against the OpenAI-compatible *shape*, not
against a vendor, so moving between the two is a change of `OPENAI_BASE_URL`
and nothing else.

## Verify

```bash
ollama pull qwen3:8b
ollama run qwen3:8b "Reply with exactly: OK"

CHAT_MODEL=qwen3:8b   cargo run --bin toolcall_smoke
CHAT_MODEL=gpt-oss:20b cargo run --bin toolcall_smoke
```

`toolcall_smoke` defines two tools, asks a question needing one, runs it 20
times and prints the rate. **That number is the baseline — record it below
before tuning anything.**

The assertion is deliberately narrow: a response whose `content` contains
`{"name": "read_file"}` is a **failure**, not a success. That is the dominant
`mistral-nemo` behaviour, and a check that grepped the body for a brace would
score it as working. Only a populated `tool_calls` array counts
(`Completion::used_tool_channel`, tested in `model/client.rs`).

## Results

**Not run on this machine. Ollama is not installed here** (`command -v
ollama` finds nothing), so no model has been pulled and no rate has been
measured. The harness is written and tested against recorded response
shapes; the numbers below are blank because measuring them needs the
runtime, not because the measurement was skipped.

| model | valid `tool_calls` /20 | correct tool /20 | answered in prose /20 | date |
|---|---|---|---|---|
| `qwen3:8b` | pending | pending | pending | — |
| `gpt-oss-20b` | pending | pending | pending | — |

Fill these in from `toolcall_smoke`'s own output. A figure typed here without
having run it is worse than a blank row.
