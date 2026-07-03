//! Plot-thread / arc tracker (#606): a fourth persistent, incrementally-updated
//! bible pass alongside characters (#74), timeline (#75), and setting (#77). It
//! extracts the manuscript's subplots and thematic throughlines as structured
//! threads — each with a status, its setup, its intended/actual payoff, the
//! chapter it was last advanced in, and free notes — written to `bible/plot.md`,
//! and flags **dangling** threads (a setup with no payoff, a thread left open or
//! abandoned) as triageable findings. Like the other trackers it reuses the
//! shared [`bible`] format and the incremental-scoping machinery (#90); it tracks
//! and flags only, never writing manuscript prose. See `docs/ai-features.md`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{bible, incremental, AiError, Request, MAX_OUTPUT_TOKENS};

/// The result of a plot pass: the tracked threads and the dangling-thread
/// findings.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Plot {
    #[serde(default)]
    pub threads: Vec<Thread>,
    #[serde(default)]
    pub findings: Vec<ThreadFinding>,
}

/// One plot thread / arc: a subplot or thematic throughline tracked from setup to
/// payoff. Structured tracking only — a note of where the thread stands, never
/// prose the author should write.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Thread {
    /// A short name for the thread, e.g. "The stolen locket" or "Trust vs. duty".
    pub name: String,
    /// Where the thread stands: e.g. "open", "resolved", "abandoned", "setup".
    pub status: String,
    /// How/where the thread is introduced (the setup).
    #[serde(default)]
    pub setup: String,
    /// How/where the thread pays off — empty when it hasn't yet.
    #[serde(default)]
    pub payoff: String,
    /// The chapter (title) the thread was last advanced in.
    #[serde(default)]
    pub last_seen: String,
    /// Any extra structured notes on the thread.
    #[serde(default)]
    pub notes: String,
}

/// A dangling-thread finding: a subplot or throughline set up but not paid off,
/// or otherwise left hanging.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct ThreadFinding {
    /// The thread involved, e.g. "The stolen locket".
    pub thread: String,
    /// What's dangling, e.g. "introduced in ch. 2, never resolved".
    pub issue: String,
    /// Chapters involved (titles).
    #[serde(default)]
    pub chapters: Vec<String>,
}

const SYSTEM: &str = "You are a developmental editor tracking a novel's plot \
threads. Read the manuscript and (1) identify the distinct plot threads and \
thematic throughlines — subplots, promises, mysteries, character arcs framed as \
throughlines, recurring themes — and for each give a short name, a status (one \
of: setup, open, developing, resolved, abandoned), how it is set up, how it pays \
off (empty if it doesn't), the chapter title it was last advanced in, and any \
concise notes; and (2) flag DANGLING threads — a thread set up but never paid \
off, a promise or mystery left unanswered, a subplot abandoned mid-book — each \
with the chapters involved. Be faithful to the text; report only genuine threads \
and genuine dangling issues, not stylistic choices. Reference chapters by title. \
Track and flag only — never write or suggest prose.";

/// The JSON Schema for a plot reply.
pub fn schema() -> Value {
    let thread = super::object_schema(
        vec![
            ("name", super::string_field()),
            ("status", super::string_field()),
            ("setup", super::string_field()),
            ("payoff", super::string_field()),
            ("last_seen", super::string_field()),
            ("notes", super::string_field()),
        ],
        &["name", "status", "setup", "payoff", "last_seen", "notes"],
    );
    super::object_schema(
        vec![
            ("threads", super::array_of(thread)),
            (
                "findings",
                super::array_of(super::chapters_finding("thread", "issue")),
            ),
        ],
        &["threads", "findings"],
    )
}

/// Build the plot request: the manuscript as cached context, schema attached.
pub fn build_request(manuscript: String, model: &str) -> Request {
    super::manuscript_request(
        SYSTEM,
        "Extract the plot threads and dangling-thread findings as specified.",
        manuscript,
        model,
        schema(),
    )
}

const REVISE_SYSTEM: &str = "You are a developmental editor maintaining an \
existing plot-thread tracker. You are given the current tracker and only the \
chapters that have changed since it was built; the tracker also records threads \
from the rest of the work — earlier chapters, and in a multi-book series the \
earlier books — that are NOT shown to you. Revise it: update the status, payoff, \
and last-seen chapter of the threads the given chapters affect, add threads they \
introduce, and drop what they contradict — keeping the threads you can't see \
intact. Then check the whole set of threads against the given chapters for \
genuine DANGLING threads (a setup with no payoff, an unanswered promise or \
mystery, an abandoned subplot), INCLUDING threads the tracker records from \
material not shown. Be faithful to the text; report only genuine dangling issues, \
each with the chapters involved. Reference chapters by title. Track and flag only \
— never write prose. Return the complete revised plot tracker and findings.";

/// Build an incremental revision request (#90): revise the existing plot tracker
/// from only the changed chapters.
pub fn build_incremental_request(
    prior_bible: &str,
    changed_chapters: &str,
    model: &str,
) -> Request {
    incremental::revise_request(
        REVISE_SYSTEM,
        schema(),
        MAX_OUTPUT_TOKENS,
        prior_bible,
        changed_chapters,
        model,
    )
}

/// Parse a structured plot reply.
pub fn parse(text: &str) -> Result<Plot, AiError> {
    super::parse_json(text, "plot")
}

