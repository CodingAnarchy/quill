//! Editorial read (#293): a manuscript pass that *points out* issues for the
//! author to consider — across three lenses, **structural**, **continuity**, and
//! **line-editing** — and never suggests the fix.
//!
//! This is the *Analysis* category of the assistant-not-co-writer rule (see
//! CLAUDE.md): it flags problems the author acts on, like an editor's margin
//! notes, but it must never offer a rewrite, a corrected sentence, or any prose
//! the author could adopt. Findings flow into the revision-triage list.

use serde::Deserialize;
use serde_json::{json, Value};

use super::{AiError, Request};

/// One editorial flag: which lens it came from, the chapter(s) it concerns, an
/// optional verbatim span to pinpoint it, and the plain-language concern.
#[derive(Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Note {
    /// The lens: `"structural"`, `"continuity"`, or `"line"`.
    pub focus: String,
    /// The chapter(s) involved, by title.
    #[serde(default)]
    pub chapters: Vec<String>,
    /// A short verbatim quote of the spot (line-level issues), to find it by.
    #[serde(default)]
    pub location: Option<String>,
    /// The issue, in the author's terms — the observation, never a fix.
    pub concern: String,
}

const SYSTEM: &str = "You are an editor giving a manuscript a read. Surface issues \
for the author to consider, across three lenses:\n\
- structural: pacing that drags or rushes, weak or absent stakes, a sagging \
middle, a scene or chapter without a clear purpose, a setup with no payoff, an \
unresolved thread;\n\
- continuity: contradictions across the draft — a fact, object, place, timeline, \
or character trait that conflicts between chapters;\n\
- line: grammar, usage, agreement, tense slips, typos, and genuinely awkward \
constructions.\n\
For each issue give its lens in `focus` (\"structural\", \"continuity\", or \
\"line\"), the chapter(s) it concerns by title in `chapters`, and a plain, \
specific `concern` in the author's terms. For a line-level issue, also put a \
short VERBATIM quote of the span in `location` so the author can find it.\n\
CRITICAL — you only POINT OUT issues, you never fix them: do NOT suggest a \
rewrite, a corrected sentence, replacement wording, or any prose the author \
could adopt. Name the problem and where it is; the author decides what to do. \
Report only genuine issues, not stylistic preferences dressed up as errors. \
Reference chapters by title.";

/// The JSON schema for an editorial reply.
pub fn schema() -> Value {
    let note = super::object_schema(
        vec![
            (
                "focus",
                json!({ "type": "string", "enum": ["structural", "continuity", "line"] }),
            ),
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
    super::manuscript_request(SYSTEM, "Give the manuscript an editorial read and report the issues as specified — flags only, never a fix.", manuscript, model, schema())
}

/// Parse a structured editorial reply.
pub fn parse(text: &str) -> Result<Vec<Note>, AiError> {
    #[derive(Deserialize)]
    struct Reply {
        notes: Vec<Note>,
    }
    let reply: Reply = super::parse_json(text, "editorial")?;
    Ok(reply.notes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_each_lens() {
        let json = r#"{"notes":[
            {"focus":"structural","chapters":["Five"],"concern":"the pacing sags through the middle"},
            {"focus":"continuity","chapters":["Three","Six"],"concern":"the locket is lost in 3 but used in 6"},
            {"focus":"line","chapters":["Two"],"location":"she could of known","concern":"usage error (could of → could have)"}
        ]}"#;
        let notes = parse(json).unwrap();
        assert_eq!(notes.len(), 3);
        assert_eq!(notes[0].focus, "structural");
        assert_eq!(notes[1].chapters, ["Three", "Six"]);
        assert_eq!(notes[2].location.as_deref(), Some("she could of known"));
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
        // The constrained lens set is in the schema.
        let s = schema();
        assert_eq!(
            s["properties"]["notes"]["items"]["properties"]["focus"]["enum"][0],
            "structural"
        );
    }
}
