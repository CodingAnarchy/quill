//! A rough, local token/cost estimate for an AI pass — shown before an expensive
//! whole-manuscript request so the author can decide before spending. Deliberately
//! approximate and offline (no `count_tokens` round-trip): input tokens from the
//! text size, dollars as a ceiling using the model's published rate.

/// A pass's estimated size and cost.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Estimate {
    /// Estimated input tokens.
    pub tokens: u32,
    /// Estimated dollar cost (an upper bound: input + the full `max_tokens` out).
    pub dollars: f64,
}

/// USD per 1M tokens (input, output) for a model. Cached pricing; defaults to
/// the Opus tier for unknown models so the estimate never under-warns.
fn rate(model: &str) -> (f64, f64) {
    match model {
        m if m.starts_with("claude-haiku") => (1.0, 5.0),
        m if m.starts_with("claude-sonnet") => (3.0, 15.0),
        _ => (5.0, 25.0), // opus tier / unknown
    }
}

/// Estimate a request's size and cost. `input_chars` is the prompt text length
/// (input tokens ≈ chars / 4); `max_tokens` bounds the output for a worst-case
/// dollar figure.
pub fn estimate(input_chars: usize, max_tokens: u32, model: &str) -> Estimate {
    let tokens = (input_chars / 4) as u32;
    let (in_rate, out_rate) = rate(model);
    let dollars =
        (tokens as f64 * in_rate + max_tokens as f64 * out_rate) / 1_000_000.0;
    Estimate { tokens, dollars }
}

/// A compact human label, e.g. "~31k tokens · ~$0.18 on claude-opus-4-8".
pub fn label(est: &Estimate, model: &str) -> String {
    let tokens = if est.tokens >= 1000 {
        format!("~{}k tokens", est.tokens / 1000)
    } else {
        format!("~{} tokens", est.tokens)
    };
    format!("{tokens} · ~${:.2} on {model}", est.dollars)
}

/// A label that compares the two backends for the routing confirm (#372): the
/// remote dollar estimate beside the free on-device alternative, so the choice
/// is grounded in a visible rationale rather than a hidden heuristic.
pub fn compare_label(est: &Estimate, model: &str) -> String {
    format!("{} — or free on-device", label(est, model))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimate_scales_with_size_and_model() {
        // 400k chars ≈ 100k input tokens; opus = $5/1M in, $25/1M out.
        let opus = estimate(400_000, 8192, "claude-opus-4-8");
        assert_eq!(opus.tokens, 100_000);
        // 100k*5/1e6 + 8192*25/1e6 = 0.5 + 0.2048 ≈ 0.7048
        assert!((opus.dollars - 0.7048).abs() < 1e-3, "{}", opus.dollars);

        // Haiku is far cheaper for the same request.
        let haiku = estimate(400_000, 8192, "claude-haiku-4-5");
        assert!(haiku.dollars < opus.dollars);
    }

    #[test]
    fn unknown_model_uses_the_opus_tier() {
        assert_eq!(estimate(4000, 100, "mystery").dollars, estimate(4000, 100, "claude-opus-4-8").dollars);
    }

    #[test]
    fn label_is_compact() {
        let est = estimate(124_000, 8192, "claude-opus-4-8"); // 31k tokens
        let l = label(&est, "claude-opus-4-8");
        assert!(l.contains("~31k tokens"), "{l}");
        assert!(l.contains("on claude-opus-4-8"), "{l}");
    }

    #[test]
    fn compare_label_shows_both_backends() {
        let est = estimate(124_000, 8192, "claude-opus-4-8");
        let l = compare_label(&est, "claude-opus-4-8");
        assert!(l.contains("on claude-opus-4-8"), "the remote price: {l}");
        assert!(l.contains("free on-device"), "the local alternative: {l}");
    }
}
