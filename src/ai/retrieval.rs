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
//! - **Tools** ([`RETRIEVAL_TOOLS`]): [`READ_CHAPTER`], [`SEARCH_MANUSCRIPT`],
//!   [`READ_BIBLE`], [`LIST_CHAPTERS`]. Each returns a `tool_result` the model acts on.
//! - **Compaction.** As accumulated excerpts approach the window, the oldest raw
//!   chapter/search dumps are replaced with short notes (the bible seed and the
//!   running excerpts' citations are kept), so a long sweep stays within the window
//!   rather than overflowing.
//! - **Turn cap.** [`RETRIEVAL_MAX_TURNS`] (default 24, tunable) — larger than the
//!   assistant's [`MAX_TURNS`] of 8, since slow-but-correct is acceptable offline. On
//!   hitting the cap the loop still **answers with what it gathered** and discloses
//!   incompleteness; it never errors out or returns nothing.
//!
//! ## The `search:manuscript` seam (#831)
//!
//! [`ManuscriptSearch`] is an **interface** — `query → ranked locations + jumpable
//! citations` — so a future embedding-backed impl (#831) slots behind it with no
//! change to the loop. This module ships a [`LexicalSearch`] impl (no new model
//! class), independently deliverable today.

use std::collections::BTreeSet;
use std::fmt::Write as _;

use serde_json::{json, Value};

use crate::ai::embed::Chapter;
use crate::ai::tool_loop::{
    self, build_body, tool_result_message, tool_use_message, Converse, ToolCall, Turn,
};
use crate::ai::{Answer, Coverage, Response, Usage, QA_SYSTEM_RETRIEVAL};

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

/// The character budget the accumulated excerpts stay under before [`compact`] fires
/// (#856). Sized against the on-device window
/// ([`STANDARD_CONTEXT_TOKENS`](crate::ai::model_select::STANDARD_CONTEXT_TOKENS) tokens ≈
/// 4 chars/token, the same ratio the cost estimate uses), leaving generous headroom
/// for the bible seed, the tool catalog, the question, and the reply. A retrieval
/// sweep that keeps reading is summarized down to here rather than overflowing.
pub const fn excerpt_char_budget(window_tokens: u32) -> usize {
    // ~4 chars/token; reserve ~40% of the window for seed + catalog + question + reply.
    (window_tokens as usize) * 4 * 3 / 5
}

/// One location a [`ManuscriptSearch`] turned up: the chapter it's in (a jumpable
/// citation, exactly like a whole-book Ask citation) and the surrounding excerpt.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchLocation {
    /// The chapter title — a jumpable citation.
    pub chapter: String,
    /// The excerpt around the match, for the model to read in the tool result.
    pub excerpt: String,
}

/// The `search:manuscript` **interface** (#831): `query → ranked locations with
/// jumpable citations`. The retrieval loop calls only this trait, so a future
/// embedding-backed impl (semantic recall, paraphrase/synonym matches) slots in with
/// no change to the loop. This module ships a [`LexicalSearch`] impl; #831 provides a
/// semantic one behind the same seam. Read-only: it reads prose to locate + cite it,
/// never writes it.
pub trait ManuscriptSearch {
    /// The chapters this search knows about, in manuscript order — the corpus the
    /// loop lists and reads from. Titles are the citations.
    fn chapters(&self) -> &[Chapter];

    /// Locate `query` in the manuscript, most-relevant first, capped at `limit`. Each
    /// location carries its chapter (a jumpable citation) and a surrounding excerpt.
    fn search(&self, query: &str, limit: usize) -> Vec<SearchLocation>;
}

/// The lexical `search:manuscript` implementation shipped today (#856): a
/// case-insensitive substring scan over the chapter bodies, returning the chapters
/// (most matches first) with an excerpt around the first hit. No new model class —
/// independently deliverable, and #831 slots a semantic impl behind the same
/// [`ManuscriptSearch`] interface with no change to the loop. Owns its corpus so it
/// can be moved into the worker thread.
pub struct LexicalSearch {
    chapters: Vec<Chapter>,
}

impl LexicalSearch {
    /// Build a lexical search over `chapters` (the active book, assembled on the UI
    /// thread and moved in).
    pub const fn new(chapters: Vec<Chapter>) -> Self {
        Self { chapters }
    }
}

