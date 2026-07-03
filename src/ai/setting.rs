//! Setting & worldbuilding: a structured pass that extracts the setting bible —
//! geography/locations, magic/tech system rules, factions, cultures, world
//! history — to `bible/setting.md`, records a place/term gazetteer (for naming
//! consistency, #78), and flags prose that violates an established rule. The
//! bible file reuses the shared [`bible`] format; the rule violations surface as
//! triageable findings. See `docs/ai-features.md` §4.

use std::fmt::Write;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{bible, incremental, AiError, Request, MAX_OUTPUT_TOKENS};

/// The result of a setting pass: the categorized bible, the gazetteer, and the
/// rule-consistency findings.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
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
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Category {
    pub name: String,
    #[serde(default)]
    pub entries: Vec<Entry>,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub detail: String,
}

/// A place where the prose breaks an established worldbuilding rule.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
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
    let entry = super::object_schema(
        vec![
            ("name", super::string_field()),
            ("detail", super::string_field()),
        ],
        &["name", "detail"],
    );
    let category = super::object_schema(
        vec![
            ("name", super::string_field()),
            ("entries", super::array_of(entry)),
        ],
        &["name", "entries"],
    );
    super::object_schema(
        vec![
            ("categories", super::array_of(category)),
            ("gazetteer", super::string_array()),
            (
                "findings",
                super::array_of(super::chapters_finding("rule", "violation")),
            ),
        ],
        &["categories", "gazetteer", "findings"],
    )
}

/// Build the setting request: the manuscript as cached context, schema attached.
pub fn build_request(manuscript: String, model: &str) -> Request {
    super::manuscript_request(
        SYSTEM,
        "Extract the setting bible, gazetteer, and rule findings as specified.",
        manuscript,
        model,
        schema(),
    )
}

const CONCEPT_SYSTEM: &str = "You are a worldbuilding editor helping an author \
start a novel from a concept, before any draft exists. From the concept or \
premise, INVENT a starter setting bible: group it into categories such as \
Geography & locations, the rules of any magic or technology system, Factions, \
Cultures, and World history — each a list of named entries with a concise detail. \
Also list the invented place names and terms as a gazetteer. These are \
first-draft suggestions the author will reshape, not canon. Do NOT write prose or \
scenes. Leave `findings` empty — there's no manuscript to check rules against.";

/// Build a generative request (#257): draft a starter setting bible from a
/// concept / premise (no manuscript), into the same schema the extractive pass
/// returns.
pub fn build_concept_request(concept: &str, model: &str) -> Request {
    super::manuscript_request(
        CONCEPT_SYSTEM,
        "Draft the starter setting bible and gazetteer as specified.",
        format!("Concept / premise:\n{concept}"),
        model,
        schema(),
    )
}

const REVISE_SYSTEM: &str = "You are a worldbuilding editor maintaining an \
existing setting bible. You are given the current bible and only the chapters \
that have changed since it was built; the bible also records the world \
established by the rest of the work — earlier chapters, and in a multi-book \
series the earlier books — that is NOT shown to you. Revise it: update the \
categories, entries, and gazetteer the given chapters affect, add anything newly \
established, and drop what they contradict. Preserve everything the given \
chapters don't touch exactly as the bible records it. Then check the given \
chapters against the whole recorded world for genuine rule violations, INCLUDING \
violations of rules the bible records from material not shown. Be faithful to the \
text; report only genuine violations, each with the chapters involved. Reference \
chapters by title. Return the complete revised setting bible, gazetteer, and \
findings.";

/// Build an incremental revision request (#90): revise the existing setting bible
/// from only the changed chapters.
pub fn build_incremental_request(
    prior_bible: &str,
    changed_chapters: &str,
    model: &str,
) -> Request {
    incremental::revise_request(
        REVISE_SYSTEM,
        schema(),
        MAX_OUTPUT_TOKENS,
        prior_bible,
        changed_chapters,
        model,
    )
}

/// Parse a structured setting reply.
pub fn parse(text: &str) -> Result<Setting, AiError> {
    super::parse_json(text, "setting")
}

/// Render the setting bible file via [`bible::render`]: a section per category, a
/// gazetteer section, and the rule findings.
pub fn render(s: &Setting, notes: &str, today: &str) -> String {
    let mut body = String::new();

    for cat in &s.categories {
        let _ = writeln!(body, "## {}", cat.name);
        for e in &cat.entries {
            let _ = writeln!(body, "- **{}** — {}", e.name, e.detail);
        }
        body.push('\n');
    }

    if !s.gazetteer.is_empty() {
        body.push_str("## Gazetteer\n");
        for term in &s.gazetteer {
            let _ = writeln!(body, "- {term}");
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
            let _ = writeln!(body, "- **{}:** {}{}", f.rule, f.violation, where_);
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
    fn concept_request_carries_the_concept_not_a_manuscript() {
        let req = build_concept_request("a city built inside a dormant volcano", "claude-opus-4-8");
        assert!(req.cached_context.contains("dormant volcano"));
        assert!(
            req.system.as_deref().unwrap().contains("INVENT"),
            "asks to invent, not extract"
        );
        assert!(
            req.json_schema.is_some(),
            "same schema as the extractive pass"
        );
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
        assert!(matches!(parse("nope"), Err(AiError::ModelNonCompliance(_))));
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
