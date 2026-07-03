//! Social media copy (#237): ready-to-post promotional posts for the finished
//! book, shaped to each platform's conventions (X, Facebook, Instagram). Like the
//! marketing copy (#236) and submission material (#81) this produces prose and is
//! explicitly **off-manuscript** — written under `marketing/social/`, never into
//! the novel — so it stays within the assistant-not-co-writer rule. See
//! `docs/ai-features.md`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{numbered_sections, AiError, Request};

/// A round of social posts, grouped by platform.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Social {
    #[serde(default)]
    pub x: Vec<String>,
    #[serde(default)]
    pub facebook: Vec<String>,
    #[serde(default)]
    pub instagram: Vec<String>,
}

impl Social {
    /// The artifacts as `(title, filename, body)` for writing under
    /// `marketing/social/` — one file per platform, each post a numbered section.
    /// Skips a platform with no posts so it writes no empty file.
    pub fn artifacts(&self) -> Vec<(&'static str, &'static str, String)> {
        [
            ("X posts", "x.md", &self.x),
            ("Facebook posts", "facebook.md", &self.facebook),
            ("Instagram posts", "instagram.md", &self.instagram),
        ]
        .into_iter()
        .filter(|(_, _, posts)| !posts.is_empty())
        .map(|(title, file, posts)| (title, file, numbered_sections("Post", posts)))
        .collect()
    }
}

const SYSTEM: &str = "You are a book-marketing social-media copywriter. From the \
manuscript, write OFF-MANUSCRIPT, ready-to-post copy promoting the finished book, \
shaped to each platform's conventions: X (Twitter) — very short and punchy, under \
~280 characters, 1-2 hashtags; Facebook — a longer conversational post (a few \
sentences) with a clear call-to-action and sparing hashtags; Instagram — a caption \
with an evocative hook, a short body, and a cluster of relevant hashtags at the \
end. Write a few posts per platform, each spoiler-free. This is marketing \
material, never part of the novel, and never a rewrite of the author's prose. \
Return the posts per platform as plain text.";

/// The JSON Schema for a social-posts reply.
pub fn schema() -> Value {
    super::object_schema(
        vec![
            ("x", super::string_array()),
            ("facebook", super::string_array()),
            ("instagram", super::string_array()),
        ],
        &["x", "facebook", "instagram"],
    )
}

/// Build the social-posts request: the manuscript as cached context, schema
/// attached. Manuscript-only for now, like the marketing copy (#104).
pub fn build_request(manuscript: String, model: &str) -> Request {
    super::manuscript_request(
        SYSTEM,
        "Write the social posts per platform as specified.",
        manuscript,
        model,
        schema(),
    )
}

/// Parse a structured social-posts reply.
pub fn parse(text: &str) -> Result<Social, AiError> {
    super::parse_json(text, "social")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_posts_per_platform() {
        let json = r#"{"x":["x1"],"facebook":["fb1","fb2"],"instagram":["ig1"]}"#;
        let s = parse(json).unwrap();
        assert_eq!(s.x, ["x1"]);
        assert_eq!(s.facebook, ["fb1", "fb2"]);
        assert_eq!(s.instagram, ["ig1"]);
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::ModelNonCompliance(_))));
    }

    #[test]
    fn artifacts_is_one_file_per_platform_with_numbered_posts() {
        let s = Social {
            x: vec!["Short hook #book".into()],
            facebook: vec!["A longer post.".into(), "Another.".into()],
            instagram: vec!["Caption.".into()],
        };
        let a = s.artifacts();
        assert_eq!(a.len(), 3);
        assert_eq!(
            a.iter().map(|(_, f, _)| *f).collect::<Vec<_>>(),
            ["x.md", "facebook.md", "instagram.md"]
        );
        assert!(a[1].2.contains("## Post 1") && a[1].2.contains("## Post 2"));
    }

    #[test]
    fn artifacts_skips_a_platform_with_no_posts() {
        let s = Social {
            x: vec!["only x".into()],
            facebook: vec![],
            instagram: vec![],
        };
        let a = s.artifacts();
        assert_eq!(a.len(), 1);
        assert_eq!(a[0].1, "x.md");
    }

    #[test]
    fn request_carries_schema_and_model() {
        let req = build_request("m".into(), "claude-opus-4-8");
        assert_eq!(req.model, "claude-opus-4-8");
        assert!(req.json_schema.is_some());
    }
}
