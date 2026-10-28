//! Foreshadow / payoff scan (#1047): an **author-invoked** pass that pairs a
//! CONCRETE seeded detail — a Chekhov's-gun object, a named threat, a promised
//! meeting, a planted clue — with its later resolution, and flags the ones that
//! never pay off. Each finding names the setup passage (a short verbatim quote)
//! and the chapter it was planted in, so the author has a checklist of loose ends.
//!
//! This is the *Analysis* category of the assistant-not-co-writer rule (see
//! CLAUDE.md): it surfaces observations the author acts on, never a rewrite, a
//! drafted payoff, or replacement prose. Findings flow into the shared
//! revision-triage list like every other analysis pass. Off by default; the author
//! decides whether to pay off, cut, or keep each dangling setup.
//!
//! **Distinct from the plot pass (`plot.rs`).** The plot tracker tracks *thematic*
//! threads and subplots — throughlines, arcs, mysteries — as a persistent bible
//! artifact with setup→payoff *status*. This pass is narrower and concrete: it
//! matches a *specific planted detail* (an object, a named threat, a promise, a
//! clue) to its resolution and flags only the unpaid ones. It writes no bible file
//! and keeps no promise ledger; it is a one-shot scan for dangling concrete
//! setups. See `docs/ai-features.md`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AiError, Request};

/// The result of a foreshadow pass: the unpaid concrete-setup findings.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Foreshadow {
    #[serde(default)]
    pub findings: Vec<UnpaidSetup>,
}

/// One unpaid setup: a concrete seeded detail introduced then never resolved. It
/// names the `setup` passage (a short verbatim quote so the author can find it),
/// what the planted `detail` is + that it's unpaid, and the `chapters` it was
/// planted in. Never a fix and never a drafted payoff — the author decides whether
/// to pay it off, cut it, or keep it.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct UnpaidSetup {
    /// A short verbatim quote of the passage that plants the detail, so the author
    /// can find the setup.
    #[serde(default)]
    pub setup: String,
    /// What the concrete detail is and why it reads as unpaid — the observation,
    /// never a drafted resolution (e.g. "the letter she pockets in ch. 1 is never
    /// opened or mentioned again").
    pub detail: String,
    /// Chapters the setup is planted in (titles); the first is the jump target.
    #[serde(default)]
    pub chapters: Vec<String>,
}

/// The system prompt for the foreshadow / payoff scan (#1047). It pins the pass to
/// CONCRETE seeded details (not thematic threads — that's the plot pass's job), and
/// draws the findings-only line hard: it flags a dangling setup, it never drafts,
/// suggests, or writes the payoff.
const SYSTEM: &str = "You are a developmental editor doing a FORESHADOW / PAYOFF scan \
the author explicitly asked for. Read the manuscript and find CONCRETE seeded details \
that are introduced and then never paid off — a specific planted object (a \
Chekhov's gun), a named threat, a promised meeting or confrontation, a planted clue, \
a stated intention to do a specific thing. For each such detail, judge whether it is \
RESOLVED anywhere later in the manuscript (used, met, answered, or deliberately \
paid off). Report ONLY the UNPAID ones — a concrete detail set up and then dropped. \
For each, quote a SHORT verbatim `setup` passage so the author can find where it was \
planted, give a plain `detail` naming the concrete thing and why it reads as unpaid, \
and list the `chapters` (by title) the setup appears in.\n\
CRITICAL BOUNDARIES:\n\
- You only OBSERVE and FLAG. Do NOT draft, suggest, or write the payoff, do NOT \
propose how to resolve it, and do NOT rewrite or edit any prose. Name the dangling \
setup and where it is; the author decides whether to pay it off, cut it, or keep it.\n\
- Flag CONCRETE seeded details only — specific planted objects, named threats, \
promised meetings, planted clues. Do NOT flag broad thematic threads, character \
arcs, motifs, or subplots (a separate plot-thread pass tracks those); this scan is \
for concrete Chekhov's-gun details, not throughlines.\n\
- Flag only a detail that is genuinely PLANTED then DROPPED, not one that is paid \
off later (even subtly) and not a deliberate red herring the text resolves as such. \
When in doubt that a detail was really set up as a promise, leave it be.";

