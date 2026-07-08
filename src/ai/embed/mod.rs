//! On-device semantic retrieval for grounded-Ask (#831): a local text-embedding
//! index over the manuscript's chapters, exposed as a `search(query) → ranked
//! passages + jumpable citations` primitive.
//!
//! "Ask the manuscript" (#609) sends the whole active book as context and leans on
//! the model to cite; that caps Ask at one context window and only cites as well as
//! full-context recall allows. This module indexes chapters into fixed-width
//! embedding vectors, so a question can *retrieve* the most relevant passages
//! (ranked by cosine similarity) and feed only those — with their chapter
//! citations — to the Q&A request. That scales past a single book's window and
//! catches paraphrase/synonym matches lexical search misses. It **only ever reads**
//! prose to retrieve and cite it; it never writes, drafts, or edits — squarely
//! inside Quill's AI boundary.
//!
//! The design is split so the retrieval logic is **hermetically testable** without
//! a model (#531): the [`Embedder`] trait is the seam, chunking / cosine ranking /
//! index storage / [`Index::search`] are pure and exercised with an injected fake
//! embedder, and only [`LocalEmbedder`] (behind `#[cfg(feature = "local")]`) pulls
//! in candle. When no embedder is available the caller falls back to the whole-book
//! Ask path unchanged.
//!
//! The index is content-addressed like the bible's incremental state (#90): each
//! chapter's passages are keyed to its content digest ([`crate::ai::incremental`]),
//! so a refresh re-embeds only the chapters whose text changed and the index can't
//! drift from the manuscript.
//!
//! The retrieval core (chunking, ranking, index storage) is always compiled so the
//! hermetic unit tests exercise it in the normal suite (#531), but its only *non*-
//! test caller — the Ask retrieval step in [`crate::app::assist`] — is behind
//! `#[cfg(feature = "local")]`, since retrieval needs the candle embedder. So in a
//! default (no-`local`) non-test build every item is genuinely unreachable; the
//! module-level allow below scopes the dead-code silence to exactly that build,
//! rather than deleting logic the tests and the `local` build both need (#406).
#![cfg_attr(not(feature = "local"), allow(dead_code))]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::incremental::{digest, Digests};
use super::AiError;

/// The maximum characters a single passage carries at [`Breadth::Normal`], the
/// default. Chapters are split into passages at paragraph boundaries and packed up
/// to this width, so each embedded unit is a coherent span the embedder's context
/// window can hold and a citation points at a real, jumpable chapter. Small enough
/// that the vector captures one idea, large enough that a passage isn't a fragment.
/// [`Breadth`] scales this per question.
const PASSAGE_CHARS: usize = 1_200;

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

/// A retrievable span of a chapter (#831): the chapter it came from (title + file,
/// so a hit stays a jumpable citation exactly like today's Ask) and the prose the
/// vector was built from. The `file` keys the index's per-chapter content digest;
/// the `title` is what surfaces as the citation and what the Q&A request cites.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Passage {
    /// The cited chapter's title — the jumpable citation (matches `Finding.chapters`).
    pub title: String,
    /// The chapter's file, the index's per-chapter digest key.
    pub file: String,
    /// The passage prose — what was embedded and what feeds the grounded request.
    pub text: String,
}

/// A passage plus its embedding vector and the digest of the chapter it belongs to.
/// Persisted in the [`Index`]; the digest lets a refresh drop a changed chapter's
/// stale passages and re-embed only what moved.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct IndexedPassage {
    passage: Passage,
    /// The chapter's content digest when this passage was embedded.
    digest: String,
    /// The (L2-normalized) embedding vector.
    vector: Vec<f32>,
}

/// A ranked retrieval hit: a passage and its cosine similarity to the query, in
/// `[-1, 1]` (higher is more relevant). The `search:manuscript` primitive returns
/// these newest-relevance-first; the caller turns them into grounded context +
/// jumpable citations.
#[derive(Clone, Debug, PartialEq)]
pub struct Hit {
    pub passage: Passage,
    pub score: f32,
}

/// A near-duplicate / echo pair (#920): two distinct passages whose vectors are
/// close, with their cosine similarity. The echo-detection pass surfaces these as
/// findings — each pair carries *both* jumpable citations and the score, so the
/// author can compare the two spans and decide whether the repetition is a
/// mistake or a deliberate reprise. Observation only: it flags the pair, never
/// edits either passage.
#[derive(Clone, Debug, PartialEq)]
pub struct EchoPair {
    pub a: Passage,
    pub b: Passage,
    pub score: f32,
}

/// The seam between the index and a model: turn texts into fixed-width embedding
/// vectors. `LocalEmbedder` is the candle implementation; tests inject a fake so
/// the whole retrieval path runs with no model download (#531).
///
/// An embedder **only reads** text to vectorize it — it never generates prose, so
/// it's inside the AI boundary by construction.
pub trait Embedder {
    /// Embed each text into a vector. The returned vectors should be L2-normalized
    /// (so a dot product is cosine similarity); [`Index`] normalizes defensively
    /// regardless. One vector per input, in order.
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AiError>;

