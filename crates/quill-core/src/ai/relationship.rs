//! Relationship-trajectory analysis (#1046): an **author-invoked** pass that tracks
//! how the bond between a chosen character *pair* moves across the book — not the
//! static current-state snapshot the character pass and the relationship index
//! (#707) already capture, but a per-chapter *trajectory* (allied → estranged →
//! reconciled) — and flags **inconsistencies** as findings.
//!
//! Two kinds of drift are worth flagging, and only these: a chapter's state that
//! **contradicts** an earlier established one (a coldness where there was no prior
//! break), and a **reversal that happens off-page** — the state flips between
//! chapters with no on-page beat setting it up. Each finding names the pair and the
//! chapters involved; the trajectory itself is the per-chapter states so the author
//! can *see* the arc.
//!
//! This is the *Analysis* category of the assistant-not-co-writer rule (see
//! CLAUDE.md): it **observes and flags only**. It never rewrites the arc, proposes a
//! "better" trajectory, suggests how to set up a missing beat, or produces any prose
//! the author could adopt — it surfaces the drift; the author decides what to
//! change. Off by default, run per pair, on the author's request. See
//! `docs/ai-features.md`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AiError, Request};

/// The result of a relationship-trajectory pass for one character pair: the
/// per-chapter states (the trajectory) plus the inconsistency findings drawn from
/// it. Never a rewrite — the author reads the arc and decides.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Trajectory {
    /// The pair this trajectory tracks, as the author would recognize it (e.g.
    /// "Alice / Bob") — echoed back so a finding and the states name the same pair.
    #[serde(default)]
    pub pair: String,
    /// The relationship's state per chapter, in reading order — the arc, not one
    /// label. Chapters where the pair doesn't appear are simply omitted.
    #[serde(default)]
    pub states: Vec<PairState>,
    /// The inconsistencies observed in the trajectory — contradictions and
    /// off-page reversals. Empty when the arc reads as consistent.
    #[serde(default)]
    pub findings: Vec<TrajectoryFinding>,
}

/// The relationship's state at one chapter: a short state label (e.g. "allied",
/// "estranged", "reconciled") plus the chapter it holds in. This is *tracking* — a
/// note on where the bond stands, never edited prose.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct PairState {
    /// The chapter (title) this state holds in.
    pub chapter: String,
    /// A short label for where the bond stands in this chapter (e.g. "allied",
    /// "estranged", "reconciled", "tense").
    pub state: String,
}

/// One inconsistency in the trajectory: what kind of drift it is, a plain note on
/// the observed contradiction / off-page reversal, and the chapters involved.
/// Observation only — it names the drift and where it is, never how to fix it.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct TrajectoryFinding {
    /// The kind of inconsistency: a `contradiction` (a state that contradicts an
    /// earlier established one) or an `off-page reversal` (the state flips with no
    /// on-page beat setting it up). A short free label the model supplies.
    #[serde(default)]
    pub kind: String,
    /// The observed inconsistency — what drifts and why it reads as unset-up.
    /// Never a rewrite or a proposed fix.
    pub detail: String,
    /// The chapters involved (titles) — the earlier established state and the one
    /// that contradicts / reverses it.
    #[serde(default)]
    pub chapters: Vec<String>,
}

/// Build the system prompt for a chosen `pair` (#1046). The pair is interpolated so
/// the model tracks *only* that bond, and the prompt draws the findings-only line
/// hard so the pass can't drift into rewriting or proposing an arc.
fn system_for(pair: &str) -> String {
    format!(
        "You are a developmental editor tracking how ONE relationship changes across a \
novel — the bond between this character pair the author chose: \"{pair}\". Read the \
manuscript and do two things, both TRACKING and FLAGGING only.\n\
1. TRAJECTORY: for each chapter where the pair meaningfully appears together, give a \
SHORT `state` label for where their bond stands (e.g. \"allied\", \"estranged\", \
\"reconciled\", \"tense\", \"wary\") and the `chapter` (by title), in reading order. \
This is the arc — a per-chapter trajectory, not one overall label. Omit chapters \
where the pair doesn't figure.\n\
2. FINDINGS: flag INCONSISTENCIES in that trajectory, and only these two kinds:\n\
   - a `contradiction` — a chapter's state that contradicts an earlier ESTABLISHED \
state (e.g. a sudden coldness with no prior break on the page);\n\
   - an `off-page reversal` — the state flips between chapters (allied→estranged, \
estranged→reconciled) with NO on-page beat, scene, or line setting up the change.\n\
For each finding give its `kind`, a plain `detail` naming what drifts and why it \
reads as unset-up, and the `chapters` (by title) involved — the earlier state and \
the one that contradicts or reverses it. If the arc reads as consistent, return no \
findings.\n\
CRITICAL BOUNDARIES:\n\
- You only OBSERVE and FLAG. Do NOT rewrite the arc, propose a \"better\" or fixed \
trajectory, suggest how to set up a missing beat, draft a reconciliation scene, or \
offer any prose the author could adopt. Name the drift and where it is; the author \
decides what to change.\n\
- Track ONLY the pair the author named (\"{pair}\"). Do not track other pairs or \
raise concerns about them.\n\
- Flag only genuine inconsistencies — a real contradiction or a reversal with no \
on-page setup. A gradual, on-page shift is a consistent arc, not a finding; when in \
doubt, leave it be."
    )
}

