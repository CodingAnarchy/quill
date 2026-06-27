//! Ad creative & copy (#251): platform-ready ad fields — headlines, body /
//! primary text, suggested targeting, and an ad-image brief — shaped to Amazon
//! Ads, Meta (Facebook/Instagram), and X, drafted from the manuscript and any
//! existing marketing copy (#236/#237). Off-manuscript (written under
//! `marketing/ads/`), never the novel, so it stays within the
//! assistant-not-co-writer rule. No account/API here — that's #252-#254. The ad
//! image is a *brief*; the author renders one with the image features (#238/#239)
//! or supplies their own. See `docs/ai-features.md`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{bible, AiError, Request, MAX_OUTPUT_TOKENS};

/// A round of ad creative — one ad per platform.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct AdCreative {
    #[serde(default)]
    pub amazon: Ad,
    #[serde(default)]
    pub meta: Ad,
    #[serde(default)]
    pub x: Ad,
}

/// One platform's ad fields. The shapes are uniform; the platform's conventions
/// (lengths, what goes where) are applied by the model per the system prompt.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Ad {
    #[serde(default)]
    pub headlines: Vec<String>,
    /// Primary text / descriptions / tweet copy, as the platform uses.
    #[serde(default)]
    pub body: Vec<String>,
    /// Suggested keywords (Amazon) or audience targeting (Meta / X).
    #[serde(default)]
    pub targeting: Vec<String>,
    /// A one-line description of the ideal ad image.
    #[serde(default)]
    pub image_brief: String,
}

impl AdCreative {
    /// The artifacts as `(title, filename, body)` for writing under
    /// `marketing/ads/` — one file per platform. Skips a platform with no content.
    pub fn artifacts(&self) -> Vec<(&'static str, &'static str, String)> {
        [("Amazon Ads", "amazon.md", &self.amazon), ("Meta Ads", "meta.md", &self.meta), ("X Ads", "x.md", &self.x)]
            .into_iter()
            .map(|(title, file, ad)| (title, file, render_ad(ad)))
            .filter(|(_, _, body)| !body.trim().is_empty())
            .collect()
    }
}

/// Render one platform's ad fields as a Markdown section block.
fn render_ad(ad: &Ad) -> String {
    let mut s = String::new();
    bible::bullet_section(&mut s, "Headlines", &ad.headlines);
    bible::bullet_section(&mut s, "Primary text / descriptions", &ad.body);
    bible::bullet_section(&mut s, "Suggested targeting", &ad.targeting);
    let brief = ad.image_brief.trim();
    if !brief.is_empty() {
        s.push_str(&format!(
            "## Ad image\n{brief}\n\nUse an image from `marketing/images/` (Character \
             portrait / Scene illustration), or create one to this brief.\n"
        ));
    }
    s
}

const SYSTEM: &str = "You are a book-advertising copywriter. From the manuscript \
(and any existing marketing copy provided), draft platform-ready ad CREATIVE for \
three platforms — spoiler-free, on-brand, and never a rewrite of the author's \
prose. Shape each platform's fields to its conventions: Amazon Ads — a few short, \
punchy headlines and relevant keywords as `targeting`; Meta (Facebook/Instagram) \
— a primary text (one or two sentences) in `body`, short headlines, and audience \
targeting (interests/demographics) in `targeting`; X (Twitter) — tweet-length ad \
copy (<=280 chars) in `body`, short headlines, and audience targeting. For each \
platform also write a one-line `image_brief` describing the ideal ad image. This \
is marketing material, never part of the novel.";

/// The JSON Schema for an ad-creative reply.
pub fn schema() -> Value {
    let ad = super::object_schema(
        vec![
            ("headlines", super::string_array()),
            ("body", super::string_array()),
            ("targeting", super::string_array()),
            ("image_brief", super::string_field()),
        ],
        &["headlines", "body", "targeting", "image_brief"],
    );
    super::object_schema(
        vec![("amazon", ad.clone()), ("meta", ad.clone()), ("x", ad)],
        &["amazon", "meta", "x"],
    )
}

/// Build the ad-creative request: the manuscript as cached context, any existing
/// marketing copy folded into the prompt to stay on-brand, schema attached.
pub fn build_request(manuscript: String, marketing: &str, model: &str) -> Request {
    let marketing = marketing.trim();
    let user = if marketing.is_empty() {
        "Draft the per-platform ad creative as specified.".to_string()
    } else {
        format!("Existing marketing copy to stay on-brand with:\n\n{marketing}\n\nDraft the per-platform ad creative as specified.")
    };
    Request {
        system: Some(SYSTEM.to_string()),
        cached_context: manuscript,
        user,
        model: model.to_string(),
        max_tokens: MAX_OUTPUT_TOKENS,
        json_schema: Some(schema()),
    }
}

/// Parse a structured ad-creative reply.
pub fn parse(text: &str) -> Result<AdCreative, AiError> {
    super::parse_json(text, "ad-creative")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> AdCreative {
        let ad = |h: &str| Ad {
            headlines: vec![h.into()],
            body: vec!["A locket. A reckoning.".into()],
            targeting: vec!["fantasy readers".into()],
            image_brief: "a fog-bound coast at dawn".into(),
        };
        AdCreative { amazon: ad("Unearth the secret"), meta: ad("She came home to bury it"), x: ad("Some doors stay shut") }
    }

    #[test]
    fn parse_reads_all_platforms() {
        let m = parse(r#"{"amazon":{"headlines":["h"],"body":[],"targeting":["k"],"image_brief":""},"meta":{},"x":{}}"#)
            .unwrap();
        assert_eq!(m.amazon.headlines, ["h"]);
        assert_eq!(m.amazon.targeting, ["k"]);
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::ModelNonCompliance(_))));
    }

    #[test]
    fn artifacts_is_one_file_per_platform() {
        let a = sample().artifacts();
        assert_eq!(a.iter().map(|(_, f, _)| *f).collect::<Vec<_>>(), ["amazon.md", "meta.md", "x.md"]);
        assert!(a[0].2.contains("## Headlines") && a[0].2.contains("Unearth the secret"));
        assert!(a[0].2.contains("## Suggested targeting") && a[0].2.contains("## Ad image"));
    }

    #[test]
    fn artifacts_skips_an_empty_platform() {
        let mut c = sample();
        c.x = Ad::default();
        let a = c.artifacts();
        assert_eq!(a.len(), 2, "the empty platform writes no file");
        assert!(a.iter().all(|(_, f, _)| *f != "x.md"));
    }

    #[test]
    fn request_folds_in_existing_marketing_copy() {
        let req = build_request("MS".into(), "Blurb: a locket.", "claude-opus-4-8");
        assert!(req.user.contains("on-brand") && req.user.contains("a locket"));
        assert_eq!(req.cached_context, "MS");
    }

    #[test]
    fn request_without_marketing_just_instructs() {
        let req = build_request("MS".into(), "  ", "claude-opus-4-8");
        assert!(!req.user.contains("on-brand"));
    }
}
