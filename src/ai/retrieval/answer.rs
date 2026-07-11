//! Assembling and disclosing the final answer (#856, #1056): [`compact`] (the
//! running-context compaction), [`finish`] (build the grounded [`Answer`] from the
//! model reply + gathered excerpts), [`coverage_disclosure`] (render the coverage
//! metadata as the human-readable hedge), and [`answer_response`] (wrap the answer
//! as the shared Q&A [`Response`]).

use std::collections::BTreeSet;

use serde_json::{json, Value};

use crate::ai::{Answer, Coverage, Response, Usage};

use super::seed::{BibleSeed, Excerpt};

/// Compact the running context when the accumulated excerpts exceed `budget` (#856):
/// replace the OLDEST raw tool results with a one-line note (tool + argument + the
/// chapters it cited), keeping their citations, until the retained bodies fit. The
/// bible seed (message 0) and the newest excerpts stay verbatim, so recent detail and
/// the global index survive while an old chapter dump shrinks to its signal.
pub(super) fn compact(messages: &mut Vec<Value>, excerpts: &mut [Excerpt], budget: usize) {
    let total: usize = excerpts.iter().map(|e| e.body.len()).sum();
    if total <= budget {
        return;
    }
    // Rebuild the message list from the seed + compacted/kept excerpts. Walk oldest
    // first, compacting until the kept raw bodies fit the budget.
    let mut kept_len = total;
    let mut rebuilt: Vec<Value> = vec![messages[0].clone()];
    for e in excerpts.iter_mut() {
        if kept_len > budget && e.body.len() > COMPACT_NOTE_MAX {
            kept_len -= e.body.len();
            let cites = if e.citations.is_empty() {
                String::new()
            } else {
                format!(" (chapters: {})", e.citations.join(", "))
            };
            e.body = format!(
                "[earlier {} \"{}\" — captured{}]",
                e.tool, e.argument, cites
            );
            kept_len += e.body.len();
        }
        rebuilt.push(json!({
            "role": "user",
            "content": [ { "type": "text", "text": e.body.clone() } ],
        }));
    }
    *messages = rebuilt;
}

/// A raw excerpt shorter than this is left alone by [`compact`] — already near its
/// note size, so summarizing it saves nothing.
const COMPACT_NOTE_MAX: usize = 200;

/// Assemble the final [`Answer`] from the model's reply and the accumulated excerpts
/// (#856, #1056): the answer prose, the citations gathered across every chapter
/// actually read/searched (deduped, in encounter order), a structured [`Coverage`]
/// record of the read-set and how complete it was, and a disclosure derived from that
/// coverage so a holistic answer stays hedged. `total_chapters` is the size of the
/// manuscript the loop read from, so coverage reads as a fraction ("6 of 40").
/// `incomplete` is set when the turn cap was reached. Never emits manuscript prose:
/// the answer is analysis, the citations and the read-set are chapter titles.
pub(super) fn finish(
    text: &str,
    excerpts: &[Excerpt],
    bible: &BibleSeed,
    total_chapters: usize,
    incomplete: bool,
) -> Answer {
    let text = text.trim();
    // Prefer the model's own {answer, citations} JSON when it produced one; otherwise
    // treat the reply as plain answer prose.
    let mut answer = crate::ai::parse_answer(text);
    if answer.answer.trim().is_empty() {
        answer.answer = if text.is_empty() {
            "I could not find enough in the manuscript to answer confidently.".to_string()
        } else {
            text.to_string()
        };
    }

    // The citations are the chapters actually read/searched — the ground truth of what
    // the answer could draw from. Merge the model's cited titles with the gathered
    // ones, deduped in encounter order, so a citation always points at a chapter the
    // loop really touched.
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut citations: Vec<String> = Vec::new();
    for title in answer
        .citations
        .iter()
        .cloned()
        .chain(excerpts.iter().flat_map(|e| e.citations.iter().cloned()))
    {
        let title = title.trim().to_string();
        if !title.is_empty() && seen.insert(title.to_lowercase()) {
            citations.push(title);
        }
    }
    answer.citations = citations;

    // The coverage metadata (#1056): the read-set (exactly the merged citations — the
    // chapters the loop touched), the corpus size, and the reasons coverage may be
    // partial. This is metadata about the read, asserted separately from the prose.
    let coverage = Coverage {
        chapters_read: answer.citations.clone(),
        chapters_total: total_chapters,
        no_bible: bible.is_empty(),
        cap_reached: incomplete,
    };

    // Disclose the retrieval nature so a holistic answer stays hedged (#856, #1056).
    // The disclosure is DERIVED from the coverage metadata — a human-readable rendering
    // of the same facts, appended so it reads in the answer overlay. It never adds or
    // alters the answer prose above it; the metadata is the source of truth.
    answer.answer = format!(
        "{}{}",
        answer.answer.trim_end(),
        coverage_disclosure(&coverage)
    );
    answer.coverage = Some(coverage);
    answer
}

/// Render the coverage metadata (#1056) as the human-readable disclosure appended to
/// an offline answer: what fraction of the book the loop read and why coverage may be
/// partial ("Answered offline from 6 of 40 chapters read…"). Purely derived from
/// [`Coverage`] — the metadata is the source of truth, this is its rendering — so the
/// disclosure and the structured coverage can never disagree. A well-covered answer
/// (every chapter read, a bible present, no cap) gets a plain, unhedged note.
fn coverage_disclosure(coverage: &Coverage) -> String {
    let read = coverage.chapters_read.len();
    let total = coverage.chapters_total;
    let mut out = if !coverage.is_hedged() {
        // Read the whole book, with a bible, uninterrupted: no hedge to make.
        String::from("\n\n(Answered offline from a full read of the manuscript")
    } else if total > 0 {
        format!(
            "\n\n(Answered offline from {read} of {total} chapters read, not the whole manuscript"
        )
    } else {
        String::from(
            "\n\n(Answered offline from targeted excerpts, not a full read of the manuscript",
        )
    };
    if coverage.no_bible {
        out.push_str("; no story bible was available, so coverage may be partial");
    }
    if coverage.cap_reached {
        out.push_str("; the retrieval turn cap was reached, so coverage is incomplete");
    }
    out.push_str(".)");
    out
}

/// Wrap a loop [`Answer`] as the [`Response`] the shared Q&A dispatch consumes
/// (#856), so the retrieval loop flows through the same `Outcome::Answer` →
/// `apply_qa_result` → `parse_answer` path as whole-book Ask — one Q&A path, jumpable
/// citations unchanged. The response text is the `{answer, citations}` JSON
/// `parse_answer` reads back; a serialize failure falls back to the answer prose.
pub fn answer_response(answer: &Answer) -> Response {
    let text = serde_json::to_string(answer).unwrap_or_else(|_| answer.answer.clone());
    Response {
        text,
        usage: Usage::default(),
    }
}
