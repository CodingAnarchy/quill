//! Marketing copy (#236): back-cover blurb variants and one-line taglines
//! generated from the finished manuscript, for an indie author promoting the
//! book. Like the submission material (#81) this produces prose, and it is
//! explicitly **off-manuscript** — written to `marketing/`, never into the novel
//! — so it stays within the assistant-not-co-writer rule. See
//! `docs/ai-features.md`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{numbered_sections, AiError, Request};

/// A round of marketing copy: several blurb variants and several taglines.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Marketing {
    #[serde(default)]
    pub blurbs: Vec<String>,
    #[serde(default)]
    pub taglines: Vec<String>,
}

impl Marketing {
    /// The artifacts as `(title, filename, body)` for writing under `marketing/`:
    /// the blurb variants as numbered sections, the taglines as a bullet list.
    /// Skips an artifact with no content so an empty round writes no empty file.
    pub fn artifacts(&self) -> Vec<(&'static str, &'static str, String)> {
        let blurbs = numbered_sections("Variant", &self.blurbs);
        let taglines: String = self
            .taglines
            .iter()
            .map(|t| format!("- {}\n", t.trim()))
            .collect();
        [
            ("Back-cover blurbs", "blurbs.md", blurbs),
            ("Taglines", "taglines.md", taglines),
        ]
        .into_iter()
        .filter(|(_, _, body)| !body.trim().is_empty())
        .collect()
    }
}

const SYSTEM: &str = "You are a book-marketing copywriter. From the manuscript, \
write OFF-MANUSCRIPT marketing copy: (1) several distinct back-cover blurb \
variants — each about 100-150 words, hooky and spoiler-free, and each taking a \
DIFFERENT angle (e.g. one led by the protagonist, one by the central stakes, one \
by mood/tone); (2) several short one-line taglines — punchy, memorable, \
spoiler-free. This is marketing material, never part of the novel, and never a \
rewrite of the author's prose. Return the blurbs and taglines as plain text.";

/// The JSON Schema for a marketing-copy reply.
pub fn schema() -> Value {
    super::object_schema(
        vec![
            ("blurbs", super::string_array()),
            ("taglines", super::string_array()),
        ],
        &["blurbs", "taglines"],
    )
}

/// Build the marketing-copy request: the manuscript as cached context, schema
/// attached. Manuscript-only for now; the bible (names/stakes) could sharpen the
/// copy later, but blurbs/taglines come mostly from the prose and tone (#104).
pub fn build_request(manuscript: String, model: &str) -> Request {
    super::manuscript_request(
        SYSTEM,
        "Write the blurb variants and taglines as specified.",
        manuscript,
        model,
        schema(),
    )
}

/// Parse a structured marketing-copy reply.
pub fn parse(text: &str) -> Result<Marketing, AiError> {
    super::parse_json(text, "marketing")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_blurbs_and_taglines() {
        let json = r#"{"blurbs":["B1","B2"],"taglines":["T1"]}"#;
        let m = parse(json).unwrap();
        assert_eq!(m.blurbs, ["B1", "B2"]);
        assert_eq!(m.taglines, ["T1"]);
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::ModelNonCompliance(_))));
    }

    #[test]
    fn artifacts_number_blurbs_and_bullet_taglines() {
        let m = Marketing {
            blurbs: vec!["A hook.".into(), "Another.".into()],
            taglines: vec!["Run.".into()],
        };
        let a = m.artifacts();
        assert_eq!(a.len(), 2);
        assert_eq!(a[0].1, "blurbs.md");
        assert!(a[0].2.contains("## Variant 1") && a[0].2.contains("## Variant 2"));
        assert!(a[0].2.contains("A hook."));
        assert_eq!(a[1].1, "taglines.md");
        assert!(a[1].2.contains("- Run."));
    }

    #[test]
    fn artifacts_skips_an_empty_section() {
        let m = Marketing {
            blurbs: vec!["only a blurb".into()],
            taglines: vec![],
        };
        let a = m.artifacts();
        assert_eq!(a.len(), 1, "no taglines → no taglines file");
        assert_eq!(a[0].1, "blurbs.md");
    }

    #[test]
    fn request_carries_schema_and_model() {
        let req = build_request("m".into(), "claude-opus-4-8");
        assert_eq!(req.model, "claude-opus-4-8");
        assert!(req.json_schema.is_some());
    }
}
