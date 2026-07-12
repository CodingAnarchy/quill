//! Assembling and disclosing the final answer (#856, #1056): [`compact`] (the
//! running-context compaction), [`finish`] (build the grounded [`Answer`] from the
//! model reply + gathered excerpts), [`coverage_disclosure`] (render the coverage
//! metadata as the human-readable hedge), and [`answer_response`] (wrap the answer
//! as the shared Q&A [`Response`]).

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{json, Value};

use crate::ai::embed::Chapter;
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
/// coverage so a holistic answer stays hedged. `chapters` is the manuscript the loop
/// read from, so its size is the coverage denominator ("6 of 40"). `incomplete` is set
/// when the turn cap was reached. Never emits manuscript prose: the answer is analysis,
/// the citations and the read-set are chapter titles.
///
/// `chapters` is passed so `finish` knows both the corpus size (the coverage
/// denominator) and the real chapter titles — but the read-set that forms the coverage
/// NUMERATOR is the chapters the loop **actually touched** (the excerpt-derived
/// citations), NOT whatever the model claimed. The model's own cited titles are only
/// kept when a real read backs them; a hallucinated citation — whether an invented
/// title or a real chapter the loop never opened — is dropped
/// (#qa-citation-coverage), so it cannot inflate the read-set into a false "full read"
/// claim and no phantom citation points at a chapter that was never read.
pub(super) fn finish(
    text: &str,
    excerpts: &[Excerpt],
    bible: &BibleSeed,
    chapters: &[Chapter],
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

    // The ground truth of what the answer could draw from is the chapters the loop
    // ACTUALLY read/searched — the excerpt-derived citations (the tool executor records
    // a citation only for a read it truly resolved). Those titles, lowercased, are the
    // allowlist a citation must be on to count, and each key maps to the CANONICAL
    // read-side title — the exact casing the loop resolved. The MODEL's own cited titles
    // are unvalidated claims: after reading one chapter it can list "Chapter 1..40", an
    // invented title, or the right chapter with the wrong casing (`chapter 7` vs
    // `Chapter 7`). So a model citation is kept only when it names a chapter that was
    // really read; anything else is dropped. When it matches case-insensitively we store
    // the canonical read-side title (not the model's raw casing), so the stored citation
    // is exactly the chapter title and the exact-match jump resolves (#1145). This keeps
    // the coverage numerator honest and every citation jumpable. `chapters` supplies the
    // coverage denominator.
    let mut touched: BTreeMap<String, String> = BTreeMap::new();
    for title in excerpts.iter().flat_map(|e| e.citations.iter()) {
        let canonical = title.trim();
        if canonical.is_empty() {
            continue;
        }
        // First read wins the canonical casing, matching the encounter-order dedup below.
        touched
            .entry(canonical.to_lowercase())
            .or_insert_with(|| canonical.to_string());
    }
    let total_chapters = chapters.len();

    // Merge the model's cited titles with the gathered ones, deduped in encounter
    // order, dropping any the loop never actually read — so a citation always points at
    // a chapter the loop really touched. A kept citation is stored as the canonical
    // read-side title, so a mis-cased-but-valid model citation still jumps (#1145).
    let mut seen: BTreeSet<String> = BTreeSet::new();
    let mut citations: Vec<String> = Vec::new();
    for title in answer
        .citations
        .iter()
        .cloned()
        .chain(excerpts.iter().flat_map(|e| e.citations.iter().cloned()))
    {
        let key = title.trim().to_lowercase();
        if key.is_empty() {
            continue;
        }
        if let Some(canonical) = touched.get(&key) {
            if seen.insert(key) {
                citations.push(canonical.clone());
            }
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
