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
use super::continuity::Continuity;
use super::scaffold::Scaffold;
use super::setting::Setting;
use super::{character, continuity, scaffold, setting, AiError, Request};

/// The bible extracted from imported notes: characters, a setting bible, the
/// story timeline, and a beat outline. Any section the notes don't cover comes
/// back empty.
#[derive(Deserialize, Clone, Debug, Default)]
pub struct Imported {
    #[serde(default)]
    pub characters: Vec<Character>,
    #[serde(default)]
    pub setting: Setting,
    #[serde(default)]
    pub timeline: Continuity,
    #[serde(default)]
    pub outline: Scaffold,
}

const SYSTEM: &str = "You are a story-bible editor. The author has written notes \
about their story — characters, setting, the world's magic or technology systems, \
factions, history, a timeline of events, an outline of beats — in their own \
free-form way. Read the notes and organize what they contain into a structured \
story bible, exactly as specified:\n\
- character profiles;\n\
- a setting bible: group worldbuilding into categories such as Geography & \
locations, the rules of any magic or technology system, Factions, Cultures, and \
World history, plus a gazetteer of names/terms;\n\
- a timeline of concrete story events in chronological order, each with a \
`story_order` rank (tag each with the section/chapter name the notes give it, or \
leave it blank);\n\
- an outline: the beats, grouped by chapter/section as the notes label them.\n\
Extract only what the notes support — do not invent beyond them, and OMIT a \
section the notes say nothing about rather than padding it. Do NOT write story \
prose or scenes. There is no manuscript to check against, so leave each \
character's `drift`, the setting `findings`, the timeline `findings`, and the \
outline `gaps` empty.";

/// The combined schema, composed from each pass's own schema so it always
/// matches their parsers: a character list, a setting bible, a timeline, and an
/// outline.
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "characters": character::items_schema(),
            "setting": setting::schema(),
            "timeline": continuity::schema(),
            "outline": scaffold::schema(),
        },
        "required": ["characters", "setting", "timeline", "outline"],
        "additionalProperties": false
    })
}

/// Build the import request: the author's notes as cached context, the combined
/// schema attached.
pub fn build_request(notes: String, model: &str) -> Request {
    super::manuscript_request(SYSTEM, "Organize these notes into the story bible as specified.", notes, model, schema())
}

/// Parse a structured import reply.
pub fn parse(text: &str) -> Result<Imported, AiError> {
    super::parse_json(text, "import")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_every_section() {
        let json = r#"{
            "characters": [{"name": "Mara"}],
            "setting": {"categories": [{"name": "Magic", "entries": [{"name": "Tidecalling", "detail": "pull the sea"}]}], "gazetteer": ["Saltmarsh"], "findings": []},
            "timeline": {"events": [{"description": "Mara finds the locket", "chapter": "One", "story_order": 1}], "findings": []},
            "outline": {"outline": [{"chapter": "One", "beats": ["Mara arrives home"]}], "gaps": []}
        }"#;
        let imported = parse(json).unwrap();
        assert_eq!(imported.characters[0].name, "Mara");
        assert_eq!(imported.setting.gazetteer, ["Saltmarsh"]);
        assert_eq!(imported.timeline.events.len(), 1);
        assert_eq!(imported.outline.outline[0].beats, ["Mara arrives home"]);
    }

    #[test]
    fn parse_tolerates_missing_sections() {
        // Notes that only describe characters — no setting/timeline/outline.
        let imported = parse(r#"{"characters": [{"name": "Mara"}]}"#).unwrap();
        assert_eq!(imported.characters.len(), 1);
        assert!(imported.setting.categories.is_empty());
        assert!(imported.timeline.events.is_empty());
        assert!(imported.outline.outline.is_empty());
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("not json"), Err(AiError::Api(_))));
    }

    #[test]
    fn schema_composes_each_pass() {
        let s = schema();
        assert!(s["properties"]["characters"]["type"] == "array");
        assert!(s["properties"]["setting"]["properties"]["categories"].is_object());
        assert!(s["properties"]["timeline"]["properties"]["events"].is_object());
        assert!(s["properties"]["outline"]["properties"]["outline"].is_object());
    }
}