/// How many characters of context to show around a lexical match — enough to judge
/// the hit without dumping the chapter (that's what [`READ_CHAPTER`] is for).
const EXCERPT_RADIUS: usize = 160;

/// A window of `body` around the byte offset `at`, clamped to char boundaries and
/// trimmed, with ellipses where it was cut. Used to render a search excerpt.
fn excerpt_around(body: &str, at: usize, radius: usize) -> String {
    let start = body[..at]
        .char_indices()
        .rev()
        .nth(radius)
        .map_or(0, |(i, _)| i);
    let end = body[at..]
        .char_indices()
        .nth(radius)
        .map_or(body.len(), |(i, _)| at + i);
    let mut out = String::new();
    if start > 0 {
        out.push('…');
    }
    out.push_str(body[start..end].trim());
    if end < body.len() {
        out.push('…');
    }
    out
}

impl ManuscriptSearch for LexicalSearch {
    fn chapters(&self) -> &[Chapter] {
        &self.chapters
    }

    fn search(&self, query: &str, limit: usize) -> Vec<SearchLocation> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Vec::new();
        }
        // (chapter index, match count, first-match byte offset).
        let mut hits: Vec<(usize, usize, usize)> = Vec::new();
        for (i, ch) in self.chapters.iter().enumerate() {
            let hay = ch.body.to_lowercase();
            let count = hay.matches(&needle).count();
            if count > 0 {
                let at = hay.find(&needle).unwrap_or(0);
                hits.push((i, count, at));
            }
        }
        // Most matches first; ties keep manuscript order (stable sort on index).
        hits.sort_by(|a, b| b.1.cmp(&a.1));
        hits.truncate(limit);
        hits.into_iter()
            .map(|(i, _, at)| {
                let ch = &self.chapters[i];
                SearchLocation {
                    chapter: ch.title.clone(),
                    excerpt: excerpt_around(&ch.body, at, EXCERPT_RADIUS),
                }
            })
            .collect()
    }
}

/// The compact story-bible seed + a bible-section reader (#856): the global index the
/// loop opens with, and the source [`READ_BIBLE`] serves. Sections are named
/// (`characters`, `setting`, …) with their rendered text; a short seed lists the
/// section names + a preview so the model knows what it can read. Assembled on the UI
/// thread (it reads the bible files) and moved into the worker.
#[derive(Debug, Clone, Default)]
pub struct BibleSeed {
    /// `(section name, full text)`, in a stable order. Empty when no bible exists.
    sections: Vec<(String, String)>,
}

impl BibleSeed {
    /// Build a seed from named sections (drops empty ones).
    pub fn new(sections: Vec<(String, String)>) -> Self {
        Self {
            sections: sections
                .into_iter()
                .filter(|(_, text)| !text.trim().is_empty())
                .collect(),
        }
    }

    /// Whether any bible section exists — when false the loop is search-only and the
    /// answer discloses the weaker coverage (#856 acceptance criteria).
    pub const fn is_empty(&self) -> bool {
        self.sections.is_empty()
    }

    /// The opening index block: the section names and a short preview of each, so the
    /// model knows what global structure it can pull in via [`READ_BIBLE`]. A brief
    /// note when no bible exists.
    fn overview(&self) -> String {
        if self.sections.is_empty() {
            return "No story bible has been built yet — answer from manuscript search \
                    and chapter reads alone, and note that coverage may be partial."
                .to_string();
        }
        let mut out = String::from("Story bible sections available via read:bible:\n");
        for (name, text) in &self.sections {
            let preview: String = text.chars().take(200).collect();
            let _ = writeln!(out, "- {name}: {}", preview.replace('\n', " ").trim());
        }
        out
    }

    /// The full text of a named bible section, or `None` — the [`READ_BIBLE`] backend.
    fn section(&self, name: &str) -> Option<&str> {
        let name = name.trim().to_lowercase();
        self.sections
            .iter()
            .find(|(n, _)| n.to_lowercase() == name)
            .map(|(_, text)| text.as_str())
    }
}

/// One accumulated tool result, kept so it can be compacted later (#856).
struct Excerpt {
    /// The tool that produced it (for the compaction note).
    tool: String,
    /// The lookup input.
    argument: String,
    /// The raw result text (chapter prose, search excerpts, bible section).
    body: String,
    /// The chapters this excerpt drew from — the citations it contributes, kept even
    /// after the raw body is compacted away.
    citations: Vec<String>,
}