/// The JSON Schema for a trajectory reply — the echoed `pair`, the per-chapter
/// `states`, and the inconsistency `findings`.
pub fn schema() -> Value {
    let state = super::object_schema(
        vec![
            ("chapter", super::string_field()),
            ("state", super::string_field()),
        ],
        &["chapter", "state"],
    );
    let finding = super::object_schema(
        vec![
            ("kind", super::string_field()),
            ("detail", super::string_field()),
            ("chapters", super::string_array()),
        ],
        &["detail", "chapters"],
    );
    super::object_schema(
        vec![
            ("pair", super::string_field()),
            ("states", super::array_of(state)),
            ("findings", super::array_of(finding)),
        ],
        &["findings"],
    )
}

/// Build the trajectory request: the manuscript as cached context, the author's
/// chosen `pair` baked into the system prompt, schema attached (#1046). The
/// manuscript is only *read* to track and flag — never edited.
pub fn build_request(manuscript: String, pair: &str, model: &str) -> Request {
    super::manuscript_request(
        &system_for(pair),
        "Report the relationship trajectory and inconsistency findings for the named pair as specified — tracking and flags only, never a rewrite.",
        manuscript,
        model,
        schema(),
    )
}

/// Parse a structured trajectory reply.
pub fn parse(text: &str) -> Result<Trajectory, AiError> {
    super::parse_json(text, "relationship")
}

impl TrajectoryFinding {
    /// The one-line summary + chapters for this finding, shared by the apply handler
    /// and the single-finding re-check so both produce byte-identical summaries
    /// (#608). The trajectory context (the state labels for the involved chapters)
    /// is folded in when available, so the author sees the arc at the drift.
    pub fn summarize(&self) -> String {
        if self.kind.trim().is_empty() {
            self.detail.clone()
        } else {
            format!("{}: {}", self.kind.trim(), self.detail)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_states_and_findings() {
        let json = r#"{
            "pair":"Alice / Bob",
            "states":[
                {"chapter":"One","state":"allied"},
                {"chapter":"Five","state":"estranged"}
            ],
            "findings":[
                {"kind":"off-page reversal","detail":"they turn estranged between Two and Five with no on-page break","chapters":["Two","Five"]}
            ]
        }"#;
        let t = parse(json).unwrap();
        assert_eq!(t.pair, "Alice / Bob");
        assert_eq!(t.states.len(), 2);
        assert_eq!(t.states[0].chapter, "One");
        assert_eq!(t.states[0].state, "allied");
        assert_eq!(t.findings.len(), 1);
        assert_eq!(t.findings[0].kind, "off-page reversal");
        assert_eq!(t.findings[0].chapters, ["Two", "Five"]);
    }

    #[test]
    fn parse_reads_a_consistent_arc_with_no_findings() {
        let json =
            r#"{"pair":"Alice / Bob","states":[{"chapter":"One","state":"allied"}],"findings":[]}"#;
        let t = parse(json).unwrap();
        assert!(t.findings.is_empty(), "a consistent arc yields no findings");
        assert_eq!(t.states.len(), 1);
    }

    #[test]
    fn parse_tolerates_missing_optional_fields() {
        // Only `findings` is required by the schema; states/pair default cleanly.
        let t = parse(r#"{"findings":[]}"#).unwrap();
        assert!(t.pair.is_empty());
        assert!(t.states.is_empty());
        assert!(t.findings.is_empty());
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::ModelNonCompliance(_))));
    }

    #[test]
    fn request_carries_the_pair_manuscript_and_schema() {
        let req = build_request("the manuscript".into(), "Alice / Bob", "claude-opus-4-8");
        assert_eq!(req.model, "claude-opus-4-8");
        assert_eq!(req.cached_context, "the manuscript");
        assert!(req.json_schema.is_some());
        // The chosen pair is baked into the system prompt so the pass tracks only it.
        let system = req.system.as_deref().unwrap_or("");
        assert!(system.contains("Alice / Bob"), "{system}");
        // Findings-only: the prompt forbids rewriting / proposing an arc.
        assert!(system.contains("Do NOT rewrite the arc"), "{system}");
        assert!(system.contains("off-page reversal"), "{system}");
    }

    #[test]
    fn schema_declares_the_trajectory_and_finding_shape() {
        let s = schema();
        // The per-chapter states carry chapter + state, no prose field.
        let state = &s["properties"]["states"]["items"];
        assert_eq!(state["required"], serde_json::json!(["chapter", "state"]));
        // Each finding carries kind / detail / chapters — observation fields only.
        let item = &s["properties"]["findings"]["items"];
        assert_eq!(item["required"], serde_json::json!(["detail", "chapters"]));
        assert_eq!(
            item["properties"]["kind"],
            serde_json::json!({ "type": "string" })
        );
    }

    #[test]
    fn schema_has_no_field_that_could_carry_edited_prose() {
        // The whole schema — states + findings — exposes only tracking / observation
        // fields, so a compliant reply structurally cannot smuggle manuscript prose.
        let s = schema();
        let state_keys: Vec<&str> = s["properties"]["states"]["items"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            state_keys
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>(),
            ["chapter", "state"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
        );
        let finding_keys: Vec<&str> = s["properties"]["findings"]["items"]["properties"]
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(
            finding_keys
                .iter()
                .copied()
                .collect::<std::collections::BTreeSet<_>>(),
            ["chapters", "detail", "kind"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
        );
    }

    #[test]
    fn summarize_prefixes_the_kind() {
        let f = TrajectoryFinding {
            kind: "contradiction".into(),
            detail: "cold in Six after warmth in Five".into(),
            chapters: vec!["Five".into(), "Six".into()],
        };
        assert_eq!(
            f.summarize(),
            "contradiction: cold in Six after warmth in Five"
        );
    }

    #[test]
    fn summarize_falls_back_to_detail_when_kind_is_blank() {
        let f = TrajectoryFinding {
            kind: "  ".into(),
            detail: "unset-up reversal".into(),
            chapters: vec![],
        };
        assert_eq!(f.summarize(), "unset-up reversal");
    }
}
