//! Research-notebook fact-check (#1049): an **author-invoked** pass that checks the
//! manuscript prose against the author's *own* research notebook (`bible/research.md`,
//! #932) and flags passages that **contradict** a note the author wrote — e.g. "per
//! your note 'Feudal Timeline', Lord Grey ruled 1432–1456, but chapter 12 has him
//! ruling in 1470." Each finding cites the relevant note by title and quotes the
//! offending manuscript passage + its chapter.
//!
//! This is the *Analysis* category of the assistant-not-co-writer rule (see
//! CLAUDE.md): it surfaces observations the author acts on, never a rewrite, a
//! corrected sentence, or replacement prose. Findings flow into the shared
//! revision-triage list like every other analysis pass.
//!
//! **Grounded in the author's ledger — not external knowledge.** The pass is seeded
//! with the *content* of the author's research notes and instructed to flag only
//! contradictions of *those notes*. It introduces **no external facts of its own** and
//! makes **no** representation / sensitivity / bias judgment. That is the line that
//! keeps it distinct from the cultural & historical accuracy pass (#973): accuracy
//! checks the manuscript against a named *external* concern the author supplies;
//! fact-check checks it against the author's *own written* notebook. If the notebook
//! is empty, there is nothing to check and the pass has no basis to flag anything.
//! Off by default, author-invoked. See `docs/ai-features.md`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AiError, Request};

/// The result of a fact-check pass: the contradictions of the author's own notes.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct FactCheck {
    #[serde(default)]
    pub findings: Vec<FactCheckFinding>,
}

/// One fact-check observation: a manuscript passage that contradicts a note the
/// author wrote, with the note it contradicts named. Never a fix — the author
/// decides what to change (or whether the note itself is what's out of date).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct FactCheckFinding {
    /// The title of the author's research note this passage contradicts — one of the
    /// `## ` headings from `bible/research.md`, so the author can find the note.
    pub note: String,
    /// A short verbatim quote of the manuscript passage that contradicts the note,
    /// so the author can find it. Never edited prose — a locator only.
    #[serde(default)]
    pub passage: String,
    /// How the passage contradicts the cited note — the observation, never a
    /// rewrite. E.g. "the note dates his reign 1432–1456; here he rules in 1470".
    pub detail: String,
    /// Chapters the contradicting passage appears in (titles); the first is the
    /// jump target.
    #[serde(default)]
    pub chapters: Vec<String>,
}

/// Build the system prompt for a fact-check against the author's research notebook
/// (#1049). The notebook's *content* is interpolated so the model checks the
/// manuscript against exactly those notes — and the prompt draws the grounded-in-
/// the-author's-ledger line hard so the pass can't drift into introducing external
/// facts or into a sensitivity / representation review.
fn system_for(research: &str) -> String {
    format!(
        "You are a continuity-minded copyeditor doing a RESEARCH-NOTEBOOK FACT-CHECK \
that the author explicitly asked for. Below is the author's OWN research notebook — \
the facts THEY recorded for their story (invented or researched). Read the \
manuscript and flag ONLY passages that CONTRADICT one of these notes: a date, name, \
place, relationship, rule, or detail in the prose that disagrees with what a note \
says. For each contradiction, name the `note` it contradicts (by its `## ` heading \
title, verbatim), quote a SHORT verbatim `passage` from the manuscript so the author \
can find it, give a plain `detail` explaining how the passage disagrees with that \
note, and list the `chapters` (by title) it appears in.\n\
=== THE AUTHOR'S RESEARCH NOTEBOOK (the ONLY source of truth for this check) ===\n\
{research}\n\
=== END OF NOTEBOOK ===\n\
CRITICAL BOUNDARIES:\n\
- Check ONLY against the notes above. Do NOT introduce facts of your own, world \
knowledge, or anything not written in the notebook. If a manuscript detail isn't \
covered by a note, it is NOT a finding — you have no basis to judge it. Every \
finding MUST cite a note that exists above.\n\
- You only OBSERVE and FLAG. Do NOT rewrite, correct, suggest replacement wording, \
or offer any prose the author could adopt. Name the contradiction and where it is; \
the author decides what to change (the prose or the note).\n\
- This is a factual-consistency check against the author's ledger. It is NOT a \
sensitivity, representation, bias, or political-correctness review — do NOT make \
representation judgments or raise concerns the notebook doesn't speak to.\n\
- Flag only genuine contradictions of a note, not restatements, elaborations, or \
details a note simply doesn't mention. When in doubt, leave it be."
    )
}

