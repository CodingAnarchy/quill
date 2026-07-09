//! Chunking and retrieval breadth (#831, #931): splitting a chapter's prose into
//! fixed-width passages at paragraph boundaries, and the [`Breadth`] knob that
//! scales the passage width and top-K per question. Pure and hermetically
//! testable — no model, no candle — so it runs in the normal suite (#531).

use serde::{Deserialize, Serialize};

use super::Passage;

/// The maximum characters a single passage carries at [`Breadth::Normal`], the
/// default. Chapters are split into passages at paragraph boundaries and packed up
/// to this width, so each embedded unit is a coherent span the embedder's context
/// window can hold and a citation points at a real, jumpable chapter. Small enough
/// that the vector captures one idea, large enough that a passage isn't a fragment.
/// [`Breadth`] scales this per question.
pub(super) const PASSAGE_CHARS: usize = 1_200;

/// The retrieval breadth a grounded-Ask question requests (#931): how coarsely the
/// manuscript is chunked and how many passages the answer is grounded in. One
/// question wants the *arc* of a thread (broad, few large spans); another wants a
/// *specific* word or fact (fine, many small spans). Breadth trades passage size
/// against passage count so both read well, and it's a property of the built index
/// (a breadth change re-chunks, like an embedder-model change), so the vectors
/// always match the granularity the answer is asked at.
///
/// [`Normal`](Breadth::Normal) is the default and reproduces the pre-#931 behavior
/// exactly ([`PASSAGE_CHARS`] wide, the caller's base top-K).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Breadth {
    /// Finer detail: smaller passages, more of them retrieved — for a question that
    /// hinges on a specific word, line, or fact.
    Fine,
    /// The default: unchanged from before #931 — [`PASSAGE_CHARS`]-wide passages and
    /// the caller's base top-K.
    #[default]
    Normal,
    /// Broader context: larger passages, fewer of them retrieved — for a question
    /// about the arc or shape of a thread across scenes.
    Broad,
}

impl Breadth {
    /// The passage width this breadth chunks at. Fine is a fraction of
    /// [`PASSAGE_CHARS`] (tighter spans, finer citations); Broad a multiple (whole
    /// scenes in one span); Normal is [`PASSAGE_CHARS`] unchanged.
    pub const fn passage_chars(self) -> usize {
        match self {
            Self::Fine => PASSAGE_CHARS / 2,
            Self::Normal => PASSAGE_CHARS,
            Self::Broad => PASSAGE_CHARS * 3,
        }
    }

    /// How many passages to retrieve, scaled from the caller's `base` top-K so the
    /// *amount of prose* fed to the answer stays comparable across breadths: Fine
    /// retrieves more (its passages are smaller), Broad fewer (its passages are
    /// larger), Normal the base unchanged. Always at least one when `base > 0`.
    pub const fn top_k(self, base: usize) -> usize {
        match self {
            Self::Fine => base * 2,
            Self::Normal => base,
            // Halve, but never below 1 when the caller asked for any.
            Self::Broad if base == 0 => 0,
            Self::Broad => {
                let k = base / 2;
                if k == 0 {
                    1
                } else {
                    k
                }
            }
        }
    }
}

/// Split a chapter's prose into passages of at most `breadth`'s width
/// ([`Breadth::passage_chars`]), breaking at paragraph boundaries (blank lines) so
/// each passage is a coherent span. A paragraph longer than the cap becomes its own
/// passage (rather than being split mid-sentence) — the embedder truncates to its
/// window if need be. Empty/whitespace chapters yield no passages. A broader breadth
/// packs more paragraphs per passage (coarser scene context); a finer one fewer
/// (tighter detail).
pub fn chunk_chapter(title: &str, file: &str, body: &str, breadth: Breadth) -> Vec<Passage> {
    let cap = breadth.passage_chars();
    let mut out = Vec::new();
    let mut current = String::new();
    let mut flush = |buf: &mut String| {
        let trimmed = buf.trim();
        if !trimmed.is_empty() {
            out.push(Passage {
                title: title.to_string(),
                file: file.to_string(),
                text: trimmed.to_string(),
            });
        }
        buf.clear();
    };
    for para in body.split("\n\n") {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }
        // Adding this paragraph would overflow the passage → flush what we have and
        // start fresh (unless the current passage is empty, so a single oversized
        // paragraph still lands in its own passage rather than being dropped).
        if !current.is_empty() && current.len() + para.len() + 2 > cap {
            flush(&mut current);
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(para);
        if current.len() >= cap {
            flush(&mut current);
        }
    }
    flush(&mut current);
    out
}
