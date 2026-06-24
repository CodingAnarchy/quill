//! In-context thesaurus / word search (#290): the author names a word or phrase
//! and (optionally) the line it sits in, and gets candidate words — or short
//! phrases — that fit, each with a brief definition, tunable by count, how
//! closely the meaning matches, syllable count, language of origin, and rarity.
//!
//! A **reference tool, not a co-writer** (see CLAUDE.md): the surrounding line is
//! used only to judge *fit*; the pass returns only candidates for the requested
//! slot, never rewriting the line or producing prose the author could drop in
//! beyond the requested word/phrase itself.

use serde::Deserialize;
use serde_json::{json, Value};

use super::{AiError, Request, MAX_OUTPUT_TOKENS};

/// One candidate the word search returns.
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Word {
    /// The candidate word or short phrase.
    pub text: String,
    /// A brief definition / sense.
    pub definition: String,
    /// Syllable count, when known.
    #[serde(default)]
    pub syllables: Option<u32>,
    /// Language of origin (e.g. "Anglo-Saxon", "Latinate", "Greek"), when known.
    #[serde(default)]
    pub origin: Option<String>,
    /// Rarity (e.g. "everyday", "common", "literary", "rare", "archaic").
    #[serde(default)]
    pub rarity: Option<String>,
    /// An optional usage note (connotation, register).
    #[serde(default)]
    pub note: Option<String>,
}

const SYSTEM: &str = "You are a thesaurus and word-search assistant for a \
novelist. The author names ONE word or short phrase they want alternatives for, \
and may add constraints in plain language: how many results, how closely the \
meaning should match (a near-synonym vs. a loosely associated word), syllable \
count, language of origin (e.g. Anglo-Saxon, Latinate, Greek), and rarity (from \
everyday to rare or archaic). Honor the constraints the author gives and leave \
the rest open. Return candidate words — or short phrases, when the author asks \
for a phrase — that fit, each with a brief definition and, where you can, its \
syllable count, origin, and rarity. If a surrounding line is provided, use it \
ONLY to judge which candidates fit that context.\n\
You are a reference tool, not a co-writer: return ONLY candidates for the \
requested word or phrase. Do NOT rewrite the surrounding line, suggest edits to \
it or any other prose, or produce any sentence the author could drop in beyond \
the requested word/phrase itself.";

/// The JSON schema for a word-search reply.
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "words": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "text": { "type": "string" },
                        "definition": { "type": "string" },
                        "syllables": { "type": "integer" },
                        "origin": { "type": "string" },
                        "rarity": { "type": "string" },
                        "note": { "type": "string" }
                    },
                    "required": ["text", "definition"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["words"],
        "additionalProperties": false
    })
}

/// Build the word-search request: the author's word/phrase + constraints, and the
/// surrounding line (when given) for fit only. No manuscript context — it's a
/// small, cheap lookup.
pub fn build_request(request: &str, context: &str, model: &str) -> Request {
    let context = context.trim();
    let user = if context.is_empty() {
        format!("Word or phrase, with any constraints:\n{request}")
    } else {
        format!(
            "Word or phrase, with any constraints:\n{request}\n\n\
             Surrounding line (use only to judge fit — do not rewrite it):\n{context}"
        )
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

/// Parse a structured word-search reply.
pub fn parse(text: &str) -> Result<Vec<Word>, AiError> {
    #[derive(Deserialize)]
    struct Reply {
        words: Vec<Word>,
    }
    let reply: Reply =
        super::parse_json(text, "word-search")?;
    Ok(reply.words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_candidates() {
        let json = r#"{"words":[
            {"text":"melancholy","definition":"a pensive sadness","syllables":4,"origin":"Latinate","rarity":"common"},
            {"text":"woebegone","definition":"sorrowful","note":"slightly archaic"}
        ]}"#;
        let words = parse(json).unwrap();
        assert_eq!(words.len(), 2);
        assert_eq!(words[0].text, "melancholy");
        assert_eq!(words[0].syllables, Some(4));
        assert_eq!(words[1].origin, None);
        assert_eq!(words[1].note.as_deref(), Some("slightly archaic"));
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::Api(_))));
    }

    #[test]
    fn request_includes_context_only_when_given() {
        let with = build_request("sad, 2 syllables", "she was feeling", "claude-opus-4-8");
        assert!(with.user.contains("Surrounding line") && with.user.contains("she was feeling"));
        let without = build_request("sad", "  ", "claude-opus-4-8");
        assert!(!without.user.contains("Surrounding line"));
        // No manuscript is ever attached — it's a cheap, off-manuscript lookup.
        assert!(with.cached_context.is_empty());
    }
}
