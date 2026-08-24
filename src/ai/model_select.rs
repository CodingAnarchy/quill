//! Hardware-aware Qwen model selection (#85): a data-driven table of quantized
//! Qwen options and a pure picker that chooses the largest one a machine can run
//! comfortably. Kept dependency-free and always compiled (so the selection logic
//! is unit-tested in the normal suite); the hardware probe and the `hf-hub`
//! download that act on a choice live in the `local`-gated backend.
//!
//! The on-device family is **Qwen3 dense** (#789): candle 0.11's
//! `quantized_qwen3` loads and generates from the official `Qwen/Qwen3-*-GGUF`
//! repos on Metal (verified in #735). Qwen3's architecture differs from Qwen2.5's
//! in the way that matters most here — **8 KV heads at every size** (vs Qwen2.5's
//! 2 at the small tiers), so the KV cache is 2–4× larger per token and the memory
//! table was re-derived from the Qwen3 model cards, not reused (#790). Qwen3's
//! native context window is **40960** tokens (up from Qwen2.5's 32768), and it has
//! no ~1M-context GGUF, so the old extended-context 7B-1M tier is dropped (#791).
//!
//! Always compiled (so the picker is unit-tested in the normal suite); the items
//! whose only non-test caller is the `local`-gated backend carry a tightly-scoped
//! `#[cfg_attr(not(feature = "local"), allow(dead_code))]` rather than a
//! module-wide allowance (#406).

/// The best candle device this machine offers — Metal, then CUDA, then CPU. The
/// two local backends (text #124 and image #240) share this probe.
#[cfg(feature = "local")]
pub fn best_device() -> candle_core::Device {
    use candle_core::Device;
    Device::new_metal(0)
        .or_else(|_| Device::new_cuda(0))
        .unwrap_or(Device::Cpu)
}

/// The accelerator a model will actually run on, which decides *which* memory
/// pool bounds it (#728):
/// - `Metal` — Apple Silicon **unified** memory: GPU and CPU share one pool, but
///   Metal caps a single working set at ~⅔–¾ of total RAM
///   (`recommendedMaxWorkingSetSize`), so the usable GPU budget is a fraction of
///   system RAM, not all of it.
/// - `Cuda` — a discrete GPU with its **own** VRAM, separate from and usually far
///   smaller than system RAM; the weights + KV cache must fit VRAM, so a 64 GB /
///   8 GB-VRAM box is bounded by the 8 GB.
/// - `Cpu` — no GPU: bounded by system RAM, and capped to a small model because
///   big ones are painfully slow without a GPU.
#[cfg_attr(not(feature = "local"), allow(dead_code))] // only the local backend's recommendation path calls these (#406)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accel {
    /// Apple Silicon unified memory (Metal): budget is a fraction of total RAM.
    Metal,
    /// Discrete GPU (CUDA): budget is its own VRAM, not system RAM.
    Cuda,
    /// CPU only — big models are too slow, so selection caps the size.
    Cpu,
}

/// The probed memory picture the picker reasons over (#728), so [`recommend`]
/// stays pure and unit-testable: the `local`-gated probe in the backend fills
/// this in from the real device, and the tests inject it directly.
#[cfg_attr(not(feature = "local"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Hardware {
    /// Which accelerator the model runs on — decides which pool bounds the pick.
    pub accel: Accel,
    /// Total system RAM in GB (rounded down). The bound on CPU, and the fallback
    /// pool a Metal budget is derived from when the probe couldn't read the
    /// working-set size.
    pub total_ram_gb: u32,
    /// The GPU memory pool the model actually runs in, in GB, when the probe could
    /// read it: VRAM for `Cuda`, `recommendedMaxWorkingSetSize` for `Metal`.
    /// `None` for `Cpu`, and for a GPU whose pool couldn't be probed (then a Metal
    /// budget falls back to [`METAL_WORKING_SET_FRACTION`] of `total_ram_gb`).
    pub device_mem_gb: Option<f64>,
}

/// The fraction of unified RAM Metal will let a single working set use — Apple's
/// `recommendedMaxWorkingSetSize` is ~⅔–¾ of total on Apple Silicon; ⅔ is the
/// conservative end, so a pick sized to it comfortably fits the real cap (#728).
const METAL_WORKING_SET_FRACTION: f64 = 0.67;

/// Headroom kept free for the OS and the editor running alongside inference
/// (#728): the picker sizes against the budget *minus* this reserve, so it never
/// chooses a model that merely fits and then thrashes. A flat GB reserve models
/// the fixed cost of "everything else on the machine"; it's deliberately generous
/// because the author edits while the model runs.
const HEADROOM_RESERVE_GB: f64 = 4.0;

/// Bytes per element in the KV cache. The candle path
/// (`candle_transformers::models::quantized_qwen3`, 0.11) dequantizes to and
/// **caches K/V in f32** — there is no `to_dtype(f16/bf16)` in that model — so the
/// cache is 4 bytes/element, double the fp16 figure often quoted for llama.cpp
/// (#728). Confirmed for Qwen3 in #735/#790: on the GPU path (candle's
/// `ConcatKvCache`) K/V flow from the `QMatMul` projections dequantized to f32 and
/// grow unbounded, one token's `cat` per step — 4 bytes/element, the same linear
/// growth as the old Qwen2.5 path. (The only bounded/f32-fused cache candle 0.11
/// adds is CPU-gated and fixed at 4096 tokens, which Quill never hits on
/// Metal/CUDA.) Revisit if a KV-quantized path lands.
const KV_BYTES_PER_ELEM: f64 = 4.0;

