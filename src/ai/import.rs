//! Importing existing notes into the story bible (#283): an extractive AI pass
//! that reads Markdown notes the author already wrote — about characters,
//! setting, the world — and organizes them into the project's bible documents
//! (character profiles + a setting bible) in Quill's format.
//!
//! Off-manuscript, like every bible pass: it tracks and organizes the author's
//! material, never writing story prose. The notes are the *input* (the author's
//! source is preserved separately under `bible/sources/`); this pass only shapes
//! them into the structured, mergeable bible files.

use serde::Deserialize;
use serde_json::{json, Value};

use super::character::Character;
use super::setting::Setting;
use super::{character, setting, AiError, Request, MAX_OUTPUT_TOKENS};

/// The bible extracted from imported notes: characters and a setting bible.
#[derive(Deserialize, Clone, Debug, Default)]
pub struct Imported {
    #[serde(default)]
    pub characters: Vec<Character>,
    #[serde(default)]
    pub setting: Setting,
}

const SYSTEM: &str = "You are a story-bible editor. The author has written notes \
about their story — characters, setting, world, factions, history — in their own \
free-form way. Read the notes and organize what they contain into a structured \
story bible: character profiles and a setting bible, exactly as specified. \
Extract only what the notes support — do not invent beyond them. Do NOT write \
story prose or scenes. Leave each character's `drift` and the setting `findings` \
empty: there is no manuscript to check against. Omit a section the notes say \
nothing about (no characters, or no setting) rather than padding it.";

/// The combined schema: a character list plus a setting bible, composed from the
/// character and setting passes' own schemas so it always matches their parsers.
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "characters": character::items_schema(),
            "setting": setting::schema(),
        },
        "required": ["characters", "setting"],
        "additionalProperties": false
    })
}

/// Build the import request: the author's notes as cached context, the combined
/// schema attached.
pub fn build_request(notes: String, model: &str) -> Request {
    Request {
        system: Some(SYSTEM.to_string()),
        cached_context: notes,
        user: "Organize these notes into the story bible as specified.".to_string(),
        model: model.to_string(),
        max_tokens: MAX_OUTPUT_TOKENS,
        json_schema: Some(schema()),
    }
}

/// Parse a structured import reply.
pub fn parse(text: &str) -> Result<Imported, AiError> {
    serde_json::from_str(text).map_err(|e| AiError::Api(format!("bad import JSON: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_characters_and_setting() {
        let json = r#"{
            "characters": [{"name": "Mara"}],
            "setting": {"categories": [{"name": "Places", "entries": [{"name": "Saltmarsh", "detail": "a fog-bound coast"}]}], "gazetteer": ["Saltmarsh"], "findings": []}
        }"#;
        let imported = parse(json).unwrap();
        assert_eq!(imported.characters.len(), 1);
        assert_eq!(imported.characters[0].name, "Mara");
        assert_eq!(imported.setting.gazetteer, ["Saltmarsh"]);
    }

    #[test]
    fn parse_tolerates_missing_sections() {
        // Notes that only describe characters — no setting object.
        let imported = parse(r#"{"characters": [{"name": "Mara"}]}"#).unwrap();
        assert_eq!(imported.characters.len(), 1);
        assert!(imported.setting.categories.is_empty());
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("not json"), Err(AiError::Api(_))));
    }

    #[test]
    fn schema_composes_both_passes() {
        let s = schema();
        assert!(s["properties"]["characters"]["type"] == "array");
        assert!(s["properties"]["setting"]["properties"]["categories"].is_object());
    }
}
