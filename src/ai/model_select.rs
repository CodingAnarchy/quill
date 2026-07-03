//! Hardware-aware Qwen model selection (#85): a data-driven table of quantized
//! Qwen options and a pure picker that chooses the largest one a machine can run
//! comfortably. Kept dependency-free and always compiled (so the selection logic
//! is unit-tested in the normal suite); the hardware probe and the `hf-hub`
//! download that act on a choice live in the `local`-gated backend.
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
/// (`candle_transformers::models::quantized_qwen2`, 0.9) dequantizes to and
/// **caches K/V in f32** — there is no `to_dtype(f16/bf16)` in that model — so the
/// cache is 4 bytes/element, double the fp16 figure often quoted for llama.cpp
/// (#728). Revisit if a KV-quantized path lands.
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
    /// Approximate on-disk / download size, GB (Q4_K_M). Because the GGUF is
    /// mostly the quantized weights, this doubles as the **in-memory weight
    /// footprint** — the first term of the computed requirement (#728). (7B/14B
    /// ship as multiple shards, #726; this is their combined size.)
    pub size_gb: f64,
    /// Transformer layers — the first factor of the per-token KV-cache size
    /// (#728). From the Qwen2.5 model cards (28 for 7B, 48 for 14B, …).
    pub layers: u32,
    /// Key/value attention heads. Qwen2.5 uses grouped-query attention (GQA), so
    /// this is far fewer than the query heads — it's the KV heads that size the
    /// cache. From the model cards (e.g. 2 for 0.5B/1.5B/3B, 4 for 7B/14B).
    pub kv_heads: u32,
    /// Per-head dimension (128 across the Qwen2.5 family). `kv_heads * head_dim`
    /// is the K (and V) width cached per token per layer.
    pub head_dim: u32,
    /// The model's input context window, in tokens (#378). The router routes a
    /// pass on-device only when an available model's window can hold it, so a
    /// long-context model can take a whole-manuscript pass the standard ones
    /// can't.
    pub context_tokens: u32,
    /// A one-line characterization for the model manager (#379), so the author
    /// chooses on quality/speed/context rather than guessing at a size id.
    pub description: &'static str,
}

/// The context window of the standard (non-extended) Qwen2.5-Instruct models, in
/// tokens — used as the local bound when the author hasn't configured a model set
/// (the single recommended model is one of these).
pub const STANDARD_CONTEXT_TOKENS: u32 = 32_768;

