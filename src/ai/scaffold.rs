//! Scaffolding: a structured pass that produces a **beat outline** of the
//! manuscript (scene beats as short notes — never prose) written to
//! `bible/outline.md`, and surfaces structural **gaps** (a chapter lacking a
//! clear goal, a missing transition, an unresolved thread) as findings the
//! author can act on. Per Quill's assistant-not-co-writer rule, this never
//! drafts or suggests prose — only structure and notes. See `docs/ai-features.md`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{bible, AiError, Request};

/// A structural pass: the beat outline plus the gaps found in it.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Scaffold {
    #[serde(default)]
    pub outline: Vec<OutlineItem>,
    #[serde(default)]
    pub gaps: Vec<Gap>,
}

/// One chapter's beats — short scene notes, not prose.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct OutlineItem {
    pub chapter: String,
    #[serde(default)]
    pub beats: Vec<String>,
}

/// A structural gap (a hole the author should fill), tied to a chapter.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Gap {
    pub detail: String,
    pub chapter: String,
}

const SYSTEM: &str = "You are a structural editor producing a beat outline. Read \
the novel manuscript and (1) for each chapter, list its key story beats as short \
notes (a beat is a one-line note like \"Mara discovers the locket\" — NOT prose); \
and (2) list structural gaps: a chapter lacking a clear goal or stakes, a missing \
transition, a setup with no payoff, an unresolved thread — each tied to the \
chapter it concerns. Do NOT write, draft, rewrite, or suggest any prose; produce \
only structural notes. Reference chapters by title.";

/// The JSON Schema for a scaffolding reply.
pub fn schema() -> Value {
    let outline_item = super::object_schema(
        vec![
            ("chapter", super::string_field()),
            ("beats", super::string_array()),
        ],
        &["chapter", "beats"],
    );
    let gap = super::object_schema(
        vec![
            ("detail", super::string_field()),
            ("chapter", super::string_field()),
        ],
        &["detail", "chapter"],
    );
    super::object_schema(
        vec![
            ("outline", super::array_of(outline_item)),
            ("gaps", super::array_of(gap)),
        ],
        &["outline", "gaps"],
    )
}

/// Build the scaffolding request: the manuscript as cached context, schema attached.
pub fn build_request(manuscript: String, model: &str) -> Request {
    super::manuscript_request(
        SYSTEM,
        "Produce the beat outline and the structural gaps as specified.",
        manuscript,
        model,
        schema(),
    )
}

/// Parse a structured scaffolding reply.
pub fn parse(text: &str) -> Result<Scaffold, AiError> {
    super::parse_json(text, "scaffold")
}

/// Render the outline bible file via [`bible::render`]: a section of beats per
/// chapter, then a gaps section.
pub fn render_outline(s: &Scaffold, notes: &str, today: &str) -> String {
    let mut body = String::new();

    for item in &s.outline {
        body.push_str(&format!("## {}\n", item.chapter));
        for beat in &item.beats {
            body.push_str(&format!("- {beat}\n"));
        }
        body.push('\n');
    }

    if !s.gaps.is_empty() {
        body.push_str("## Gaps\n");
        for g in &s.gaps {
            body.push_str(&format!("- **{}:** {}\n", g.chapter, g.detail));
        }
        body.push('\n');
    }

    bible::render("title: Outline\n", &body, notes, today)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Scaffold {
        Scaffold {
            outline: vec![OutlineItem {
                chapter: "One".into(),
                beats: vec!["Mara finds the locket".into(), "Tomas warns her".into()],
            }],
            gaps: vec![Gap {
                detail: "no clear goal for the chapter".into(),
                chapter: "Two".into(),
            }],
        }
    }

    #[test]
    fn parse_reads_outline_and_gaps() {
        let json = r#"{"outline":[{"chapter":"One","beats":["x"]}],"gaps":[{"detail":"y","chapter":"Two"}]}"#;
        let s = parse(json).unwrap();
        assert_eq!(s.outline.len(), 1);
        assert_eq!(s.outline[0].beats, vec!["x"]);
        assert_eq!(s.gaps[0].chapter, "Two");
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::ModelNonCompliance(_))));
    }

    #[test]
    fn render_outline_emits_beats_and_gaps() {
        let file = render_outline(&sample(), "## Notes\n", "2026-06-19");
        assert!(file.contains("title: Outline"));
        assert!(file.contains("## One"));
        assert!(file.contains("- Mara finds the locket"));
        assert!(file.contains("## Gaps"));
        assert!(file.contains("no clear goal"));
    }
}