/// Runtime overhead beyond weights + KV cache: activation buffers, the compute
/// graph, and framework allocations. A modest flat margin — the weights and, for
/// long-context models, the KV cache dominate (#728).
const RUNTIME_OVERHEAD_GB: f64 = 1.0;

/// Bytes in a GB, for turning the byte-level KV formula into GB.
const BYTES_PER_GB: f64 = 1_000_000_000.0;

/// One quantized Qwen option. `tokenizer_repo` is the base instruct repo (which
/// ships `tokenizer.json`); the GGUF lives in the `-GGUF` companion repo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Model {
    pub params: &'static str,
    pub gguf_repo: &'static str,
    pub gguf_file: &'static str,
    pub tokenizer_repo: &'static str,
    /// The GGUF quantization label for this row (#789). The Qwen3 family is
    /// **mixed-quant**: the official 0.6B/1.7B GGUF repos ship **only `Q8_0`** (no
    /// `Q4_K_M`), while 4B and up ship `Q4_K_M`. Shown in the model manager so the
    /// author sees what they're getting.
    pub quant: &'static str,
    /// Approximate on-disk / download size, GB. Because the GGUF is mostly the
    /// quantized weights, this doubles as the **in-memory weight footprint** — the
    /// first term of the computed requirement (#728). From each Qwen3 GGUF repo's
    /// file listing (Q8_0 for the two smallest, Q4_K_M from 4B up).
    pub size_gb: f64,
    /// Transformer layers — the first factor of the per-token KV-cache size
    /// (#728). From the Qwen3 model cards (`num_hidden_layers`: 28 for 0.6B/1.7B,
    /// 36 for 4B/8B, 40 for 14B).
    pub layers: u32,
    /// Key/value attention heads. Qwen3 uses grouped-query attention (GQA) with
    /// **8 KV heads across the whole dense family** (`num_key_value_heads = 8` on
    /// every card, 0.6B–14B) — notably more than Qwen2.5's 2 at the small tiers,
    /// which is why the KV cache is 2–4× larger and the table was re-derived (#790).
    pub kv_heads: u32,
    /// Per-head dimension. Qwen3 fixes this at **128** on every card
    /// (`head_dim = 128`, decoupled from `hidden_size / num_attention_heads`), and
    /// candle reads it straight from the GGUF (`qwen3.attention.key_length`).
    /// `kv_heads * head_dim` is the K (and V) width cached per token per layer.
    pub head_dim: u32,
    /// The model's input context window, in tokens (#378). The router routes a
    /// pass on-device only when an available model's window can hold it. Qwen3's
    /// native window is [`STANDARD_CONTEXT_TOKENS`] (40960) across the dense family;
    /// it ships no extended-context GGUF, so every row is 40960 (#791).
    pub context_tokens: u32,
    /// A one-line characterization for the model manager (#379), so the author
    /// chooses on quality/speed/context rather than guessing at a size id.
    pub description: &'static str,
}

/// Qwen3's native context window, in tokens — 40960 across the dense family
/// (`Qwen/Qwen3-*` model cards; up from Qwen2.5's 32768). Used as the local
/// routing bound when the author hasn't configured a model set (the single
/// recommended model is one of these). Qwen3 has **no** ~1M-context GGUF — longer
/// windows need runtime YaRN, which candle's quantized path doesn't wire up — so
/// there is no extended-context tier and this window bounds every model (#791).
pub const STANDARD_CONTEXT_TOKENS: u32 = 40_960;

/// The context the [`recommend`]ation sizes the KV cache against — a realistic
/// working window, **not** the full 40960 native window (#790). The router only
/// ever routes a pass on-device when it fits the model's window, and the
/// assistant's structured passes (a scene note, a bible entry, a continuity flag)
/// run far below the native cap; the full-window KV cache is only paid when the
/// author actually feeds a whole-manuscript pass. Sizing the *recommendation* at
/// the full 40960 would let Qwen3's 8-KV-head cache (~9–13 GB at 40960) dominate
/// and exclude every model on ordinary hardware — a mis-recommendation, the exact
/// failure #728 fixed. So the pick is sized against this working context, while
/// [`Model::required_mem_gb`] at [`Model::context_tokens`] still reports the
/// worst-case full-window footprint for the model manager and headroom checks.
#[cfg_attr(not(feature = "local"), allow(dead_code))]
const RECOMMEND_CONTEXT_TOKENS: u32 = 8_192;

