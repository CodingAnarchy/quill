//! Hardware-aware Qwen model selection (#85): a data-driven table of quantized
//! Qwen options and a pure picker that chooses the largest one a machine can run
//! comfortably. Kept dependency-free and always compiled (so the selection logic
//! is unit-tested in the normal suite); the hardware probe and the `hf-hub`
//! download that act on a choice live in the `local`-gated backend.
//!
//! Always compiled (so the picker is tested in the normal suite) but its only
//! non-test consumer is that gated backend, so it carries a `dead_code`
//! allowance in default builds; the settings surface that exposes the
//! recommendation to the author is #87.
#![allow(dead_code)]

/// The best candle device this machine offers — Metal, then CUDA, then CPU. The
/// two local backends (text #124 and image #240) share this probe.
#[cfg(feature = "local")]
pub fn best_device() -> candle_core::Device {
    use candle_core::Device;
    Device::new_metal(0).or_else(|_| Device::new_cuda(0)).unwrap_or(Device::Cpu)
}

/// Whether the machine has a GPU accelerator candle can use, or is CPU-only.
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
}

/// Quantized Qwen2.5-Instruct options, smallest first (Q4_K_M throughout — the
/// quality-per-byte sweet spot). Update as new Qwen releases land.
pub const MODELS: &[Model] = &[
    Model { params: "0.5B", gguf_repo: "Qwen/Qwen2.5-0.5B-Instruct-GGUF", gguf_file: "qwen2.5-0.5b-instruct-q4_k_m.gguf", tokenizer_repo: "Qwen/Qwen2.5-0.5B-Instruct", size_gb: 0.4, min_ram_gb: 3 },
    Model { params: "1.5B", gguf_repo: "Qwen/Qwen2.5-1.5B-Instruct-GGUF", gguf_file: "qwen2.5-1.5b-instruct-q4_k_m.gguf", tokenizer_repo: "Qwen/Qwen2.5-1.5B-Instruct", size_gb: 1.0, min_ram_gb: 6 },
    Model { params: "3B", gguf_repo: "Qwen/Qwen2.5-3B-Instruct-GGUF", gguf_file: "qwen2.5-3b-instruct-q4_k_m.gguf", tokenizer_repo: "Qwen/Qwen2.5-3B-Instruct", size_gb: 2.0, min_ram_gb: 12 },
    Model { params: "7B", gguf_repo: "Qwen/Qwen2.5-7B-Instruct-GGUF", gguf_file: "qwen2.5-7b-instruct-q4_k_m.gguf", tokenizer_repo: "Qwen/Qwen2.5-7B-Instruct", size_gb: 4.7, min_ram_gb: 24 },
    Model { params: "14B", gguf_repo: "Qwen/Qwen2.5-14B-Instruct-GGUF", gguf_file: "qwen2.5-14b-instruct-q4_k_m.gguf", tokenizer_repo: "Qwen/Qwen2.5-14B-Instruct", size_gb: 9.0, min_ram_gb: 48 },
];

impl Model {
    /// The stable id for this model — its parameter size (`"7B"`), used as the
    /// config key in `[ai] local_models` and the cache key for the loaded model.
    pub fn id(&self) -> &'static str {
        self.params
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

/// Below this many input tokens a request is "small" — a thesaurus lookup, a
/// single-line check — and the smallest downloaded model serves it fastest.
const SMALL_REQUEST_TOKENS: u32 = 2_000;

/// Pick the best downloaded model for a request of `input_tokens` (#377): the
/// smallest available model for a small request (lowest latency), the largest for
/// a substantial one (best quality). `available` must be ascending by size (as
/// [`available`] returns it); `None` only when nothing is downloaded.
pub fn pick_for(input_tokens: u32, available: &[&'static Model]) -> Option<&'static Model> {
    if input_tokens <= SMALL_REQUEST_TOKENS {
        available.first().copied()
    } else {
        available.last().copied()
    }
}

/// On a CPU-only machine, don't recommend anything bigger than this — larger
/// models are painfully slow without a GPU.
const CPU_MAX_PARAMS: &str = "3B";

/// A recommendation: the chosen model (if any is viable) and the human reasoning.
pub struct Recommendation {
    pub model: Option<&'static Model>,
    pub reasoning: String,
}

/// Pick the largest Qwen that fits `ram_gb` (and, on CPU, isn't too big to be
/// usable). `None` with an explanation when the machine is too small.
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

fn ram_label(gb: u32) -> String {
    format!("{gb} GB")
}

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
    fn pick_for_takes_the_small_model_for_small_asks_and_the_big_one_otherwise() {
        let models = available(&["0.5B".into(), "7B".into()]);
        // A thesaurus-sized request → the smallest (fastest).
        assert_eq!(pick_for(500, &models).map(|m| m.params), Some("0.5B"));
        // A whole-manuscript-sized request → the largest available (best quality).
        assert_eq!(pick_for(80_000, &models).map(|m| m.params), Some("7B"));
        // Nothing downloaded → nothing to pick.
        assert!(pick_for(500, &[]).is_none());
    }
}
