//! Prose & style analysis: the *semantic* checks that need understanding rather
//! than counting — POV consistency, tense consistency, per-POV voice drift, and
//! context-aware repeated phrasing. Pure analysis surfaced as findings; per
//! Quill's assistant-not-co-writer rule it never rewrites or suggests prose, and
//! it leaves *frequency* metrics (crutch words, etc.) to the local-analysis
//! style report (#70). See `docs/ai-features.md`.

use std::fmt::{self, Display};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AiError, Request};

/// The closed set of prose-style categories the prompt asks for (#1082):
/// POV / tense / voice / repetition. Deserialized straight from the model reply;
/// an out-of-contract value degrades to [`Unknown`](Self::Unknown) rather than a
/// silent raw string, and [`Display`] reproduces the exact label the model emits
/// (note the model's `POV` is upper-case) so a rendered finding reads identically.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum StyleKind {
    #[serde(rename = "POV")]
    Pov,
    #[serde(rename = "tense")]
    Tense,
    #[serde(rename = "voice")]
    Voice,
    #[serde(rename = "repetition")]
    Repetition,
    /// Any value outside the prompt's closed set. Renders as `"unknown"`.
    #[default]
    #[serde(other)]
    Unknown,
}

impl Display for StyleKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Pov => "POV",
            Self::Tense => "tense",
            Self::Voice => "voice",
            Self::Repetition => "repetition",
            Self::Unknown => "unknown",
        })
    }
}

/// The result of a prose/style pass: the style observations.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Style {
    #[serde(default)]
    pub findings: Vec<StyleFinding>,
}

/// One style observation.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct StyleFinding {
    /// A short category: POV / tense / voice / repetition (#1082). A value outside
    /// that set deserializes to [`StyleKind::Unknown`].
    pub kind: StyleKind,
    pub detail: String,
    /// Chapters involved (titles).
    #[serde(default)]
    pub chapters: Vec<String>,
}

const SYSTEM: &str = "You are a line editor doing prose-style ANALYSIS — \
observations only, never rewrites or suggestions. Read the manuscript and flag: \
POV consistency (head-hopping, or a POV character knowing something they could \
not); tense consistency within a scene; per-POV voice drift (a character's \
narration or dialogue voice changing unintentionally between scenes); and \
context-aware repeated phrasing (the same distinctive phrase or image reused so \
it stands out). Report each as a finding with a short kind (POV / tense / voice / \
repetition), the detail, and the chapters involved. Do NOT tally word \
frequencies — that is a separate report. Do NOT rewrite or suggest prose; surface \
the issue for the author to address.";

/// The JSON Schema for a prose/style reply.
pub fn schema() -> Value {
    super::object_schema(
        vec![(
            "findings",
            super::array_of(super::chapters_finding("kind", "detail")),
        )],
        &["findings"],
    )
}

/// Build the prose/style request: the manuscript as cached context, schema attached.
pub fn build_request(manuscript: String, model: &str) -> Request {
    super::manuscript_request(
        SYSTEM,
        "Report the prose/style findings as specified.",
        manuscript,
        model,
        schema(),
    )
}

/// Parse a structured prose/style reply.
pub fn parse(text: &str) -> Result<Style, AiError> {
    super::parse_json(text, "style")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_findings() {
        let json = r#"{"findings":[{"kind":"POV","detail":"head-hop","chapters":["Two"]}]}"#;
        let s = parse(json).unwrap();
        assert_eq!(s.findings.len(), 1);
        assert_eq!(s.findings[0].kind, StyleKind::Pov);
        // Every field survives the parse, not just kind — detail and chapters too.
        assert_eq!(s.findings[0].detail, "head-hop");
        assert_eq!(s.findings[0].chapters, ["Two"]);
    }

    // A clean manuscript is an empty findings list, not an error — the model
    // returned a valid, schema-conforming reply with nothing to flag (matching the
    // foreshadow/factcheck passes).
    #[test]
    fn parse_reads_empty_findings() {
        let s = parse(r#"{"findings":[]}"#).unwrap();
        assert!(s.findings.is_empty());
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::ModelNonCompliance(_))));
    }

    #[test]
    fn kind_round_trips_valid_values_to_the_same_label() {
        // Each valid category deserializes to its variant and renders back to the
        // exact string the model emitted (POV stays upper-case) — output byte-identical.
        for (raw, kind) in [
            ("POV", StyleKind::Pov),
            ("tense", StyleKind::Tense),
            ("voice", StyleKind::Voice),
            ("repetition", StyleKind::Repetition),
        ] {
            let json = format!(r#"{{"findings":[{{"kind":"{raw}","detail":"d","chapters":[]}}]}}"#);
            let s = parse(&json).unwrap();
            assert_eq!(s.findings[0].kind, kind);
            assert_eq!(s.findings[0].kind.to_string(), raw);
        }
    }

    #[test]
    fn malformed_kind_degrades_to_unknown_not_a_raw_string() {
        // A model typo doesn't error and doesn't leak the raw string into output.
        let json = r#"{"findings":[{"kind":"tesne","detail":"d","chapters":[]}]}"#;
        let s = parse(json).unwrap();
        assert_eq!(s.findings[0].kind, StyleKind::Unknown);
        assert_eq!(s.findings[0].kind.to_string(), "unknown");
    }

    #[test]
    fn request_carries_schema_and_model() {
        let req = build_request("m".into(), "claude-opus-4-8");
        assert_eq!(req.model, "claude-opus-4-8");
        assert!(req.json_schema.is_some());
    }
}