/// The JSON Schema for a foreshadow reply — a list of `{setup, detail, chapters}`
/// findings. Only observation fields; no field can carry a drafted payoff or edited
/// prose.
pub fn schema() -> Value {
    let finding = super::object_schema(
        vec![
            ("setup", super::string_field()),
            ("detail", super::string_field()),
            ("chapters", super::string_array()),
        ],
        &["detail", "chapters"],
    );
    super::object_schema(vec![("findings", super::array_of(finding))], &["findings"])
}

/// Build the foreshadow request: the manuscript as cached context, schema attached
/// (#1047). The manuscript is only *read* to pair setups with payoffs and flag the
/// unpaid ones — never edited, and no payoff is drafted.
pub fn build_request(manuscript: String, model: &str) -> Request {
    super::manuscript_request(
        SYSTEM,
        "Report the unpaid concrete-setup findings as specified — flag dangling setups only, never a payoff.",
        manuscript,
        model,
        schema(),
    )
}

/// Parse a structured foreshadow reply.
pub fn parse(text: &str) -> Result<Foreshadow, AiError> {
    super::parse_json(text, "foreshadow")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_findings() {
        let json = r#"{"findings":[{"setup":"she pocketed the brass key","detail":"the brass key is never used or mentioned again","chapters":["One"]}]}"#;
        let f = parse(json).unwrap();
        assert_eq!(f.findings.len(), 1);
        assert_eq!(f.findings[0].setup, "she pocketed the brass key");
        assert!(f.findings[0].detail.contains("brass key"));
        assert_eq!(f.findings[0].chapters, ["One"]);
    }

    #[test]
    fn parse_reads_empty_findings() {
        let f = parse(r#"{"findings":[]}"#).unwrap();
        assert!(f.findings.is_empty());
    }

    #[test]
    fn parse_defaults_optional_setup() {
        // A lenient reply may omit the (optional) setup quote; detail + chapters
        // are required.
        let f = parse(
            r#"{"findings":[{"detail":"the threat is never carried out","chapters":["Two"]}]}"#,
        )
        .unwrap();
        assert_eq!(f.findings[0].setup, "");
        assert_eq!(f.findings[0].detail, "the threat is never carried out");
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::ModelNonCompliance(_))));
    }

    #[test]
    fn request_carries_manuscript_and_schema() {
        let req = build_request("the manuscript".into(), "claude-opus-4-8");
        assert_eq!(req.model, "claude-opus-4-8");
        assert_eq!(req.cached_context, "the manuscript");
        assert!(req.json_schema.is_some());
    }

    #[test]
    fn prompt_pins_concrete_details_and_forbids_drafting_the_payoff() {
        let req = build_request("m".into(), "claude-opus-4-8");
        let system = req.system.as_deref().unwrap_or("");
        // Concrete Chekhov's-gun details, explicitly NOT thematic threads (that's
        // the plot pass) — the distinction from plot.rs is baked into the prompt.
        assert!(system.contains("CONCRETE"), "{system}");
        assert!(system.contains("Chekhov"), "{system}");
        assert!(
            system.contains("Do NOT flag broad thematic threads"),
            "distinct from the plot-thread pass: {system}"
        );
        // Findings-only: it never drafts, suggests, or writes the payoff.
        assert!(
            system.contains("Do NOT draft, suggest, or write the payoff"),
            "flags only, never a drafted payoff: {system}"
        );
    }

    #[test]
    fn schema_requires_detail_and_chapters_per_finding() {
        let s = schema();
        let item = &s["properties"]["findings"]["items"];
        assert_eq!(item["required"], serde_json::json!(["detail", "chapters"]));
        assert_eq!(
            item["properties"]["setup"],
            serde_json::json!({ "type": "string" })
        );
    }

    #[test]
    fn schema_has_no_field_for_a_drafted_payoff() {
        // The schema can only carry observation fields — the flagged setup, a
        // why-note, and chapters. There is no field a model could use to return a
        // drafted or suggested payoff / edited prose.
        let s = schema();
        let props = &s["properties"]["findings"]["items"]["properties"];
        let keys: std::collections::BTreeSet<&str> = props
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            keys,
            ["chapters", "detail", "setup"].into_iter().collect(),
            "only observation fields — no payoff/prose field: {keys:?}"
        );
    }
}