/// A read-only tool executor over the corpus (#856): resolves a [`ToolCall`] to its
/// result text and the chapters it cited. The closed set of lookups — the code-layer
/// guarantee that the loop can only read, never write (the boundary, enforced).
struct Tools<'a, S: ManuscriptSearch> {
    search: &'a S,
    bible: &'a BibleSeed,
}

impl<S: ManuscriptSearch> Tools<'_, S> {
    /// Run one tool call, returning `(result text, cited chapters)`. An unknown tool
    /// or a missing target returns a plain note (never an error that kills the loop).
    fn run(&self, name: &str, argument: &str) -> (String, Vec<String>) {
        match name {
            READ_CHAPTER => self
                .search
                .chapters()
                .iter()
                .find(|c| c.title.eq_ignore_ascii_case(argument.trim()))
                .map_or_else(
                    || (format!("No chapter titled \"{argument}\"."), Vec::new()),
                    |ch| (ch.body.clone(), vec![ch.title.clone()]),
                ),
            SEARCH_MANUSCRIPT => {
                let hits = self.search.search(argument, SEARCH_LIMIT);
                if hits.is_empty() {
                    return (format!("No matches for \"{argument}\"."), Vec::new());
                }
                let mut out = String::new();
                let mut cites = Vec::new();
                for h in &hits {
                    let _ = writeln!(out, "[{}] {}", h.chapter, h.excerpt);
                    cites.push(h.chapter.clone());
                }
                (out, cites)
            }
            READ_BIBLE => self.bible.section(argument).map_or_else(
                || {
                    (
                        format!("No bible section \"{argument}\". {}", self.bible.overview()),
                        Vec::new(),
                    )
                },
                |text| (text.to_string(), Vec::new()),
            ),
            LIST_CHAPTERS => {
                let list = self
                    .search
                    .chapters()
                    .iter()
                    .map(|c| format!("- {}", c.title))
                    .collect::<Vec<_>>()
                    .join("\n");
                (list, Vec::new())
            }
            other => (format!("Unknown tool \"{other}\"."), Vec::new()),
        }
    }
}

/// How many search locations one `search:manuscript` call returns.
const SEARCH_LIMIT: usize = 6;

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
    let total_chapters = search.chapters().len();

    // The opening context: the bible seed as a global index + the chapter list, then
    // the question — NEVER the whole manuscript (the #856 key difference from the
    // assistant pane, which caches the whole book).
    let mut messages: Vec<Value> = vec![seed_message(bible, search, question)];
    let mut excerpts: Vec<Excerpt> = Vec::new();
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
            Turn::Final(text) => return Ok(finish(&text, &excerpts, bible, total_chapters, false)),
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
        Ok(Turn::Final(text)) => Ok(finish(&text, &excerpts, bible, total_chapters, hit_cap)),
        _ => Ok(finish("", &excerpts, bible, total_chapters, true)),
    }
}

/// Run a batch of tool calls, appending the model's `tool_use` turn and the results
/// (both the wire `tool_result`s and the retained [`Excerpt`]s) to the running state.
fn run_calls<S: ManuscriptSearch>(
    tools: &Tools<'_, S>,
    calls: &[ToolCall],
    messages: &mut Vec<Value>,
    excerpts: &mut Vec<Excerpt>,
) {
    messages.push(tool_use_message(calls));
    let mut results = Vec::new();
    for call in calls {
        let argument = call.input["argument"].as_str().unwrap_or("").to_string();
        let (body, citations) = tools.run(&call.name, &argument);
        results.push(tool_loop::tool_result_block(&call.id, &body));
        excerpts.push(Excerpt {
            tool: call.name.clone(),
            argument,
            body,
            citations,
        });
    }
    messages.push(tool_result_message(results));
}

/// The opening seed message (#856): the bible overview as the global index and the
/// chapter list as the map, then the question. This is the whole starting context —
/// no manuscript prose, so it fits the window.
fn seed_message<S: ManuscriptSearch>(bible: &BibleSeed, search: &S, question: &str) -> Value {
    let chapters = search
        .chapters()
        .iter()
        .map(|c| format!("- {}", c.title))
        .collect::<Vec<_>>()
        .join("\n");
    let seed = format!(
        "{}\n\nChapters (read with read:chapter, search with search:manuscript):\n{}\n\n\
         Question: {}",
        bible.overview(),
        chapters,
        question.trim(),
    );
    json!({ "role": "user", "content": [ { "type": "text", "text": seed } ] })
}

