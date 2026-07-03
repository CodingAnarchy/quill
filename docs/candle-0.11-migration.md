# candle 0.9 → 0.11 upgrade and Qwen3 / Qwen3.5 evaluation (#735)

**Outcome.** The dependency upgrade to candle 0.11.0 was **completed and shipped**:
`candle-core`/`candle-transformers` are at 0.11.0, `tokenizers` at 0.22, and the
local backend builds, runs, and generates end-to-end on macOS/Metal with no source
changes. This document is the migration assessment (deliverables 2–5) that rides
alongside the code change.

The model family recommendation is **upgrade-only for now** — stay on Qwen2.5,
keep the candle bump. Qwen3 loads and works through candle's new `quantized_qwen3`
path (verified on a real GGUF on Metal), but switching the on-device family is a
larger, separate piece of work: Qwen3's default reasoning (`<think>`) behaviour,
a different KV-cache footprint (more KV heads → a bigger cache), and a new model
catalog all need their own ticket. Qwen3.5 is **not switchable** at all on candle
0.11 (see §3). Follow-ups are filed in §5.

---

## 1. The upgrade (deliverable 1) — done, green

Pins changed in `Cargo.toml`:

| crate | before | after |
| --- | --- | --- |
| `candle-core` | 0.9 | **0.11** |
| `candle-transformers` | 0.9 | **0.11** |
| `tokenizers` | 0.20 | **0.22** |
| `hf-hub` | 0.4 | 0.4 (unchanged) |