/// Quantized Qwen3 **dense** options, ascending by weight size (#789). Mixed-quant
/// by necessity: the official `Qwen/Qwen3-0.6B-GGUF` / `Qwen3-1.7B-GGUF` repos ship
/// **only `Q8_0`**, so the two smallest tiers are Q8_0 and 4B/8B/14B are `Q4_K_M`
/// (the quality-per-byte sweet spot). Native context is 40960 across the family
/// ([`STANDARD_CONTEXT_TOKENS`]); Qwen3 has no ~1M GGUF, so — unlike the old Qwen2.5
/// table — there is no extended-context row (#791). Update as new Qwen3 releases
/// land. Every row's GGUF filename is release-gate-vetted against its HuggingFace
/// repo by the `#[ignore]`d `catalog_gguf_files_exist_on_huggingface` check (#726),
/// so a row that names a file that doesn't exist (renamed, sharded, wrong quant) is
/// caught before a user's download 404s — run it whenever a row's repo/file changes.
///
/// The `layers` / `kv_heads` / `head_dim` columns are the Qwen3 model-card
/// architecture (`num_hidden_layers`, `num_key_value_heads`, `head_dim`); they
/// feed the computed memory requirement in [`Model::required_mem_gb`] — see that
/// method for the derivation a reader can check against each card (#790). Every row
/// has **8 KV heads and head_dim 128**, so the KV cache scales only with `layers`.
pub const MODELS: &[Model] = &[
    Model {
        params: "0.6B",
        gguf_repo: "Qwen/Qwen3-0.6B-GGUF",
        gguf_file: "Qwen3-0.6B-Q8_0.gguf",
        tokenizer_repo: "Qwen/Qwen3-0.6B",
        quant: "Q8_0",
        size_gb: 0.64,
        layers: 28,
        kv_heads: 8,
        head_dim: 128,
        context_tokens: STANDARD_CONTEXT_TOKENS,
        description: "Tiny — instant, for quick lookups",
    },
    Model {
        params: "1.7B",
        gguf_repo: "Qwen/Qwen3-1.7B-GGUF",
        gguf_file: "Qwen3-1.7B-Q8_0.gguf",
        tokenizer_repo: "Qwen/Qwen3-1.7B",
        quant: "Q8_0",
        size_gb: 1.83,
        layers: 28,
        kv_heads: 8,
        head_dim: 128,
        context_tokens: STANDARD_CONTEXT_TOKENS,
        description: "Small — fast, for light passes",
    },
    Model {
        params: "4B",
        gguf_repo: "Qwen/Qwen3-4B-GGUF",
        gguf_file: "Qwen3-4B-Q4_K_M.gguf",
        tokenizer_repo: "Qwen/Qwen3-4B",
        quant: "Q4_K_M",
        size_gb: 2.50,
        layers: 36,
        kv_heads: 8,
        head_dim: 128,
        context_tokens: STANDARD_CONTEXT_TOKENS,
        description: "Compact — a balance for modest machines",
    },
    Model {
        params: "8B",
        gguf_repo: "Qwen/Qwen3-8B-GGUF",
        gguf_file: "Qwen3-8B-Q4_K_M.gguf",
        tokenizer_repo: "Qwen/Qwen3-8B",
        quant: "Q4_K_M",
        size_gb: 5.03,
        layers: 36,
        kv_heads: 8,
        head_dim: 128,
        context_tokens: STANDARD_CONTEXT_TOKENS,
        description: "Capable — solid quality at 40k context",
    },
    Model {
        params: "14B",
        gguf_repo: "Qwen/Qwen3-14B-GGUF",
        gguf_file: "Qwen3-14B-Q4_K_M.gguf",
        tokenizer_repo: "Qwen/Qwen3-14B",
        quant: "Q4_K_M",
        size_gb: 9.00,
        layers: 40,
        kv_heads: 8,
        head_dim: 128,
        context_tokens: STANDARD_CONTEXT_TOKENS,
        description: "Largest — best quality at 40k context",
    },
];

/// One on-device **text-embedding** option for the grounded-Ask retrieval index
/// (#831). Unlike the generative [`Model`] table (quantized GGUF Qwen), an
/// embedding model is a small BERT-family encoder loaded from safetensors + its
/// `config.json` + `tokenizer.json` — the standard sentence-transformers layout
/// candle's `bert` model reads. It emits one fixed-width vector per passage, which
/// the index ranks by cosine similarity; it never generates prose, so it sits
/// squarely inside the AI boundary (it indexes prose to *retrieve* it, #831).
///
/// The whole struct is data only, always compiled (so the catalog and the index's
/// pure logic are unit-tested in the normal suite); the candle loader that acts on
/// a row lives in the `local`-gated [`crate::ai::embed`] backend.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EmbedModel {
    /// The stable id / config key (`"minilm"`), used in `[ai] embed_model` and as
    /// the index's model tag so a model swap invalidates a stale index.
    pub id: &'static str,
    /// The HuggingFace repo holding `model.safetensors`, `config.json`, and
    /// `tokenizer.json` — the sentence-transformers layout candle's `bert` reads.
    pub repo: &'static str,
    /// The safetensors weights file in `repo` (usually `model.safetensors`).
    pub weights_file: &'static str,
    /// Output embedding width (the BERT hidden size), for a caller that pre-sizes
    /// buffers or sanity-checks a fake embedder in tests.
    pub dim: usize,
    /// Approximate download / on-disk size, MB — surfaced in the model manager.
    pub size_mb: u32,
    /// A one-line characterization for the model manager.
    pub description: &'static str,
}

/// On-device text-embedding options for the retrieval index (#831). One small,
/// well-tested sentence-transformers encoder to start: `all-MiniLM-L6-v2`
/// (22M params, 384-dim, ~90 MB) — the canonical candle `bert` embedding model,
/// fast on CPU and tiny to fetch. Each row's `weights_file` is release-gate-vetted
/// against its HuggingFace repo by the `#[ignore]`d
/// `embed_catalog_files_exist_on_huggingface` check (#726), so a row naming a file
/// that doesn't exist is caught before a user's download 404s.
pub const EMBED_MODELS: &[EmbedModel] = &[EmbedModel {
    id: "minilm",
    repo: "sentence-transformers/all-MiniLM-L6-v2",
    weights_file: "model.safetensors",
    dim: 384,
    size_mb: 90,
    description: "MiniLM-L6 — 384-dim, ~90 MB, fast on CPU",
}];

impl EmbedModel {
    /// The default embedding model — the first (and smallest) catalog row.
    pub const fn default_model() -> &'static Self {
        &EMBED_MODELS[0]
    }

    /// The embedding model with the given [`id`](Self::id), or `None` if unknown.
    pub fn by_id(id: &str) -> Option<&'static Self> {
        EMBED_MODELS.iter().find(|m| m.id == id)
    }

    /// The HuggingFace `resolve` URL for this row's safetensors weights — the exact
    /// file [`crate::ai::embed`] fetches. Used by the release-gated catalog vetting
    /// check to HEAD every row and catch a filename that doesn't exist *before* a
    /// user's download 404s (#726), exactly like [`Model::gguf_resolve_url`]. A live
    /// file answers with a `302` redirect to the CDN.
    #[cfg(test)]
    pub fn weights_resolve_url(&self) -> String {
        format!(
            "https://huggingface.co/{}/resolve/main/{}",
            self.repo, self.weights_file
        )
    }
}

