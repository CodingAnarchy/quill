//! Distill a chapter into a visual brief for a scene illustration (#247): a
//! read-only text pass that finds the chapter's most depictable moment and
//! describes only what an illustrator would draw (setting, subjects, composition,
//! mood). It produces an off-manuscript image prompt, never manuscript prose, so
//! it stays within the assistant-not-co-writer rule. See `docs/ai-features.md`.

use super::{Request, MAX_OUTPUT_TOKENS};

/// A visual brief is short; cap output well under the analysis bound.
const MAX_TOKENS: u32 = 512;

const SYSTEM: &str = "You are an art director preparing a single illustration for \
a novel. From the chapter text, pick its most visually striking, pivotal moment \
and describe ONLY what an illustrator would draw: the setting, the key subject(s) \
and their appearance and action, the composition, and the mood. Write one concise \
paragraph — a visual brief in present tense. No dialogue, no plot exposition, no \
quoted or paraphrased prose; just the picture. Don't invent people or places that \
aren't in the text.";

/// Build the distill request: the chapter as cached context, free-form reply (the
/// brief is plain text, fed straight into the image prompt).
pub fn build_request(chapter: String, model: &str) -> Request {
    Request {
        system: Some(SYSTEM.to_string()),
        cached_context: chapter,
        user: "Write the visual brief for this chapter's key scene.".to_string(),
        model: model.to_string(),
        max_tokens: MAX_TOKENS.min(MAX_OUTPUT_TOKENS),
        json_schema: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_caches_the_chapter_and_is_free_form() {
        let req = build_request("the whole chapter".into(), "claude-opus-4-8");
        assert_eq!(req.cached_context, "the whole chapter");
        assert!(req.json_schema.is_none(), "free-form text, no schema");
        assert!(req.system.as_deref().unwrap().contains("illustrator"));
        assert!(req.max_tokens <= 512);
    }
}
