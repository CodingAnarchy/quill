//! Story brainstorming (#255): the first *generative* AI pass — it works from a
//! premise or open prompt, not the manuscript, so it's usable at chapter zero. It
//! produces idea notes (premise variations, "what if" questions, themes, central
//! conflicts) written to an off-manuscript brainstorm doc, never prose scenes — so
//! it stays within the assistant-not-co-writer rule. See `docs/ai-features.md`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{bible, AiError, Request, MAX_OUTPUT_TOKENS};

/// A round of brainstorm ideas from a premise.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Brainstorm {
    #[serde(default)]
    pub premises: Vec<String>,
    #[serde(default)]
    pub what_ifs: Vec<String>,
    #[serde(default)]
    pub themes: Vec<String>,
    #[serde(default)]
    pub conflicts: Vec<String>,
}

const SYSTEM: &str = "You are a brainstorming partner for a novelist starting from \
a spark. From the author's premise or idea, brainstorm: alternative premise \
variations that take it in different directions; \"what if\" questions that open \
the story up; possible themes; and central conflicts that could drive it. These \
are idea notes for the author to choose among and develop — NOT prose, NOT scenes, \
NOT a draft of the novel. Keep each a short, concrete note.";

/// The JSON Schema for a brainstorm reply.
pub fn schema() -> Value {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    json!({
        "type": "object",
        "properties": {
            "premises": strings,
            "what_ifs": strings,
            "themes": strings,
            "conflicts": strings
        },
        "required": ["premises", "what_ifs", "themes", "conflicts"],
        "additionalProperties": false
    })
}

/// Build the brainstorm request: the premise as context, schema attached. No
/// manuscript is sent — this works on an empty project.
pub fn build_request(premise: &str, model: &str) -> Request {
    Request {
        system: Some(SYSTEM.to_string()),
        cached_context: format!("Premise / idea:\n{premise}"),
        user: "Brainstorm story ideas from this premise as specified.".to_string(),
        model: model.to_string(),
        max_tokens: MAX_OUTPUT_TOKENS,
        json_schema: Some(schema()),
    }
}

/// Parse a structured brainstorm reply.
pub fn parse(text: &str) -> Result<Brainstorm, AiError> {
    serde_json::from_str(text).map_err(|e| AiError::Api(format!("bad brainstorm JSON: {e}")))
}

/// Render the brainstorm bible doc: a section per idea kind, then the author's
/// `notes`, via [`bible::render`]. `today` is the generation date.
pub fn render(b: &Brainstorm, notes: &str, today: &str) -> String {
    let mut body = String::new();
    let section = |body: &mut String, heading: &str, items: &[String]| {
        if !items.is_empty() {
            body.push_str(&format!("## {heading}\n"));
            for item in items {
                body.push_str(&format!("- {item}\n"));
            }
            body.push('\n');
        }
    };
    section(&mut body, "Premises", &b.premises);
    section(&mut body, "What if", &b.what_ifs);
    section(&mut body, "Themes", &b.themes);
    section(&mut body, "Central conflicts", &b.conflicts);
    bible::render("title: Brainstorm\n", &body, notes, today)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_all_idea_kinds() {
        let json = r#"{"premises":["p"],"what_ifs":["w"],"themes":["t"],"conflicts":["c"]}"#;
        let b = parse(json).unwrap();
        assert_eq!(b.premises, ["p"]);
        assert_eq!(b.what_ifs, ["w"]);
        assert_eq!(b.conflicts, ["c"]);
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::Api(_))));
    }

    #[test]
    fn request_carries_the_premise_and_no_manuscript() {
        let req = build_request("a lighthouse keeper finds a body", "claude-opus-4-8");
        assert!(req.cached_context.contains("lighthouse keeper"));
        assert!(req.json_schema.is_some());
    }

    #[test]
    fn render_has_sections_and_preserves_notes() {
        let b = Brainstorm {
            premises: vec!["a quiet town with a secret".into()],
            what_ifs: vec!["what if the keeper knew the victim".into()],
            themes: vec!["isolation".into()],
            conflicts: vec!["truth vs. self-preservation".into()],
        };
        let file = render(&b, "## Notes\n\nmine.\n", "2026-06-22");
        assert!(file.contains("title: Brainstorm"));
        assert!(file.contains("## Premises") && file.contains("a quiet town"));
        assert!(file.contains("## What if") && file.contains("## Central conflicts"));
        assert_eq!(bible::extract_notes(&file).trim(), "## Notes\n\nmine.");
    }
}