    /// A stable tag identifying this embedder's model, stored in the index so a
    /// model swap invalidates a stale index built by a different embedder.
    fn model_tag(&self) -> String;
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

/// L2-normalize a vector in place so a dot product is cosine similarity. A
/// zero-norm vector (all zeros) is left as-is (its similarity to anything is 0).
fn normalize(v: &mut [f32]) {
    let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm > f32::EPSILON {
        for x in v.iter_mut() {
            *x /= norm;
        }
    }
}

/// Cosine similarity of two vectors assumed L2-normalized (a plain dot product).
/// Mismatched lengths score 0 (a defensive guard; the index keeps widths uniform).
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() {
        return 0.0;
    }
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

/// One chapter's live text as the index sees it: its title, file, and body. The app
/// assembles these from the active book so the index never holds more than it needs.
#[derive(Clone, Debug)]
pub struct Chapter {
    pub title: String,
    pub file: String,
    pub body: String,
}

/// The semantic retrieval index (#831): the manuscript's chapters chunked into
/// passages, each with its embedding vector and the content digest of its chapter.
/// Persisted per project; refreshed incrementally against the current chapter
/// digests so only changed chapters are re-embedded (#90), and answers a
/// `search(query) → ranked passages + jumpable citations` query.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Index {
    /// The embedder model tag the index was built with; a mismatch forces a rebuild.
    #[serde(default)]
    model_tag: String,
    /// The [`Breadth`] the passages were chunked at (#931); a change forces a
    /// re-chunk, since a different-width passage set isn't comparable. Absent in a
    /// pre-#931 index → [`Breadth::Normal`], the width those passages were built at,
    /// so an old index isn't needlessly rebuilt.
    #[serde(default)]
    breadth: Breadth,
    /// Every indexed passage, across all chapters.
    #[serde(default)]
    passages: Vec<IndexedPassage>,
}

impl Index {
    /// The per-chapter digests this index currently reflects (`file → digest`), so a
    /// caller can [`incremental::decide`](super::incremental::decide) whether a
    /// refresh is even needed. One entry per file that has at least one passage.
    pub fn digests(&self) -> Digests {
        let mut out: Digests = BTreeMap::new();
        for p in &self.passages {
            out.insert(p.passage.file.clone(), p.digest.clone());
        }
        out
    }

    /// The number of indexed passages — a status/test accessor. `#[cfg(test)]`: only
    /// the hermetic tests read it; production drives the index through `refresh` /
    /// `search`, so gating it keeps the `local` build off the dead-code radar (#406).
    #[cfg(test)]
    pub const fn passage_count(&self) -> usize {
        self.passages.len()
    }

    /// Refresh the index against `chapters` using `embedder`, chunked at `breadth`
    /// (#931), re-embedding only what changed (#90): a chapter whose digest already
    /// matches keeps its passages untouched; a changed/new chapter is re-chunked and
    /// re-embedded; a chapter no longer present is dropped. A changed embedder model
    /// (tag mismatch) *or a changed breadth* rebuilds everything — the stored vectors
    /// were built at a different granularity and aren't comparable. Returns how many
    /// chapters were (re-)embedded, so the caller can report the cost. The embedder is
    /// called **once** for all changed passages, so a big refresh is one batched
    /// inference.
    pub fn refresh(
        &mut self,
        chapters: &[Chapter],
        breadth: Breadth,
        embedder: &dyn Embedder,
    ) -> Result<usize, AiError> {
        let tag = embedder.model_tag();
        if self.model_tag != tag || self.breadth != breadth {
            // A different embedder model or breadth → the old vectors aren't
            // comparable (different model, or different chunk granularity); rebuild.
            self.passages.clear();
            self.model_tag = tag;
            self.breadth = breadth;
        }

        // Digest every current chapter; decide which are unchanged (keep) vs stale.
        let current: BTreeMap<&str, (String, &Chapter)> = chapters
            .iter()
            .map(|c| (c.file.as_str(), (digest(&c.body), c)))
            .collect();
        let prior = self.digests();

        // Keep the passages of chapters whose digest is unchanged and still present.
        let mut kept: Vec<IndexedPassage> = Vec::new();
        for p in &self.passages {
            if let Some((dig, _)) = current.get(p.passage.file.as_str()) {
                if *dig == p.digest {
                    kept.push(p.clone());
                }
            }
        }

        // Chunk every changed/new chapter; embed them all in one batch.
        let mut changed_chapters = 0usize;
        let mut new_passages: Vec<(Passage, String)> = Vec::new();
        for (file, (dig, chapter)) in &current {
            let unchanged = prior.get(*file) == Some(dig);
            if unchanged {
                continue;
            }
            changed_chapters += 1;
            for passage in chunk_chapter(&chapter.title, &chapter.file, &chapter.body, breadth) {
                new_passages.push((passage, dig.clone()));
            }
        }

        if !new_passages.is_empty() {
            let texts: Vec<String> = new_passages.iter().map(|(p, _)| p.text.clone()).collect();
            let vectors = embedder.embed(&texts)?;
            if vectors.len() != new_passages.len() {
                return Err(AiError::Internal(format!(
                    "embedder returned {} vectors for {} passages",
                    vectors.len(),
                    new_passages.len()
                )));
            }
            for ((passage, digest), mut vector) in new_passages.into_iter().zip(vectors) {
                normalize(&mut vector);
                kept.push(IndexedPassage {
                    passage,
                    digest,
                    vector,
                });
            }
        }

        self.passages = kept;
        Ok(changed_chapters)
    }

