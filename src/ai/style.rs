//! Prose & style analysis: the *semantic* checks that need understanding rather
//! than counting — POV consistency, tense consistency, per-POV voice drift, and
//! context-aware repeated phrasing. Pure analysis surfaced as findings; per
//! Quill's assistant-not-co-writer rule it never rewrites or suggests prose, and
//! it leaves *frequency* metrics (crutch words, etc.) to the local-analysis
//! style report (#70). See `docs/ai-features.md`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AiError, Request};

/// The result of a prose/style pass: the style observations.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Style {
    #[serde(default)]
    pub findings: Vec<StyleFinding>,
}

/// One style observation.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct StyleFinding {
    /// A short category: "POV", "tense", "voice", or "repetition".
    pub kind: String,
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
        assert_eq!(s.findings[0].kind, "POV");
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
    fn request_carries_schema_and_model() {
        let req = build_request("m".into(), "claude-opus-4-8");
        assert_eq!(req.model, "claude-opus-4-8");
        assert!(req.json_schema.is_some());
    }
}
