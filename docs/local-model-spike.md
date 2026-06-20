# Spike: local-inference runtime for the offline Qwen backend (#84)

**Question.** How should Quill run a local Qwen model so the AI features (#5) work
fully offline — in-process in Rust (candle-transformers, mistral.rs, llama.cpp
bindings) or via an external runner (ollama)?

**Recommendation.** Run the model **in-process in Rust, no daemon** — that's the
right author experience and all three in-process candidates generate fine on
Metal. The one piece that isn't turnkey in-process today is **structured output**
(every Quill tracker constrains the model to a JSON schema), so that is the
deciding sub-question for the integration ticket (#87): of **candle** (pure Rust,
DIY constrained decoding), **llama.cpp via a Rust binding** (mature GBNF grammar,
but the binding's sampler is currently broken), and **mistral.rs** (pure Rust,
advertises constrained generation — untested), pick the one that delivers
reliable schema-shaped JSON at least risk. **ollama** is the proven fallback
(works today, structured output and all) at the cost of a background daemon.

This was tested, not assumed — all three numbers below are from real runs on the
same model and machine. (An earlier draft of this doc recommended llama.cpp over
candle outright; the candle POC narrowed the gap — see [POC results](#poc-results).)

## Why structured output is the decisive criterion

Quill's backend trait (`quill-ai`) carries a `json_schema` on every request, and
the trackers (characters, timeline, setting, naming, style) all depend on getting
valid, schema-shaped JSON back — not prose to re-parse. With the Claude backend
that's the API's structured-output mode. A local backend has to match it.

There are two ways to get reliable JSON from a local model:

1. **Constrained decoding** — mask the sampler each step to a grammar so only
   tokens that keep the output valid can be chosen. llama.cpp ships this (GBNF
   grammars, plus a `json_schema_to_grammar` converter). It cannot produce
   invalid JSON.
2. **Prompt-and-hope + parse/repair** — ask for JSON, then parse and retry. Works
   often on big models, fails more on the small quantized models that fit
   consumer hardware.

A 1.5–7B quantized model is exactly where (1) matters most, so a runtime's
constrained-decoding story is the first thing to weigh, not the last.

## Options compared

| Dimension | llama.cpp (GGUF) | candle (HF) | ollama |
|---|---|---|---|
| Process model | **in-process** (Rust binding) or `llama-server` | **in-process**, pure Rust | external **daemon** |
| Structured output | **native GBNF + schema→grammar** | none built-in, but **raw logit access → DIY-feasible** | native (uses llama.cpp GBNF) |
| Quantization | **GGUF** (k-quants, IQ, …), broad | GGUF + safetensors, narrower | GGUF |
| Accel: Metal / CUDA / CPU | **all, mature** | Metal/CUDA/CPU, younger | all (via llama.cpp) |
| Packaging / footprint | C++ build (cmake) in the dep tree; one binary | **pure Rust, no cmake** (~1 min build); one binary | separate install + daemon |
| Model management | you fetch the GGUF | you fetch weights | **`ollama pull`**, easy |
| Cross-platform | macOS/Linux/Windows | macOS/Linux/Windows | macOS/Linux/Windows |
| Author moving parts | **none** (in-process) | **none** | a daemon to install & keep running |

The honest trade: candle is the cleanest build (pure Rust, ~1 min, no cmake) and
ran competitively (below). Its gap is no *built-in* structured output — but
because candle exposes the raw logits each decode step, grammar/JSON-schema
constraint is implementable on top (mask the logits before sampling), so the gap
is work-on-us, not a wall. llama.cpp ships that work but its mature engine is C++
(and the current Rust binding's sampler is broken — below). ollama is the easiest
to get working and manages models for the author, but it's a background daemon to
install, run, and version — the moving part the in-process route avoids.

## POC results

Both runtimes ran the **same** model end-to-end on the spike machine — **Apple
M5, 24 GB, Metal 4, macOS arm64, Rust 1.94** — on **Qwen2.5-1.5B-Instruct,
Q4_K_M GGUF (1.0 GB)**. Task: extract a character as JSON from a passage. Code in
`/tmp/quill-spike` (throwaway). The numbers are a single small model on one
machine — directional, not a benchmark suite.

### ollama (daemon)

Schema-constrained output via `/api/chat` with `format` set to a JSON Schema
produced **valid, correct, schema-shaped JSON on the first try**:

```json
{"name":"Mara Vance","role":"protagonist","traits":["hands scarred","temper short", …]}
```

- warm load 67 ms · generation **96.9 tok/s** · total 809 ms
- the registry pull timed out (IPv6), but ollama ran fine on a local GGUF via a
  one-line `Modelfile` — so it doesn't strictly need its registry.

### in-process (llama-cpp-2 0.1.150, embeds llama.cpp, Metal by default on macOS)

- **Generation on Metal works**: warm load 107 ms · **79.5 tok/s** · produced JSON
  (`{"protagonist":"Mara Vance"}`) from a prompt. Same order of magnitude as
  ollama; our POC loop is naive (single-token batches), so the gap isn't
  fundamental.
- **Grammar-constrained sampling is broken in this binding version.**
  `LlamaSampler::grammar(...)` aborts at init —
  `llama-grammar.cpp:940 GGML_ASSERT(!stacks.empty())` — for *any* grammar,
  including a trivial `root ::= "ok"`. This is a **Rust-binding maturity gap, not
  a llama.cpp gap**: ollama drove the same llama.cpp's GBNF on the same model with
  no trouble.
- Build cost is real: compiling llama.cpp via cmake (one-time, a few minutes) and
  a deep build-dep tree (bindgen, cc, cmake, clang-sys).

### in-process (candle 0.9, pure Rust, Metal)

- **Generation on Metal works**: load 606 ms · **88.6 tok/s** · produced the same
  JSON (`{"protagonist":"Mara Vance"}`) from a prompt. Faster generation than the
  llama.cpp binding in these naive loops; slower model load (candle's GGUF
  dequant/setup).
- **Build is the cleanest of the three**: ~58 s, pure Rust, no cmake/C++ — just
  `candle-core`, `candle-transformers`, `tokenizers` (needs the model's
  `tokenizer.json` alongside the GGUF, since candle doesn't read the tokenizer
  embedded in the GGUF the way llama.cpp does).
- **No built-in structured output.** The POC sampled with `Sampling::ArgMax`, so
  JSON here is prompt-driven, not guaranteed. But the loop has the full logits
  tensor in hand before each `sample()` — so a grammar/JSON-schema mask applied to
  those logits would give constrained decoding in pure Rust. That's real work (a
  JSON state machine, or an existing Rust GBNF crate), but it's tractable, not a
  wall.

## Recommendation & rationale

1. **Go in-process, no daemon.** All three in-process candidates generated fine on
   Metal at ~80–90 tok/s; a single binary with nothing to install or keep running
   is the right author experience and worth preferring over ollama's daemon.
2. **GGUF + Metal, regardless of engine.** GGUF is the common quant format across
   every option, Metal worked out of the box everywhere, and Q4_K_M of a small
   Qwen is ~1 GB and fast on an M5. This is settled.
3. **Structured output is the one unsolved in-process piece — let #87 decide it
   empirically.** None of the in-process options gives turnkey schema-shaped JSON
   today:
   - **llama.cpp via `llama-cpp-2`** — mature GBNF + schema→grammar engine, but the
     binding's grammar sampler asserts in 0.1.150 (needs a fixed/pinned/patched
     version, or a different binding).
   - **candle** — cleanest pure-Rust build and competitive speed, but constrained
     decoding is DIY (mask the logits it already exposes, via a JSON state machine
     or a Rust GBNF crate).
   - **mistral.rs** — pure Rust, advertises constrained/structured generation;
     untested here, worth a focused try.
   #87 should prototype structured output through each and pick the lowest-risk
   path to reliable schema-shaped JSON.
4. **Ship against ollama first if needed.** It's the only option proven *today* for
   structured output end-to-end (schema-constrained JSON, first try). If the
   in-process structured path needs time, ship the local backend on ollama and
   treat the daemon as a temporary cost — the `quill-ai` trait makes the engine
   swappable later without touching the features.

Net: **in-process over a daemon**; **GGUF/Metal settled**; **engine (candle vs
llama.cpp-binding vs mistral.rs) decided by whichever lands reliable structured
output first** — closer than it looked before the candle POC, which built cleanest
and ran competitively and whose only gap is tractable.

### Is llama.cpp preferable to candle?

For structured output *today*, yes — llama.cpp's grammar engine works (proven via
ollama), candle's doesn't exist yet. But for an *in-process Rust* backend the gap
is small: candle builds cleaner (pure Rust, no cmake), ran faster than the
llama.cpp binding here, and exposes the logits needed to add constrained decoding
ourselves — while the llama.cpp Rust binding's grammar path is itself currently
broken. So neither is turnkey in-process; candle is a genuine contender, not ruled
out. The deciding factor is which reaches reliable schema-shaped JSON with least
work and maintenance, which #87 settles by prototyping both (plus mistral.rs).

## Impact on the implementation tickets

- **#87 (backend behind the `quill-ai` trait + switching)** — go in-process; its
  first and gating job is reliable structured output. Prototype the trait's
  `json_schema` → schema-shaped JSON through each contender (candle + a logit mask;
  `llama-cpp-2` with the grammar regression resolved; mistral.rs constrained
  generation) and pick the lowest-risk one, with ollama as the fallback transport.
- **#86 (install & preparation)** — acquire a **GGUF** quant (works for every
  option), not safetensors. Note candle additionally needs the model's
  `tokenizer.json` fetched alongside the GGUF; if shipping the ollama fallback,
  wrap the GGUF in a local `Modelfile` rather than depending on the registry
  (which was flaky here).
- **#85 (hardware-aware model selection)** — choose a Qwen **GGUF quant** to fit
  detected RAM/VRAM (e.g. Q4_K_M as the default sweet spot; smaller IQ quants on
  tight machines). The 1.5B Q4_K_M used here is ~1 GB and ran at ~80–97 tok/s on
  an M5; size the default tier from that.