    /// The `search:manuscript` primitive (#831): rank the indexed passages by cosine
    /// similarity to `query` and return the top `top_k`, most-relevant first. Embeds
    /// the query with the same `embedder` the index was built with. An empty index or
    /// `top_k == 0` returns nothing (the caller then falls back to whole-book Ask).
    ///
    /// This is retrieval only — it reads prose to rank and cite it, never writes it.
    pub fn search(
        &self,
        query: &str,
        top_k: usize,
        embedder: &dyn Embedder,
    ) -> Result<Vec<Hit>, AiError> {
        if self.passages.is_empty() || top_k == 0 {
            return Ok(Vec::new());
        }
        let mut q = embedder
            .embed(std::slice::from_ref(&query.to_string()))?
            .into_iter()
            .next()
            .ok_or_else(|| AiError::Internal("embedder returned no query vector".into()))?;
        normalize(&mut q);
        Ok(self.rank(&q, top_k))
    }

    /// Rank against an already-embedded, normalized query vector — the pure core of
    /// [`search`](Self::search), split out so tests can drive ranking with an
    /// injected query vector and no embedder round-trip.
    pub fn rank(&self, query: &[f32], top_k: usize) -> Vec<Hit> {
        let mut hits: Vec<Hit> = self
            .passages
            .iter()
            .map(|p| Hit {
                passage: p.passage.clone(),
                score: cosine(query, &p.vector),
            })
            .collect();
        // Descending by score; a stable tie-break keeps runs reproducible.
        hits.sort_by(|a, b| {
            b.score
                .total_cmp(&a.score)
                .then_with(|| a.passage.file.cmp(&b.passage.file))
        });
        hits.truncate(top_k);
        hits
    }

    /// Semantic "find similar scenes" (#924): rank the indexed passages by cosine
    /// similarity to `query` and return the top `top_k`, but drop any passage whose
    /// prose is byte-identical to `query`. That exclusion is what turns the shared
    /// [`search`](Self::search) primitive into a *find-similar-elsewhere*: when the
    /// query is a passage the author selected, its own vector would otherwise
    /// dominate the ranking (a passage is maximally similar to itself), so a
    /// self-match is filtered out and the author sees the *other* passages that echo
    /// it. A typed query rarely matches a passage verbatim, so the filter is a no-op
    /// there and this behaves like plain search.
    ///
    /// Reads prose to rank + cite it, never writes it — squarely inside the AI
    /// boundary, like every other retrieval path here.
    pub fn similar(
        &self,
        query: &str,
        top_k: usize,
        embedder: &dyn Embedder,
    ) -> Result<Vec<Hit>, AiError> {
        if self.passages.is_empty() || top_k == 0 {
            return Ok(Vec::new());
        }
        let mut q = embedder
            .embed(std::slice::from_ref(&query.to_string()))?
            .into_iter()
            .next()
            .ok_or_else(|| AiError::Internal("embedder returned no query vector".into()))?;
        normalize(&mut q);
        // Rank over one extra slot, then drop the self-match, so excluding the
        // query's own passage still leaves `top_k` results when the index has them.
        let mut hits = self.rank(&q, top_k + 1);
        hits.retain(|h| h.passage.text != query);
        hits.truncate(top_k);
        Ok(hits)
    }

    /// Retrieve the prior passages most relevant to a changed chapter, for a
    /// retrieval-backed incremental pass (#929): rank the index against `query` (the
    /// changed chapter's prose) and return the top `top_k`, but **exclude every
    /// passage whose chapter is in `changed`**. Those changed chapters are already
    /// sent to the pass in full, so retrieval's job is to surface the *prior* scenes
    /// that bear on the change — where a character last appeared, the place a rule
    /// was established — that the incremental request would otherwise never see
    /// (today it sends only the changed chapters and a summarized bible). The hits
    /// keep their chapter titles, so each stays a jumpable citation exactly like the
    /// full-context path.
    ///
    /// `changed` is matched by `Passage::file` (the index's per-chapter key), so a
    /// changed chapter contributes nothing to its own retrieved context. An empty
    /// index or `top_k == 0` returns nothing (the caller then falls back to the
    /// whole-book context). Reads prose to retrieve + cite it, never writes it —
    /// inside the AI boundary like every retrieval path here.
    pub fn relevant_prior(
        &self,
        query: &str,
        changed: &[String],
        top_k: usize,
        embedder: &dyn Embedder,
    ) -> Result<Vec<Hit>, AiError> {
        if self.passages.is_empty() || top_k == 0 {
            return Ok(Vec::new());
        }
        let mut q = embedder
            .embed(std::slice::from_ref(&query.to_string()))?
            .into_iter()
            .next()
            .ok_or_else(|| AiError::Internal("embedder returned no query vector".into()))?;
        normalize(&mut q);
        Ok(self.rank_prior(&q, changed, top_k))
    }

    /// Rank the prior passages against an already-embedded, normalized query vector,
    /// dropping any passage from a `changed` chapter — the pure core of
    /// [`relevant_prior`](Self::relevant_prior), split out so tests can drive the
    /// selection with an injected query vector and no embedder round-trip (#531).
    /// Ranks over the full index, filters the changed chapters, then truncates, so
    /// excluding the changed chapters still leaves `top_k` prior hits when the index
    /// holds them.
    pub fn rank_prior(&self, query: &[f32], changed: &[String], top_k: usize) -> Vec<Hit> {
        let mut hits = self.rank(query, self.passages.len());
        hits.retain(|h| !changed.iter().any(|f| f == &h.passage.file));
        hits.truncate(top_k);
        hits
    }

