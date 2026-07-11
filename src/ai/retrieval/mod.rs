//! Offline "Ask the manuscript" via a bible-grounded agentic **read-loop** (#856).
//!
//! Cloud Q&A stuffs the whole novel into one cached prompt ([`build_qa_request`]),
//! which fits Claude's 1M window. The on-device model holds only ~40 960 tokens
//! ([`STANDARD_CONTEXT_TOKENS`](crate::ai::model_select::STANDARD_CONTEXT_TOKENS)),
//! so a whole book overflows it — the offline-oversized
//! branch #849 could previously only *refuse*. This module answers instead: seed the
//! model with the compact **story bible** as a global index, then let it call
//! read-only lookup tools — read a chapter, lexically search the manuscript, read a
//! bible file — accumulating excerpts (with a compaction step and a higher turn cap)
//! until it can answer, then return the answer WITH CHAPTER CITATIONS.
//!
//! ## Within the AI boundary (non-negotiable — see `CLAUDE.md`)
//!
//! Every tool is a **read-only lookup**. The output is an
//! [`Answer`](crate::ai::Answer) *about* the manuscript plus its chapter citations —
//! it NEVER drafts, rewrites, or produces manuscript prose. This changes *how context
//! is gathered*, not *what the AI produces*: Q&A already answers questions; this only
//! makes it work offline at scale. The executor is a closed allowlist of read tools,
//! so — exactly like the assistant pane's two-layer guard (#308) — the loop has no
//! capability to write prose even if the model asked.
//!
//! ## The loop
//!
//! - **Seed.** The opening context is `bible seed + chapter list`, never the whole
//!   manuscript (it wouldn't fit — the key difference from the assistant pane, which
//!   caches the whole book as its first block). The bible carries the diffuse, global
//!   structure naive top-k retrieval drops, and its entries already cite chapters,
//!   giving the model its first read targets. With no bible, the loop still runs
//!   (search-only) and the answer discloses the weaker coverage.
//! - **Tools** ([`retrieval_tools`]): [`READ_CHAPTER`], [`SEARCH_MANUSCRIPT`],
//!   [`READ_BIBLE`], [`LIST_CHAPTERS`]. Each returns a `tool_result` the model acts on.
//! - **Compaction.** As accumulated excerpts approach the window, the oldest raw
//!   chapter/search dumps are replaced with short notes (the bible seed and the
//!   running excerpts' citations are kept), so a long sweep stays within the window
//!   rather than overflowing.
//! - **Turn cap.** [`RETRIEVAL_MAX_TURNS`] (default 24, tunable) — larger than the
//!   assistant's [`MAX_TURNS`](tool_loop::MAX_TURNS) of 8, since slow-but-correct is
//!   acceptable offline. On hitting the cap the loop still **answers with what it
//!   gathered** and discloses incompleteness; it never errors out or returns nothing.
//!
//! ## The `search:manuscript` seam (#831)
//!
//! [`ManuscriptSearch`] is an **interface** — `query → ranked locations + jumpable
//! citations` — so a future embedding-backed impl (#831) slots behind it with no
//! change to the loop. This module ships a [`LexicalSearch`] impl (no new model
//! class), independently deliverable today.
//!
//! ## Module layout (#968-style split)
//!
//! Split by concern, each file well under the ~1,000-line convention, with the public
//! surface re-exported here so `crate::ai::retrieval::…` callers are unaffected:
//! - [`search`]: the [`ManuscriptSearch`] seam, [`SearchLocation`], the
//!   [`LexicalSearch`] impl (with its precomputed lowercased-body cache), and the
//!   excerpt renderer.
//! - [`seed`]: [`BibleSeed`], the retained `Excerpt`, and the opening `seed_message`.
//! - [`tools`]: the read-only `Tools` executor, `run_calls`, and the search limit.
//! - [`answer`]: `compact`, `finish`, the coverage disclosure, and [`answer_response`].
//! - the orchestrator [`run_retrieval`] and the tool catalog / budgets live here.

use serde_json::{json, Value};

use crate::ai::tool_loop::{build_body, Converse, Turn};
use crate::ai::{Answer, QA_SYSTEM_RETRIEVAL};

mod answer;
mod search;
mod seed;
mod tools;

#[cfg(test)]
mod tests;

pub use answer::answer_response;
// `SearchLocation` is the public return type of `ManuscriptSearch::search`; it's part
// of the module's public surface even though no in-crate caller names it directly.
#[allow(
    unused_imports,
    reason = "public API surface — the ManuscriptSearch::search return type"
)]
pub use search::SearchLocation;
pub use search::{LexicalSearch, ManuscriptSearch};
pub use seed::BibleSeed;

use answer::{compact, finish};
use seed::seed_message;
use tools::{run_calls, Tools};

/// The tool the model calls to read one chapter's full prose by title.
pub const READ_CHAPTER: &str = "read:chapter";
/// The `search:manuscript` tool (#831): a lexical (or, later, semantic) search that
/// returns jumpable chapter locations for a query — the seam #831 slots behind.
pub const SEARCH_MANUSCRIPT: &str = "search:manuscript";
/// The tool the model calls to read one story-bible section by name.
pub const READ_BIBLE: &str = "read:bible";
/// The tool the model calls to list the chapters (titles + any bible-known metadata).
pub const LIST_CHAPTERS: &str = "list:chapters";

