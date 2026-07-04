//! Theme / motif tracker (#828): a fifth persistent, incrementally-updated bible
//! pass alongside characters (#74), timeline (#75), setting (#77), and plot
//! (#606). It reads the manuscript and extracts its **motifs** — recurring
//! images, symbols, and thematic figures — as structured entries: each with a
//! short name, a status, the meaning it carries, where it recurs (the chapters),
//! and free notes, written to `bible/theme.md`; and it flags **dangling** motifs
//! (an image or symbol set up but never paid off, a motif that drops out
//! mid-book) as triageable findings. Like the other trackers it reuses the shared
//! [`bible`] format and the incremental-scoping machinery (#90); it tracks and
//! flags only, never writing or suggesting manuscript prose. See
//! `docs/ai-features.md`.

use std::fmt::Write;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{bible, incremental, AiError, Request, MAX_OUTPUT_TOKENS};

/// The result of a theme pass: the tracked motifs and the dangling-motif findings.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Theme {
    #[serde(default)]
    pub motifs: Vec<Motif>,
    #[serde(default)]
    pub findings: Vec<MotifFinding>,
}

/// One motif / image / symbol: a recurring figure tracked from where it's
/// introduced through where it recurs. Structured tracking only — a note of where
/// the motif stands and what it carries, never prose the author should write.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Motif {
    /// A short name for the motif, e.g. "Mirrors" or "The recurring storm".
    pub name: String,
    /// Where the motif stands: e.g. "recurring", "established", "faded", "resolved".
    pub status: String,
    /// The meaning / theme the motif carries, e.g. "self-deception".
    #[serde(default)]
    pub meaning: String,
    /// The chapters (titles) the motif appears in.
    #[serde(default)]
    pub appearances: Vec<String>,
    /// Any extra structured notes on the motif.
    #[serde(default)]
    pub notes: String,
}

/// A dangling-motif finding: an image or symbol set up but never developed or paid
/// off, or a motif that drops out of the book partway through.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct MotifFinding {
    /// The motif involved, e.g. "The recurring storm".
    pub motif: String,
    /// What's dangling, e.g. "introduced in ch. 2, never returns".
    pub issue: String,
    /// Chapters involved (titles).
    #[serde(default)]
    pub chapters: Vec<String>,
}

const SYSTEM: &str = "You are a developmental editor tracking a novel's motifs and \
themes. Read the manuscript and (1) identify the distinct motifs — recurring \
images, symbols, and thematic figures — and for each give a short name, a status \
(one of: seeded, established, recurring, faded, resolved), the meaning or theme it \
carries, the chapter titles it appears in, and any concise notes; and (2) flag \
DANGLING motifs — an image or symbol set up but never developed or paid off, a \
motif that appears once and never returns, a thematic figure that drops out \
mid-book — each with the chapters involved. Be faithful to the text; report only \
genuine motifs and genuine dangling issues, not incidental description. Reference \
chapters by title. Track and flag only — never write or suggest prose.";

/// The JSON Schema for a theme reply.
pub fn schema() -> Value {
    let motif = super::object_schema(
        vec![
            ("name", super::string_field()),
            ("status", super::string_field()),
            ("meaning", super::string_field()),
            ("appearances", super::string_array()),
            ("notes", super::string_field()),
        ],
        &["name", "status", "meaning", "appearances", "notes"],
    );
    super::object_schema(
        vec![
            ("motifs", super::array_of(motif)),
            (
                "findings",
                super::array_of(super::chapters_finding("motif", "issue")),
            ),
        ],
        &["motifs", "findings"],
    )
}

/// Build the theme request: the manuscript as cached context, schema attached.
pub fn build_request(manuscript: String, model: &str) -> Request {
    super::manuscript_request(
        SYSTEM,
        "Extract the motifs and dangling-motif findings as specified.",
        manuscript,
        model,
        schema(),
    )
}

const REVISE_SYSTEM: &str = "You are a developmental editor maintaining an existing \
theme / motif tracker. You are given the current tracker and only the chapters \
that have changed since it was built; the tracker also records motifs from the \
rest of the work — earlier chapters, and in a multi-book series the earlier books \
— that are NOT shown to you. Revise it: update the status, meaning, and \
appearances of the motifs the given chapters affect, add motifs they introduce, \
and drop what they contradict — keeping the motifs you can't see intact. Then \
check the whole set of motifs against the given chapters for genuine DANGLING \
motifs (an image or symbol set up but never developed or paid off, a motif that \
drops out mid-book), INCLUDING motifs the tracker records from material not shown. \
Be faithful to the text; report only genuine dangling issues, each with the \
chapters involved. Reference chapters by title. Track and flag only — never write \
prose. Return the complete revised theme tracker and findings.";

/// Build an incremental revision request (#90): revise the existing theme tracker
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

/// Parse a structured theme reply.
pub fn parse(text: &str) -> Result<Theme, AiError> {
    super::parse_json(text, "theme")
}

