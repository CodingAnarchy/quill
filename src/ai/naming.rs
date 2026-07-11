//! Naming & term consistency: a cheap, high-volume pass that scans the
//! manuscript for spelling variants of the same proper noun or invented term
//! ("Mara"/"Marah", "lightsteel"/"light-steel"). It's checked against the known
//! entity list (the setting gazetteer + character names) when available, and
//! reported as reviewable findings — never written to the prose. Runs on a
//! Haiku-tier model by design. See `docs/ai-features.md`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::AiError;
use super::Request;

/// The model for this pass — cheap and high-volume by design.
pub const MODEL: &str = "claude-haiku-4-5";
/// Output bound for this pass (findings are short; cheaper than the default).
pub const MAX_TOKENS: u32 = 4096;

/// The result of a naming pass: one finding per inconsistent name/term cluster.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Naming {
    #[serde(default)]
    pub findings: Vec<NameFinding>,
}

/// One name/term spelled inconsistently across the manuscript.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct NameFinding {
    /// The most likely intended spelling.
    pub name: String,
    /// The other spellings found.
    #[serde(default)]
    pub variants: Vec<String>,
    /// Chapters where the variants appear.
    #[serde(default)]
    pub chapters: Vec<String>,
}

const SYSTEM: &str = "You are a copy editor checking name and term consistency. \
Scan the manuscript for spelling variants of the same proper noun or invented \
term — e.g. \"Mara\"/\"Marah\", \"lightsteel\"/\"light-steel\". For each genuine \
inconsistency, report the most likely intended spelling, the variant spellings \
found, and the chapters (by title) where the variants appear. Report only true \
inconsistencies of one name/term — never two distinct names. Do not rewrite or \
suggest prose.";

/// The JSON Schema for a naming reply.
pub fn schema() -> Value {
    let finding = super::object_schema(
        vec![
            ("name", super::string_field()),
            ("variants", super::string_array()),
            ("chapters", super::string_array()),
        ],
        &["name", "variants", "chapters"],
    );
    super::object_schema(vec![("findings", super::array_of(finding))], &["findings"])
}

/// Build the naming request: the manuscript as cached context, the known entity
/// list (if any) as focus, schema attached. Always runs on the Haiku-tier model.
pub fn build_request(manuscript: String, entities: &[String]) -> Request {
    let user = if entities.is_empty() {
        "Find name/term spelling inconsistencies as specified.".to_string()
    } else {
        format!(
            "Known names and terms to check first (also catch any others):\n{}\n\n\
             Find name/term spelling inconsistencies as specified.",
            entities.join(", ")
        )
    };
    Request {
        system: Some(SYSTEM.to_string()),
        cached_context: manuscript,
        user,
        model: MODEL.to_string(),
        max_tokens: MAX_TOKENS,
        json_schema: Some(schema()),
    }
}

/// Parse a structured naming reply.
pub fn parse(text: &str) -> Result<Naming, AiError> {
    super::parse_json(text, "naming")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_findings() {
        let json = r#"{"findings":[{"name":"Mara","variants":["Marah"],"chapters":["Two"]}]}"#;
        let n = parse(json).unwrap();
        assert_eq!(n.findings.len(), 1);
        assert_eq!(n.findings[0].name, "Mara");
        assert_eq!(n.findings[0].variants, vec!["Marah"]);
        // The chapters (the jump target for triage) survive the parse too.
        assert_eq!(n.findings[0].chapters, ["Two"]);
    }

    // A clean manuscript — every name spelled consistently — is an empty findings
    // list, not an error (matching the foreshadow/factcheck passes).
    #[test]
    fn parse_reads_empty_findings() {
        let n = parse(r#"{"findings":[]}"#).unwrap();
        assert!(n.findings.is_empty());
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::ModelNonCompliance(_))));
    }

    #[test]
    fn request_uses_haiku_and_lists_known_entities() {
        let req = build_request(
            "the manuscript".into(),
            &["Mara".into(), "Aldermoor".into()],
        );
        assert_eq!(req.model, "claude-haiku-4-5");
        assert!(req.user.contains("Mara, Aldermoor"));
        assert!(req.json_schema.is_some());
    }

    #[test]
    fn request_without_entities_omits_the_list() {
        let req = build_request("m".into(), &[]);
        assert!(!req.user.contains("Known names"));
    }
}
