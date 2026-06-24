//! Generate an author workflow from a plain-language description (#232): map the
//! author's words ("save, refresh the character bible, then export to Word") onto
//! an ordered list of the registry's action tokens. The request constrains the
//! steps to a closed vocabulary (the valid action / workflow names) so the model
//! can't invent a step, and carries an `unmapped` list so it can report parts of
//! the request it couldn't fulfil. This configures tooling, never manuscript prose
//! — within Quill's AI boundary. See `docs/ai-features.md`.

use serde::Deserialize;
use serde_json::{json, Value};

use super::{AiError, Request};

/// A small reply — a name plus a handful of step tokens — so a tight bound is fine.
const MAX_TOKENS: u32 = 1024;

/// A workflow the model proposed from a description: a name, the ordered step
/// tokens (each guaranteed to be a valid action / workflow name by the schema),
/// and any requested steps it couldn't map to an available action.
#[derive(Deserialize, Clone, Debug, Default, PartialEq)]
pub struct GeneratedWorkflow {
    pub name: String,
    #[serde(default)]
    pub steps: Vec<String>,
    #[serde(default)]
    pub unmapped: Vec<String>,
}

const SYSTEM: &str = "You configure author workflows for a novel-writing editor. \
Given a plain-language description of a routine, produce an ordered list of action \
steps that carries it out, plus a short kebab-case name for it. Choose steps ONLY \
from the provided action vocabulary — never invent an action or emit any other \
string. Put any part of the request that has no matching action into `unmapped` \
(as a short phrase) rather than approximating it. Keep the steps in the order the \
author described. Do not write or suggest any manuscript prose.";

/// Render the allowed vocabulary — each `token` with a human description — as the
/// context the model chooses steps from.
fn vocab_context(vocab: &[(String, String)]) -> String {
    let mut s = String::from("Available actions (use the exact token on the left):\n");
    for (token, desc) in vocab {
        s.push_str(&format!("- {token}: {desc}\n"));
    }
    s
}

/// The JSON Schema for a reply: `steps` is constrained to the vocabulary tokens.
fn schema(tokens: &[String]) -> Value {
    json!({
        "type": "object",
        "properties": {
            "name": { "type": "string" },
            "steps": { "type": "array", "items": { "enum": tokens } },
            "unmapped": { "type": "array", "items": { "type": "string" } }
        },
        "required": ["name", "steps", "unmapped"],
        "additionalProperties": false
    })
}

/// Build the generation request: the vocabulary as context, the author's
/// description as the user message, the steps constrained to the vocabulary.
pub fn build_request(description: &str, vocab: &[(String, String)], model: &str) -> Request {
    let tokens: Vec<String> = vocab.iter().map(|(t, _)| t.clone()).collect();
    Request {
        system: Some(SYSTEM.to_string()),
        cached_context: vocab_context(vocab),
        user: format!("Describe-to-workflow request:\n{description}"),
        model: model.to_string(),
        max_tokens: MAX_TOKENS,
        json_schema: Some(schema(&tokens)),
    }
}

/// Parse a generated-workflow reply.
pub fn parse(text: &str) -> Result<GeneratedWorkflow, AiError> {
    super::parse_json(text, "workflow")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_constrains_steps_to_the_vocabulary() {
        let s = schema(&["save".into(), "export:docx".into()]);
        let items_enum = &s["properties"]["steps"]["items"]["enum"];
        assert_eq!(items_enum, &json!(["save", "export:docx"]));
    }

    #[test]
    fn build_request_carries_the_description_and_vocab() {
        let vocab = vec![("save".into(), "Save".into())];
        let req = build_request("save then export", &vocab, "claude-opus-4-8");
        assert!(req.user.contains("save then export"));
        assert!(req.cached_context.contains("save: Save"));
        assert!(req.json_schema.is_some());
    }

    #[test]
    fn parse_reads_name_steps_and_unmapped() {
        let g = parse(r#"{"name":"revise","steps":["save","ai:continuity"],"unmapped":["email my editor"]}"#)
            .unwrap();
        assert_eq!(g.name, "revise");
        assert_eq!(g.steps, ["save", "ai:continuity"]);
        assert_eq!(g.unmapped, ["email my editor"]);
    }

    #[test]
    fn parse_tolerates_missing_optional_arrays() {
        let g = parse(r#"{"name":"x"}"#).unwrap();
        assert!(g.steps.is_empty() && g.unmapped.is_empty());
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::Api(_))));
    }
}
