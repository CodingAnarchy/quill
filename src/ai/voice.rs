//! Character voice analysis (#183): check the manuscript against the character
//! bible's voice profiles (#74) and flag (a) dialogue lines that read out of
//! character for their speaker, and (b) POV/narration whose voice has drifted from
//! that character's established voice or isn't distinct enough from other POV
//! characters'. Pure analysis surfaced as findings; per Quill's
//! assistant-not-co-writer rule it never rewrites or suggests replacement dialogue
//! or narration — it surfaces the issue for the author to fix. See
//! `docs/ai-features.md`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AiError, Request, MAX_OUTPUT_TOKENS};

/// The result of a voice pass: the voice-consistency observations.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Voice {
    #[serde(default)]
    pub findings: Vec<VoiceFinding>,
}

/// One voice observation, grounded in a character's voice profile.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct VoiceFinding {
    /// "dialogue" (a line out of character) or "narration" (POV voice drift / not
    /// distinct from another POV).
    pub kind: String,
    /// The character whose voice the finding concerns.
    #[serde(default)]
    pub character: String,
    pub detail: String,
    /// Chapters involved (titles).
    #[serde(default)]
    pub chapters: Vec<String>,
}

const SYSTEM: &str = "You are a developmental editor doing character-VOICE \
ANALYSIS — observations only, never rewrites or suggestions. You are given the \
character voice profiles from the story bible and the manuscript. Judge the \
manuscript against those profiles and flag two kinds of issue: `dialogue` — a line \
of dialogue that reads out of character for its speaker, measured against that \
speaker's voice profile; and `narration` — a POV character's narration whose voice \
has drifted from their established narrative voice, or isn't distinct enough from \
another POV character's. For each finding give the kind (dialogue / narration), \
the character it concerns, the detail (what's off and why, referencing the voice \
notes), and the chapters involved. Reference chapters by title. Do NOT rewrite, \
draft, or suggest replacement dialogue or narration — surface the issue for the \
author to fix.";

/// The JSON Schema for a voice-analysis reply.
pub fn schema() -> Value {
    let finding = super::object_schema(
        vec![
            ("kind", super::string_field()),
            ("character", super::string_field()),
            ("detail", super::string_field()),
            ("chapters", super::string_array()),
        ],
        &["kind", "character", "detail", "chapters"],
    );
    super::object_schema(vec![("findings", super::array_of(finding))], &["findings"])
}

/// Build the voice-analysis request: the manuscript as cached context (the large,
/// slow-changing part) and the character voice profiles in the user message.
pub fn build_request(manuscript: String, voice_notes: &str, model: &str) -> Request {
    Request {
        system: Some(SYSTEM.to_string()),
        cached_context: manuscript,
        user: format!(
            "Character voice profiles (from the story bible):\n\n{voice_notes}\n\n\
             Report the voice findings as specified."
        ),
        model: model.to_string(),
        max_tokens: MAX_OUTPUT_TOKENS,
        json_schema: Some(schema()),
    }
}

/// Parse a structured voice-analysis reply.
pub fn parse(text: &str) -> Result<Voice, AiError> {
    super::parse_json(text, "voice")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_findings() {
        let json = r#"{"findings":[{"kind":"dialogue","character":"Mara","detail":"too formal","chapters":["Two"]}]}"#;
        let v = parse(json).unwrap();
        assert_eq!(v.findings.len(), 1);
        assert_eq!(v.findings[0].kind, "dialogue");
        assert_eq!(v.findings[0].character, "Mara");
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::ModelNonCompliance(_))));
    }

    #[test]
    fn request_carries_the_voice_notes_and_caches_the_manuscript() {
        let req = build_request("MANUSCRIPT".into(), "Mara: clipped, dry", "claude-opus-4-8");
        assert_eq!(req.cached_context, "MANUSCRIPT", "manuscript stays the cacheable part");
        assert!(req.user.contains("Mara: clipped, dry"), "voice notes travel in the prompt");
        assert!(req.json_schema.is_some());
    }
}
