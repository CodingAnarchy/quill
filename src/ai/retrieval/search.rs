//! The `search:manuscript` seam (#831) and its lexical impl (#856): the
//! [`ManuscriptSearch`] interface (`query → ranked locations + jumpable
//! citations`), the [`LexicalSearch`] impl shipped today, and the
//! [`excerpt_around`] excerpt renderer. A future embedding-backed impl slots
//! behind [`ManuscriptSearch`] with no change to the loop.

use crate::ai::embed::Chapter;

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
///
/// The lowercased chapter bodies are precomputed once at construction and reused on
/// every `search:manuscript` call, so a retrieval loop that searches many times
/// doesn't re-lowercase the whole corpus each time (the case-insensitive match runs
/// against the cache, not `body.to_lowercase()` per call).
pub struct LexicalSearch {
    chapters: Vec<Chapter>,
    /// `chapters[i].body.to_lowercase()`, precomputed once so repeated searches
    /// don't re-lowercase the corpus. Parallel to `chapters` by index.
    lowered: Vec<String>,
}

impl LexicalSearch {
    /// Build a lexical search over `chapters` (the active book, assembled on the UI
    /// thread and moved in). The lowercased bodies are computed once here so every
    /// later `search` matches against the cache rather than re-lowercasing.
    pub fn new(chapters: Vec<Chapter>) -> Self {
        let lowered = chapters.iter().map(|c| c.body.to_lowercase()).collect();
        Self { chapters, lowered }
    }
}

/// How many characters of context to show around a lexical match — enough to judge
/// the hit without dumping the chapter (that's what
/// [`READ_CHAPTER`](crate::ai::retrieval::READ_CHAPTER) is for).
const EXCERPT_RADIUS: usize = 160;

/// A window of `body` around the byte offset `at`, clamped to char boundaries and
/// trimmed, with ellipses where it was cut. Used to render a search excerpt.
pub(super) fn excerpt_around(body: &str, at: usize, radius: usize) -> String {
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
        // (chapter index, match count, first-match byte offset). Match against the
        // precomputed lowercased bodies so repeated searches don't re-lowercase.
        let mut hits: Vec<(usize, usize, usize)> = Vec::new();
        for (i, hay) in self.lowered.iter().enumerate() {
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
