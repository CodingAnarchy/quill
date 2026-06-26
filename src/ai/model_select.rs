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
    Device::new_metal(0).or_else(|_| Device::new_cuda(0)).unwrap_or(Device::Cpu)
}

/// Whether the machine has a GPU accelerator candle can use, or is CPU-only.
#[cfg_attr(not(feature = "local"), allow(dead_code))] // only the local backend's recommendation path calls these (#406)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accel {
    /// Metal (Apple Silicon) or CUDA.
    Gpu,
    /// CPU only — big models are too slow, so selection caps the size.
    Cpu,
}

/// One quantized Qwen option. `tokenizer_repo` is the base instruct repo (which
/// ships `tokenizer.json`); the GGUF lives in the `-GGUF` companion repo.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Model {
    pub params: &'static str,
    pub gguf_repo: &'static str,
    pub gguf_file: &'static str,
    pub tokenizer_repo: &'static str,
    /// Approximate on-disk / download size, GB (Q4_K_M).
    pub size_gb: f64,
    /// Total system RAM (GB) below which this model isn't recommended — set
    /// conservatively, since the author is editing alongside inference.
    pub min_ram_gb: u32,
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

/// Quantized Qwen options, ascending by RAM (Q4_K_M throughout — the
/// quality-per-byte sweet spot). The `*-1M` rows are the extended-context Qwen2.5
/// variants (#378): the same weights, but a ~1M-token window for whole-book work.
/// Update as new Qwen releases land. (The catalog is vetted for download/load in
/// #379.)
pub const MODELS: &[Model] = &[
    Model { params: "0.5B", gguf_repo: "Qwen/Qwen2.5-0.5B-Instruct-GGUF", gguf_file: "qwen2.5-0.5b-instruct-q4_k_m.gguf", tokenizer_repo: "Qwen/Qwen2.5-0.5B-Instruct", size_gb: 0.4, min_ram_gb: 3, context_tokens: STANDARD_CONTEXT_TOKENS, description: "Tiny — instant, for quick lookups" },
    Model { params: "1.5B", gguf_repo: "Qwen/Qwen2.5-1.5B-Instruct-GGUF", gguf_file: "qwen2.5-1.5b-instruct-q4_k_m.gguf", tokenizer_repo: "Qwen/Qwen2.5-1.5B-Instruct", size_gb: 1.0, min_ram_gb: 6, context_tokens: STANDARD_CONTEXT_TOKENS, description: "Small — fast, for light passes" },
    Model { params: "3B", gguf_repo: "Qwen/Qwen2.5-3B-Instruct-GGUF", gguf_file: "qwen2.5-3b-instruct-q4_k_m.gguf", tokenizer_repo: "Qwen/Qwen2.5-3B-Instruct", size_gb: 2.0, min_ram_gb: 12, context_tokens: STANDARD_CONTEXT_TOKENS, description: "Compact — a balance for modest machines" },
    Model { params: "7B", gguf_repo: "Qwen/Qwen2.5-7B-Instruct-GGUF", gguf_file: "qwen2.5-7b-instruct-q4_k_m.gguf", tokenizer_repo: "Qwen/Qwen2.5-7B-Instruct", size_gb: 4.7, min_ram_gb: 24, context_tokens: STANDARD_CONTEXT_TOKENS, description: "Capable — solid quality at 32k context" },
    // Extended-context (#378): a ~1M-token window in the 7B footprint, for
    // whole-manuscript / series passes on-device. Long context is memory-hungry
    // (the KV cache), hence the higher RAM floor than the standard 7B.
    Model { params: "7B-1M", gguf_repo: "bartowski/Qwen2.5-7B-Instruct-1M-GGUF", gguf_file: "Qwen2.5-7B-Instruct-1M-Q4_K_M.gguf", tokenizer_repo: "Qwen/Qwen2.5-7B-Instruct-1M", size_gb: 4.7, min_ram_gb: 32, context_tokens: 1_010_000, description: "Long context — whole-book passes (~1M tokens)" },
    Model { params: "14B", gguf_repo: "Qwen/Qwen2.5-14B-Instruct-GGUF", gguf_file: "qwen2.5-14b-instruct-q4_k_m.gguf", tokenizer_repo: "Qwen/Qwen2.5-14B-Instruct", size_gb: 9.0, min_ram_gb: 48, context_tokens: STANDARD_CONTEXT_TOKENS, description: "Largest — best quality at 32k context" },
];

