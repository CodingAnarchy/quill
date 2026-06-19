//! Timeline & continuity: a structured pass that extracts story events into an
//! ordered timeline (story vs narrative order) and flags continuity
//! contradictions (a character in two places, an age/season that doesn't add up,
//! an object used after it's destroyed). The timeline is written to
//! `bible/timeline.md`; the contradictions are surfaced as a triageable findings
//! list. See `docs/ai-features.md` §2.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{bible, AiError, Request};

/// The result of a continuity pass: the ordered events and the contradictions.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Continuity {
    #[serde(default)]
    pub events: Vec<Event>,
    #[serde(default)]
    pub findings: Vec<Finding>,
}

/// One story event. The model returns events in **narrative** order (order of
/// appearance) and assigns each a `story_order` rank for chronological sorting.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Event {
    pub description: String,
    /// The chapter (title) the event appears in.
    pub chapter: String,
    /// Chronological rank in the story's timeline (1 = earliest).
    #[serde(default)]
    pub story_order: u32,
}

/// One continuity contradiction.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Finding {
    /// A short category, e.g. "location", "age", "object", "timeline".
    pub kind: String,
    pub detail: String,
    /// Chapters involved (titles).
    #[serde(default)]
    pub chapters: Vec<String>,
}

const SYSTEM: &str = "You are a continuity editor. Read the novel manuscript and \
(1) extract the concrete story events, listed in narrative order (order of \
appearance) with a `story_order` rank giving their true chronological order, each \
tagged with the chapter title it appears in; and (2) cross-reference the \
manuscript for genuine continuity contradictions — a character in two places at \
once, an age/season/eye-color that doesn't add up, an object used after it was \
lost or destroyed — reporting each with the chapters involved. Be faithful to the \
text; report only real contradictions, not stylistic choices. Reference chapters \
by title.";

/// The JSON Schema for a continuity reply.
pub fn schema() -> Value {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    json!({
        "type": "object",
        "properties": {
            "events": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "description": { "type": "string" },
                        "chapter": { "type": "string" },
                        "story_order": { "type": "integer" }
                    },
                    "required": ["description", "chapter", "story_order"],
                    "additionalProperties": false
                }
            },
            "findings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "kind": { "type": "string" },
                        "detail": { "type": "string" },
                        "chapters": strings
                    },
                    "required": ["kind", "detail", "chapters"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["events", "findings"],
        "additionalProperties": false
    })
}

/// Build the continuity request: the manuscript as cached context, schema attached.
pub fn build_request(manuscript: String, model: &str) -> Request {
    Request {
        system: Some(SYSTEM.to_string()),
        cached_context: manuscript,
        user: "Extract the timeline and continuity findings as specified.".to_string(),
        model: model.to_string(),
        max_tokens: 8192,
        json_schema: Some(schema()),
    }
}

/// Parse a structured continuity reply.
pub fn parse(text: &str) -> Result<Continuity, AiError> {
    serde_json::from_str(text).map_err(|e| AiError::Api(format!("bad continuity JSON: {e}")))
}

/// Render the timeline bible file: events in narrative order, then sorted into
/// story (chronological) order, via [`bible::render`]. `today` is the date.
pub fn render_timeline(c: &Continuity, notes: &str, today: &str) -> String {
    let mut body = String::new();

    body.push_str("## Timeline (narrative order)\n");
    for e in &c.events {
        body.push_str(&format!("- **{}:** {}\n", e.chapter, e.description));
    }
    body.push('\n');

    let mut chrono: Vec<&Event> = c.events.iter().collect();
    chrono.sort_by_key(|e| e.story_order);
    body.push_str("## Timeline (story order)\n");
    for e in &chrono {
        body.push_str(&format!("- {} _({})_\n", e.description, e.chapter));
    }
    body.push('\n');

    if !c.findings.is_empty() {
        body.push_str("## Continuity findings\n");
        for f in &c.findings {
            let where_ = if f.chapters.is_empty() {
                String::new()
            } else {
                format!(" ({})", f.chapters.join(", "))
            };
            body.push_str(&format!("- **{}:** {}{}\n", f.kind, f.detail, where_));
        }
        body.push('\n');
    }

    bible::render("title: Timeline\n", &body, notes, today)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Continuity {
        Continuity {
            events: vec![
                Event { description: "Mara finds the locket".into(), chapter: "The Find".into(), story_order: 2 },
                Event { description: "Tomas hides the locket".into(), chapter: "The Reckoning".into(), story_order: 1 },
            ],
            findings: vec![Finding {
                kind: "object".into(),
                detail: "the locket is described as silver in ch.1 and gold in ch.2".into(),
                chapters: vec!["The Find".into(), "The Reckoning".into()],
            }],
        }
    }

    #[test]
    fn parse_reads_events_and_findings() {
        let json = r#"{"events":[{"description":"x","chapter":"One","story_order":1}],"findings":[]}"#;
        let c = parse(json).unwrap();
        assert_eq!(c.events.len(), 1);
        assert_eq!(c.events[0].chapter, "One");
        assert!(c.findings.is_empty());
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::Api(_))));
    }

    #[test]
    fn render_timeline_orders_both_ways_and_lists_findings() {
        let file = render_timeline(&sample(), "## Notes\n", "2026-06-19");
        assert!(file.contains("title: Timeline"));
        assert!(file.contains("Timeline (narrative order)"));
        assert!(file.contains("Timeline (story order)"));
        assert!(file.contains("Continuity findings"));
        // Story order sorts "Tomas hides" (1) before "Mara finds" (2); narrative
        // order keeps "Mara finds" first.
        let story = file.split("story order)").nth(1).unwrap();
        let hides = story.find("Tomas hides").unwrap();
        let finds = story.find("Mara finds").unwrap();
        assert!(hides < finds, "story order must sort by story_order");
    }
}