/// Compact the running context when the accumulated excerpts exceed `budget` (#856):
/// replace the OLDEST raw tool results with a one-line note (tool + argument + the
/// chapters it cited), keeping their citations, until the retained bodies fit. The
/// bible seed (message 0) and the newest excerpts stay verbatim, so recent detail and
/// the global index survive while an old chapter dump shrinks to its signal.
fn compact(messages: &mut Vec<Value>, excerpts: &mut [Excerpt], budget: usize) {
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
fn finish(
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ai::model_select::STANDARD_CONTEXT_TOKENS;
    use crate::ai::tool_loop::MAX_TURNS;
    use crate::ai::AiError;
    use std::cell::RefCell;

    fn chapter(title: &str, body: &str) -> Chapter {
        Chapter {
            title: title.to_string(),
            file: format!("{title}.md"),
            body: body.to_string(),
        }
    }

    /// A big corpus that would blow the on-device window if sent whole — the
    /// oversized-offline case the loop exists for.
    fn oversized_corpus() -> Vec<Chapter> {
        (1..=40)
            .map(|i| {
                let filler = "Mara walked the quiet road. ".repeat(200);
                let body = if i == 7 {
                    format!("{filler}\nMara pressed the silver locket to her chest.\n{filler}")
                } else if i == 12 {
                    format!("{filler}\nShe finally spoke of her father, long gone.\n{filler}")
                } else {
                    filler
                };
                chapter(&format!("Chapter {i}"), &body)
            })
            .collect()
    }

    fn bible_with_sections() -> BibleSeed {
        BibleSeed::new(vec![
            (
                "characters".to_string(),
                "Mara — protagonist; carries a silver locket introduced in Chapter 7.".to_string(),
            ),
            (
                "timeline".to_string(),
                "Ch.7: the locket. Ch.12: Mara speaks of her father.".to_string(),
            ),
        ])
    }

    /// A mock [`Converse`] that replays a canned script of turns, so a test drives the
    /// loop with no live model, no network, no keychain (project rule #531).
    struct MockConverse {
        script: RefCell<Vec<Turn>>,
        /// Every request body the loop sent, for assertions on seed / compaction.
        bodies: RefCell<Vec<Value>>,
    }

    impl MockConverse {
        fn new(script: Vec<Turn>) -> Self {
            Self {
                script: RefCell::new(script.into_iter().rev().collect()),
                bodies: RefCell::new(Vec::new()),
            }
        }
    }

    impl Converse for MockConverse {
        fn respond(&self, body: &Value) -> Result<Turn, AiError> {
            self.bodies.borrow_mut().push(body.clone());
            self.script
                .borrow_mut()
                .pop()
                .ok_or_else(|| AiError::ApiMalformed("mock script exhausted".into()))
        }
    }

    fn call(name: &str, argument: &str) -> Turn {
        Turn::ToolCalls(vec![ToolCall {
            id: format!("t-{name}"),
            name: name.to_string(),
            input: json!({ "argument": argument }),
        }])
    }

    #[test]
    fn loop_reads_tools_then_answers_with_citations_offline_oversized() {
        // The model searches, reads the chapter it found, then answers — over a corpus
        // far too big to send whole to the 40 960-token local window.
        let corpus = oversized_corpus();
        let search = LexicalSearch::new(corpus);
        let bible = bible_with_sections();
        let mock = MockConverse::new(vec![
            call(SEARCH_MANUSCRIPT, "locket"),
            call(READ_CHAPTER, "Chapter 7"),
            Turn::Final(
                r#"{"answer":"The locket is introduced in Chapter 7.","citations":["Chapter 7"]}"#
                    .to_string(),
            ),
        ]);
        let answer = run_retrieval(
            &mock,
            &search,
            &bible,
            "Where is the locket introduced?",
            RETRIEVAL_MAX_TURNS,
            STANDARD_CONTEXT_TOKENS,
        )
        .unwrap();
        assert!(answer.answer.contains("Chapter 7"));
        assert!(
            answer.citations.contains(&"Chapter 7".to_string()),
            "cites the chapter it actually read: {:?}",
            answer.citations
        );
        // Read-only disclosure so a holistic answer stays hedged (#856): it read 1 of
        // the 40 chapters, so the disclosure says so rather than reading as exhaustive.
        assert!(
            answer.answer.contains("1 of 40 chapters read"),
            "discloses it did not read the whole manuscript: {}",
            answer.answer
        );
    }

    #[test]
    fn citations_point_at_the_chapters_actually_read() {
        // The model reads Ch.7 and Ch.12; even if it forgets to list a citation, the
        // gathered reads supply it — a citation always points at a touched chapter.
        let search = LexicalSearch::new(oversized_corpus());
        let bible = bible_with_sections();
        let mock = MockConverse::new(vec![
            call(READ_CHAPTER, "Chapter 7"),
            call(READ_CHAPTER, "Chapter 12"),
            // Answer omits citations entirely — the loop backfills from the reads.
            Turn::Final(r#"{"answer":"Yes; see the two chapters.","citations":[]}"#.to_string()),
        ]);
        let answer = run_retrieval(
            &mock,
            &search,
            &bible,
            "Does Mara mention her father and carry a locket?",
            RETRIEVAL_MAX_TURNS,
            STANDARD_CONTEXT_TOKENS,
        )
        .unwrap();
        assert_eq!(
            answer.citations,
            vec!["Chapter 7".to_string(), "Chapter 12".to_string()],
            "citations are exactly the chapters read, in order"
        );
    }

    #[test]
    fn loop_compacts_when_excerpts_grow() {
        // Reading many big chapters overflows the excerpt budget, so old raw dumps are
        // compacted to notes — the running context stays bounded rather than growing
        // unboundedly with every read.
        let search = LexicalSearch::new(oversized_corpus());
        let bible = bible_with_sections();
        let reads: Vec<Turn> = (1..=8)
            .map(|i| call(READ_CHAPTER, &format!("Chapter {i}")))
            .collect();
        let mut script = reads;
        script.push(Turn::Final(
            r#"{"answer":"Done.","citations":[]}"#.to_string(),
        ));
        let answer = run_retrieval(
            &MockConverse::new(script),
            &search,
            &bible,
            "Summarize the opening.",
            RETRIEVAL_MAX_TURNS,
            // A small window forces compaction after a couple of big reads.
            2048,
        )
        .unwrap();
        // The gathered reads still contribute their citations even after compaction.
        assert!(
            answer.citations.contains(&"Chapter 1".to_string()),
            "an early, compacted read still cites its chapter: {:?}",
            answer.citations
        );
        // Directly assert compaction bounded the retained bodies.
        let mut messages = vec![super::seed_message(&bible, &search, "q")];
        let mut excerpts: Vec<Excerpt> = (1..=8)
            .map(|i| Excerpt {
                tool: READ_CHAPTER.to_string(),
                argument: format!("Chapter {i}"),
                body: "x".repeat(4000),
                citations: vec![format!("Chapter {i}")],
            })
            .collect();
        compact(&mut messages, &mut excerpts, 4096);
        let retained: usize = excerpts.iter().map(|e| e.body.len()).sum();
        assert!(
            retained <= 4096 + 4000,
            "compaction bounds retained bodies near the budget (kept newest verbatim): {retained}"
        );
        assert!(
            excerpts[0].body.contains("earlier read:chapter"),
            "the oldest excerpt is compacted to a note"
        );
        assert!(
            excerpts[0].body.contains("Chapter 1"),
            "the note keeps the citation"
        );
    }

    #[test]
    fn a_bible_answerable_question_does_not_over_read() {
        // A question the bible seed already answers is closed on the FIRST turn with no
        // chapter reads — the loop doesn't over-read when the global index suffices.
        let search = LexicalSearch::new(oversized_corpus());
        let bible = bible_with_sections();
        let mock = MockConverse::new(vec![Turn::Final(
            r#"{"answer":"Mara is the protagonist.","citations":[]}"#.to_string(),
        )]);
        let answer = run_retrieval(
            &mock,
            &search,
            &bible,
            "Who is the protagonist?",
            RETRIEVAL_MAX_TURNS,
            STANDARD_CONTEXT_TOKENS,
        )
        .unwrap();
        assert!(answer.answer.contains("Mara"));
        // Exactly one model call — no tool round-trips.
        assert_eq!(
            mock.bodies.borrow().len(),
            1,
            "answered from the seed alone"
        );
    }

    #[test]
    fn no_bible_still_runs_search_only_and_discloses() {
        // With no story bible the loop still runs (search-only) and the answer discloses
        // the weaker coverage (#856 acceptance criteria).
        let search = LexicalSearch::new(oversized_corpus());
        let bible = BibleSeed::default();
        assert!(bible.is_empty());
        let mock = MockConverse::new(vec![
            call(SEARCH_MANUSCRIPT, "locket"),
            Turn::Final(r#"{"answer":"Chapter 7.","citations":["Chapter 7"]}"#.to_string()),
        ]);
        let answer = run_retrieval(
            &mock,
            &search,
            &bible,
            "Where is the locket?",
            RETRIEVAL_MAX_TURNS,
            STANDARD_CONTEXT_TOKENS,
        )
        .unwrap();
        assert!(
            answer.answer.contains("no story bible"),
            "discloses the missing bible: {}",
            answer.answer
        );
    }

    #[test]
    fn turn_cap_still_answers_and_discloses_incomplete() {
        // A model that keeps calling tools forever is stopped at the cap, but the loop
        // still answers with what it gathered and discloses incompleteness — never
        // errors out or returns nothing (#856).
        let search = LexicalSearch::new(oversized_corpus());
        let bible = bible_with_sections();
        // The final "best answer" request (empty tools) also returns a tool call, so
        // the loop must synthesize the disclosed fallback.
        let script: Vec<Turn> = (0..RETRIEVAL_MAX_TURNS + 1)
            .map(|_| call(READ_CHAPTER, "Chapter 7"))
            .collect();
        let mock = MockConverse::new(script);
        let answer = run_retrieval(
            &mock,
            &search,
            &bible,
            "Trace every thread.",
            RETRIEVAL_MAX_TURNS,
            STANDARD_CONTEXT_TOKENS,
        )
        .unwrap();
        assert!(
            answer.answer.contains("incomplete"),
            "discloses the cap was hit: {}",
            answer.answer
        );
        assert!(
            answer.citations.contains(&"Chapter 7".to_string()),
            "still cites what it gathered"
        );
    }

    #[test]
    fn answer_is_never_manuscript_prose_only_answer_and_citations() {
        // The output is an Answer (analysis) + chapter-title citations — never a chunk
        // of manuscript prose. Even if the model echoes prose, the loop returns the
        // structured answer, and citations are titles, not bodies.
        let search = LexicalSearch::new(oversized_corpus());
        let bible = bible_with_sections();
        let mock = MockConverse::new(vec![
            call(READ_CHAPTER, "Chapter 7"),
            Turn::Final(
                r#"{"answer":"The locket appears in Chapter 7.","citations":["Chapter 7"]}"#
                    .to_string(),
            ),
        ]);
        let answer = run_retrieval(
            &mock,
            &search,
            &bible,
            "Where is the locket?",
            RETRIEVAL_MAX_TURNS,
            STANDARD_CONTEXT_TOKENS,
        )
        .unwrap();
        // Citations are chapter titles, not prose bodies.
        for c in &answer.citations {
            assert!(
                !c.contains("Mara walked the quiet road"),
                "a citation is a title, not prose"
            );
        }
        // The answer is the analysis + disclosure, not the chapter's raw filler.
        assert!(!answer
            .answer
            .contains(&"Mara walked the quiet road. ".repeat(10)));
    }

    #[test]
    fn coverage_metadata_records_the_read_set_and_a_coverage_sense() {
        // #1056: the answer carries structured coverage — WHICH chapters it read and
        // HOW COMPLETE that was — as metadata, asserted separately from the prose.
        let search = LexicalSearch::new(oversized_corpus());
        let bible = bible_with_sections();
        let mock = MockConverse::new(vec![
            call(READ_CHAPTER, "Chapter 7"),
            call(READ_CHAPTER, "Chapter 12"),
            Turn::Final(
                r#"{"answer":"Both threads appear.","citations":["Chapter 7","Chapter 12"]}"#
                    .to_string(),
            ),
        ]);
        let answer = run_retrieval(
            &mock,
            &search,
            &bible,
            "Trace the locket and the father thread.",
            RETRIEVAL_MAX_TURNS,
            STANDARD_CONTEXT_TOKENS,
        )
        .unwrap();
        let coverage = answer
            .coverage
            .as_ref()
            .expect("offline answer has coverage");
        // The read-set: exactly the two chapters it read, in order.
        assert_eq!(
            coverage.chapters_read,
            vec!["Chapter 7".to_string(), "Chapter 12".to_string()],
            "coverage records the chapters actually read"
        );
        // The coverage sense: 2 of 40, a bible was present, the cap wasn't hit.
        assert_eq!(coverage.chapters_total, 40, "the whole corpus size");
        assert!(!coverage.no_bible);
        assert!(!coverage.cap_reached);
        // Metadata, not prose: assert the coverage independently of the answer text.
        assert!(coverage.is_hedged(), "2 of 40 is a partial read, so hedged");
    }

    #[test]
    fn a_holistic_partial_read_is_hedged_in_metadata_and_disclosure() {
        // A holistic question the loop can't fully cover (it read a handful of 40
        // chapters) is hedged accordingly — both in the coverage metadata and in the
        // derived, prose-independent disclosure (#1056).
        let search = LexicalSearch::new(oversized_corpus());
        let bible = bible_with_sections();
        let mock = MockConverse::new(vec![
            call(READ_CHAPTER, "Chapter 7"),
            Turn::Final(
                r#"{"answer":"The book is broadly about a journey.","citations":["Chapter 7"]}"#
                    .to_string(),
            ),
        ]);
        let answer = run_retrieval(
            &mock,
            &search,
            &bible,
            "What is the book about, overall?",
            RETRIEVAL_MAX_TURNS,
            STANDARD_CONTEXT_TOKENS,
        )
        .unwrap();
        let coverage = answer.coverage.as_ref().unwrap();
        assert!(
            coverage.chapters_read.len() < coverage.chapters_total,
            "read fewer than all chapters"
        );
        assert!(coverage.is_hedged(), "a partial read is hedged");
        // The disclosure is derived metadata, appended after the prose — it discloses
        // the fraction read and does NOT alter the answer prose itself.
        assert!(
            answer.answer.contains("1 of 40 chapters read"),
            "discloses the coverage fraction: {}",
            answer.answer
        );
        assert!(
            answer.answer.contains("not the whole manuscript"),
            "hedges a holistic answer: {}",
            answer.answer
        );
    }

    #[test]
    fn disclosure_is_metadata_and_never_alters_the_answer_prose() {
        // #1056: the coverage disclosure is metadata about the read — it's appended
        // after the model's prose and never rewrites it. The answer prose the model
        // produced survives verbatim ahead of the disclosure.
        let search = LexicalSearch::new(oversized_corpus());
        let bible = bible_with_sections();
        let prose = "The locket is introduced in Chapter 7.";
        let mock = MockConverse::new(vec![
            call(READ_CHAPTER, "Chapter 7"),
            Turn::Final(format!(
                r#"{{"answer":"{prose}","citations":["Chapter 7"]}}"#
            )),
        ]);
        let answer = run_retrieval(
            &mock,
            &search,
            &bible,
            "Where is the locket introduced?",
            RETRIEVAL_MAX_TURNS,
            STANDARD_CONTEXT_TOKENS,
        )
        .unwrap();
        // The model's prose is intact and leads the answer; the disclosure follows it.
        assert!(
            answer.answer.starts_with(prose),
            "the answer prose is unaltered: {}",
            answer.answer
        );
        // The coverage read-set matches the disclosure fraction — one source of truth.
        let coverage = answer.coverage.as_ref().unwrap();
        assert_eq!(coverage.chapters_read, vec!["Chapter 7".to_string()]);
        let (read, total) = (coverage.chapters_read.len(), coverage.chapters_total);
        assert!(answer
            .answer
            .contains(&format!("{read} of {total} chapters read")));
    }

    #[test]
    fn a_well_covered_answer_is_not_spuriously_hedged() {
        // When the loop reads EVERY chapter of a small book, with a bible and no cap,
        // coverage is complete — the answer is not hedged (#1056). Guards against a
        // spurious "may be incomplete" on an answer that genuinely covered the book.
        let small = vec![
            chapter("One", "Mara found the locket by the shore."),
            chapter("Two", "She spoke of her father at last."),
        ];
        let search = LexicalSearch::new(small);
        let bible = bible_with_sections();
        let mock = MockConverse::new(vec![
            call(READ_CHAPTER, "One"),
            call(READ_CHAPTER, "Two"),
            Turn::Final(
                r#"{"answer":"The locket and the father both appear.","citations":["One","Two"]}"#
                    .to_string(),
            ),
        ]);
        let answer = run_retrieval(
            &mock,
            &search,
            &bible,
            "Do the locket and the father both appear?",
            RETRIEVAL_MAX_TURNS,
            STANDARD_CONTEXT_TOKENS,
        )
        .unwrap();
        let coverage = answer.coverage.as_ref().unwrap();
        assert_eq!(
            coverage.chapters_read.len(),
            coverage.chapters_total,
            "read all"
        );
        assert!(!coverage.is_hedged(), "a full read is not hedged");
        assert!(
            answer.answer.contains("full read of the manuscript"),
            "an unhedged answer says so plainly: {}",
            answer.answer
        );
        assert!(
            !answer.answer.contains("may be partial") && !answer.answer.contains("incomplete"),
            "not spuriously hedged: {}",
            answer.answer
        );
    }

    #[test]
    fn coverage_survives_the_response_round_trip() {
        // The offline answer is serialized to a Response and re-parsed by the shared
        // Q&A dispatch (#856); the coverage metadata must survive that round-trip so
        // the presenter can surface it (#1056).
        let search = LexicalSearch::new(oversized_corpus());
        let bible = bible_with_sections();
        let mock = MockConverse::new(vec![
            call(READ_CHAPTER, "Chapter 7"),
            Turn::Final(r#"{"answer":"Chapter 7.","citations":["Chapter 7"]}"#.to_string()),
        ]);
        let answer = run_retrieval(
            &mock,
            &search,
            &bible,
            "Where is the locket?",
            RETRIEVAL_MAX_TURNS,
            STANDARD_CONTEXT_TOKENS,
        )
        .unwrap();
        let response = answer_response(&answer);
        let reparsed = crate::ai::parse_answer(&response.text);
        assert_eq!(
            reparsed.coverage, answer.coverage,
            "coverage survives serialize → parse_answer"
        );
    }

    #[test]
    fn search_manuscript_interface_is_exercised_via_the_lexical_impl() {
        // The `search:manuscript` seam (#831) is driven through the lexical impl:
        // query → ranked locations with jumpable chapter citations.
        let search = LexicalSearch::new(oversized_corpus());
        let hits = search.search("locket", SEARCH_LIMIT);
        assert!(!hits.is_empty(), "the lexical impl finds the term");
        assert_eq!(hits[0].chapter, "Chapter 7", "cites the chapter it's in");
        assert!(
            hits[0].excerpt.contains("locket"),
            "the excerpt shows the match"
        );
        // An empty query yields nothing (no panic).
        assert!(search.search("   ", SEARCH_LIMIT).is_empty());
    }

    #[test]
    fn retrieval_cap_is_larger_than_the_assistant_cap() {
        // The retrieval loop gets a higher cap than the assistant's 8, tunable, but the
        // shared assistant cap is unchanged (#856).
        const { assert!(RETRIEVAL_MAX_TURNS > MAX_TURNS) };
        assert_eq!(MAX_TURNS, 8, "the assistant cap stays 8");
    }

    #[test]
    fn retrieval_tools_are_read_only_lookups_with_an_argument() {
        let tools = retrieval_tools();
        let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
        assert_eq!(
            names,
            vec![READ_CHAPTER, SEARCH_MANUSCRIPT, READ_BIBLE, LIST_CHAPTERS]
        );
        // No tool writes prose — the catalog IS the allowlist (the boundary in code).
        for t in &tools {
            let name = t["name"].as_str().unwrap();
            assert!(
                name.starts_with("read:")
                    || name.starts_with("search:")
                    || name.starts_with("list:"),
                "every tool is a read-only lookup: {name}"
            );
            assert_eq!(
                t["input_schema"]["properties"]["argument"]["type"],
                "string"
            );
        }
    }
}
