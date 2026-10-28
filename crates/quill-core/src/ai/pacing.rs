//! Pacing & tension reading (#1048): an **author-invoked** pass that reads the
//! manuscript's scene-level tension/quiet rhythm — a sagging middle, a run of
//! high-tension chapters with no breath, a flat stretch — and reports it as
//! LOCATED, PLAIN-LANGUAGE **observations** in the revision-triage list. Each
//! observation names the stretch (chapters/scenes) it refers to.
//!
//! This is the *Analysis* category of the assistant-not-co-writer rule (see
//! CLAUDE.md), and it sits closest to the co-writer line — so the boundary is
//! enforced three ways:
//!
//! 1. The pass ONLY points out rhythm ("this stretch reads low-tension", "three
//!    chapters of rising tension with no quiet beat"). It NEVER suggests a cut, an
//!    addition, a reorder, or reworded prose — any output proposing a change to the
//!    text is out of scope.
//! 2. The schema carries no field that could hold a suggested fix or prose — only
//!    the `observation` and the `chapters` it locates (mirroring editorial/accuracy:
//!    `{ observation, chapters }`, no "suggestion"/"fix"/"rewrite" field).
//! 3. The prompt forbids proposing changes and describes a deliberate slow passage /
//!    quiet interlude NEUTRALLY as an observation, never as a problem to fix.
//!
//! To keep the reading grounded rather than free-associative, a deterministic
//! per-chapter pacing *signal* (word count, sentence-length cadence, dialogue
//! share) computed locally from the manuscript is fed alongside the prose as read-
//! only context ([`signal_context`]). The output is still observation-only findings.
//! Off by default, on the author's request. See `docs/ai-features.md`.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{AiError, Request};
use crate::analysis::{dialogue_split, sentences};
use crate::buffer::count_words;

/// The result of a pacing pass: the located rhythm observations.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Pacing {
    #[serde(default)]
    pub observations: Vec<PacingObservation>,
}

/// One pacing observation: what the rhythm reads like over a stretch, and which
/// chapters/scenes that stretch spans. Never a fix, a cut, an addition, a reorder,
/// or reworded prose — the author decides what, if anything, to change.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct PacingObservation {
    /// The rhythm the stretch reads like, in plain language — the observation only
    /// (e.g. "these chapters read low-tension after the midpoint reveal"), never a
    /// suggested change.
    pub observation: String,
    /// The chapters/scenes the observation locates itself to (titles).
    #[serde(default)]
    pub chapters: Vec<String>,
}

/// One chapter's deterministic pacing signal (#1048), computed locally from its
/// prose — a grounding input to the reading, not a finding. Word count is the
/// coarse "how much happens here"; mean sentence length is prose cadence (short,
/// clipped sentences read faster/tenser); dialogue share separates scene from
/// summary. All three are measured, never inferred.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ChapterSignal<'a> {
    pub title: &'a str,
    pub words: usize,
    pub mean_sentence_words: f64,
    pub dialogue_pct: f64,
}

/// Compute a chapter's deterministic pacing signal from its title and prose.
pub fn chapter_signal<'a>(title: &'a str, prose: &str) -> ChapterSignal<'a> {
    let words = count_words(prose);
    let lens: Vec<usize> = sentences(prose)
        .iter()
        .map(|s| count_words(s))
        .filter(|&w| w > 0)
        .collect();
    let mean_sentence_words = if lens.is_empty() {
        0.0
    } else {
        lens.iter().sum::<usize>() as f64 / lens.len() as f64
    };
    ChapterSignal {
        title,
        words,
        mean_sentence_words,
        dialogue_pct: dialogue_split(prose).dialogue_pct(),
    }
}

/// Render the per-chapter deterministic signals as a compact, read-only context
/// block the model reads *alongside* the manuscript to ground the reading (#1048).
/// It is measured data, not an instruction — the reading still comes from the prose;
/// this only keeps the located observations honest about where the rhythm shifts.
pub fn signal_context(signals: &[ChapterSignal]) -> String {
    let mut out = String::from(
        "Per-chapter pacing signal (measured locally; grounding only, not findings):\n",
    );
    for s in signals {
        let _ = writeln!(
            out,
            "- {}: {} words, mean sentence {:.0} words, {:.0}% dialogue",
            s.title, s.words, s.mean_sentence_words, s.dialogue_pct
        );
    }
    out
}

/// The system prompt (#1048). It draws the boundary hard: OBSERVE rhythm and where
/// it shifts, never propose a change; and describe a deliberate slow passage /
/// quiet interlude neutrally as an observation, not a problem to fix.
const SYSTEM: &str = "You are giving a manuscript a PACING & TENSION reading the \
author asked for. Read it as a sequence of scenes and chapters and report where the \
tension/quiet RHYTHM sits and where it shifts — a sagging middle, a run of \
high-tension chapters with no quiet beat to breathe, a flat low-tension stretch, an \
abrupt spike or drop. A measured per-chapter pacing signal (word count, sentence \
cadence, dialogue share) is provided as grounding; use it to locate observations, \
not as findings in itself.\n\
For each observation give a plain, specific `observation` describing what the \
rhythm READS LIKE over a stretch, and list the `chapters` (by title) that stretch \
spans, so the author can find it.\n\
CRITICAL BOUNDARIES — this is observation only, and it sits right at the line \
between analysis and co-writing:\n\
- You ONLY POINT OUT rhythm. Do NOT suggest a cut, an addition, a reorder, a scene \
to move, a beat to insert, tightening, expansion, or any reworded prose. Do NOT \
propose ANY change to the text. Name what the rhythm reads like and where it is; \
the author decides what, if anything, to do.\n\
- A slow passage or quiet interlude is often DELIBERATE. Describe such a stretch \
NEUTRALLY as an observation of its rhythm (\"this stretch reads low-tension / \
reflective\"), never as a problem, a flaw, a drag, or something to fix.\n\
- Report only rhythm you can actually locate across named chapters, not vague \
impressions or matters of taste.";