    /// Echo / near-duplicate detection (#920): scan every pair of indexed passages
    /// and return those whose cosine similarity is at least `threshold`, as
    /// [`EchoPair`]s carrying both passages and the score. Highest-similarity pairs
    /// first (a stable file/text tie-break keeps runs reproducible). A pair of
    /// passages *within the same chapter* is skipped — a repeated refrain in one
    /// scene is usually deliberate, and the interesting echoes are cross-chapter;
    /// this also keeps a long chapter's adjacent passages from flooding the results.
    ///
    /// Pure over the stored (already-normalized) vectors — no embedder round-trip,
    /// so tests drive it with injected vectors. O(n²) in the passage count, run on
    /// the worker thread; the manuscript's passage count is small enough (a passage
    /// is ~1,200 chars) that this stays well under a frame for a whole book.
    ///
    /// Reads prose to compare + cite it, never writes it (within the AI boundary).
    pub fn echo_pairs(&self, threshold: f32) -> Vec<EchoPair> {
        let mut pairs = Vec::new();
        for (i, p) in self.passages.iter().enumerate() {
            for q in &self.passages[i + 1..] {
                // Cross-chapter only: a within-chapter repeat is usually intended,
                // and the echoes an author misses span scenes.
                if p.passage.file == q.passage.file {
                    continue;
                }
                let score = cosine(&p.vector, &q.vector);
                if score >= threshold {
                    pairs.push(EchoPair {
                        a: p.passage.clone(),
                        b: q.passage.clone(),
                        score,
                    });
                }
            }
        }
        pairs.sort_by(|x, y| {
            y.score
                .total_cmp(&x.score)
                .then_with(|| x.a.file.cmp(&y.a.file))
                .then_with(|| x.b.file.cmp(&y.b.file))
                .then_with(|| x.a.text.cmp(&y.a.text))
                .then_with(|| x.b.text.cmp(&y.b.text))
        });
        pairs
    }
}

// --- Persistence: the index lives next to the bible state -------------------

/// The index file for a project (`bible/.embed-index.json`), beside the bible's
/// incremental state (#90). JSON (not TOML) because the payload is mostly dense
/// float arrays.
fn index_path(root: &Path) -> PathBuf {
    root.join("bible").join(".embed-index.json")
}

