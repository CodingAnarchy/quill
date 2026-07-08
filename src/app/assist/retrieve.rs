//! Semantic retrieval for grounded-Ask (#831): the app-side glue between the
//! embedding index ([`crate::ai::embed`]) and the "Ask the manuscript" flow.
//!
//! Assembles the active book's chapters into the index's input (UI thread), decides
//! whether a usable embedding model is on disk, and — off the worker thread —
//! refreshes the persisted index and retrieves the passages a question pulls up,
//! rendering them into the same manuscript-shaped context [`ai::build_qa_request`]
//! consumes. When no embedding model is available (no `local` feature, or its
//! weights aren't downloaded), retrieval returns `None` and Ask falls back to the
//! whole-book path exactly as before — graceful degradation, no new Ask fork.
//!
//! Everything here reads prose to retrieve and cite it; it never writes prose,
//! staying inside Quill's AI boundary.

use super::super::*;

use crate::ai::embed::{Breadth, Chapter};
#[cfg(feature = "local")]
use crate::ai::embed::{Index, LocalEmbedder};
use crate::ai::model_select::EmbedModel;

/// How many retrieved passages feed the grounded request (#831). Enough to cover a
/// question that spans a few chapters, small enough to stay well under any model's
/// window — the whole point of retrieval over whole-book context. Only the
/// `local` retrieval path reads it (the fallback build ignores retrieval).
#[cfg_attr(not(feature = "local"), allow(dead_code))]
pub(super) const ASK_TOP_K: usize = 12;

impl App {
    /// The active book's chapters as the retrieval index's input (#831): each
    /// chapter's title, file, and live body (the active chapter from the buffer, the
    /// rest from disk — the same source [`chapter_text`](App::chapter_text) uses).
    /// Assembled on the UI thread, then moved into the worker closure.
    pub(crate) fn ask_index_chapters(&self) -> Vec<Chapter> {
        let book = self.project.active_book();
        self.project
            .book_chapters(book)
            .iter()
            .map(|c| {
                let body = self
                    .project
                    .index_of_file(&c.file)
                    .map_or_else(String::new, |i| self.chapter_text(i));
                Chapter {
                    title: c.title.clone(),
                    file: c.file.clone(),
                    body,
                }
            })
            .collect()
    }

    /// The embedding model grounded-Ask should use, if one is usable **right now**
    /// (#831): the configured `[ai] embed_model` (or the default), but only when its
    /// weights are already on disk — retrieval never triggers an implicit download,
    /// so a machine without the model just uses the whole-book Ask path. `None`
    /// without the `local` feature (nothing on-device can embed).
    pub(crate) fn ask_embed_model(&self) -> Option<&'static EmbedModel> {
        let model = if self.ai.embed_model.is_empty() {
            EmbedModel::default_model()
        } else {
            EmbedModel::by_id(&self.ai.embed_model)?
        };
        worker::embed_model_downloaded(model).then_some(model)
    }
}

/// How many prior passages a retrieval-backed incremental pass folds into its
/// context (#929). A handful — enough to carry the scenes a changed chapter bears
/// on (where a character/place last appeared, a rule was set) without re-sending a
/// meaningful fraction of the book, which would defeat retrieval's point of
/// focusing the pass. Only the `local` retrieval path reads it.
#[cfg_attr(not(feature = "local"), allow(dead_code))]
pub(in crate::app) const CONTEXT_TOP_K: usize = 8;

