//! Character voice analysis (#183): check the manuscript against the character
//! bible's voice profiles (#74) and flag (a) dialogue lines that read out of
//! character for their speaker, and (b) POV/narration whose voice has drifted from
//! that character's established voice or isn't distinct enough from other POV
//! characters'. Pure analysis surfaced as findings; per Quill's
//! assistant-not-co-writer rule it never rewrites or suggests replacement dialogue
//! or narration — it surfaces the issue for the author to fix. See
//! `docs/ai-features.md`.

use std::fmt::{self, Display};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AiError, Request, MAX_OUTPUT_TOKENS};

/// The closed set of voice-finding categories the prompt asks for (#1082):
/// `dialogue` (a line out of character) or `narration` (POV voice drift / not
/// distinct). Deserialized straight from the model reply; an out-of-contract value
/// degrades to [`Unknown`](Self::Unknown) rather than a silent raw string, and
/// [`Display`] reproduces the exact lowercase label so a rendered finding reads
/// identically.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum VoiceKind {
    Dialogue,
    Narration,
    /// Any value outside the prompt's closed set. Renders as `"unknown"`.
    #[default]
    #[serde(other)]
    Unknown,
}

impl Display for VoiceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Dialogue => "dialogue",
            Self::Narration => "narration",
            Self::Unknown => "unknown",
        })
    }
}

/// The result of a voice pass: the voice-consistency observations.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Voice {
    #[serde(default)]
    pub findings: Vec<VoiceFinding>,
}

/// One voice observation, grounded in a character's voice profile.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct VoiceFinding {
    /// `dialogue` (a line out of character) or `narration` (POV voice drift / not
    /// distinct from another POV) (#1082). A value outside that set deserializes to
    /// [`VoiceKind::Unknown`].
    pub kind: VoiceKind,
    /// The character whose voice the finding concerns (for a `dialogue` finding,
    /// the speaker of the flagged line).
    #[serde(default)]
    pub character: String,
    /// For a line-level `dialogue` finding (#1050), a SHORT verbatim quote of the
    /// specific spoken line that reads out of character for its speaker given the
    /// scene, so the author can land on the exact line. Empty for a holistic /
    /// narration finding that isn't anchored to one line. This is the author's own
    /// prose quoted back for locating — never a rewrite or replacement.
    #[serde(default)]
    pub line: String,
    pub detail: String,
    /// Chapters involved (titles).
    #[serde(default)]
    pub chapters: Vec<String>,
}

const SYSTEM: &str = "You are a developmental editor doing character-VOICE \
ANALYSIS — observations only, never rewrites or suggestions. You are given the \
character voice profiles from the story bible and the manuscript. Judge the \
manuscript against those profiles and flag two kinds of issue: `dialogue` — a \
SPECIFIC line of dialogue that reads out of character for its speaker at the moment \
it's spoken, measured against that speaker's voice profile and the scene it's in (a \
formal character suddenly slangy, a terse character monologuing); and `narration` — \
a POV character's narration whose voice has drifted from their established narrative \
voice, or isn't distinct enough from another POV character's. For each finding give \
the kind (dialogue / narration), the `character` it concerns (for a dialogue \
finding, the speaker of the line), the `detail` (what's off and why, referencing the \
voice notes), and the `chapters` involved (by title). For a `dialogue` finding, also \
quote in `line` the SHORT verbatim spoken line that reads false, so the author can \
find the exact line; leave `line` empty for narration. Attribute each dialogue line \
to the character who speaks it. Flag only lines you are confident read out of \
character given the speaker's profile and the scene — when a line could plausibly be \
in character, leave it unflagged (bias toward precision, not recall). Do NOT rewrite, \
draft, or suggest replacement dialogue or narration, and do NOT put any corrected or \
alternative wording in `line` — `line` is the author's own text quoted back only so \
they can locate it. Surface the issue for the author to fix.";