impl Index {
    /// Load the persisted index, or the empty default when it's missing/unreadable
    /// (best effort — a corrupt file just means the next refresh rebuilds).
    pub fn load(root: &Path) -> Self {
        std::fs::read_to_string(index_path(root))
            .ok()
            .and_then(|t| serde_json::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Persist the index (creating `bible/`). Best effort — a write failure is
    /// returned for the caller to surface but never blocks the retrieval that
    /// already succeeded in memory.
    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        let path = index_path(root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = serde_json::to_string(self).map_err(std::io::Error::other)?;
        std::fs::write(path, body)
    }
}

#[cfg(feature = "local")]
pub use local::LocalEmbedder;

#[cfg(feature = "local")]
mod local;

#[cfg(test)]
mod tests {
    use super::*;

    /// A deterministic fake embedder for hermetic tests (#531): maps each text to a
    /// fixed vector by a keyword → axis scheme, so ranking is predictable with no
    /// model download. Each configured keyword owns one axis; a text's vector counts
    /// its keyword hits per axis, so a query and a passage sharing a keyword rank
    /// high and unrelated ones rank low.
    struct FakeEmbedder {
        keywords: Vec<&'static str>,
    }

    impl Embedder for FakeEmbedder {
        fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AiError> {
            Ok(texts
                .iter()
                .map(|t| {
                    let lower = t.to_lowercase();
                    self.keywords
                        .iter()
                        .map(|k| lower.matches(k).count() as f32)
                        .collect()
                })
                .collect())
        }
        fn model_tag(&self) -> String {
            "fake-v1".into()
        }
    }

    /// An embedder that counts how many texts it was asked to embed, so a test can
    /// prove a refresh re-embeds only the changed chapters (#90).
    struct Counting {
        calls: std::cell::Cell<usize>,
    }

    impl Embedder for Counting {
        fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AiError> {
            self.calls.set(self.calls.get() + texts.len());
            Ok(texts.iter().map(|t| vec![t.len() as f32]).collect())
        }
        fn model_tag(&self) -> String {
            "count-v1".into()
        }
    }

    /// A second embedder with a *different* model tag, so a test can prove swapping
    /// the embedding model invalidates the index and rebuilds it.
    struct Other;

    impl Embedder for Other {
        fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AiError> {
            Ok(texts.iter().map(|_| vec![1.0, 0.0]).collect())
        }
        fn model_tag(&self) -> String {
            "other-v1".into()
        }
    }

    fn chapter(title: &str, file: &str, body: &str) -> Chapter {
        Chapter {
            title: title.into(),
            file: file.into(),
            body: body.into(),
        }
    }

    #[test]
    fn chunk_splits_on_paragraphs_and_caps_length() {
        let body = "First paragraph.\n\nSecond paragraph.";
        let passages = chunk_chapter("One", "one.md", body, Breadth::Normal);
        // Both short paragraphs pack into a single passage.
        assert_eq!(passages.len(), 1);
        assert!(passages[0].text.contains("First") && passages[0].text.contains("Second"));
        assert_eq!(passages[0].title, "One");
        assert_eq!(passages[0].file, "one.md");

        // A body far over the cap yields multiple passages, none empty.
        let big = "word ".repeat(PASSAGE_CHARS); // ~5x the cap
        let paras = format!("{big}\n\n{big}\n\n{big}");
        let many = chunk_chapter("Big", "big.md", &paras, Breadth::Normal);
        assert!(many.len() >= 3, "oversized chapter splits: {}", many.len());
        assert!(many.iter().all(|p| !p.text.trim().is_empty()));
    }

    #[test]
    fn empty_chapter_yields_no_passages() {
        assert!(chunk_chapter("Empty", "e.md", "   \n\n  ", Breadth::Normal).is_empty());
    }

    #[test]
    fn breadth_controls_passage_granularity() {
        // Same body chunked at each breadth (#931): finer → more, smaller passages;
        // broader → fewer, larger ones; normal in between. A paragraph a bit under the
        // Fine cap, so several pack into one Normal/Broad passage but each fills a
        // Fine one.
        let para = "word ".repeat(Breadth::Fine.passage_chars() * 3 / 4 / 5);
        let body = format!("{para}\n\n{para}\n\n{para}\n\n{para}\n\n{para}\n\n{para}");

        let fine = chunk_chapter("C", "c.md", &body, Breadth::Fine);
        let normal = chunk_chapter("C", "c.md", &body, Breadth::Normal);
        let broad = chunk_chapter("C", "c.md", &body, Breadth::Broad);

        // Finer breadth splits into strictly more passages than broader breadth.
        assert!(
            fine.len() > normal.len(),
            "fine yields more passages than normal: {} vs {}",
            fine.len(),
            normal.len()
        );
        assert!(
            normal.len() > broad.len(),
            "normal yields more passages than broad: {} vs {}",
            normal.len(),
            broad.len()
        );
        // Every breadth still covers the whole body (no prose dropped) and each
        // passage honors its breadth's cap.
        for (b, passages) in [
            (Breadth::Fine, &fine),
            (Breadth::Normal, &normal),
            (Breadth::Broad, &broad),
        ] {
            assert!(!passages.is_empty());
            assert!(
                passages.iter().all(|p| p.text.len() <= b.passage_chars()),
                "passages fit the {b:?} cap"
            );
        }
    }

    #[test]
    fn breadth_scales_top_k_around_the_base() {
        // Broad retrieves fewer passages (they're larger), Fine more (smaller),
        // Normal the base unchanged — so the prose fed to the answer stays comparable.
        assert_eq!(
            Breadth::Normal.top_k(12),
            12,
            "normal is the base unchanged"
        );
        assert!(Breadth::Broad.top_k(12) < 12, "broad retrieves fewer");
        assert!(Breadth::Fine.top_k(12) > 12, "fine retrieves more");
        // Broad never drops to zero when the caller asked for any passages.
        assert_eq!(Breadth::Broad.top_k(1), 1, "broad keeps at least one");
        assert_eq!(Breadth::Broad.top_k(0), 0, "top_k(0) stays zero");
    }

    #[test]
    fn changing_breadth_rebuilds_the_index() {
        // A breadth change re-chunks the manuscript at the new granularity (#931),
        // exactly like an embedder-model swap — the stored vectors aren't comparable
        // across widths. One long chapter so Fine and Broad chunk it differently.
        let embedder = FakeEmbedder {
            keywords: vec!["a"],
        };
        let para = "alpha ".repeat(Breadth::Fine.passage_chars() / 6);
        let body = format!("{para}\n\n{para}\n\n{para}\n\n{para}");
        let chapters = vec![chapter("Long", "long.md", &body)];

        let mut index = Index::default();
        index.refresh(&chapters, Breadth::Broad, &embedder).unwrap();
        let broad_count = index.passage_count();

        // Re-refreshing at the same breadth over an unchanged manuscript is a no-op.
        let n = index.refresh(&chapters, Breadth::Broad, &embedder).unwrap();
        assert_eq!(
            n, 0,
            "same breadth + unchanged manuscript re-embeds nothing"
        );

        // A finer breadth rebuilds everything at the finer granularity.
        let n = index.refresh(&chapters, Breadth::Fine, &embedder).unwrap();
        assert_eq!(n, 1, "the breadth change re-embeds the chapter");
        assert!(
            index.passage_count() > broad_count,
            "finer breadth yields more passages: {} vs {broad_count}",
            index.passage_count()
        );
    }

    #[test]
    fn search_at_any_breadth_resolves_jumpable_citations() {
        // Retrieval at each breadth still ranks the right chapter first and keeps its
        // title/file, so the answer's citations stay jumpable exactly as today (#931).
        let embedder = FakeEmbedder {
            keywords: vec!["locket", "harbor"],
        };
        let chapters = vec![
            chapter("The Locket", "one.md", "She clutched the silver locket."),
            chapter("The Harbor", "two.md", "Ships crowded the busy harbor."),
        ];
        for breadth in [Breadth::Fine, Breadth::Normal, Breadth::Broad] {
            let mut index = Index::default();
            index.refresh(&chapters, breadth, &embedder).unwrap();
            let hits = index
                .search("where is the locket", breadth.top_k(4), &embedder)
                .unwrap();
            assert!(!hits.is_empty(), "{breadth:?} retrieves passages");
            assert_eq!(
                hits[0].passage.title, "The Locket",
                "{breadth:?} ranks the locket chapter first"
            );
            assert_eq!(
                hits[0].passage.file, "one.md",
                "{breadth:?} keeps the citation's file"
            );
        }
    }

    #[test]
    fn search_ranks_the_relevant_passage_first_and_cites_it() {
        let embedder = FakeEmbedder {
            keywords: vec!["locket", "harbor", "sword"],
        };
        let chapters = vec![
            chapter(
                "The Locket",
                "one.md",
                "She clutched the silver locket tightly.",
            ),
            chapter(
                "The Harbor",
                "two.md",
                "Ships crowded the busy harbor at dawn.",
            ),
            chapter(
                "The Duel",
                "three.md",
                "He raised his sword against the knight.",
            ),
        ];
        let mut index = Index::default();
        let embedded = index
            .refresh(&chapters, Breadth::Normal, &embedder)
            .unwrap();
        assert_eq!(embedded, 3, "all three chapters embedded on first build");
        assert_eq!(index.passage_count(), 3);

        // A query about the locket ranks the locket chapter first, and it's cited.
        let hits = index.search("where is the locket", 2, &embedder).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].passage.title, "The Locket");
        assert_eq!(hits[0].passage.file, "one.md");
        assert!(hits[0].score > hits[1].score || hits[1].score == 0.0);
    }