/// Render the theme / motif bible file via [`bible::render`]: one section per motif
/// with its status/meaning/appearances/notes, then the dangling-motif findings.
pub fn render(t: &Theme, notes: &str, today: &str) -> String {
    let mut body = String::new();

    body.push_str("## Motifs\n");
    for m in &t.motifs {
        let _ = writeln!(body, "### {} — {}", m.name, m.status);
        if !m.meaning.is_empty() {
            let _ = writeln!(body, "- **Meaning:** {}", m.meaning);
        }
        if !m.appearances.is_empty() {
            let _ = writeln!(body, "- **Appears in:** {}", m.appearances.join(", "));
        }
        if !m.notes.is_empty() {
            let _ = writeln!(body, "- **Notes:** {}", m.notes);
        }
        body.push('\n');
    }

    if !t.findings.is_empty() {
        body.push_str("## Dangling motifs\n");
        for f in &t.findings {
            let where_ = if f.chapters.is_empty() {
                String::new()
            } else {
                format!(" ({})", f.chapters.join(", "))
            };
            let _ = writeln!(body, "- **{}:** {}{}", f.motif, f.issue, where_);
        }
        body.push('\n');
    }

    bible::render("title: Themes & motifs\n", &body, notes, today)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Theme {
        Theme {
            motifs: vec![
                Motif {
                    name: "Mirrors".into(),
                    status: "recurring".into(),
                    meaning: "self-deception".into(),
                    appearances: vec!["The Reckoning".into(), "Ashes".into()],
                    notes: "pairs with the locket".into(),
                },
                Motif {
                    name: "The recurring storm".into(),
                    status: "faded".into(),
                    meaning: "coming upheaval".into(),
                    appearances: vec!["Ashes".into()],
                    notes: String::new(),
                },
            ],
            findings: vec![MotifFinding {
                motif: "The recurring storm".into(),
                issue: "introduced in ch. 3, never returns".into(),
                chapters: vec!["Ashes".into()],
            }],
        }
    }

    #[test]
    fn schema_requires_motifs_and_findings() {
        let s = schema();
        assert_eq!(s["required"], serde_json::json!(["motifs", "findings"]));
        let motif = &s["properties"]["motifs"]["items"];
        assert_eq!(
            motif["required"],
            serde_json::json!(["name", "status", "meaning", "appearances", "notes"])
        );
        // Dangling-motif findings reuse the shared two-strings-plus-chapters shape.
        let finding = &s["properties"]["findings"]["items"];
        assert_eq!(
            finding["required"],
            serde_json::json!(["motif", "issue", "chapters"])
        );
    }

    #[test]
    fn parse_reads_motifs_and_findings() {
        let json = r#"{"motifs":[{"name":"Mirrors","status":"recurring","meaning":"m","appearances":["One"],"notes":""}],"findings":[]}"#;
        let t = parse(json).unwrap();
        assert_eq!(t.motifs.len(), 1);
        assert_eq!(t.motifs[0].name, "Mirrors");
        assert_eq!(t.motifs[0].status, "recurring");
        assert_eq!(t.motifs[0].appearances, vec!["One".to_string()]);
        assert!(t.findings.is_empty());
    }

    #[test]
    fn parse_defaults_optional_fields() {
        // The optional fields tolerate omission (a lenient mock reply).
        let json = r#"{"motifs":[{"name":"Mirrors","status":"recurring"}],"findings":[]}"#;
        let t = parse(json).unwrap();
        assert_eq!(t.motifs[0].meaning, "");
        assert!(t.motifs[0].appearances.is_empty());
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::ModelNonCompliance(_))));
    }

    #[test]
    fn incremental_request_carries_prior_and_changed() {
        let req = build_incremental_request("PRIOR BIBLE", "CHANGED CH", "claude-opus-4-8");
        assert!(req.cached_context.contains("PRIOR BIBLE"));
        assert!(req.cached_context.contains("CHANGED CH"));
        assert!(req.system.as_deref().unwrap().contains("dangling"));
        assert!(req.json_schema.is_some(), "same schema as the full pass");
    }

    #[test]
    fn render_emits_motifs_and_dangling_findings() {
        let file = render(&sample(), "## Notes\n", "2026-06-19");
        assert!(file.contains("title: Themes & motifs"));
        assert!(file.contains("## Motifs"));
        assert!(file.contains("### Mirrors — recurring"));
        assert!(file.contains("**Meaning:** self-deception"));
        assert!(file.contains("**Appears in:** The Reckoning, Ashes"));
        assert!(file.contains("**Notes:** pairs with the locket"));
        assert!(file.contains("## Dangling motifs"));
        assert!(file.contains("introduced in ch. 3, never returns"));
        // A motif with no notes omits the Notes line rather than blanking it.
        let storm = file.split("The recurring storm").nth(1).unwrap();
        let next = storm.find("## Dangling").unwrap();
        assert!(!storm[..next].contains("**Notes:**"));
    }

    #[test]
    fn render_round_trips_through_the_bible_format() {
        let file = render(&sample(), "## Notes\n\nKeep me.\n", "2026-06-19");
        assert_eq!(bible::extract_notes(&file).trim(), "## Notes\n\nKeep me.");
        assert!(!bible::is_hand_edited(&file), "freshly rendered is clean");
    }
}
