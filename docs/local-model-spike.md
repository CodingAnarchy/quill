# Spike: local-inference runtime for the offline Qwen backend (#84)

**Question.** How should Quill run a local Qwen model so the AI features (#5) work
fully offline — in-process in Rust (candle-transformers, mistral.rs, llama.cpp
bindings) or via an external runner (ollama)?

**Recommendation.** Use **llama.cpp as the inference engine** (GGUF + GBNF
grammar), reached **in-process through Rust bindings** (no daemon) as the target,
with **ollama as the proven fallback**. Do **not** use candle as the primary
engine. The decisive reason is structured output: every Quill tracker constrains
the model to a JSON schema, llama.cpp does this natively (and it's proven on this
hardware below), and candle has no native constrained decoding.

This was tested, not assumed — see [POC results](#poc-results).

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
| Structured output | **native GBNF + schema→grammar** | **none built-in** (hand-roll logit masking) | native (uses llama.cpp GBNF) |
| Quantization | **GGUF** (k-quants, IQ, …), broad | GGUF + safetensors, narrower | GGUF |
| Accel: Metal / CUDA / CPU | **all, mature** | Metal/CUDA/CPU, younger | all (via llama.cpp) |
| Packaging / footprint | C++ build (cmake) in the dep tree; one binary | **pure Rust, no C toolchain**; one binary | separate install + daemon |
| Model management | you fetch the GGUF | you fetch weights | **`ollama pull`**, easy |
| Cross-platform | macOS/Linux/Windows | macOS/Linux/Windows | macOS/Linux/Windows |
| Author moving parts | **none** (in-process) | **none** | a daemon to install & keep running |

The honest trade: candle's appeal is a pure-Rust build (no C++/cmake in the
tree). But it loses on the one feature Quill needs most. ollama is the easiest to
get working and manages models for the author, but it's a background daemon to
install, run, and version — a moving part the in-process route avoids.

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

### candle

Not run. candle can load and run a quantized Qwen on Metal, but it has **no
native constrained decoding**, so the structured-output path — the spike's
decisive criterion — would have to be hand-built (per-step logit masking against
a grammar) or replaced with parse-and-retry. That is exactly the work llama.cpp
already does and proved above, so a candle POC wouldn't change the recommendation.

## Recommendation & rationale

1. **Engine: llama.cpp / GGUF.** It's the only option with mature, native
   structured output (GBNF + schema→grammar), proven here on the target model and
   Metal, with the broadest quantization and acceleration support. This rules out
   candle as the primary engine.
2. **Integration: in-process Rust binding, no daemon — target.** Best author
   experience (single binary, nothing to install or keep running, Metal
   automatic). **Contingent on getting grammar-constrained decoding working:** the
   spike found `llama-cpp-2` 0.1.150's grammar sampler asserts, so the integration
   ticket must validate GBNF end-to-end before committing — by pinning/patching a
   working binding version, or evaluating **mistral.rs** (pure-Rust, advertises
   constrained/structured generation), as part of that ticket.
3. **Fallback: ollama.** If a reliable in-process grammar path can't be landed in
   time, ship the local backend against ollama first (proven, easy model
   management) and treat the daemon as a temporary cost. The `quill-ai` trait
   makes the engine swappable later without touching the features.

Net: prefer **llama.cpp over candle** decisively (structured output); prefer
**in-process over a daemon** where the binding allows it.

## Impact on the implementation tickets

- **#87 (backend behind the `quill-ai` trait + switching)** — target the
  llama.cpp engine in-process; its first job is to make grammar-constrained JSON
  work end-to-end (resolve the `llama-cpp-2` grammar regression or adopt
  mistral.rs), with ollama as the fallback transport. Map the trait's
  `json_schema` to a GBNF grammar via schema→grammar.
- **#86 (install & preparation)** — acquire a **GGUF** quant (engine-agnostic
  across llama.cpp and ollama), not safetensors; if shipping the ollama fallback,
  wrap it in a local `Modelfile` rather than depending on the registry (which was
  flaky here).
- **#85 (hardware-aware model selection)** — choose a Qwen **GGUF quant** to fit
  detected RAM/VRAM (e.g. Q4_K_M as the default sweet spot; smaller IQ quants on
  tight machines). The 1.5B Q4_K_M used here is ~1 GB and ran at ~80–97 tok/s on
  an M5; size the default tier from that.
