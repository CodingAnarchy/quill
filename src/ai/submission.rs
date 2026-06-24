//! Submission material: a synopsis, back-cover blurb, and query-letter draft
//! generated from the finished manuscript. This is the **only** AI feature that
//! produces prose, and it is explicitly **off-manuscript** — submission/marketing
//! artifacts written to `submission/`, never into the novel — so it stays within
//! the assistant-not-co-writer rule. See `docs/ai-features.md`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{AiError, Request};

/// The three submission documents.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Submission {
    pub synopsis: String,
    pub blurb: String,
    pub query: String,
}

impl Submission {
    /// The artifacts as `(title, filename, text)`, for writing to `submission/`.
    pub fn artifacts(&self) -> [(&'static str, &'static str, &str); 3] {
        [
            ("Synopsis", "synopsis.md", &self.synopsis),
            ("Back-cover blurb", "blurb.md", &self.blurb),
            ("Query letter", "query.md", &self.query),
        ]
    }
}

const SYSTEM: &str = "You are a publishing assistant. From the manuscript, write \
three OFF-MANUSCRIPT submission documents: (1) a synopsis — about one page, \
covering the full plot including the ending; (2) a back-cover blurb — ~150 words, \
hooky, no spoilers; (3) a query-letter draft — a hook paragraph, a short pitch, \
and a placeholder for the author's bio. These are submission and marketing \
materials, never part of the novel. Return each as plain prose.";

/// The JSON Schema for a submission reply.
pub fn schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "synopsis": { "type": "string" },
            "blurb": { "type": "string" },
            "query": { "type": "string" }
        },
        "required": ["synopsis", "blurb", "query"],
        "additionalProperties": false
    })
}

/// Build the submission request: the manuscript as cached context, schema attached.
pub fn build_request(manuscript: String, model: &str) -> Request {
    super::manuscript_request(SYSTEM, "Write the synopsis, blurb, and query letter as specified.", manuscript, model, schema())
}

/// Parse a structured submission reply.
pub fn parse(text: &str) -> Result<Submission, AiError> {
    super::parse_json(text, "submission")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_the_three_documents() {
        let json = r#"{"synopsis":"S","blurb":"B","query":"Q"}"#;
        let s = parse(json).unwrap();
        assert_eq!(s.synopsis, "S");
        assert_eq!(s.blurb, "B");
        assert_eq!(s.query, "Q");
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::ModelNonCompliance(_))));
    }

    #[test]
    fn artifacts_map_to_filenames() {
        let s = Submission { synopsis: "S".into(), blurb: "B".into(), query: "Q".into() };
        let a = s.artifacts();
        assert_eq!(a[0].1, "synopsis.md");
        assert_eq!(a[1].1, "blurb.md");
        assert_eq!(a[2].1, "query.md");
        assert_eq!(a[0].2, "S");
    }
}
