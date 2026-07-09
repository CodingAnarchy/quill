//! The semantic retrieval index (#831): the manuscript's chapters chunked into
//! passages with their embedding vectors, refreshed incrementally against chapter
//! digests (#90), and the ranking queries built on it — plain search, find-similar
//! (#924), retrieval-backed incremental context (#929), and echo detection (#920).
//! The ranking core is pure over stored vectors, so the hermetic tests drive it
//! with an injected embedder and no model download (#531). Persistence lives here
//! too: the index file sits beside the bible's incremental state.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::chunk::chunk_chapter;
use super::math::{cosine, normalize};
use super::{Breadth, Chapter, EchoPair, Embedder, Hit, IndexedPassage, Passage};
use crate::ai::incremental::{digest, Digests};
use crate::ai::AiError;

/// The semantic retrieval index (#831): the manuscript's chapters chunked into
/// passages, each with its embedding vector and the content digest of its chapter.
/// Persisted per project; refreshed incrementally against the current chapter
/// digests so only changed chapters are re-embedded (#90), and answers a
/// `search(query) → ranked passages + jumpable citations` query.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Index {
    /// The embedder model tag the index was built with; a mismatch forces a rebuild.
    #[serde(default)]
    pub(super) model_tag: String,
    /// The [`Breadth`] the passages were chunked at (#931); a change forces a
    /// re-chunk, since a different-width passage set isn't comparable. Absent in a
    /// pre-#931 index → [`Breadth::Normal`], the width those passages were built at,
    /// so an old index isn't needlessly rebuilt.
    #[serde(default)]
    pub(super) breadth: Breadth,
    /// Every indexed passage, across all chapters.
    #[serde(default)]
    pub(super) passages: Vec<IndexedPassage>,
}

impl Index {
    /// The per-chapter digests this index currently reflects (`file → digest`), so a
    /// caller can [`incremental::decide`](crate::ai::incremental::decide) whether a
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