impl Model {
    /// The stable id for this model — its parameter size (`"4B"`), used as the
    /// config key in `[ai] local_models` and the cache key for the loaded model.
    pub const fn id(&self) -> &'static str {
        self.params
    }

    /// The KV cache this model holds at `context_tokens`, in GB (#728). The cache
    /// stores a key **and** a value vector per token, per layer, of width
    /// `kv_heads * head_dim`, at [`KV_BYTES_PER_ELEM`] bytes each:
    ///
    /// ```text
    /// KV = 2 (K+V) * layers * kv_heads * head_dim * bytes_per_elem * tokens
    /// ```
    ///
    /// Worked check against the Qwen3-4B card (36 layers, 8 KV heads, head_dim 128,
    /// f32): per token that's `2*36*8*128*4 = 294,912` bytes ≈ 288 KiB — ~2.6× the
    /// old Qwen2.5-3B row's per-token cost, entirely from the jump 2→8 KV heads
    /// (#790). At the 8k working window the recommendation uses that's ≈ 2.4 GB, and
    /// at the full 40960 native window ≈ 12.1 GB — which is why the recommendation
    /// is sized against a working context, not the full window (see
    /// [`RECOMMEND_CONTEXT_TOKENS`]).
    #[cfg_attr(not(feature = "local"), allow(dead_code))]
    pub fn kv_cache_gb(&self, context_tokens: u32) -> f64 {
        let per_token = 2.0
            * f64::from(self.layers)
            * f64::from(self.kv_heads)
            * f64::from(self.head_dim)
            * KV_BYTES_PER_ELEM;
        per_token * f64::from(context_tokens) / BYTES_PER_GB
    }

    /// The memory this model needs to run at `context_tokens`, in GB (#728) —
    /// derived from its real footprint rather than an unexplained constant:
    ///
    /// ```text
    /// requirement = weights (GGUF size) + KV cache(context) + runtime overhead
    /// ```
    ///
    /// with the KV cache from [`kv_cache_gb`](Model::kv_cache_gb) and a flat
    /// [`RUNTIME_OVERHEAD_GB`] margin for activations and framework buffers. A
    /// reader can verify each term: `size_gb` against the model card's GGUF size,
    /// the KV term against the architecture columns, and the overhead as the
    /// documented margin.
    #[cfg_attr(not(feature = "local"), allow(dead_code))]
    pub fn required_mem_gb(&self, context_tokens: u32) -> f64 {
        self.size_gb + self.kv_cache_gb(context_tokens) + RUNTIME_OVERHEAD_GB
    }

    /// The HuggingFace `resolve` URL for this row's GGUF weights — the exact file
    /// [`crate::ai::local::LocalBackend::for_model`] fetches (`hf-hub`'s
    /// `model(repo).get(file)` resolves the same path). Used by the release-gated
    /// catalog vetting check (`catalog_gguf_files_exist_on_huggingface`) to HEAD
    /// every row and catch a filename that doesn't exist *before* a user's download
    /// 404s (#726). A live file answers this URL with a `302` redirect to the CDN.
    ///
    /// Only the vetting check consumes it, so it's `#[cfg(test)]`-gated — it isn't a
    /// production affordance, and gating it also keeps it off the `dead_code` radar
    /// in every non-test build (default and `--features local`).
    #[cfg(test)]
    pub fn gguf_resolve_url(&self) -> String {
        format!(
            "https://huggingface.co/{}/resolve/main/{}",
            self.gguf_repo, self.gguf_file
        )
    }

    /// A compact context-window label for the model manager (#379): `"40k"`, `"1M"`.
    pub fn context_label(&self) -> String {
        if self.context_tokens >= 1_000_000 {
            format!("{}M", self.context_tokens / 1_000_000)
        } else {
            format!("{}k", self.context_tokens / 1_000)
        }
    }
}

/// The model with the given [`id`](Model::id), or `None` if unknown.
pub fn by_id(id: &str) -> Option<&'static Model> {
    MODELS.iter().find(|m| m.id() == id)
}

/// Resolve the author's chosen model ids (`[ai] local_models`) against the model
/// table, in ascending size order regardless of how they were listed; unknown
/// ids are dropped. Empty when none are configured — the caller then falls back
/// to the single [`recommend`]ed model.
pub fn available(ids: &[String]) -> Vec<&'static Model> {
    MODELS
        .iter()
        .filter(|m| ids.iter().any(|id| id == m.id()))
        .collect()
}

/// The context window that bounds on-device routing for the configured set
/// (#378): the largest window among the chosen models, or — when none are
/// configured — the standard window, since the single recommended model is one of
/// the standard ones. The router routes a pass local only when it fits this.
pub fn local_window(ids: &[String]) -> u32 {
    available(ids)
        .iter()
        .map(|m| m.context_tokens)
        .max()
        .unwrap_or(STANDARD_CONTEXT_TOKENS)
}

/// Below this many input tokens a request is "small" — a thesaurus lookup, a
/// single-line check — and the smallest downloaded model serves it fastest.
const SMALL_REQUEST_TOKENS: u32 = 2_000;