/// The JSON Schema for a voice-analysis reply.
pub fn schema() -> Value {
    let finding = super::object_schema(
        vec![
            ("kind", super::string_field()),
            ("character", super::string_field()),
            // The verbatim spoken line for a line-level dialogue finding (#1050);
            // optional so a holistic / narration finding can omit it.
            ("line", super::string_field()),
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
        assert_eq!(v.findings[0].kind, VoiceKind::Dialogue);
        assert_eq!(v.findings[0].character, "Mara");
    }

    #[test]
    fn kind_round_trips_valid_values_to_the_same_label() {
        // Each valid category deserializes to its variant and renders back to the
        // exact lowercase string the model emitted — output stays byte-identical.
        for (raw, kind) in [
            ("dialogue", VoiceKind::Dialogue),
            ("narration", VoiceKind::Narration),
        ] {
            let json = format!(
                r#"{{"findings":[{{"kind":"{raw}","character":"Mara","detail":"d","chapters":[]}}]}}"#
            );
            let v = parse(&json).unwrap();
            assert_eq!(v.findings[0].kind, kind);
            assert_eq!(v.findings[0].kind.to_string(), raw);
        }
    }

    #[test]
    fn malformed_kind_degrades_to_unknown_not_a_raw_string() {
        // A model typo doesn't error and doesn't leak the raw string into output.
        let json =
            r#"{"findings":[{"kind":"dilaogue","character":"Mara","detail":"d","chapters":[]}]}"#;
        let v = parse(json).unwrap();
        assert_eq!(v.findings[0].kind, VoiceKind::Unknown);
        assert_eq!(v.findings[0].kind.to_string(), "unknown");
    }

    #[test]
    fn parse_reads_a_line_level_dialogue_finding() {
        // A line-level dialogue finding (#1050) quotes the specific spoken line and
        // names the speaker + chapter.
        let json = r#"{"findings":[{"kind":"dialogue","character":"Mara","line":"Yeah, whatever, dude.","detail":"slangy — her profile is clipped and formal","chapters":["Two"]}]}"#;
        let v = parse(json).unwrap();
        assert_eq!(v.findings[0].character, "Mara");
        assert_eq!(v.findings[0].line, "Yeah, whatever, dude.");
        assert_eq!(v.findings[0].chapters, ["Two"]);
    }

    #[test]
    fn line_defaults_to_empty_when_omitted() {
        // A narration finding (or a pre-#1050 reply) omits `line`; it defaults to
        // empty rather than failing to parse.
        let json = r#"{"findings":[{"kind":"narration","character":"Ivo","detail":"POV voice drift","chapters":["One"]}]}"#;
        let v = parse(json).unwrap();
        assert!(v.findings[0].line.is_empty());
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::ModelNonCompliance(_))));
    }

    #[test]
    fn request_carries_the_voice_notes_and_caches_the_manuscript() {
        let req = build_request("MANUSCRIPT".into(), "Mara: clipped, dry", "claude-opus-4-8");
        assert_eq!(
            req.cached_context, "MANUSCRIPT",
            "manuscript stays the cacheable part"
        );
        assert!(
            req.user.contains("Mara: clipped, dry"),
            "voice notes travel in the prompt"
        );
        assert!(req.json_schema.is_some());
    }

    #[test]
    fn prompt_directs_line_level_speaker_attributed_flags_never_a_rewrite() {
        // The deepened prompt (#1050) asks for a specific line, attributed to its
        // speaker, and forbids putting any rewrite in `line`.
        let req = build_request("m".into(), "Mara: clipped", "claude-opus-4-8");
        let system = req.system.as_deref().unwrap_or("");
        assert!(system.contains("SPECIFIC line"), "{system}");
        assert!(system.contains("verbatim spoken line"), "{system}");
        assert!(system.contains("bias toward precision"), "{system}");
        assert!(system.contains("do NOT put any corrected"), "{system}");
        assert!(system.contains("Do NOT rewrite"), "{system}");
    }

    #[test]
    fn schema_offers_line_but_does_not_require_it() {
        // `line` is an available field (so a dialogue finding can quote the line) but
        // is optional (narration findings omit it).
        let s = schema();
        let item = &s["properties"]["findings"]["items"];
        assert_eq!(
            item["properties"]["line"],
            serde_json::json!({ "type": "string" })
        );
        let required = item["required"].as_array().unwrap();
        assert!(
            !required.iter().any(|r| r == "line"),
            "line stays optional: {required:?}"
        );
        // No field carries a replacement line — only observation fields.
        let keys: std::collections::BTreeSet<&str> = item["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["chapters", "character", "detail", "kind", "line"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            "only observation fields; none carries edited prose: {keys:?}"
        );
    }
}
