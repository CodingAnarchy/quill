//! Editorial read (#293): a manuscript pass that *points out* line-level issues
//! for the author to consider — grammar, usage, agreement, tense slips, typos,
//! and genuinely awkward constructions — and never suggests the fix.
//!
//! Structural, continuity, and pacing observations belong to the dedicated
//! passes. This is the *Analysis* category of the assistant-not-co-writer rule
//! (see CLAUDE.md): it flags problems the author acts on, like an editor's
//! margin notes, but it must never offer a rewrite, a corrected sentence, or any
//! prose the author could adopt. Findings flow into the revision-triage list.

use std::fmt::{self, Display};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{AiError, Request};

/// The closed set of editorial lenses the prompt asks for. Only [`Line`](Self::Line)
/// is in contract; structural and continuity work belongs to those dedicated
/// passes. An out-of-contract value degrades to [`Unknown`](Self::Unknown) and is
/// dropped on parse rather than surfaced as a finding.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum EditorialFocus {
    Line,
    /// Any value outside the prompt's closed set (including the retired
    /// `structural` / `continuity` labels). Renders as `"unknown"`.
    #[default]
    #[serde(other)]
    Unknown,
}

impl Display for EditorialFocus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Line => "line",
            Self::Unknown => "unknown",
        })
    }
}

/// One editorial flag: the line-level concern, the chapter(s) it sits in, and a
/// verbatim span so the author can find it.
#[derive(Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Note {
    /// Always [`EditorialFocus::Line`] after [`parse`](parse); other lenses are
    /// dropped rather than surfaced.
    pub focus: EditorialFocus,
    /// The chapter(s) involved, by title.
    #[serde(default)]
    pub chapters: Vec<String>,
    /// A short verbatim quote of the spot, to find it by.
    #[serde(default)]
    pub location: Option<String>,
    /// The issue, in the author's terms — the observation, never a fix.
    pub concern: String,
}

const SYSTEM: &str = "You are an editor giving a manuscript a LINE-LEVEL read. \
Surface grammar, usage, agreement, tense slips, typos, and genuinely awkward \
constructions for the author to consider.\n\
Do NOT report structural issues (pacing, stakes, a sagging middle, a scene \
without a purpose, a setup with no payoff) — those belong to other passes.\n\
Do NOT report continuity contradictions (a fact, object, place, timeline, or \
character trait that conflicts between chapters) — those belong to the \
continuity pass.\n\
For each issue set `focus` to \"line\", name the chapter(s) it concerns by \
title in `chapters`, put a short VERBATIM quote of the span in `location` so \
the author can find it, and give a plain, specific `concern` in the author's \
terms.\n\
CRITICAL — you only POINT OUT issues, you never fix them: do NOT suggest a \
rewrite, a corrected sentence, replacement wording, or any prose the author \
could adopt. Name the problem and where it is; the author decides what to do. \
Report only genuine issues, not stylistic preferences dressed up as errors. \
Reference chapters by title.";

/// The JSON schema for an editorial reply.
pub fn schema() -> Value {
    let note = super::object_schema(
        vec![
            ("focus", json!({ "type": "string", "enum": ["line"] })),
            ("chapters", super::string_array()),
            ("location", super::string_field()),
            ("concern", super::string_field()),
        ],
        &["focus", "concern"],
    );
    super::object_schema(vec![("notes", super::array_of(note))], &["notes"])
}

/// Build the editorial request: the manuscript as cached context, schema attached.
pub fn build_request(manuscript: String, model: &str) -> Request {
    super::manuscript_request(
        SYSTEM,
        "Give the manuscript a line-level editorial read and report the issues \
         as specified — flags only, never a fix.",
        manuscript,
        model,
        schema(),
    )
}

/// Parse a structured editorial reply, keeping only line-level notes. A mixed
/// fixture that also reports structural or continuity items drops those rather
/// than surfacing them as editorial findings.
pub fn parse(text: &str) -> Result<Vec<Note>, AiError> {
    #[derive(Deserialize)]
    struct Reply {
        notes: Vec<Note>,
    }
    let reply: Reply = super::parse_json(text, "editorial")?;
    Ok(reply
        .notes
        .into_iter()
        .filter(|n| n.focus == EditorialFocus::Line)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_keeps_only_line_notes() {
        let json = r#"{"notes":[
            {"focus":"structural","chapters":["Five"],"concern":"the pacing sags through the middle"},
            {"focus":"continuity","chapters":["Three","Six"],"concern":"the locket is lost in 3 but used in 6"},
            {"focus":"line","chapters":["Two"],"location":"she could of known","concern":"usage error (could of → could have)"}
        ]}"#;
        let notes = parse(json).unwrap();
        assert_eq!(notes.len(), 1);
        assert_eq!(notes[0].focus, EditorialFocus::Line);
        assert_eq!(notes[0].chapters, ["Two"]);
        assert_eq!(notes[0].location.as_deref(), Some("she could of known"));
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::ModelNonCompliance(_))));
    }

    #[test]
    fn request_attaches_the_manuscript_and_schema() {
        let req = build_request("the manuscript".into(), "claude-opus-4-8");
        assert_eq!(req.cached_context, "the manuscript");
        assert!(req.json_schema.is_some());
        let s = schema();
        assert_eq!(
            s["properties"]["notes"]["items"]["properties"]["focus"]["enum"],
            serde_json::json!(["line"])
        );
        assert!(SYSTEM.contains("LINE-LEVEL"));
        assert!(!SYSTEM.contains("three lenses"));
    }
}
