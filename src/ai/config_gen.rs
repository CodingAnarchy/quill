//! Generate configuration changes from a plain-language description (#303): map
//! "give me a warm dark theme" or "turn on Vim mode" onto concrete `key = value`
//! settings. The request constrains the keys to a closed vocabulary (the real
//! settings, enumerated by the caller) so the model can't invent one, and carries
//! an `unmapped` list for parts of the request that match no setting. This
//! configures the editor, never manuscript prose — within Quill's AI boundary.
//! The proposal is validated and confirmed by the author before anything is
//! written. See `docs/ai-features.md`.

use serde::Deserialize;
use serde_json::{json, Value};

use super::{AiError, Request};

/// A small reply — a handful of key/value pairs — so a tight bound is fine.
const MAX_TOKENS: u32 = 1024;

/// One proposed setting: a key from the vocabulary and the value to set it to,
/// both as strings (the value is validated and parsed by the caller before it's
/// shown or written).
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ConfigChange {
    pub key: String,
    pub value: String,
}

/// The settings the model proposed from a description: the concrete changes (each
/// key guaranteed valid by the schema) and any requested change it couldn't map
/// to a real setting.
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
pub struct GeneratedConfig {
    #[serde(default)]
    pub changes: Vec<ConfigChange>,
    #[serde(default)]
    pub unmapped: Vec<String>,
}

const SYSTEM: &str = "You configure a novel-writing editor. Given a plain-language \
description of how the author wants it set up, produce concrete settings that carry \
it out: each a `key` chosen ONLY from the provided settings vocabulary and a `value` \
appropriate to it. Never invent a key or emit one outside the vocabulary. Values: \
booleans are \"true\"/\"false\"; a width is an integer; a theme name is one of its \
listed options; a color is an ANSI name (e.g. \"cyan\"), a hex string (\"#1e1e24\" \
or \"#abc\"), an \"r,g,b\" triple, or a 0–255 index. For a mood like \"warm\" or \
\"cool\", choose fitting colors for the relevant theme fields. Put any part of the \
request that maps to no available setting into `unmapped` (a short phrase) rather \
than approximating it. Do not write or suggest any manuscript prose.";

/// The JSON Schema for a reply: each change's `key` is constrained to the
/// vocabulary; the `value` is a free string (validated by the caller).
fn schema(keys: &[String]) -> Value {
    json!({
        "type": "object",
        "properties": {
            "changes": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "key": { "enum": keys },
                        "value": { "type": "string" }
                    },
                    "required": ["key", "value"],
                    "additionalProperties": false
                }
            },
            "unmapped": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["changes", "unmapped"],
        "additionalProperties": false
    })
}

/// Build the generation request: the settings vocabulary as context, the author's
/// description as the user message, the keys constrained to the vocabulary.
pub fn build_request(description: &str, vocab: &[(String, String)], model: &str) -> Request {
    let keys: Vec<String> = vocab.iter().map(|(k, _)| k.clone()).collect();
    Request {
        system: Some(SYSTEM.to_string()),
        cached_context: super::vocab_context("Available settings (use the exact key on the left):", vocab),
        user: format!("Describe-to-config request:\n{description}"),
        model: model.to_string(),
        max_tokens: MAX_TOKENS,
        json_schema: Some(schema(&keys)),
    }
}

/// Parse a generated-config reply.
pub fn parse(text: &str) -> Result<GeneratedConfig, AiError> {
    super::parse_json(text, "config")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_constrains_keys_to_the_vocabulary() {
        let s = schema(&["editor.vim".into(), "theme.name".into()]);
        let key_enum = &s["properties"]["changes"]["items"]["properties"]["key"]["enum"];
        assert_eq!(key_enum, &json!(["editor.vim", "theme.name"]));
    }

    #[test]
    fn build_request_carries_the_description_and_vocab() {
        let vocab = vec![("editor.vim".into(), "Vim modal editing (true/false)".into())];
        let req = build_request("turn on vim", &vocab, "claude-opus-4-8");
        assert!(req.user.contains("turn on vim"));
        assert!(req.cached_context.contains("editor.vim:"));
        assert!(req.json_schema.is_some());
    }

    #[test]
    fn parse_reads_changes_and_unmapped() {
        let g = parse(
            r#"{"changes":[{"key":"editor.vim","value":"true"},{"key":"theme.name","value":"dark"}],"unmapped":["bigger font"]}"#,
        )
        .unwrap();
        assert_eq!(g.changes.len(), 2);
        assert_eq!(g.changes[0], ConfigChange { key: "editor.vim".into(), value: "true".into() });
        assert_eq!(g.unmapped, ["bigger font"]);
    }

    #[test]
    fn parse_tolerates_missing_optional_arrays() {
        let g = parse(r#"{"changes":[]}"#).unwrap();
        assert!(g.changes.is_empty() && g.unmapped.is_empty());
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::ModelNonCompliance(_))));
    }
}