/// The JSON Schema for a pacing reply — a list of `{observation, chapters}`. There
/// is deliberately no field that could carry a suggested change or prose (#1048).
pub fn schema() -> Value {
    let observation = super::object_schema(
        vec![
            ("observation", super::string_field()),
            ("chapters", super::string_array()),
        ],
        &["observation", "chapters"],
    );
    super::object_schema(
        vec![("observations", super::array_of(observation))],
        &["observations"],
    )
}

/// Build the pacing request: the manuscript as cached context with the deterministic
/// per-chapter `signal` prepended for grounding, schema attached (#1048). The
/// manuscript is only *read* to observe its rhythm — never edited, never rewritten.
pub fn build_request(manuscript: String, signal: &str, model: &str) -> Request {
    let context = if signal.is_empty() {
        manuscript
    } else {
        format!("{signal}\n{manuscript}")
    };
    super::manuscript_request(
        SYSTEM,
        "Give the manuscript a pacing & tension reading and report the located observations as specified — observations only, never a suggested change.",
        context,
        model,
        schema(),
    )
}

/// Parse a structured pacing reply.
pub fn parse(text: &str) -> Result<Pacing, AiError> {
    super::parse_json(text, "pacing")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_reads_observations() {
        let json = r#"{"observations":[
            {"observation":"the middle third reads low-tension after the reveal","chapters":["Five","Six","Seven"]},
            {"observation":"three chapters of rising tension with no quiet beat","chapters":["Nine","Ten","Eleven"]}
        ]}"#;
        let p = parse(json).unwrap();
        assert_eq!(p.observations.len(), 2);
        assert!(p.observations[0].observation.contains("low-tension"));
        assert_eq!(p.observations[0].chapters, ["Five", "Six", "Seven"]);
        assert_eq!(p.observations[1].chapters, ["Nine", "Ten", "Eleven"]);
    }

    #[test]
    fn parse_reads_empty_observations() {
        let p = parse(r#"{"observations":[]}"#).unwrap();
        assert!(p.observations.is_empty());
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::ModelNonCompliance(_))));
    }

    #[test]
    fn request_attaches_manuscript_signal_and_schema() {
        let signal = signal_context(&[ChapterSignal {
            title: "One",
            words: 1200,
            mean_sentence_words: 14.0,
            dialogue_pct: 30.0,
        }]);
        let req = build_request("the manuscript".into(), &signal, "claude-opus-4-8");
        assert_eq!(req.model, "claude-opus-4-8");
        // The prose is read as context, with the deterministic signal prepended.
        assert!(req.cached_context.contains("the manuscript"));
        assert!(
            req.cached_context.contains("Per-chapter pacing signal"),
            "the measured signal grounds the reading: {}",
            req.cached_context
        );
        assert!(req.json_schema.is_some());
    }

    // --- The prompt forbids proposing any change ----------------------------

    #[test]
    fn prompt_forbids_suggesting_a_change() {
        let req = build_request("m".into(), "", "claude-opus-4-8");
        let system = req.system.as_deref().unwrap_or("");
        // Observation only: the prompt forbids proposing a change to the text.
        assert!(
            system.contains("Do NOT propose ANY change"),
            "observation only, no fixes: {system}"
        );
        assert!(
            system.contains("Do NOT suggest a cut"),
            "no cut/add/reorder/reword: {system}"
        );
        // A deliberate slow passage is described neutrally, never as a problem.
        assert!(
            system.contains("DELIBERATE") && system.contains("NEUTRALLY"),
            "deliberate slow passages are neutral observations: {system}"
        );
    }

    // --- The schema has no field that could carry a suggested fix or prose ---

    #[test]
    fn schema_carries_only_observation_fields_no_fix() {
        let s = schema();
        let item = &s["properties"]["observations"]["items"];
        assert_eq!(
            item["required"],
            serde_json::json!(["observation", "chapters"])
        );
        let props = item["properties"].as_object().unwrap();
        let keys: std::collections::BTreeSet<&str> = props.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            ["chapters", "observation"]
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>(),
            "no field carries a suggested change or prose — only the observation + \
             its location: {keys:?}"
        );
        // Belt and suspenders: none of the change-carrying field names exist.
        for banned in [
            "suggestion",
            "fix",
            "rewrite",
            "revision",
            "change",
            "prose",
        ] {
            assert!(
                !props.contains_key(banned),
                "schema must not carry a `{banned}` field"
            );
        }
    }

    #[test]
    fn chapter_signal_is_deterministic_and_measured() {
        let sig = chapter_signal("One", "He ran. \"Stop!\" she cried. The night was long.");
        assert_eq!(sig.title, "One");
        assert!(sig.words > 0);
        assert!(sig.mean_sentence_words > 0.0);
        // "Stop!" is quoted, so some of the words register as dialogue.
        assert!(sig.dialogue_pct > 0.0);
        // Empty prose yields an all-zero (but valid) signal, never a panic/NaN.
        let empty = chapter_signal("Two", "");
        assert_eq!(empty.words, 0);
        assert!(empty.mean_sentence_words.abs() < f64::EPSILON);
        assert!(empty.dialogue_pct.abs() < f64::EPSILON);
    }
}