- candle 0.11 pulls `tokenizers` 0.22.2 transitively, so `tokenizers` had to move
  with it (0.20 no longer resolves under candle 0.11). `hf-hub` **0.4** still
  resolves cleanly alongside candle 0.11 — no bump needed, and `build_hub()`
  (#727) is unchanged. The macOS `metal` feature carries over unchanged.
- **No source changes were required.** The 0.9 → 0.11 API surface that Quill
  touches is unchanged (see §2), so `src/ai/local.rs`, `src/ai/model_select.rs`,
  and `src/ai/tool_loop.rs` compile as-is.

### Verification

All gates green (default and `--features local`):

- `cargo fmt --check`, `cargo build`, `cargo clippy --all-targets`, `cargo test`
- `cargo build --features local`, `cargo clippy --features local --all-targets`,
  `cargo test --features local` (1535 passed, 18 ignored — the ignored ones are
  the network/model-download tests, kept hermetic per #531)

End-to-end on Metal (the AC1 runtime check), via the existing ignored test:

```sh
cargo test --features local auto_download_then_generate -- --ignored --nocapture
```

downloaded `Qwen/Qwen2.5-0.5B-Instruct-GGUF`, loaded it with
`quantized_qwen2::ModelWeights::from_gguf`, ran a full constrained generation pass
on `Metal(MetalDevice)`, and returned schema-valid JSON
(`{"name":"Aldermoor","kind":"place"}`). Local AI works on candle 0.11.

---

## 2. Migration assessment: 0.9 → 0.11 breaking changes touching Quill (deliverable 2)

The headline is that **the API Quill depends on did not break**. The signatures of
the two calls the backend makes are byte-for-byte identical between 0.9.2 and
0.11.0:

```rust
// quantized_qwen2::ModelWeights — identical in 0.9 and 0.11
pub fn from_gguf<R: std::io::Seek + std::io::Read>(
    ct: gguf_file::Content, reader: &mut R, device: &Device,
) -> Result<Self>;
pub fn forward(&mut self, x: &Tensor, index_pos: usize) -> Result<Tensor>;
```

Per touched file:

- **`src/ai/local.rs`** — `gguf_file::Content::read`, `Qwen2::from_gguf(content,
  &mut file, &device)`, and `model.forward(&input, pos)` all keep their exact
  signatures. `Device::new_metal(0)` / `new_cuda(0)`, `Tensor::new`, `unsqueeze`,
  `squeeze`, `to_dtype`, `to_vec1::<f32>()` are unchanged. The Metal probe
  (`Device::Metal(m) => m.metal_device().recommended_max_working_set_size()`) is
  unchanged. **No edits.**
- **`src/ai/model_select.rs`** — only `best_device()` touches candle
  (`Device::new_metal`/`new_cuda`). Unchanged API. The one candle-*fact* baked
  into this file that the upgrade could invalidate is the KV-cache dtype comment
  (`KV_BYTES_PER_ELEM = 4.0`, "caches K/V in f32"). It's still correct for the
  Qwen2.5 path Quill runs (§4). **No edits.**
- **`src/ai/tool_loop.rs`** — pure JSON protocol (`serde_json`); it does not
  reference candle at all. **No edits.**
- **`tokenizers` 0.20 → 0.22** — `Tokenizer::from_file`, `.encode(text, true)`,
  `.get_ids()`, `.decode(ids, true)` are unchanged for Quill's usage. **No edits.**

What *did* change in candle 0.11 is almost entirely **additive** and lives in code
Quill doesn't call yet: the new `quantized_qwen3` / `quantized_qwen3_moe` model
impls, and a reworked `candle_nn::kv_cache` module (`ConcatKvCache`,
`InterleavedKvCache`, `RawInterleavedKvCache`). None of it is on the Qwen2.5 path.

**Bottom line:** for the code Quill ships, 0.9 → 0.11 is a clean version bump. The
breaking-change risk is all in the *optional* model-family switch, not the upgrade.

---

## 3. Qwen3 / Qwen3.5 evaluation (deliverable 3)

### Qwen3: loads and works (verified on Metal)

Empirically confirmed against a real GGUF, not assumed. A throwaway probe loaded
`Qwen/Qwen3-0.6B-GGUF` (`Qwen3-0.6B-Q8_0.gguf`, GGUF `general.architecture ==
"qwen3"`) through candle 0.11's `quantized_qwen3::ModelWeights::from_gguf` on
`Metal(MetalDevice)` and generated coherent text
(`"<think>\nOkay, the user is asking for the capital of France..."`). So:

- ✅ `quantized_qwen3::from_gguf` **loads** an official Qwen3 GGUF on Metal.
- ✅ `forward(&input, offset)` has the same shape as the qwen2 path, so the decode
  loop in `local.rs` would drive it with only a type swap
  (`Qwen2` → `Qwen3` alias).
- ✅ Tied embeddings are handled (`output.weight` falls back to
  `token_embd.weight`) — needed because Qwen3 0.6B/1.7B/4B set
  `tie_word_embeddings = true`.

**Caveat — Qwen3 is a hybrid reasoning model.** By default it emits a
`<think>…</think>` block before its answer (seen directly in the probe output).
Implications for Quill:

- The **constrained-JSON path** (`constrain::Constraint`, used by every tracker
  and the tool loop) is *unaffected*: the constraint rejects any token that isn't
  a valid schema prefix from the very first generated token, so the model can
  never emit `<think>` when a schema is active. Structured output still works.
- The **unconstrained path** (plain Q&A, the assistant's final text turn) *would*
  surface raw `<think>…</think>` reasoning as prose. Switching to Qwen3 needs the
  prompt to disable thinking — Qwen3 honours `enable_thinking=false` in its chat
  template (equivalently a `/no_think` directive), or the backend must strip the
  `<think>…</think>` span before returning `Response.text`. This is a small,
  contained change but it's a *required* one, so it belongs in the switch ticket,
  not this upgrade.

**Quant/repo caveat.** The official `Qwen/Qwen3-0.6B-GGUF` and
`Qwen/Qwen3-1.7B-GGUF` repos ship **only `Q8_0`** — no `Q4_K_M`. `Q4_K_M` (Quill's
current quality-per-byte sweet spot) is only in the `Qwen3-4B-GGUF` and larger
repos. So a Qwen3 catalog can't use one uniform quant across all sizes the way the
Qwen2.5 table does; the small tiers would be `Q8_0` (≈ still small in absolute
terms) and the 4B+ tiers `Q4_K_M`. See §5's catalog.

### Qwen3.5: NOT switchable on candle 0.11

Confirmed against the live model config, not assumed:

- `Qwen/Qwen3.5-4B/config.json` → `architectures: ["Qwen3_5ForConditionalGeneration"]`,
  `model_type: "qwen3_5"`, with an `image_token_id`, a `text_config`/vision split,
  and a `video_preprocessor_config.json` in the repo. **Qwen3.5 is a multimodal
  (vision-language) conditional-generation model, not a plain text LM.**
- candle 0.11 has **no `quantized_qwen3_5`** impl. Its `quantized_qwen3` hard-codes
  GGUF metadata keys under the `qwen3.*` prefix
  (`qwen3.attention.head_count`, `qwen3.block_count`, …). A `qwen3_5` GGUF is
  exported under a different arch, so `md_get("qwen3.…")` would fail with
  "cannot find … in metadata" — it will **not** load through the qwen3 path.
- The third-party `unsloth/Qwen3.5-9B-MTP-GGUF` is a **multi-token-prediction
  (MTP)** variant; candle's dense `quantized_qwen3` doesn't implement MTP either.
- The base `Qwen/Qwen3.5` repo returns 401 (gated); the sized repos
  (`Qwen3.5-4B`, `Qwen3.5-9B`) are open, but that's moot given the arch mismatch.

**Conclusion:** a straight Qwen3.5 swap is blocked by candle, full stop. It would
need an upstream `quantized_qwen3_5` impl (and Quill would then be adopting a VL
model — out of scope for the text backend). Do not pursue Qwen3.5 now.

### MoE note

candle 0.11 also ships `quantized_qwen3_moe`, but its `from_gguf` requires
`qwen3moe.*` metadata (a different arch again). The Qwen3 sizes Quill would
actually recommend (0.6B–14B) are **dense**, so the plain `quantized_qwen3` path
is the right one; MoE isn't needed.

---

## 4. KV-cache finding (deliverable 4)

**The 0.11 Qwen3 path does *not* fix, and does not meaningfully change, the #728
memory math on the hardware Quill targets (Metal/CUDA).**

What changed in candle 0.11: the qwen3 attention uses the new
`candle_nn::kv_cache` types instead of a hand-rolled `Option<(Tensor, Tensor)>`.
There are three, chosen by device:

- **GPU (Metal/CUDA):** `ConcatKvCache` — which is *exactly* the old behaviour. Its
  `append` does `Tensor::cat([cache, new], dim)` and stores whatever dtype it's
  handed. In `quantized_qwen3` the K/V tensors flow from `q/k/v_proj.forward`
  (a `QMatMul` that dequantizes to **F32**, since `Embedding` is built from
  `dequantize()` which yields F32) → so **the GPU KV cache is still F32 and still
  grows unbounded, one token's worth of `cat` per step.** Same 4 bytes/element,
  same linear growth as Qwen2.5 0.9. `KV_BYTES_PER_ELEM = 4.0` remains correct.
- **CPU only:** `InterleavedKvCache` + `RawInterleavedKvCache` (the latter a
  *bounded* ring of `max_seq = 4096`) feed a fused f32 flash-decode kernel. This
  is the only bounded/interesting cache — and it's **CPU-gated** (`device.is_cpu()`),
  so Quill never hits it on Metal or CUDA. It's also fixed at 4096 tokens, not a
  long-context path.

The `dtype` field the qwen3 model reads from GGUF `general.dtype` (defaulting to
**F16**) is a red herring for the cache: it's used only for the **RoPE cos/sin
table** and the **causal mask**, not for K/V storage. No KV-quantization option
exists on this path.

**Effect on #728.** None on the numbers that matter:

- No change to the f32, unbounded GPU KV cache → the memory formula
  (`weights + 2·layers·kv_heads·head_dim·4·tokens + overhead`) stands.
- Note that Qwen3's *architecture* makes the KV cache **bigger**, not smaller, at
  the small tiers: Qwen3 uses **8 KV heads** across the whole family (0.6B–14B),
  where Qwen2.5 uses 2 KV heads at 0.5B–3B. So a same-size Qwen3 model has a
  ~2–4× larger KV cache per token than the Qwen2.5 row it replaces — the memory
  table would have to be *re-derived*, not reused, if the family switches (see §5).
- The 7B-1M "drop it / gate on a 128 GB box" decision in #728 is unchanged: candle
  0.11 offers no KV-quant or bounded-cache relief on GPU, and Qwen3 has no ~1M
  extended-context GGUF anyway (native window is 40 960; longer needs YaRN at
  runtime, which candle's quantized path doesn't wire up).

---

## 5. Recommendation and follow-ups (deliverable 5)

### Recommendation: **upgrade-only now; defer the family switch to its own ticket.**

The candle 0.11 bump is a clean, low-risk win (maintained line, new model support
available for later, no code churn, all gates green) and it's **shipped in this
PR**. Switching the on-device family to Qwen3 is worthwhile for quality but is a
distinct, non-trivial change — it needs the `<think>` handling, a re-derived
memory table (Qwen3's 8 KV heads change the KV math), a mixed-quant catalog, and
its own end-to-end validation. Bundling it here would either half-do it or bloat
this PR. Qwen3.5 is off the table on candle 0.11 (§3).

### Refreshed Qwen3 catalog (for the switch ticket, when it happens)

Dense Qwen3, `general.architecture = "qwen3"`, all verified live on HF. Native
context is **40 960** tokens across the family (up from Qwen2.5's 32 768). Arch
columns are from each model card (`num_hidden_layers`, `num_key_value_heads`,
`hidden_size / num_attention_heads`):

| params | GGUF repo | file (quant) | size | layers | kv_heads | head_dim | tokenizer repo | tie |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| 0.6B | `Qwen/Qwen3-0.6B-GGUF` | `Qwen3-0.6B-Q8_0.gguf` (Q8_0*) | 0.64 GB | 28 | 8 | 128 | `Qwen/Qwen3-0.6B` | ✓ |
| 1.7B | `Qwen/Qwen3-1.7B-GGUF` | `Qwen3-1.7B-Q8_0.gguf` (Q8_0*) | 1.83 GB | 28 | 8 | 128 | `Qwen/Qwen3-1.7B` | ✓ |
| 4B | `Qwen/Qwen3-4B-GGUF` | `Qwen3-4B-Q4_K_M.gguf` | 2.50 GB | 36 | 8 | 128 | `Qwen/Qwen3-4B` | ✓ |
| 8B | `Qwen/Qwen3-8B-GGUF` | `Qwen3-8B-Q4_K_M.gguf` | 5.03 GB | 36 | 8 | 128 | `Qwen/Qwen3-8B` | ✗ |
| 14B | `Qwen/Qwen3-14B-GGUF` | `Qwen3-14B-Q4_K_M.gguf` | 9.00 GB | 40 | 8 | 128 | `Qwen/Qwen3-14B` | ✗ |

\* The official 0.6B/1.7B GGUF repos ship **only Q8_0** — there is no Q4_K_M there,
so a Qwen3 catalog is mixed-quant (Q8_0 for the two smallest tiers, Q4_K_M from 4B
up). Third-party `Q4_K_M` GGUFs for 0.6B/1.7B exist (e.g. `unsloth/…`, `bartowski/…`)
if a uniform quant is wanted — vet download/load per #379 before adopting.

Because Qwen3 has **8 KV heads at every size** (vs Qwen2.5's 2 at 0.5–3B), the
`kv_cache_gb`/`required_mem_gb`/`min_ram` figures in `model_select.rs` are **not
reusable as-is** — every row's KV term grows, and the selection thresholds shift.
The table must be re-derived from the columns above, not copied.

### Follow-up tickets to file

1. **Switch the on-device model family to Qwen3 (dense).** Adopt the §5 catalog in
   `model_select.rs`; alias the backend model type to
   `quantized_qwen3::ModelWeights` (drop-in `from_gguf`/`forward`); handle Qwen3's
   `<think>` on the unconstrained path (disable thinking via the chat template /
   `enable_thinking=false`, or strip `<think>…</think>` from `Response.text`);
   validate end-to-end on Metal. Depends on this PR (candle 0.11).
2. **Re-derive the memory table for Qwen3's 8-KV-head architecture (#728 follow-up).**
   Qwen3's KV cache is ~2–4× the Qwen2.5 cache at the small tiers, so
   `required_mem_gb` and the recommendation thresholds change; recompute the table
   and its unit tests against the §5 arch columns. Fold into (1) or run just ahead
   of it.
3. **Revisit the 7B-1M / long-context tier for Qwen3.** Qwen3 has no ~1M GGUF
   (native 40 960; longer needs runtime YaRN, unsupported on candle's quantized
   path). Decide whether to drop the extended-context tier entirely for the Qwen3
   family or keep Qwen2.5-7B-1M as a special-case long-context model. Ties into
   #728's 7B-1M sub-decision.

### Not recommended

- **Qwen3.5** — blocked by candle (multimodal `qwen3_5` arch, no candle impl). §3.
- **hf-hub 1.0-rc** — a release candidate; stay on 0.4, which resolves fine with
  candle 0.11 and keeps `build_hub()` (#727) unchanged.