/// The JSON Schema for a fact-check reply — a list of `{note, passage, detail,
/// chapters}` findings. `note` and `detail` are required so every finding names the
/// contradicted note and the contradiction; `passage` and `chapters` locate it.
pub fn schema() -> Value {
    let finding = super::object_schema(
        vec![
            ("note", super::string_field()),
            ("passage", super::string_field()),
            ("detail", super::string_field()),
            ("chapters", super::string_array()),
        ],
        &["note", "detail", "chapters"],
    );
    super::object_schema(vec![("findings", super::array_of(finding))], &["findings"])
}

/// Build the fact-check request (#1049): the manuscript as cached context, the
/// author's research-notebook `research` content baked into the system prompt,
/// schema attached. The manuscript is only *read* to flag against the notes — never
/// edited — and the notes are the sole source the model may check against.
pub fn build_request(manuscript: String, research: &str, model: &str) -> Request {
    super::manuscript_request(
        &system_for(research),
        "Report the contradictions of the author's research notes as specified — flags only, never a fix.",
        manuscript,
        model,
        schema(),
    )
}

/// Parse a structured fact-check reply.
pub fn parse(text: &str) -> Result<FactCheck, AiError> {
    super::parse_json(text, "factcheck")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_findings() {
        let json = r#"{"findings":[{"note":"Feudal Timeline","passage":"Lord Grey ruled in 1470","detail":"the note dates his reign 1432-1456","chapters":["Twelve"]}]}"#;
        let fc = parse(json).unwrap();
        assert_eq!(fc.findings.len(), 1);
        assert_eq!(fc.findings[0].note, "Feudal Timeline");
        assert_eq!(fc.findings[0].passage, "Lord Grey ruled in 1470");
        assert!(fc.findings[0].detail.contains("1432-1456"));
        assert_eq!(fc.findings[0].chapters, ["Twelve"]);
    }

    #[test]
    fn parse_reads_empty_findings() {
        let fc = parse(r#"{"findings":[]}"#).unwrap();
        assert!(fc.findings.is_empty());
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::ModelNonCompliance(_))));
    }

    #[test]
    fn request_seeds_the_notebook_manuscript_and_schema() {
        let research = "## Feudal Timeline\n\nLord Grey ruled 1432-1456.\n";
        let req = build_request("the manuscript".into(), research, "claude-opus-4-8");
        assert_eq!(req.model, "claude-opus-4-8");
        assert_eq!(req.cached_context, "the manuscript");
        assert!(req.json_schema.is_some());
        // The author's notebook content is baked into the system prompt so the pass
        // checks against exactly those notes.
        let system = req.system.as_deref().unwrap_or("");
        assert!(system.contains("Feudal Timeline"), "{system}");
        assert!(system.contains("1432-1456"), "{system}");
    }

    #[test]
    fn prompt_grounds_in_the_notebook_and_forbids_external_facts() {
        let req = build_request("m".into(), "## N\n\nnote body\n", "claude-opus-4-8");
        let system = req.system.as_deref().unwrap_or("");
        // Grounded only in the author's ledger: no facts of the model's own.
        assert!(
            system.contains("Do NOT introduce facts of your own"),
            "grounded in the notebook, no external facts: {system}"
        );
        // Distinct from the accuracy pass: explicitly NOT a sensitivity review.
        assert!(
            system.contains("NOT a sensitivity"),
            "not a sensitivity review: {system}"
        );
        // Flags-only: the prompt forbids rewrites/fixes.
        assert!(system.contains("Do NOT rewrite"), "{system}");
    }

    #[test]
    fn schema_requires_note_and_detail_per_finding() {
        let s = schema();
        let item = &s["properties"]["findings"]["items"];
        assert_eq!(
            item["required"],
            serde_json::json!(["note", "detail", "chapters"])
        );
        assert_eq!(
            item["properties"]["note"],
            serde_json::json!({ "type": "string" })
        );
        // No field can carry edited manuscript prose — only observation locators.
        let keys: std::collections::BTreeSet<&str> = item["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["chapters", "detail", "note", "passage"]
                .into_iter()
                .collect()
        );
    }
}