/// Quantized Qwen options, ascending by size (Q4_K_M throughout — the
/// quality-per-byte sweet spot). The `*-1M` rows are the extended-context Qwen2.5
/// variants (#378): the same weights, but a ~1M-token window for whole-book work.
/// Update as new Qwen releases land. (The catalog is vetted for download/load in
/// #379.)
///
/// The `layers` / `kv_heads` / `head_dim` columns are the Qwen2.5 model-card
/// architecture (`num_hidden_layers`, `num_key_value_heads`,
/// `hidden_size / num_attention_heads`); they feed the computed memory
/// requirement in [`Model::required_mem_gb`] — see that method for the derivation
/// a reader can check against each card (#728).
pub const MODELS: &[Model] = &[
    Model {
        params: "0.5B",
        gguf_repo: "Qwen/Qwen2.5-0.5B-Instruct-GGUF",
        gguf_file: "qwen2.5-0.5b-instruct-q4_k_m.gguf",
        tokenizer_repo: "Qwen/Qwen2.5-0.5B-Instruct",
        size_gb: 0.4,
        layers: 24,
        kv_heads: 2,
        head_dim: 64, // hidden 896 / 14 heads
        context_tokens: STANDARD_CONTEXT_TOKENS,
        description: "Tiny — instant, for quick lookups",
    },
    Model {
        params: "1.5B",
        gguf_repo: "Qwen/Qwen2.5-1.5B-Instruct-GGUF",
        gguf_file: "qwen2.5-1.5b-instruct-q4_k_m.gguf",
        tokenizer_repo: "Qwen/Qwen2.5-1.5B-Instruct",
        size_gb: 1.0,
        layers: 28,
        kv_heads: 2,
        head_dim: 128, // hidden 1536 / 12 heads
        context_tokens: STANDARD_CONTEXT_TOKENS,
        description: "Small — fast, for light passes",
    },
    Model {
        params: "3B",
        gguf_repo: "Qwen/Qwen2.5-3B-Instruct-GGUF",
        gguf_file: "qwen2.5-3b-instruct-q4_k_m.gguf",
        tokenizer_repo: "Qwen/Qwen2.5-3B-Instruct",
        size_gb: 2.0,
        layers: 36,
        kv_heads: 2,
        head_dim: 128, // hidden 2048 / 16 heads
        context_tokens: STANDARD_CONTEXT_TOKENS,
        description: "Compact — a balance for modest machines",
    },
    Model {
        params: "7B",
        gguf_repo: "Qwen/Qwen2.5-7B-Instruct-GGUF",
        gguf_file: "qwen2.5-7b-instruct-q4_k_m.gguf",
        tokenizer_repo: "Qwen/Qwen2.5-7B-Instruct",
        size_gb: 4.7,
        layers: 28,
        kv_heads: 4,
        head_dim: 128, // hidden 3584 / 28 heads
        context_tokens: STANDARD_CONTEXT_TOKENS,
        description: "Capable — solid quality at 32k context",
    },
    // Extended-context (#378): a ~1M-token window in the 7B footprint, for
    // whole-manuscript / series passes on-device. Same architecture as the plain
    // 7B, but its requirement is dominated by the KV cache at ~1M tokens — with
    // candle's f32, unquantized cache that's ~108 GB, so it's only ever
    // recommended on a very large (128 GB+) GPU machine; see #728's sub-decision.
    Model {
        params: "7B-1M",
        gguf_repo: "bartowski/Qwen2.5-7B-Instruct-1M-GGUF",
        gguf_file: "Qwen2.5-7B-Instruct-1M-Q4_K_M.gguf",
        tokenizer_repo: "Qwen/Qwen2.5-7B-Instruct-1M",
        size_gb: 4.7,
        layers: 28,
        kv_heads: 4,
        head_dim: 128,
        context_tokens: 1_010_000,
        description: "Long context — whole-book passes (~1M tokens)",
    },
    Model {
        params: "14B",
        gguf_repo: "Qwen/Qwen2.5-14B-Instruct-GGUF",
        gguf_file: "qwen2.5-14b-instruct-q4_k_m.gguf",
        tokenizer_repo: "Qwen/Qwen2.5-14B-Instruct",
        size_gb: 9.0,
        layers: 48,
        kv_heads: 8,
        head_dim: 128, // hidden 5120 / 40 heads
        context_tokens: STANDARD_CONTEXT_TOKENS,
        description: "Largest — best quality at 32k context",
    },
];

impl Model {
    /// The stable id for this model — its parameter size (`"7B"`), used as the
    /// config key in `[ai] local_models` and the cache key for the loaded model.
    pub fn id(&self) -> &'static str {
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
    /// Worked check against the 7B card (28 layers, 4 KV heads, head_dim 128, f32):
    /// per token that's `2*28*4*128*4 = 114,688` bytes ≈ 112 KiB, so at the 32k
    /// standard window ≈ 3.6 GB — and at the 7B-1M's ~1M window ≈ 108 GB, which is
    /// why that model needs a very large machine (#728).
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
    /// reader can verify each term: `size_gb` against the model card's Q4_K_M GGUF
    /// size, the KV term against the architecture columns, and the overhead as the
    /// documented margin.
    #[cfg_attr(not(feature = "local"), allow(dead_code))]
    pub fn required_mem_gb(&self, context_tokens: u32) -> f64 {
        self.size_gb + self.kv_cache_gb(context_tokens) + RUNTIME_OVERHEAD_GB
    }