/// Pick the best downloaded model for a request of `input_tokens` (#377/#378):
/// among the models whose context window can hold the request, the smallest for a
/// small request (lowest latency) and the largest for a substantial one (best
/// quality). `available` must be ascending by size (as [`available`] returns it).
/// `None` when nothing downloaded can hold the request — the router then routes
/// it to the cloud. This is what lets a larger model take a bigger pass the
/// smallest ones would rather leave to a quicker peer.
pub fn pick_for(input_tokens: u32, available: &[&'static Model]) -> Option<&'static Model> {
    let mut fits = available
        .iter()
        .copied()
        .filter(|m| m.context_tokens >= input_tokens);
    if input_tokens <= SMALL_REQUEST_TOKENS {
        fits.next()
    } else {
        fits.next_back()
    }
}

/// On a CPU-only machine, don't recommend anything bigger than this — larger
/// models are painfully slow without a GPU.
#[cfg_attr(not(feature = "local"), allow(dead_code))]
const CPU_MAX_PARAMS: &str = "4B";

/// A recommendation: the chosen model (if any is viable) and the human reasoning.
#[cfg_attr(not(feature = "local"), allow(dead_code))]
pub struct Recommendation {
    pub model: Option<&'static Model>,
    pub reasoning: String,
}

/// The memory budget (GB) a model may actually use on this hardware, after
/// respecting the accelerator's pool and reserving OS/editor headroom (#728):
/// - `Cuda`: the GPU's own VRAM (the weights can't spill to system RAM), minus
///   headroom. A probed VRAM figure is required; without it the budget is 0.
/// - `Metal`: the probed unified-memory working-set cap
///   (`recommendedMaxWorkingSetSize`), or — when the probe couldn't read it —
///   [`METAL_WORKING_SET_FRACTION`] of total RAM, minus headroom.
/// - `Cpu`: total system RAM, minus headroom.
///
/// Clamped at 0 so a tiny machine yields a 0 budget (→ no model), never a
/// negative one.
#[cfg_attr(not(feature = "local"), allow(dead_code))]
fn usable_budget_gb(hw: Hardware) -> f64 {
    let pool = match hw.accel {
        Accel::Cuda => hw.device_mem_gb.unwrap_or(0.0),
        Accel::Metal => hw
            .device_mem_gb
            .unwrap_or_else(|| f64::from(hw.total_ram_gb) * METAL_WORKING_SET_FRACTION),
        Accel::Cpu => f64::from(hw.total_ram_gb),
    };
    (pool - HEADROOM_RESERVE_GB).max(0.0)
}

/// Pick the largest Qwen3 whose computed footprint (weights + KV cache at the
/// working context + overhead) fits this machine's *usable* memory budget — the
/// pool the model actually runs in (VRAM / Metal working set / RAM), minus
/// OS+editor headroom — and, on CPU, isn't too big to be usable (#728). The KV
/// term is sized at [`RECOMMEND_CONTEXT_TOKENS`], the realistic working window,
/// not Qwen3's full 40960 native window (whose 8-KV-head cache would otherwise
/// dominate and exclude every model on ordinary hardware, #790). Because the
/// budget already reserves headroom and the requirement is the real footprint, the
/// pick leaves comfortable free memory rather than merely fitting. `None` with an
/// explanation when nothing fits — the caller then uses Claude.
#[cfg_attr(not(feature = "local"), allow(dead_code))]
pub fn recommend(hw: Hardware) -> Recommendation {
    let budget = usable_budget_gb(hw);
    let cpu_cap = MODELS.iter().position(|m| m.params == CPU_MAX_PARAMS);
    let choice = MODELS
        .iter()
        .enumerate()
        .filter(|(_, m)| m.required_mem_gb(RECOMMEND_CONTEXT_TOKENS) <= budget)
        .filter(|(i, _)| hw.accel != Accel::Cpu || cpu_cap.is_none_or(|cap| *i <= cap))
        .map(|(_, m)| m)
        .next_back(); // table is ascending → last passing entry is the largest

    // Both arms build a multi-line explanation (the Some arm computes a cap note
    // first); a map_or_else closure would be less readable than this match.
    #[allow(clippy::option_if_let_else)]
    let reasoning = match choice {
        Some(m) => {
            let cap = if hw.accel == Accel::Cpu && m.params == CPU_MAX_PARAMS {
                " (capped for CPU-only; a GPU would allow a larger model)"
            } else {
                ""
            };
            format!(
                "{} usable on {} → Qwen3-{} ({}, ~{:.1} GB weights, ~{:.1} GB total at \
                 {} working ctx){}",
                budget_label(budget),
                accel_label(hw),
                m.params,
                m.quant,
                m.size_gb,
                m.required_mem_gb(RECOMMEND_CONTEXT_TOKENS),
                RECOMMEND_CONTEXT_TOKENS / 1_000,
                cap,
            )
        }
        None => format!(
            "{} usable on {} is below the ~{:.1} GB the smallest model needs (weights + KV \
             cache + overhead) — local inference isn't advisable here; use the Claude backend \
             instead",
            budget_label(budget),
            accel_label(hw),
            MODELS[0].required_mem_gb(RECOMMEND_CONTEXT_TOKENS),
        ),
    };
    Recommendation {
        model: choice,
        reasoning,
    }
}

#[cfg_attr(not(feature = "local"), allow(dead_code))]
fn budget_label(gb: f64) -> String {
    format!("{gb:.1} GB")
}

#[cfg_attr(not(feature = "local"), allow(dead_code))]
const fn accel_label(hw: Hardware) -> &'static str {
    match hw.accel {
        Accel::Metal => "Apple Silicon (unified memory)",
        Accel::Cuda => "a discrete GPU (VRAM)",
        Accel::Cpu => "CPU only",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;

    /// Env var that opts the network catalog-vetting checks in. The `cargo perf`
    /// timing sweep (`--ignored`) does **not** set it, so those two checks no-op
    /// there and the gate stays hermetic/offline (#1022). The dedicated
    /// `cargo perf-catalog` alias sets it to run them deliberately (#726).
    const CATALOG_CHECK_ENV: &str = "QUILL_CATALOG_CHECK";

    /// True only when the catalog-vetting env guard is set (to any non-empty
    /// value). When false the two HuggingFace HEAD checks return early — they're
    /// `#[ignore]`d for the network dependency, but the `--ignored` timing sweep
    /// would otherwise still run them and false-fail offline (#1022).
    fn catalog_check_opted_in() -> bool {
        env::var_os(CATALOG_CHECK_ENV).is_some_and(|v| !v.is_empty())
    }

    /// A Metal (unified-memory) machine with `ram_gb` total RAM.
    fn metal(ram_gb: u32) -> Hardware {
        Hardware {
            accel: Accel::Metal,
            total_ram_gb: ram_gb,
            device_mem_gb: None,
        }
    }

    /// A CUDA box with `vram_gb` of VRAM and (deliberately generous) `ram_gb`
    /// system RAM — to prove the pick is bounded by VRAM, not RAM.
    fn cuda(vram_gb: f64, ram_gb: u32) -> Hardware {
        Hardware {
            accel: Accel::Cuda,
            total_ram_gb: ram_gb,
            device_mem_gb: Some(vram_gb),
        }
    }

    fn cpu(ram_gb: u32) -> Hardware {
        Hardware {
            accel: Accel::Cpu,
            total_ram_gb: ram_gb,
            device_mem_gb: None,
        }
    }

    fn pick(hw: Hardware) -> Option<&'static str> {
        recommend(hw).model.map(|m| m.params)
    }

    #[test]
    fn picks_largest_model_that_fits_the_unified_budget_with_headroom() {
        // Metal budget = 0.67 * RAM - 4 GB headroom; requirement is the computed
        // footprint at the 8k working context (0.6B ~3.5, 1.7B ~4.7, 4B ~5.9,
        // 8B ~8.5, 14B ~12.7 GB) — see the derivation on `required_mem_gb`.
        assert_eq!(pick(metal(64)), Some("14B")); // budget ~38.9 → 14B fits
        assert_eq!(pick(metal(32)), Some("14B")); // budget ~17.4 → 14B (~12.7)
        assert_eq!(pick(metal(24)), Some("8B")); // budget ~12.1 → 8B, not 14B
        assert_eq!(pick(metal(16)), Some("4B")); // budget ~6.7 → 4B, not 8B
        assert_eq!(pick(metal(12)), Some("0.6B")); // budget ~4.0 → 0.6B (~3.5)
    }

    #[test]
    fn a_machine_too_small_for_any_model_gets_claude_not_a_too_large_pick() {
        // 8 GB Apple Silicon: budget ~1.4 GB < smallest model's ~3.5 GB → Claude.
        assert_eq!(pick(metal(8)), None);
        assert!(recommend(metal(8)).reasoning.contains("isn't advisable"));
    }

    #[test]
    fn cuda_is_bounded_by_vram_not_system_ram() {
        // The bug: 64 GB RAM / 8 GB VRAM was told to run a huge model. VRAM budget
        // = 8 - 4 = 4 GB → only 0.6B (~3.5 GB) fits, despite the 64 GB of RAM.
        assert_eq!(pick(cuda(8.0, 64)), Some("0.6B"));
        // A roomy 24 GB card: budget 20 GB → 14B (~12.7) fits.
        assert_eq!(pick(cuda(24.0, 64)), Some("14B"));
        // A tiny 4 GB card can't comfortably run anything → Claude.
        assert_eq!(pick(cuda(4.0, 64)), None);
    }

    #[test]
    fn cpu_only_is_capped_even_with_lots_of_ram() {
        // 64 GB but CPU-only → capped at 4B, not 14B.
        assert_eq!(pick(cpu(64)), Some("4B"));
        assert!(recommend(cpu(64)).reasoning.contains("capped for CPU"));
        // Apple Silicon with the same RAM gets the big model.
        assert_eq!(pick(metal(64)), Some("14B"));
    }

    #[test]
    fn recommendation_is_sized_at_the_working_context_not_the_full_window() {
        // The re-derivation's crux (#790): Qwen3's 8-KV-head cache at the full
        // 40960 window is huge (0.6B alone needs ~11 GB), so sizing the pick there
        // would exclude every model on an ordinary 16 GB Mac. Sized at the 8k
        // working context it fits comfortably, while the FULL-window figure the UI
        // reports really is that large.
        assert!(pick(metal(16)).is_some(), "a 16 GB Mac still gets a model");
        let m = by_id("0.6B").unwrap();
        assert!(
            m.required_mem_gb(STANDARD_CONTEXT_TOKENS) > 10.0,
            "full-window footprint is large: {}",
            m.required_mem_gb(STANDARD_CONTEXT_TOKENS)
        );
        assert!(
            m.required_mem_gb(RECOMMEND_CONTEXT_TOKENS) < 4.0,
            "working-context footprint is modest: {}",
            m.required_mem_gb(RECOMMEND_CONTEXT_TOKENS)
        );
    }

    #[test]
    fn footprint_math_is_checkable_against_the_qwen3_4b_card() {
        // 4B: 36 layers, 8 KV heads, head_dim 128, f32 KV → ~288 KiB/token — the
        // 8-KV-head arch (#790), ~4× a Qwen2.5 small tier's 2-KV-head cost.
        let m = by_id("4B").unwrap();
        // KV at 40960 ≈ 12.08 GB (2*36*8*128*4*40960 / 1e9).
        assert!(
            (m.kv_cache_gb(40_960) - 12.08).abs() < 0.05,
            "{}",
            m.kv_cache_gb(40_960)
        );
        // Requirement at the 8k working ctx = weights 2.5 + KV 2.42 + overhead 1.0
        // ≈ 5.92 GB.
        assert!(
            (m.required_mem_gb(8_192) - 5.92).abs() < 0.05,
            "{}",
            m.required_mem_gb(8_192)
        );
        // KV scales linearly with context.
        assert!(
            2.0f64
                .mul_add(-m.kv_cache_gb(8_192), m.kv_cache_gb(16_384))
                .abs()
                < 1e-6
        );
    }

    #[test]
    fn every_row_uses_qwen3s_eight_kv_head_architecture() {
        // The re-derivation's defining fact (#790): every dense Qwen3 row has 8 KV
        // heads and head_dim 128 (vs Qwen2.5's 2 KV heads at the small tiers), so
        // the KV cache scales only with `layers`.
        assert!(MODELS.iter().all(|m| m.kv_heads == 8 && m.head_dim == 128));
    }

    #[test]
    fn reasoning_names_the_model_and_size() {
        let r = recommend(metal(24));
        assert!(
            r.reasoning.contains("Qwen3-8B") && r.reasoning.contains("GB"),
            "{}",
            r.reasoning
        );
    }

    #[test]
    fn table_is_ascending_by_weight_size() {
        // The picker and `pick_for` rely on ascending order; weight size is the
        // monotonic key (with no extended-context row, #791, total footprint is now
        // monotonic too, but weights remain the ordering key).
        assert!(MODELS.windows(2).all(|w| w[0].size_gb <= w[1].size_gb));
    }

    #[test]
    fn by_id_finds_models_and_rejects_unknowns() {
        assert_eq!(by_id("8B").map(|m| m.params), Some("8B"));
        assert!(by_id("999B").is_none());
    }

    #[test]
    fn available_resolves_ids_in_size_order_and_drops_unknowns() {
        // Listed out of order, with a bogus id — comes back ascending, bogus gone.
        let ids = vec!["8B".to_string(), "0.6B".to_string(), "bogus".to_string()];
        let got: Vec<&str> = available(&ids).iter().map(|m| m.params).collect();
        assert_eq!(got, vec!["0.6B", "8B"]);
        assert!(
            available(&[]).is_empty(),
            "no config → caller falls back to recommend()"
        );
    }

    #[test]
    fn pick_for_respects_size_and_context_window() {
        let set = available(&["0.6B".into(), "8B".into()]);
        // A thesaurus-sized request → the smallest that fits (fastest).
        assert_eq!(pick_for(500, &set).map(|m| m.params), Some("0.6B"));
        // A big request that still fits the 40k window → the largest (best quality).
        assert_eq!(pick_for(20_000, &set).map(|m| m.params), Some("8B"));
        // Bigger than the Qwen3 native window → nothing fits → cloud (None).
        assert!(
            pick_for(50_000, &set).is_none(),
            "no Qwen3 model holds 50k tokens (native window is 40960)"
        );
        // Nothing downloaded → nothing to pick.
        assert!(pick_for(500, &[]).is_none());
    }

    #[test]
    fn local_window_takes_the_largest_configured_or_the_standard_default() {
        // No set configured → the standard (recommended) model's window.
        assert_eq!(local_window(&[]), STANDARD_CONTEXT_TOKENS);
        // Any Qwen3 set → the native 40960 window (no extended-context tier, #791).
        assert_eq!(
            local_window(&["0.6B".into(), "8B".into()]),
            STANDARD_CONTEXT_TOKENS
        );
    }

    #[test]
    fn every_model_uses_qwen3s_native_forty_k_window() {
        // #791: Qwen3 has no ~1M GGUF, so every row is the native 40960 window —
        // there is no extended-context tier to route a whole-book pass to.
        assert!(MODELS
            .iter()
            .all(|m| m.context_tokens == STANDARD_CONTEXT_TOKENS));
        assert_eq!(STANDARD_CONTEXT_TOKENS, 40_960);
    }

    #[test]
    fn context_label_is_compact() {
        assert_eq!(by_id("8B").unwrap().context_label(), "40k");
    }

    #[test]
    fn the_catalog_is_the_qwen3_dense_family_with_the_expected_quants() {
        // #789: five dense sizes, mixed-quant — Q8_0 for 0.6B/1.7B (the only quant
        // in those official repos), Q4_K_M from 4B up.
        let ids: Vec<&str> = MODELS.iter().map(|m| m.params).collect();
        assert_eq!(ids, vec!["0.6B", "1.7B", "4B", "8B", "14B"]);
        assert_eq!(by_id("0.6B").unwrap().quant, "Q8_0");
        assert_eq!(by_id("1.7B").unwrap().quant, "Q8_0");
        assert_eq!(by_id("14B").unwrap().quant, "Q4_K_M");
        // Repos are the official Qwen3 GGUF companions.
        assert!(MODELS
            .iter()
            .all(|m| m.gguf_repo.starts_with("Qwen/Qwen3-")));
    }

    #[test]
    fn every_model_has_a_description() {
        assert!(MODELS.iter().all(|m| !m.description.is_empty()));
    }

    /// Release gate: every catalog row's GGUF filename actually exists in its
    /// HuggingFace repo (#726). This is the concrete failure the ticket was filed
    /// for — a catalog that named a single-file GGUF Qwen ships only as multi-part
    /// shards, so [`crate::ai::local::LocalBackend::for_model`]'s single-file fetch
    /// 404s on exactly the models a capable machine is recommended. A live weights
    /// file answers its `resolve/main/<file>` URL with a `302` redirect to the CDN;
    /// a `404` means the row points at a name that isn't there (renamed, sharded, or
    /// a wrong quant). We HEAD each row without following the redirect, so the check
    /// is a few tiny requests, never a multi-gigabyte download.
    ///
    /// **Opt-in / non-hermetic on purpose** — it reaches HuggingFace, so it's
    /// `#[ignore]`d and kept out of the default and `--features local` suites (they
    /// must never touch the network, #531). It's *also* kept out of the `cargo perf`
    /// timing sweep, which runs every `--ignored` test offline: this check only does
    /// its network work when the [`CATALOG_CHECK_ENV`] guard is set, so the sweep
    /// no-ops it and stays hermetic (#1022). Run it before a release, and whenever a
    /// `MODELS` row's `gguf_repo`/`gguf_file` changes, to catch a bad filename here
    /// rather than in a user's failed download, via the dedicated alias:
    /// `cargo perf-catalog`
    #[test]
    #[ignore = "release gate: reaches HuggingFace to vet catalog filenames; not hermetic (#726)"]
    fn catalog_gguf_files_exist_on_huggingface() {
        if !catalog_check_opted_in() {
            // Hermetic by default: the `cargo perf` timing sweep runs `--ignored`
            // offline and must not reach the network. Run `cargo perf-catalog`
            // (sets QUILL_CATALOG_CHECK) to vet the catalog deliberately (#1022).
            println!("skipping catalog HEAD checks: {CATALOG_CHECK_ENV} not set");
            return;
        }
        let mut missing = Vec::new();
        for m in MODELS {
            let url = m.gguf_resolve_url();
            // HEAD without following the redirect: a present file answers 302 (→ CDN),
            // a missing one 404. `max_redirects(0)` + not-an-error keeps the raw status.
            let status = ureq::head(&url)
                .config()
                .http_status_as_error(false)
                .max_redirects(0)
                .max_redirects_will_error(false)
                .build()
                .call()
                .map_or_else(
                    |e| {
                        missing.push(format!("{} — request error: {e}", m.id()));
                        0
                    },
                    |r| r.status().as_u16(),
                );
            println!("{:>5}  HTTP {status}  {url}", m.id());
            // 302 = present (redirect to CDN); 200 also counts as present. Anything
            // else (notably 404) means the catalog names a file that isn't there.
            if status != 302 && status != 200 {
                missing.push(format!("{} — HTTP {status} at {url}", m.id()));
            }
        }
        assert!(
            missing.is_empty(),
            "catalog rows point at GGUF files that don't exist on HuggingFace \
             (a user's download would 404): {missing:?}"
        );
    }

    #[test]
    fn embed_catalog_is_well_formed() {
        // The default is the first row, every id resolves, every dim is positive.
        assert_eq!(EmbedModel::default_model().id, "minilm");
        assert_eq!(EmbedModel::by_id("minilm"), Some(&EMBED_MODELS[0]));
        assert!(EmbedModel::by_id("does-not-exist").is_none());
        assert!(EMBED_MODELS
            .iter()
            .all(|m| m.dim > 0 && !m.description.is_empty()));
    }

    /// Release gate for the embedding catalog, mirroring
    /// [`catalog_gguf_files_exist_on_huggingface`] (#726): every [`EMBED_MODELS`]
    /// row's safetensors file actually exists in its HuggingFace repo, so a bad
    /// filename is caught here rather than in a user's failed download. HEAD without
    /// following the redirect — a present file answers `302` (→ CDN), a missing one
    /// `404`. **Opt-in / non-hermetic on purpose** (reaches HuggingFace), so it's
    /// `#[ignore]`d and kept out of the default and `--features local` suites (they
    /// must never touch the network, #531), and — like its GGUF twin — out of the
    /// `cargo perf` timing sweep: it only does network work when the
    /// [`CATALOG_CHECK_ENV`] guard is set, so the offline `--ignored` sweep no-ops
    /// it and stays hermetic (#1022). Run it before a release, and whenever an
    /// `EMBED_MODELS` row's `repo`/`weights_file` changes, via the dedicated alias:
    /// `cargo perf-catalog`
    #[test]
    #[ignore = "release gate: reaches HuggingFace to vet catalog filenames; not hermetic (#726)"]
    fn embed_catalog_files_exist_on_huggingface() {
        if !catalog_check_opted_in() {
            // Hermetic by default (see the sibling GGUF check): the `cargo perf`
            // sweep runs `--ignored` offline. `cargo perf-catalog` opts in (#1022).
            println!("skipping catalog HEAD checks: {CATALOG_CHECK_ENV} not set");
            return;
        }
        let mut missing = Vec::new();
        for m in EMBED_MODELS {
            let url = m.weights_resolve_url();
            let status = ureq::head(&url)
                .config()
                .http_status_as_error(false)
                .max_redirects(0)
                .max_redirects_will_error(false)
                .build()
                .call()
                .map_or_else(
                    |e| {
                        missing.push(format!("{} — request error: {e}", m.id));
                        0
                    },
                    |r| r.status().as_u16(),
                );
            println!("{:>7}  HTTP {status}  {url}", m.id);
            if status != 302 && status != 200 {
                missing.push(format!("{} — HTTP {status} at {url}", m.id));
            }
        }
        assert!(
            missing.is_empty(),
            "embed catalog rows point at files that don't exist on HuggingFace \
             (a user's download would 404): {missing:?}"
        );
    }
}