    #[test]
    fn refresh_only_re_embeds_changed_chapters() {
        // A counting embedder proves only changed passages are re-embedded (#90).
        let embedder = Counting {
            calls: std::cell::Cell::new(0),
        };
        let mut chapters = vec![
            chapter("A", "a.md", "alpha content here"),
            chapter("B", "b.md", "beta content here"),
        ];
        let mut index = Index::default();
        index
            .refresh(&chapters, Breadth::Normal, &embedder)
            .unwrap();
        assert_eq!(embedder.calls.get(), 2, "both chapters embedded first time");

        // No change → nothing re-embedded.
        let n = index
            .refresh(&chapters, Breadth::Normal, &embedder)
            .unwrap();
        assert_eq!(n, 0);
        assert_eq!(
            embedder.calls.get(),
            2,
            "unchanged manuscript re-embeds nothing"
        );

        // Change one chapter → only it re-embeds.
        chapters[1].body = "beta content, revised".into();
        let n = index
            .refresh(&chapters, Breadth::Normal, &embedder)
            .unwrap();
        assert_eq!(n, 1);
        assert_eq!(
            embedder.calls.get(),
            3,
            "only the changed chapter re-embeds"
        );
        assert_eq!(index.passage_count(), 2);
    }

    #[test]
    fn removed_chapter_drops_from_the_index() {
        let embedder = FakeEmbedder {
            keywords: vec!["a", "b"],
        };
        let chapters = vec![chapter("A", "a.md", "aaa"), chapter("B", "b.md", "bbb")];
        let mut index = Index::default();
        index
            .refresh(&chapters, Breadth::Normal, &embedder)
            .unwrap();
        assert_eq!(index.passage_count(), 2);

        // Drop chapter B → its passage leaves the index.
        let fewer = vec![chapter("A", "a.md", "aaa")];
        index.refresh(&fewer, Breadth::Normal, &embedder).unwrap();
        assert_eq!(index.passage_count(), 1);
        assert!(index.digests().contains_key("a.md"));
        assert!(!index.digests().contains_key("b.md"));
    }

    #[test]
    fn changing_the_embedder_model_rebuilds() {
        let a = FakeEmbedder {
            keywords: vec!["x"],
        };
        let chapters = vec![chapter("A", "a.md", "xxx")];
        let mut index = Index::default();
        index.refresh(&chapters, Breadth::Normal, &a).unwrap();
        let tag_before = index.model_tag.clone();

        // A different embedder (different tag) forces a full rebuild.
        let n = index.refresh(&chapters, Breadth::Normal, &Other).unwrap();
        assert_eq!(n, 1, "the model swap re-embeds every chapter");
        assert_ne!(index.model_tag, tag_before);
    }

    #[test]
    fn empty_index_search_returns_nothing() {
        let embedder = FakeEmbedder {
            keywords: vec!["x"],
        };
        let index = Index::default();
        assert!(index.search("anything", 5, &embedder).unwrap().is_empty());
    }