    /// A compact context-window label for the model manager (#379): `"32k"`, `"1M"`.
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
/// it to the cloud. This is what lets a long-context model take a whole-book pass
/// the standard ones can't.
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
const CPU_MAX_PARAMS: &str = "3B";

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

/// Pick the largest Qwen whose computed footprint (weights + KV cache at its
/// context + overhead) fits this machine's *usable* memory budget — the pool the
/// model actually runs in (VRAM / Metal working set / RAM), minus OS+editor
/// headroom — and, on CPU, isn't too big to be usable (#728). Because the budget
/// already reserves headroom and the requirement is the real footprint, the pick
/// leaves comfortable free memory rather than merely fitting. `None` with an
/// explanation when nothing fits — the caller then uses Claude.
#[cfg_attr(not(feature = "local"), allow(dead_code))]
pub fn recommend(hw: Hardware) -> Recommendation {
    let budget = usable_budget_gb(hw);
    let cpu_cap = MODELS.iter().position(|m| m.params == CPU_MAX_PARAMS);
    let choice = MODELS
        .iter()
        .enumerate()
        .filter(|(_, m)| m.required_mem_gb(m.context_tokens) <= budget)
        .filter(|(i, _)| hw.accel != Accel::Cpu || cpu_cap.is_none_or(|cap| *i <= cap))
        .map(|(_, m)| m)
        .next_back(); // table is ascending → last passing entry is the largest

    let reasoning = match choice {
        Some(m) => {
            let cap = if hw.accel == Accel::Cpu && m.params == CPU_MAX_PARAMS {
                " (capped for CPU-only; a GPU would allow a larger model)"
            } else {
                ""
            };
            format!(
                "{} usable on {} → Qwen2.5-{} (Q4_K_M, ~{:.1} GB weights, ~{:.1} GB total at \
                 {} ctx){}",
                budget_label(budget),
                accel_label(hw),
                m.params,
                m.size_gb,
                m.required_mem_gb(m.context_tokens),
                m.context_label(),
                cap,
            )
        }
        None => format!(
            "{} usable on {} is below the ~{:.1} GB the smallest model needs (weights + KV \
             cache + overhead) — local inference isn't advisable here; use the Claude backend \
             instead",
            budget_label(budget),
            accel_label(hw),
            MODELS[0].required_mem_gb(MODELS[0].context_tokens),
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
fn accel_label(hw: Hardware) -> &'static str {
    match hw.accel {
        Accel::Metal => "Apple Silicon (unified memory)",
        Accel::Cuda => "a discrete GPU (VRAM)",
        Accel::Cpu => "CPU only",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        // footprint (0.5B ~2.2, 1.5B ~3.9, 3B ~5.4, 7B ~9.5, 14B ~22.9 GB).
        assert_eq!(pick(metal(64)), Some("14B")); // budget ~38.9 → 14B fits
        assert_eq!(pick(metal(24)), Some("7B")); // budget ~12.1 → 7B, not 14B
        assert_eq!(pick(metal(16)), Some("3B")); // budget ~6.7 → 3B, not 7B
        assert_eq!(pick(metal(12)), Some("1.5B")); // budget ~4.0 → 1.5B
    }

    #[test]
    fn a_machine_too_small_for_any_model_gets_claude_not_a_too_large_pick() {
        // 8 GB Apple Silicon: budget ~1.4 GB < smallest model's ~2.2 GB → Claude.
        assert_eq!(pick(metal(8)), None);
        assert!(recommend(metal(8)).reasoning.contains("isn't advisable"));
    }

    #[test]
    fn cuda_is_bounded_by_vram_not_system_ram() {
        // The bug: 64 GB RAM / 8 GB VRAM was told to run a huge model. VRAM budget
        // = 8 - 4 = 4 GB → only 1.5B (~3.9 GB) fits, despite the 64 GB of RAM.
        assert_eq!(pick(cuda(8.0, 64)), Some("1.5B"));
        // A roomy 24 GB card: budget 20 GB → 7B fits, 14B (~22.9) doesn't.
        assert_eq!(pick(cuda(24.0, 64)), Some("7B"));
        // A tiny 4 GB card can't comfortably run anything → Claude.
        assert_eq!(pick(cuda(4.0, 64)), None);
    }

    #[test]
    fn cpu_only_is_capped_even_with_lots_of_ram() {
        // 64 GB but CPU-only → capped at 3B, not 14B.
        assert_eq!(pick(cpu(64)), Some("3B"));
        assert!(recommend(cpu(64)).reasoning.contains("capped for CPU"));
        // Apple Silicon with the same RAM gets the big model.
        assert_eq!(pick(metal(64)), Some("14B"));
    }

    #[test]
    fn long_context_model_needs_a_very_large_machine() {
        // 7B-1M's KV cache at ~1M tokens dominates (~116 GB) → it's never the pick
        // on ordinary hardware; a huge box still tops out at 14B here.
        assert_ne!(pick(metal(128)), Some("7B-1M"));
        // Its computed requirement really is enormous, unlike the plain 7B.
        let m1 = by_id("7B-1M").unwrap();
        assert!(
            m1.required_mem_gb(m1.context_tokens) > 100.0,
            "{}",
            m1.required_mem_gb(m1.context_tokens)
        );
        let m = by_id("7B").unwrap();
        assert!(m.required_mem_gb(m.context_tokens) < 12.0);
    }

    #[test]
    fn footprint_math_is_checkable_against_the_7b_card() {
        // 7B: 28 layers, 4 KV heads, head_dim 128, f32 KV → ~112 KiB/token.
        let m = by_id("7B").unwrap();
        // KV at 32k ≈ 3.76 GB (2*28*4*128*4*32768 / 1e9).
        assert!(
            (m.kv_cache_gb(32_768) - 3.76).abs() < 0.05,
            "{}",
            m.kv_cache_gb(32_768)
        );
        // Requirement = weights 4.7 + KV 3.76 + overhead 1.0 ≈ 9.46 GB.
        assert!(
            (m.required_mem_gb(32_768) - 9.46).abs() < 0.05,
            "{}",
            m.required_mem_gb(32_768)
        );
        // KV scales linearly with context.
        assert!((m.kv_cache_gb(65_536) - 2.0 * m.kv_cache_gb(32_768)).abs() < 1e-6);
    }

    #[test]
    fn reasoning_names_the_model_and_size() {
        let r = recommend(metal(24));
        assert!(
            r.reasoning.contains("Qwen2.5-7B") && r.reasoning.contains("GB"),
            "{}",
            r.reasoning
        );
    }

    #[test]
    fn table_is_ascending_by_weight_size() {
        // The picker and `pick_for` rely on ascending order. Weight size is the
        // monotonic key (the long-context 7B-1M shares the 7B's weights but has a
        // far larger *total* footprint from its KV cache, so total isn't
        // monotonic — hence `<=` here and the ordering keyed on weights).
        assert!(MODELS.windows(2).all(|w| w[0].size_gb <= w[1].size_gb));
    }

    #[test]
    fn by_id_finds_models_and_rejects_unknowns() {
        assert_eq!(by_id("7B").map(|m| m.params), Some("7B"));
        assert!(by_id("999B").is_none());
    }

    #[test]
    fn available_resolves_ids_in_size_order_and_drops_unknowns() {
        // Listed out of order, with a bogus id — comes back ascending, bogus gone.
        let ids = vec!["7B".to_string(), "0.5B".to_string(), "bogus".to_string()];
        let got: Vec<&str> = available(&ids).iter().map(|m| m.params).collect();
        assert_eq!(got, vec!["0.5B", "7B"]);
        assert!(
            available(&[]).is_empty(),
            "no config → caller falls back to recommend()"
        );
    }

    #[test]
    fn pick_for_respects_size_and_context_window() {
        let std = available(&["0.5B".into(), "7B".into()]);
        // A thesaurus-sized request → the smallest that fits (fastest).
        assert_eq!(pick_for(500, &std).map(|m| m.params), Some("0.5B"));
        // A big request that still fits the 32k window → the largest (best quality).
        assert_eq!(pick_for(20_000, &std).map(|m| m.params), Some("7B"));
        // Bigger than any standard window → nothing fits → cloud (None).
        assert!(
            pick_for(80_000, &std).is_none(),
            "no standard model holds 80k tokens"
        );

        // With the long-context model downloaded, the big pass fits — on it (#378).
        let long = available(&["7B".into(), "7B-1M".into()]);
        assert_eq!(pick_for(80_000, &long).map(|m| m.params), Some("7B-1M"));

        // Nothing downloaded → nothing to pick.
        assert!(pick_for(500, &[]).is_none());
    }

    #[test]
    fn local_window_takes_the_largest_configured_or_the_standard_default() {
        // No set configured → the standard (recommended) model's window.
        assert_eq!(local_window(&[]), STANDARD_CONTEXT_TOKENS);
        // A standard set → still 32k.
        assert_eq!(
            local_window(&["0.5B".into(), "7B".into()]),
            STANDARD_CONTEXT_TOKENS
        );
        // A long-context model in the set raises the bound to its window.
        assert!(local_window(&["7B".into(), "7B-1M".into()]) >= 1_000_000);
    }

    #[test]
    fn extended_context_model_has_a_million_token_window() {
        let m = by_id("7B-1M").expect("the 1M variant is in the table");
        assert!(m.context_tokens >= 1_000_000, "{}", m.context_tokens);
        assert_eq!(by_id("7B").unwrap().context_tokens, STANDARD_CONTEXT_TOKENS);
    }

    #[test]
    fn context_label_is_compact() {
        assert_eq!(by_id("7B").unwrap().context_label(), "32k");
        assert_eq!(by_id("7B-1M").unwrap().context_label(), "1M");
    }

    #[test]
    fn every_model_has_a_description() {
        assert!(MODELS.iter().all(|m| !m.description.is_empty()));
    }
}