impl Model {
    /// The stable id for this model — its parameter size (`"7B"`), used as the
    /// config key in `[ai] local_models` and the cache key for the loaded model.
    pub fn id(&self) -> &'static str {
        self.params
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
    MODELS.iter().filter(|m| ids.iter().any(|id| id == m.id())).collect()
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
    let mut fits = available.iter().copied().filter(|m| m.context_tokens >= input_tokens);
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

/// Pick the largest Qwen that fits `ram_gb` (and, on CPU, isn't too big to be
/// usable). `None` with an explanation when the machine is too small.
#[cfg_attr(not(feature = "local"), allow(dead_code))]
pub fn recommend(ram_gb: u32, accel: Accel) -> Recommendation {
    let cpu_cap = MODELS.iter().position(|m| m.params == CPU_MAX_PARAMS);
    let choice = MODELS
        .iter()
        .enumerate()
        .filter(|(_, m)| ram_gb >= m.min_ram_gb)
        .filter(|(i, _)| accel == Accel::Gpu || cpu_cap.is_none_or(|cap| *i <= cap))
        .map(|(_, m)| m)
        .next_back(); // table is ascending → last passing entry is the largest

    let reasoning = match choice {
        Some(m) => {
            let cap = if accel == Accel::Cpu && m.params == CPU_MAX_PARAMS {
                " (capped for CPU-only; a GPU would allow a larger model)"
            } else {
                ""
            };
            format!(
                "{} RAM with {} → Qwen2.5-{} (Q4_K_M, ~{:.1} GB){}",
                ram_label(ram_gb),
                accel_label(accel),
                m.params,
                m.size_gb,
                cap,
            )
        }
        None => format!(
            "{} is below the {} GB needed for the smallest model — local inference \
             isn't advisable here; use the Claude backend instead",
            ram_label(ram_gb),
            MODELS[0].min_ram_gb,
        ),
    };
    Recommendation { model: choice, reasoning }
}

#[cfg_attr(not(feature = "local"), allow(dead_code))]
fn ram_label(gb: u32) -> String {
    format!("{gb} GB")
}

#[cfg_attr(not(feature = "local"), allow(dead_code))]
fn accel_label(accel: Accel) -> &'static str {
    match accel {
        Accel::Gpu => "a GPU",
        Accel::Cpu => "CPU only",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pick(ram: u32, accel: Accel) -> Option<&'static str> {
        recommend(ram, accel).model.map(|m| m.params)
    }

    #[test]
    fn picks_largest_model_that_fits() {
        assert_eq!(pick(24, Accel::Gpu), Some("7B")); // 14B needs 48
        assert_eq!(pick(48, Accel::Gpu), Some("14B"));
        assert_eq!(pick(8, Accel::Gpu), Some("1.5B")); // 3B needs 12
        assert_eq!(pick(6, Accel::Gpu), Some("1.5B"));
        assert_eq!(pick(4, Accel::Gpu), Some("0.5B")); // 1.5B needs 6
    }

    #[test]
    fn too_small_machines_get_no_recommendation() {
        assert_eq!(pick(2, Accel::Gpu), None);
        assert!(recommend(2, Accel::Gpu).reasoning.contains("isn't advisable"));
    }

    #[test]
    fn cpu_only_is_capped_even_with_lots_of_ram() {
        // 64 GB but CPU-only → capped at 3B, not 14B.
        assert_eq!(pick(64, Accel::Cpu), Some("3B"));
        assert!(recommend(64, Accel::Cpu).reasoning.contains("capped for CPU"));
        // A GPU machine with the same RAM gets the big model.
        assert_eq!(pick(64, Accel::Gpu), Some("14B"));
    }

    #[test]
    fn reasoning_names_the_model_and_size() {
        let r = recommend(24, Accel::Gpu);
        assert!(r.reasoning.contains("Qwen2.5-7B") && r.reasoning.contains("GB"), "{}", r.reasoning);
    }

    #[test]
    fn table_is_ascending_by_ram_so_next_back_is_largest() {
        assert!(MODELS.windows(2).all(|w| w[0].min_ram_gb < w[1].min_ram_gb));
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
        assert!(available(&[]).is_empty(), "no config → caller falls back to recommend()");
    }

    #[test]
    fn pick_for_respects_size_and_context_window() {
        let std = available(&["0.5B".into(), "7B".into()]);
        // A thesaurus-sized request → the smallest that fits (fastest).
        assert_eq!(pick_for(500, &std).map(|m| m.params), Some("0.5B"));
        // A big request that still fits the 32k window → the largest (best quality).
        assert_eq!(pick_for(20_000, &std).map(|m| m.params), Some("7B"));
        // Bigger than any standard window → nothing fits → cloud (None).
        assert!(pick_for(80_000, &std).is_none(), "no standard model holds 80k tokens");

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
        assert_eq!(local_window(&["0.5B".into(), "7B".into()]), STANDARD_CONTEXT_TOKENS);
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
