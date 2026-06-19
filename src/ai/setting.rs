//! Setting & worldbuilding: a structured pass that extracts the setting bible —
//! geography/locations, magic/tech system rules, factions, cultures, world
//! history — to `bible/setting.md`, records a place/term gazetteer (for naming
//! consistency, #78), and flags prose that violates an established rule. The
//! bible file reuses the shared [`bible`] format; the rule violations surface as
//! triageable findings. See `docs/ai-features.md` §4.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{bible, AiError, Request};

/// The result of a setting pass: the categorized bible, the gazetteer, and the
/// rule-consistency findings.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Setting {
    #[serde(default)]
    pub categories: Vec<Category>,
    /// Place and invented-term names, for naming consistency (#78).
    #[serde(default)]
    pub gazetteer: Vec<String>,
    #[serde(default)]
    pub findings: Vec<RuleFinding>,
}

/// One bible category (e.g. "Geography", "Magic system", "Factions").
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Category {
    pub name: String,
    #[serde(default)]
    pub entries: Vec<Entry>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Entry {
    pub name: String,
    pub detail: String,
}

/// A place where the prose breaks an established worldbuilding rule.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct RuleFinding {
    /// The established rule, e.g. "magic requires a focus object".
    pub rule: String,
    /// How the prose violates it.
    pub violation: String,
    /// Chapters involved (titles).
    #[serde(default)]
    pub chapters: Vec<String>,
}

const SYSTEM: &str = "You are a worldbuilding editor. Read the novel manuscript \
and (1) extract a setting bible, grouped into categories such as Geography & \
locations, the rules of any magic or technology system, Factions, Cultures, and \
World history — each category a list of named entries with a concise detail; (2) \
list the place names and invented terms as a gazetteer; and (3) flag places where \
the prose violates an established rule of the world (e.g. \"magic requires a \
focus, but ch. 14 casts without one\"), each with the chapters involved. Be \
faithful to the text; report only genuine rule violations. Reference chapters by \
title.";

/// The JSON Schema for a setting reply.
pub fn schema() -> Value {
    let strings = json!({ "type": "array", "items": { "type": "string" } });
    json!({
        "type": "object",
        "properties": {
            "categories": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "entries": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "name": { "type": "string" },
                                    "detail": { "type": "string" }
                                },
                                "required": ["name", "detail"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["name", "entries"],
                    "additionalProperties": false
                }
            },
            "gazetteer": strings,
            "findings": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "rule": { "type": "string" },
                        "violation": { "type": "string" },
                        "chapters": strings
                    },
                    "required": ["rule", "violation", "chapters"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["categories", "gazetteer", "findings"],
        "additionalProperties": false
    })
}

/// Build the setting request: the manuscript as cached context, schema attached.
pub fn build_request(manuscript: String, model: &str) -> Request {
    Request {
        system: Some(SYSTEM.to_string()),
        cached_context: manuscript,
        user: "Extract the setting bible, gazetteer, and rule findings as specified.".to_string(),
        model: model.to_string(),
        max_tokens: 8192,
        json_schema: Some(schema()),
    }
}

/// Parse a structured setting reply.
pub fn parse(text: &str) -> Result<Setting, AiError> {
    serde_json::from_str(text).map_err(|e| AiError::Api(format!("bad setting JSON: {e}")))
}

/// Render the setting bible file via [`bible::render`]: a section per category, a
/// gazetteer section, and the rule findings.
pub fn render(s: &Setting, notes: &str, today: &str) -> String {
    let mut body = String::new();

    for cat in &s.categories {
        body.push_str(&format!("## {}\n", cat.name));
        for e in &cat.entries {
            body.push_str(&format!("- **{}** — {}\n", e.name, e.detail));
        }
        body.push('\n');
    }

    if !s.gazetteer.is_empty() {
        body.push_str("## Gazetteer\n");
        for term in &s.gazetteer {
            body.push_str(&format!("- {term}\n"));
        }
        body.push('\n');
    }

    if !s.findings.is_empty() {
        body.push_str("## Rule findings\n");
        for f in &s.findings {
            let where_ = if f.chapters.is_empty() {
                String::new()
            } else {
                format!(" ({})", f.chapters.join(", "))
            };
            body.push_str(&format!("- **{}:** {}{}\n", f.rule, f.violation, where_));
        }
        body.push('\n');
    }

    bible::render("title: Setting\n", &body, notes, today)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Setting {
        Setting {
            categories: vec![Category {
                name: "Magic system".into(),
                entries: vec![Entry {
                    name: "Casting".into(),
                    detail: "requires a focus object".into(),
                }],
            }],
            gazetteer: vec!["Aldermoor".into(), "lightsteel".into()],
            findings: vec![RuleFinding {
                rule: "magic requires a focus".into(),
                violation: "ch. 14 casts without one".into(),
                chapters: vec!["The Duel".into()],
            }],
        }
    }

    #[test]
    fn parse_reads_categories_gazetteer_findings() {
        let json = r#"{"categories":[{"name":"Geography","entries":[]}],"gazetteer":["Aldermoor"],"findings":[]}"#;
        let s = parse(json).unwrap();
        assert_eq!(s.categories.len(), 1);
        assert_eq!(s.categories[0].name, "Geography");
        assert_eq!(s.gazetteer, vec!["Aldermoor"]);
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::Api(_))));
    }

    #[test]
    fn render_emits_sections_gazetteer_and_findings() {
        let file = render(&sample(), "## Notes\n", "2026-06-19");
        assert!(file.contains("title: Setting"));
        assert!(file.contains("## Magic system"));
        assert!(file.contains("**Casting** — requires a focus object"));
        assert!(file.contains("## Gazetteer"));
        assert!(file.contains("- lightsteel"));
        assert!(file.contains("## Rule findings"));
        assert!(file.contains("ch. 14 casts without one"));
    }
}