/// The read-only lookup tools the retrieval loop offers, in the shape the constrained
/// choice / Messages body expects (`{name, description, input_schema}`). Every tool
/// takes a single `argument` string (a chapter title, a query, a bible section) —
/// matching the constrained-choice `argument` the local backend already carries
/// (#390), so no change to the choice shape is needed. `list:chapters` ignores its
/// argument. NONE writes prose: the catalog is the allowlist.
pub fn retrieval_tools() -> Vec<Value> {
    [
        (READ_CHAPTER, "read one chapter's prose by its exact title"),
        (
            SEARCH_MANUSCRIPT,
            "lexically search the manuscript for a phrase; returns the chapters and \
             surrounding excerpts where it occurs",
        ),
        (
            READ_BIBLE,
            "read one story-bible section by name (e.g. characters, setting, timeline, plot)",
        ),
        (
            LIST_CHAPTERS,
            "list the manuscript's chapter titles in order",
        ),
    ]
    .into_iter()
    .map(|(name, desc)| {
        json!({
            "name": name,
            "description": desc,
            "input_schema": {
                "type": "object",
                "properties": {
                    "argument": { "type": "string", "description": "the lookup input" },
                },
            },
        })
    })
    .collect()
}

/// The turn cap for the offline retrieval loop (#856) — deliberately larger than the
/// assistant's [`MAX_TURNS`](tool_loop::MAX_TURNS) of 8, since each on-device turn is
/// free and slow-but-correct is the accepted trade for an answer that otherwise can't
/// exist offline. Tunable via `[ai] retrieval_max_turns`; this is the default.
pub const RETRIEVAL_MAX_TURNS: usize = 24;

/// The character budget the accumulated excerpts stay under before `compact` fires
/// (#856). Sized against the on-device window
/// ([`STANDARD_CONTEXT_TOKENS`](crate::ai::model_select::STANDARD_CONTEXT_TOKENS) tokens ≈
/// 4 chars/token, the same ratio the cost estimate uses), leaving generous headroom
/// for the bible seed, the tool catalog, the question, and the reply. A retrieval
/// sweep that keeps reading is summarized down to here rather than overflowing.
pub const fn excerpt_char_budget(window_tokens: u32) -> usize {
    // ~4 chars/token; reserve ~40% of the window for seed + catalog + question + reply.
    (window_tokens as usize) * 4 * 3 / 5
}

/// The maximum output tokens for a retrieval turn (a choice, then the final answer).
const RETRIEVAL_MAX_OUTPUT_TOKENS: u32 = 1024;

/// Drive the offline bible-grounded read-loop for `question` and return a grounded
/// [`Answer`] with chapter citations (#856).
///
/// `converse` is the transport ([`Converse`]) — the local backend natively, a mock in
/// tests. `search` is the manuscript corpus behind the [`ManuscriptSearch`] seam (a
/// [`LexicalSearch`] today, a semantic impl under #831 tomorrow). `bible` is the
/// compact global-index seed. `max_turns` is the turn cap ([`RETRIEVAL_MAX_TURNS`] by
/// default) and `window_tokens` the on-device window the excerpts are compacted
/// against.
///
/// The answer is assembled from excerpts, so it **discloses** it did not read the
/// whole manuscript (holistic answers stay hedged), and — on hitting the turn cap —
/// discloses coverage was incomplete rather than erroring. Read-only throughout: the
/// output is analysis + citations, never prose written into the book.
pub fn run_retrieval<C: Converse + ?Sized, S: ManuscriptSearch>(
    converse: &C,
    search: &S,
    bible: &BibleSeed,
    question: &str,
    max_turns: usize,
    window_tokens: u32,
) -> Result<Answer, crate::ai::AiError> {
    let tools = Tools { search, bible };
    let tool_defs = retrieval_tools();
    let budget = excerpt_char_budget(window_tokens);
    // The real chapter set: its size is the coverage denominator. `finish` validates
    // the model's citations against the chapters actually read so a hallucinated
    // citation can't inflate coverage into a false "full read" (#qa-citation-coverage).
    let chapters = search.chapters();

    // The opening context: the bible seed as a global index + the chapter list, then
    // the question — NEVER the whole manuscript (the #856 key difference from the
    // assistant pane, which caches the whole book).
    let mut messages: Vec<Value> = vec![seed_message(bible, search, question)];
    let mut excerpts: Vec<seed::Excerpt> = Vec::new();
    let mut hit_cap = false;

    for turn in 0..max_turns {
        let body = build_body(
            "auto",
            Some(QA_SYSTEM_RETRIEVAL),
            &messages,
            &tool_defs,
            RETRIEVAL_MAX_OUTPUT_TOKENS,
        );
        match converse.respond(&body)? {
            Turn::Final(text) => return Ok(finish(&text, &excerpts, bible, chapters, false)),
            Turn::ToolCalls(calls) if turn + 1 >= max_turns => {
                // The next turn would exceed the cap: run this batch's reads so their
                // signal isn't lost, then answer with what we gathered (#856: never
                // error out or return nothing on the cap).
                run_calls(&tools, &calls, &mut messages, &mut excerpts);
                hit_cap = true;
                break;
            }
            Turn::ToolCalls(calls) => {
                run_calls(&tools, &calls, &mut messages, &mut excerpts);
                compact(&mut messages, &mut excerpts, budget);
            }
        }
    }

    // Hit the cap without a final answer — ask once more for the best answer from what
    // was gathered, and if even that doesn't come back, synthesize a disclosed one.
    let body = build_body(
        "auto",
        Some(QA_SYSTEM_RETRIEVAL),
        &messages,
        &[],
        RETRIEVAL_MAX_OUTPUT_TOKENS,
    );
    match converse.respond(&body) {
        Ok(Turn::Final(text)) => Ok(finish(&text, &excerpts, bible, chapters, hit_cap)),
        _ => Ok(finish("", &excerpts, bible, chapters, true)),
    }
}