    #[test]
    fn rank_orders_by_cosine_and_respects_top_k() {
        // Drive ranking directly with injected vectors (no embedder round-trip).
        let passages = [
            ("a.md", vec![1.0, 0.0]),
            ("b.md", vec![0.0, 1.0]),
            ("c.md", vec![0.7, 0.7]),
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (file, mut vector))| {
            normalize(&mut vector);
            IndexedPassage {
                passage: Passage {
                    title: format!("Ch {i}"),
                    file: file.into(),
                    text: format!("passage {i}"),
                },
                digest: "d".into(),
                vector,
            }
        })
        .collect();
        let index = Index {
            model_tag: "fake".into(),
            breadth: Breadth::Normal,
            passages,
        };
        // Query aligned with the x-axis: a.md is nearest, then the diagonal c.md.
        let mut q = vec![1.0, 0.0];
        normalize(&mut q);
        let hits = index.rank(&q, 2);
        assert_eq!(hits.len(), 2, "top_k caps the result");
        assert_eq!(hits[0].passage.file, "a.md");
        assert_eq!(hits[1].passage.file, "c.md");
    }

    #[test]
    fn index_round_trips_through_disk() {
        let embedder = FakeEmbedder {
            keywords: vec!["gold"],
        };
        let dir = std::env::temp_dir().join(format!("quill-embed-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let chapters = vec![chapter("Gold", "g.md", "The gold coin gleamed.")];
        let mut index = Index::default();
        index
            .refresh(&chapters, Breadth::Normal, &embedder)
            .unwrap();
        index.save(&dir).unwrap();

        let loaded = Index::load(&dir);
        assert_eq!(loaded.passage_count(), 1);
        let hits = loaded.search("gold", 1, &embedder).unwrap();
        assert_eq!(hits[0].passage.title, "Gold");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn similar_excludes_the_query_passage_and_ranks_the_echo_first() {
        // The find-similar primitive (#924): querying with a passage's own prose
        // drops that passage (a self-match) and surfaces the ones that echo it.
        let embedder = FakeEmbedder {
            keywords: vec!["storm", "calm"],
        };
        let chapters = vec![
            chapter(
                "Storm A",
                "one.md",
                "A sudden storm broke over the storm-lit bay.",
            ),
            chapter(
                "Storm B",
                "two.md",
                "Another storm rolled in, a storm of wind.",
            ),
            chapter("Calm", "three.md", "The calm, calm morning was still."),
        ];
        let mut index = Index::default();
        index
            .refresh(&chapters, Breadth::Normal, &embedder)
            .unwrap();

        // Query with chapter one's exact prose: it must not return itself, and the
        // other storm-heavy passage ranks above the calm one.
        let query = "A sudden storm broke over the storm-lit bay.";
        let hits = index.similar(query, 5, &embedder).unwrap();
        assert!(
            hits.iter().all(|h| h.passage.text != query),
            "the query's own passage is excluded: {hits:?}"
        );
        assert_eq!(
            hits[0].passage.file, "two.md",
            "the echoing storm passage ranks first"
        );
        // The calm passage (no shared keyword) scores 0 and ranks last.
        assert_eq!(hits.last().unwrap().passage.file, "three.md");
    }

    #[test]
    fn similar_from_a_typed_query_behaves_like_search() {
        // A typed query rarely matches a passage verbatim, so nothing is excluded
        // and the most relevant passage still ranks first.
        let embedder = FakeEmbedder {
            keywords: vec!["locket", "harbor"],
        };
        let chapters = vec![
            chapter("Locket", "one.md", "She held the locket."),
            chapter("Harbor", "two.md", "Ships filled the harbor."),
        ];
        let mut index = Index::default();
        index
            .refresh(&chapters, Breadth::Normal, &embedder)
            .unwrap();
        let hits = index
            .similar("something about a locket", 2, &embedder)
            .unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].passage.file, "one.md");
    }

    #[test]
    fn relevant_prior_retrieves_the_bearing_chapter_and_excludes_the_changed_one() {
        // The retrieval-backed incremental pass (#929): a changed chapter re-raises
        // the locket; retrieval must surface the *prior* locket scene as context and
        // never re-send the changed chapter itself (it's already in the request).
        let embedder = FakeEmbedder {
            keywords: vec!["locket", "harbor", "sword"],
        };
        let chapters = vec![
            chapter(
                "The Locket",
                "one.md",
                "She clutched the silver locket tightly.",
            ),
            chapter(
                "The Harbor",
                "two.md",
                "Ships crowded the busy harbor at dawn.",
            ),
            chapter(
                "The Duel",
                "three.md",
                // The changed chapter: a sword scene that also mentions the locket.
                "He drew his sword, the locket at his throat.",
            ),
        ];
        let mut index = Index::default();
        index
            .refresh(&chapters, Breadth::Normal, &embedder)
            .unwrap();

        // Chapter three changed; its own prose is the query. The prior locket scene
        // (one.md) is the passage that bears on the change and must rank first; the
        // changed chapter (three.md) is excluded even though it's maximally similar
        // to itself; the unrelated harbor (two.md) scores 0.
        let query = "He drew his sword, the locket at his throat.";
        let changed = vec!["three.md".to_string()];
        let hits = index.relevant_prior(query, &changed, 5, &embedder).unwrap();
        assert!(
            hits.iter().all(|h| h.passage.file != "three.md"),
            "the changed chapter is never in its own retrieved context: {hits:?}"
        );
        assert_eq!(
            hits[0].passage.file, "one.md",
            "the prior locket scene ranks first: {hits:?}"
        );
        assert_eq!(hits[0].passage.title, "The Locket", "citation preserved");
    }

    #[test]
    fn relevant_prior_respects_top_k_after_excluding_changed() {
        // Excluding the changed chapters happens before truncation, so `top_k`
        // still yields that many *prior* hits when the index holds them.
        let embedder = FakeEmbedder {
            keywords: vec!["a"],
        };
        let chapters = vec![
            chapter("Ch1", "one.md", "aaa aaa"),
            chapter("Ch2", "two.md", "aaa"),
            chapter("Ch3", "three.md", "aaa aaa aaa"), // the changed chapter
        ];
        let mut index = Index::default();
        index
            .refresh(&chapters, Breadth::Normal, &embedder)
            .unwrap();
        let changed = vec!["three.md".to_string()];
        let hits = index.relevant_prior("aaa", &changed, 2, &embedder).unwrap();
        assert_eq!(hits.len(), 2, "two prior hits despite the excluded chapter");
        assert!(hits.iter().all(|h| h.passage.file != "three.md"));
    }

    #[test]
    fn relevant_prior_on_an_empty_index_returns_nothing() {
        // No index (embeddings unavailable / never built) → no retrieved context,
        // and the caller falls back to the whole-book incremental path (#929).
        let embedder = FakeEmbedder {
            keywords: vec!["x"],
        };
        let index = Index::default();
        assert!(index
            .relevant_prior("anything", &["a.md".into()], 5, &embedder)
            .unwrap()
            .is_empty());
    }

    #[test]
    fn echo_pairs_flags_near_duplicates_above_threshold() {
        // Echo detection (#920): three chapters, two of which are near-identical.
        // A high threshold flags only the near-duplicate; a lower one also catches
        // the softer echo, and an unrelated passage is never paired.
        // Vectors chosen directly so similarity is exact and threshold behavior is
        // unambiguous.
        let passages = [
            ("dup1.md", vec![1.0, 0.0, 0.0]),   // near-duplicate of dup2
            ("dup2.md", vec![0.99, 0.14, 0.0]), // ~0.99 cosine with dup1
            ("echo.md", vec![0.7, 0.7, 0.0]),   // a softer echo of dup1 (~0.7)
            ("far.md", vec![0.0, 0.0, 1.0]),    // orthogonal to all — never paired
        ]
        .into_iter()
        .enumerate()
        .map(|(i, (file, mut vector))| {
            normalize(&mut vector);
            IndexedPassage {
                passage: Passage {
                    title: format!("Ch {i}"),
                    file: file.into(),
                    text: format!("passage {i}"),
                },
                digest: "d".into(),
                vector,
            }
        })
        .collect();
        let index = Index {
            model_tag: "fake".into(),
            breadth: Breadth::Normal,
            passages,
        };

        // High threshold: only the near-duplicate pair (dup1 ↔ dup2).
        let near = index.echo_pairs(0.95);
        assert_eq!(
            near.len(),
            1,
            "only the near-duplicate clears 0.95: {near:?}"
        );
        let files = [near[0].a.file.as_str(), near[0].b.file.as_str()];
        assert!(files.contains(&"dup1.md") && files.contains(&"dup2.md"));
        assert!(near[0].score >= 0.95);

        // Lower threshold: the soft echo (echo.md) also pairs, and the highest-
        // similarity pair sorts first.
        let soft = index.echo_pairs(0.6);
        assert!(soft.len() >= 3, "soft echoes surface at 0.6: {soft:?}");
        assert!(
            soft[0].score >= soft[1].score,
            "pairs are ordered by descending similarity"
        );
        // The orthogonal passage is never in any pair.
        assert!(
            soft.iter()
                .all(|p| p.a.file != "far.md" && p.b.file != "far.md"),
            "an unrelated passage is never flagged: {soft:?}"
        );
    }

    #[test]
    fn echo_pairs_skips_within_chapter_repeats() {
        // Two identical passages in the *same* chapter (a deliberate in-scene
        // refrain) are not flagged; the same text across chapters is.
        let make = |file: &str, text: &str, v: Vec<f32>| {
            let mut vector = v;
            normalize(&mut vector);
            IndexedPassage {
                passage: Passage {
                    title: file.into(),
                    file: file.into(),
                    text: text.into(),
                },
                digest: "d".into(),
                vector,
            }
        };
        let index = Index {
            model_tag: "fake".into(),
            breadth: Breadth::Normal,
            passages: vec![
                make("a.md", "the refrain", vec![1.0, 0.0]),
                make("a.md", "the refrain again", vec![1.0, 0.0]), // same chapter → skipped
                make("b.md", "the refrain elsewhere", vec![1.0, 0.0]), // cross-chapter → flagged
            ],
        };
        let pairs = index.echo_pairs(0.9);
        // Only the cross-chapter pairs (a↔b, twice) are flagged; the a↔a pair isn't.
        assert!(
            pairs.iter().all(|p| p.a.file != p.b.file),
            "no within-chapter pair is flagged: {pairs:?}"
        );
        assert_eq!(
            pairs.len(),
            2,
            "both a.md passages pair with b.md: {pairs:?}"
        );
    }

    /// End-to-end retrieval with the *real* candle embedding model (#831): download
    /// MiniLM via `hf-hub`, index three chapters, and confirm a semantic query
    /// retrieves the right chapter — including a **paraphrase** the words of which
    /// don't appear in the passage, the thing lexical search misses. Ignored
    /// (needs network + disk); not hermetic, so it's kept out of the default and
    /// `--features local` suites (#531). Run with:
    /// `cargo test --features local live_retrieval -- --ignored --nocapture`
    #[test]
    #[cfg(feature = "local")]
    #[ignore = "integration: downloads the embedding model; not hermetic (#831)"]
    fn live_retrieval_finds_the_paraphrased_chapter() {
        let model = crate::ai::model_select::EmbedModel::default_model();
        let embedder = LocalEmbedder::for_model(model).expect("download + load MiniLM");

        let chapters = vec![
            chapter(
                "The Harbor",
                "one.md",
                "Fishing boats rocked against the pier as gulls wheeled overhead.",
            ),
            chapter(
                "The Duel",
                "two.md",
                "Steel rang on steel as the two swordsmen circled each other.",
            ),
            chapter(
                "The Feast",
                "three.md",
                "Long tables groaned under roast venison, warm bread, and spiced wine.",
            ),
        ];
        let mut index = Index::default();
        let embedded = index
            .refresh(&chapters, Breadth::Normal, &embedder)
            .expect("index");
        assert_eq!(embedded, 3);

        // A paraphrase — none of these words appear in the passage — should still
        // retrieve "The Feast" first, the semantic recall lexical search can't do.
        let hits = index
            .search("a big meal with lots of food", 3, &embedder)
            .expect("search");
        println!(
            "LIVE RETRIEVAL: {:?}",
            hits.iter()
                .map(|h| (&h.passage.title, h.score))
                .collect::<Vec<_>>()
        );
        assert_eq!(
            hits[0].passage.title, "The Feast",
            "paraphrase retrieves the feast chapter: {hits:?}"
        );

        // A refresh over the unchanged manuscript re-embeds nothing (incremental #90).
        assert_eq!(
            index
                .refresh(&chapters, Breadth::Normal, &embedder)
                .expect("refresh"),
            0
        );
    }
}
