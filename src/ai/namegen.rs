//! Name generator (#610): the author names what they need a name for — a
//! character, a place, or an invented term — and the constraints it should meet
//! (origin/language, syllable count, tone, era), and gets a list of candidate
//! names to choose among. The author picks one and Quill places it at the cursor;
//! Quill never writes it in on its own.
//!
//! An **option-list lookup, not a co-writer** (see CLAUDE.md): exactly the
//! thesaurus's shape (`thesaurus.rs`) turned to naming. Constraints in, candidate
//! names out — never prose, never a rewrite of the surrounding text. The pass
//! returns only names for the slot the author asked about; the author chooses and
//! places the one they want.

use serde::Deserialize;
use serde_json::{json, Value};

use super::{AiError, Request, MAX_OUTPUT_TOKENS};

/// What the author wants a name for — the one axis that shapes the candidates
/// (a person vs. a place vs. a coined term), chosen before the free-text
/// constraints. Mirrors the thesaurus's single explicit input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A character / person name.
    Character,
    /// A place name (city, region, landmark).
    Place,
    /// An invented term (an object, a concept, a made-up word).
    Term,
}

impl Kind {
    /// The three kinds, in menu order.
    pub const ALL: &'static [Kind] = &[Kind::Character, Kind::Place, Kind::Term];

    /// A short human label for the picker row and prompt.
    pub fn label(self) -> &'static str {
        match self {
            Kind::Character => "character",
            Kind::Place => "place",
            Kind::Term => "invented term",
        }
    }

    /// How the kind reads in the request to the model.
    fn phrase(self) -> &'static str {
        match self {
            Kind::Character => "a character (person) name",
            Kind::Place => "a place name (city, region, or landmark)",
            Kind::Term => "an invented term (a coined word for an object or concept)",
        }
    }
}

/// One candidate the name generator returns.
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Name {
    /// The candidate name.
    pub text: String,
    /// A brief gloss — the meaning, root, or the feel it carries.
    #[serde(default)]
    pub note: String,
    /// Syllable count, when known.
    #[serde(default)]
    pub syllables: Option<u32>,
    /// Language / cultural origin (e.g. "Old Norse", "Latinate", "invented"),
    /// when known.
    #[serde(default)]
    pub origin: Option<String>,
    /// A rough pronunciation hint, when helpful.
    #[serde(default)]
    pub pronunciation: Option<String>,
}

const SYSTEM: &str = "You are a name-generation assistant for a novelist. The \
author tells you what they need a name for — a character, a place, or an \
invented term — and may add constraints in plain language: language or cultural \
origin, syllable count, tone or feel (e.g. harsh, soft, noble, sinister), and \
era or period. Honor the constraints the author gives and leave the rest open. \
Return a list of distinct candidate names that fit, each with a brief note on \
its meaning, root, or feel, and — where you can — its syllable count, origin, \
and a pronunciation hint.\n\
You return an OPTION LIST the author chooses among, exactly like a thesaurus \
lookup: only candidate names for the slot the author described. Do NOT write, \
suggest, or edit any manuscript prose, sentences, character descriptions, or \
scenes — names only.";

/// The JSON schema for a name-generation reply.
pub fn schema() -> Value {
    let name = super::object_schema(
        vec![
            ("text", super::string_field()),
            ("note", super::string_field()),
            ("syllables", json!({ "type": "integer" })),
            ("origin", super::string_field()),
            ("pronunciation", super::string_field()),
        ],
        &["text"],
    );
    super::object_schema(vec![("names", super::array_of(name))], &["names"])
}

/// Build the name-generation request: the kind (character/place/term) and the
/// author's free-text constraints. No manuscript context — it's a small, cheap
/// option-list lookup, like the thesaurus.
pub fn build_request(kind: Kind, constraints: &str, model: &str) -> Request {
    let constraints = constraints.trim();
    let base = format!("Generate names for {}.", kind.phrase());
    let user = if constraints.is_empty() {
        base
    } else {
        format!("{base}\n\nConstraints (origin, syllables, tone, era):\n{constraints}")
    };
    Request {
        system: Some(SYSTEM.to_string()),
        cached_context: String::new(),
        user,
        model: model.to_string(),
        max_tokens: MAX_OUTPUT_TOKENS,
        json_schema: Some(schema()),
    }
}

/// Parse a structured name-generation reply.
pub fn parse(text: &str) -> Result<Vec<Name>, AiError> {
    #[derive(Deserialize)]
    struct Reply {
        names: Vec<Name>,
    }
    let reply: Reply = super::parse_json(text, "name-generation")?;
    Ok(reply.names)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_candidates() {
        let json = r#"{"names":[
            {"text":"Aldric","note":"old ruler","syllables":2,"origin":"Old English","pronunciation":"AL-drik"},
            {"text":"Ves","note":"terse, invented"}
        ]}"#;
        let names = parse(json).unwrap();
        assert_eq!(names.len(), 2);
        assert_eq!(names[0].text, "Aldric");
        assert_eq!(names[0].syllables, Some(2));
        assert_eq!(names[0].origin.as_deref(), Some("Old English"));
        assert_eq!(names[1].origin, None);
        assert_eq!(names[1].note, "terse, invented");
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::ModelNonCompliance(_))));
    }

    #[test]
    fn request_names_the_kind_and_captures_constraints() {
        let with = build_request(
            Kind::Place,
            "Norse-flavored, 2 syllables, cold and stark",
            "claude-opus-4-8",
        );
        assert!(with.user.contains("place name"));
        assert!(with.user.contains("Constraints"));
        assert!(with.user.contains("Norse-flavored"));
        // No manuscript is ever attached — it's a cheap, off-manuscript lookup.
        assert!(with.cached_context.is_empty());
    }

    #[test]
    fn request_omits_the_constraints_block_when_none_given() {
        let bare = build_request(Kind::Character, "  ", "claude-opus-4-8");
        assert!(bare.user.contains("character"));
        assert!(!bare.user.contains("Constraints"));
    }
}
