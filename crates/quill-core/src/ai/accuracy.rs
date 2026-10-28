//! Cultural & historical accuracy analysis (#973): an **author-invoked** pass that
//! checks the manuscript against a concern the author *names* — a period, place, or
//! cultural/technical domain ("1850s London", "Heian-era court protocol",
//! "18th-century sailing") — and flags passages that read as factually, period, or
//! culturally **inaccurate** relative to that named concern. Each finding is the
//! passage plus a short why-it-may-be-off note.
//!
//! This is the *Analysis* category of the assistant-not-co-writer rule (see
//! CLAUDE.md): it surfaces observations the author acts on, never a rewrite, a
//! corrected sentence, or replacement prose. Findings flow into the shared
//! revision-triage list like every other analysis pass.
//!
//! **Accuracy, not sensitivity.** This is a craft tool for getting *named* details
//! right where the author asks — it is explicitly **NOT** a sensitivity /
//! representation / bias / political-correctness review (a contrary aim, out of
//! scope for Quill). It only checks the concern the author supplied; it never makes
//! representation judgments and never imposes a check the author didn't ask for.
//! Off by default, run per concern, on the author's request. See `docs/ai-features.md`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AiError, Request};

/// The result of an accuracy pass: the inaccuracy observations for the named concern.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Accuracy {
    #[serde(default)]
    pub findings: Vec<AccuracyFinding>,
}

/// One accuracy observation: the passage that reads as off and a short note on why
/// it may be inaccurate for the author's named concern. Never a fix — the author
/// decides what to change.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct AccuracyFinding {
    /// A short verbatim quote of the passage that reads as off, so the author can
    /// find it.
    #[serde(default)]
    pub passage: String,
    /// Why the passage may be inaccurate for the named concern — the observation,
    /// never a rewrite.
    pub detail: String,
    /// Chapters involved (titles).
    #[serde(default)]
    pub chapters: Vec<String>,
}

/// Build the system prompt for a concern the author named (#973). The concern is
/// interpolated so the model checks *only* that period / place / domain, and the
/// prompt draws the accuracy-not-sensitivity line hard so the pass can't drift into
/// a representation / bias review.
fn system_for(concern: &str) -> String {
    format!(
        "You are a research-minded copyeditor doing a CULTURAL & HISTORICAL ACCURACY \
check that the author explicitly asked for. The author named this concern to check \
against: \"{concern}\". Read the manuscript and flag passages that read as \
factually, period, or culturally INACCURATE *relative to that named concern* — an \
anachronism (an object, word, technology, institution, or custom out of its time or \
place), a detail that contradicts the named setting, or a cultural/technical detail \
that a knowledgeable reader of \"{concern}\" would find wrong. For each, quote a \
SHORT verbatim `passage` so the author can find it, give a plain `detail` explaining \
why it may be off for the named concern, and list the `chapters` (by title) it \
appears in.\n\
CRITICAL BOUNDARIES:\n\
- You only OBSERVE and FLAG. Do NOT rewrite, correct, suggest replacement wording, \
or offer any prose the author could adopt. Name the issue and where it is; the \
author decides what to do.\n\
- Check ONLY the concern the author named (\"{concern}\"). This is an accuracy craft \
tool, NOT a sensitivity, representation, bias, or political-correctness review — do \
NOT make representation judgments, flag depictions as offensive, or raise concerns \
the author did not ask you to check.\n\
- Flag only genuine likely inaccuracies, not stylistic choices, deliberate \
alternate-history/fantasy conceits, or matters of taste. When in doubt, leave it be."
    )
}

/// The JSON Schema for an accuracy reply — a list of `{passage, detail, chapters}`
/// findings.
pub fn schema() -> Value {
    let finding = super::object_schema(
        vec![
            ("passage", super::string_field()),
            ("detail", super::string_field()),
            ("chapters", super::string_array()),
        ],
        &["detail", "chapters"],
    );
    super::object_schema(vec![("findings", super::array_of(finding))], &["findings"])
}

/// Build the accuracy request: the manuscript as cached context, the author's named
/// `concern` baked into the system prompt, schema attached (#973). The manuscript is
/// only *read* to flag against the named concern — never edited.
pub fn build_request(manuscript: String, concern: &str, model: &str) -> Request {
    super::manuscript_request(
        &system_for(concern),
        "Report the accuracy findings for the named concern as specified — flags only, never a fix.",
        manuscript,
        model,
        schema(),
    )
}

/// Parse a structured accuracy reply.
pub fn parse(text: &str) -> Result<Accuracy, AiError> {
    super::parse_json(text, "accuracy")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_findings() {
        let json = r#"{"findings":[{"passage":"he checked his wristwatch","detail":"wristwatches postdate the 1850s","chapters":["Two"]}]}"#;
        let a = parse(json).unwrap();
        assert_eq!(a.findings.len(), 1);
        assert_eq!(a.findings[0].passage, "he checked his wristwatch");
        assert!(a.findings[0].detail.contains("wristwatches"));
        assert_eq!(a.findings[0].chapters, ["Two"]);
    }

    #[test]
    fn parse_reads_empty_findings() {
        let a = parse(r#"{"findings":[]}"#).unwrap();
        assert!(a.findings.is_empty());
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::ModelNonCompliance(_))));
    }

    #[test]
    fn request_carries_the_concern_manuscript_and_schema() {
        let req = build_request("the manuscript".into(), "1850s London", "claude-opus-4-8");
        assert_eq!(req.model, "claude-opus-4-8");
        assert_eq!(req.cached_context, "the manuscript");
        assert!(req.json_schema.is_some());
        // The named concern is baked into the system prompt so the pass checks only it.
        let system = req.system.as_deref().unwrap_or("");
        assert!(system.contains("1850s London"), "{system}");
        // The accuracy-not-sensitivity line is explicit in the prompt.
        assert!(
            system.contains("NOT a sensitivity"),
            "prompt draws the accuracy-not-sensitivity line: {system}"
        );
        // Flags-only: the prompt forbids rewrites/fixes.
        assert!(system.contains("Do NOT rewrite"), "{system}");
    }

    #[test]
    fn schema_requires_detail_and_chapters_per_finding() {
        let s = schema();
        let item = &s["properties"]["findings"]["items"];
        assert_eq!(item["required"], serde_json::json!(["detail", "chapters"]));
        assert_eq!(
            item["properties"]["passage"],
            serde_json::json!({ "type": "string" })
        );
    }
}