/// Render the plot-thread bible file via [`bible::render`]: one section per thread
/// with its status/setup/payoff/last-seen/notes, then the dangling-thread
/// findings.
pub fn render(p: &Plot, notes: &str, today: &str) -> String {
    let mut body = String::new();

    body.push_str("## Threads\n");
    for t in &p.threads {
        body.push_str(&format!("### {} — {}\n", t.name, t.status));
        if !t.setup.is_empty() {
            body.push_str(&format!("- **Setup:** {}\n", t.setup));
        }
        if !t.payoff.is_empty() {
            body.push_str(&format!("- **Payoff:** {}\n", t.payoff));
        }
        if !t.last_seen.is_empty() {
            body.push_str(&format!("- **Last seen:** {}\n", t.last_seen));
        }
        if !t.notes.is_empty() {
            body.push_str(&format!("- **Notes:** {}\n", t.notes));
        }
        body.push('\n');
    }

    if !p.findings.is_empty() {
        body.push_str("## Dangling threads\n");
        for f in &p.findings {
            let where_ = if f.chapters.is_empty() {
                String::new()
            } else {
                format!(" ({})", f.chapters.join(", "))
            };
            body.push_str(&format!("- **{}:** {}{}\n", f.thread, f.issue, where_));
        }
        body.push('\n');
    }

    bible::render("title: Plot threads\n", &body, notes, today)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Plot {
        Plot {
            threads: vec![
                Thread {
                    name: "The stolen locket".into(),
                    status: "resolved".into(),
                    setup: "Mara pockets the locket in ch. 1".into(),
                    payoff: "returned to its owner in the finale".into(),
                    last_seen: "The Reckoning".into(),
                    notes: "mirrors the trust theme".into(),
                },
                Thread {
                    name: "Who burned the archive".into(),
                    status: "open".into(),
                    setup: "the fire opens ch. 3".into(),
                    payoff: String::new(),
                    last_seen: "Ashes".into(),
                    notes: String::new(),
                },
            ],
            findings: vec![ThreadFinding {
                thread: "Who burned the archive".into(),
                issue: "introduced in ch. 3, never answered".into(),
                chapters: vec!["Ashes".into()],
            }],
        }
    }

    #[test]
    fn schema_requires_threads_and_findings() {
        let s = schema();
        assert_eq!(s["required"], serde_json::json!(["threads", "findings"]));
        let thread = &s["properties"]["threads"]["items"];
        assert_eq!(
            thread["required"],
            serde_json::json!(["name", "status", "setup", "payoff", "last_seen", "notes"])
        );
        // Dangling-thread findings reuse the shared two-strings-plus-chapters shape.
        let finding = &s["properties"]["findings"]["items"];
        assert_eq!(
            finding["required"],
            serde_json::json!(["thread", "issue", "chapters"])
        );
    }

    #[test]
    fn parse_reads_threads_and_findings() {
        let json = r#"{"threads":[{"name":"Locket","status":"open","setup":"s","payoff":"","last_seen":"One","notes":""}],"findings":[]}"#;
        let p = parse(json).unwrap();
        assert_eq!(p.threads.len(), 1);
        assert_eq!(p.threads[0].name, "Locket");
        assert_eq!(p.threads[0].status, "open");
        assert!(p.findings.is_empty());
    }

    #[test]
    fn parse_defaults_optional_fields() {
        // The optional string fields tolerate omission (a lenient mock reply).
        let json = r#"{"threads":[{"name":"Locket","status":"open"}],"findings":[]}"#;
        let p = parse(json).unwrap();
        assert_eq!(p.threads[0].setup, "");
        assert_eq!(p.threads[0].payoff, "");
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("nope"), Err(AiError::ModelNonCompliance(_))));
    }

    #[test]
    fn incremental_request_carries_prior_and_changed() {
        let req = build_incremental_request("PRIOR BIBLE", "CHANGED CH", "claude-opus-4-8");
        assert!(req.cached_context.contains("PRIOR BIBLE"));
        assert!(req.cached_context.contains("CHANGED CH"));
        assert!(req.system.as_deref().unwrap().contains("dangling"));
        assert!(req.json_schema.is_some(), "same schema as the full pass");
    }

    #[test]
    fn render_emits_threads_and_dangling_findings() {
        let file = render(&sample(), "## Notes\n", "2026-06-19");
        assert!(file.contains("title: Plot threads"));
        assert!(file.contains("## Threads"));
        assert!(file.contains("### The stolen locket — resolved"));
        assert!(file.contains("**Setup:** Mara pockets the locket in ch. 1"));
        assert!(file.contains("**Payoff:** returned to its owner in the finale"));
        assert!(file.contains("**Last seen:** The Reckoning"));
        assert!(file.contains("## Dangling threads"));
        assert!(file.contains("introduced in ch. 3, never answered"));
        // An open thread with no payoff omits the Payoff line rather than blanking it.
        let open = file.split("Who burned the archive").nth(1).unwrap();
        let next = open.find("## Dangling").unwrap();
        assert!(!open[..next].contains("**Payoff:**"));
    }

    #[test]
    fn render_round_trips_through_the_bible_format() {
        let file = render(&sample(), "## Notes\n\nKeep me.\n", "2026-06-19");
        assert_eq!(bible::extract_notes(&file).trim(), "## Notes\n\nKeep me.");
        assert!(!bible::is_hand_edited(&file), "freshly rendered is clean");
    }
}