/// Refresh the project's embedding index against `chapters` at `breadth` and
/// retrieve the top passages for `question`, returned as the manuscript-shaped
/// context [`ai::build_qa_request`] feeds (#831, #931). `breadth` sets both the
/// passage granularity the index is (re-)chunked at and the retrieved count
/// ([`Breadth::top_k`] over [`ASK_TOP_K`]): [`Breadth::Broad`] grounds the answer in
/// fewer, larger scene-spans; [`Breadth::Fine`] in more, tighter ones;
/// [`Breadth::Normal`] is the pre-#931 behavior unchanged. Runs on the worker thread
/// (embedding is CPU/GPU-bound). `None` — so the caller falls back to sending the
/// whole book — when the index yields no hits, or on any embedding/model error
/// (retrieval is a best-effort optimization, never a hard failure of Ask).
///
/// Without the `local` feature there's no on-device embedder, so this is always
/// `None` and Ask is unchanged. Reads prose to retrieve + cite it; never writes it.
#[cfg(feature = "local")]
pub(super) fn retrieve_context(
    root: &std::path::Path,
    chapters: &[Chapter],
    model: &'static EmbedModel,
    question: &str,
    breadth: Breadth,
) -> Option<String> {
    let embedder = LocalEmbedder::for_model(model).ok()?;
    let mut index = Index::load(root);
    // Re-embed only the chapters whose content changed since the last refresh (#90),
    // re-chunking everything if the breadth changed since the last ask (#931); an
    // unchanged manuscript at the same breadth re-embeds nothing.
    index.refresh(chapters, breadth, &embedder).ok()?;
    // Persist so the next ask reuses the vectors (best effort — a write failure
    // doesn't block the retrieval that already succeeded in memory).
    let _ = index.save(root);
    let hits = index
        .search(question, breadth.top_k(ASK_TOP_K), &embedder)
        .ok()?;
    if hits.is_empty() {
        return None;
    }
    Some(ai::retrieved_context(&hits))
}

/// Retrieve the prior passages a changed chapter bears on, as the reference block a
/// retrieval-backed incremental bible pass folds into its context (#929). Refreshes
/// the semantic index against the whole book (so it's current, #90), then ranks the
/// prior passages against `query` (the changed chapters' prose) while excluding the
/// `changed` chapters themselves — they're already sent to the pass in full — and
/// renders the top hits under their chapter titles ([`ai::retrieved_context`]), so
/// each stays a jumpable citation. Runs on the worker thread (embedding is
/// CPU/GPU-bound).
///
/// `None` — so the pass degrades to the plain incremental path, sending only the
/// changed chapters — when no usable embedding model is on disk, the index turns up
/// no prior hits, or any embedding/model error occurs. Retrieval is a best-effort
/// sharpening of the pass, never a hard failure of it. Reads prose to retrieve +
/// cite it; never writes it — inside the AI boundary.
#[cfg(feature = "local")]
pub(in crate::app) fn retrieve_prior_context(
    root: &std::path::Path,
    chapters: &[Chapter],
    model: &'static EmbedModel,
    changed: &[String],
    query: &str,
) -> Option<String> {
    let embedder = LocalEmbedder::for_model(model).ok()?;
    let mut index = Index::load(root);
    // The incremental pass retrieves at the default granularity (#929); breadth is a
    // grounded-Ask control (#931).
    index.refresh(chapters, Breadth::Normal, &embedder).ok()?;
    let _ = index.save(root);
    let hits = index
        .relevant_prior(query, changed, CONTEXT_TOP_K, &embedder)
        .ok()?;
    if hits.is_empty() {
        return None;
    }
    Some(ai::retrieved_context(&hits))
}

/// Without the `local` feature there's no on-device embedder — retrieval is
/// unavailable and Ask always uses the whole-book path (#831).
#[cfg(not(feature = "local"))]
pub(super) const fn retrieve_context(
    _root: &std::path::Path,
    _chapters: &[Chapter],
    _model: &'static EmbedModel,
    _question: &str,
    _breadth: Breadth,
) -> Option<String> {
    None
}

/// Without the `local` feature there's no on-device embedder — a bible pass always
/// uses the plain incremental (changed-chapters-only) context (#929).
#[cfg(not(feature = "local"))]
pub(in crate::app) const fn retrieve_prior_context(
    _root: &std::path::Path,
    _chapters: &[Chapter],
    _model: &'static EmbedModel,
    _changed: &[String],
    _query: &str,
) -> Option<String> {
    None
}
