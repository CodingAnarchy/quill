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

/// The byte offset in `haystack` of the first case-insensitive occurrence of
/// `needle` (which the caller has already lowercased and trimmed), valid for slicing
/// `haystack` itself.
///
/// Why not just `self.lowered[i].find(&needle)` and reuse that offset? Because
/// [`str::to_lowercase`] is **not length-preserving** — `İ` (U+0130) lowercases to two
/// chars / more bytes, `ẞ` and various Turkic/German forms shift byte lengths too — so
/// an offset into the *lowercased* string is misaligned into the *original*: slicing
/// the original with it can land mid-codepoint (a `char boundary` panic) or out of
/// bounds. We instead scan char boundaries of `haystack` and, at each, lowercase just
/// the following text far enough to test the prefix, returning an offset that indexes
/// `haystack` correctly. Matching stays case-insensitive; ASCII text is unaffected.
fn find_ci(haystack: &str, needle: &str) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    // Fast path: pure-ASCII haystacks lowercase byte-for-byte, so the lowercased
    // offset is already valid for the original — no per-position lowercasing needed.
    if haystack.is_ascii() {
        return haystack.to_ascii_lowercase().find(needle);
    }
    // A candidate matches at byte `i` when the lowercase of `haystack[i..]` begins with
    // `needle`. `to_lowercase` can lengthen text (each char grows by at most a small
    // constant factor), so at each position lowercase the tail lazily and compare
    // prefixes on the fly — stopping as soon as it diverges — rather than pre-slicing a
    // guessed length. This can never under-fold the tail and is O(needle) per position.
    for (i, _) in haystack.char_indices() {
        if starts_with_lowercased(&haystack[i..], needle) {
            return Some(i);
        }
    }
    None
}

/// True when lowercasing `haystack` yields a string that starts with `needle` (which is
/// already lowercased). Lowercases `haystack` character by character, comparing against
/// `needle` as it goes and stopping at the first divergence — so it never allocates the
/// whole tail and correctly handles non-length-preserving lowercase (a single source
/// char can lowercase to several needle chars).
fn starts_with_lowercased(haystack: &str, needle: &str) -> bool {
    let mut rest = needle;
    for lc in haystack.chars().flat_map(char::to_lowercase) {
        match rest.strip_prefix(lc) {
            Some(remainder) => rest = remainder,
            None => return false,
        }
        if rest.is_empty() {
            return true;
        }
    }
    rest.is_empty()
}

/// The largest byte index `<= index` that is a char boundary of `s` (i.e. `index`
/// floored to the nearest boundary at or below it). Lets a rough offset be snapped to
/// a valid slice point so no slice ever lands mid-codepoint.
const fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        return s.len();
    }
    let mut i = index;
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// A window of `body` around the byte offset `at`, clamped to char boundaries and
/// trimmed, with ellipses where it was cut. Used to render a search excerpt. `at` is
/// snapped to a char boundary first, so even a rough offset can never slice
/// mid-codepoint.
pub(super) fn excerpt_around(body: &str, at: usize, radius: usize) -> String {
    let at = floor_char_boundary(body, at);
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
        // (chapter index, match count, first-match byte offset IN ch.body). Detection
        // and the match count run against the precomputed lowercased bodies so repeated
        // searches don't re-lowercase the corpus. The excerpt offset, however, must
        // index `ch.body` — NOT `self.lowered[i]` — because `to_lowercase` is not
        // length-preserving, so `find` on the lowercased string yields an offset that
        // can be misaligned into the original (slicing it would panic). `find_ci`
        // returns an offset valid for `ch.body` itself.
        let mut hits: Vec<(usize, usize, usize)> = Vec::new();
        for (i, hay) in self.lowered.iter().enumerate() {
            let count = hay.matches(&needle).count();
            if count > 0 {
                let at = find_ci(&self.chapters[i].body, &needle).unwrap_or(0);
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
