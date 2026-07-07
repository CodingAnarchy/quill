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

/// The maximum characters a single passage carries. Chapters are split into
/// passages at paragraph boundaries and packed up to this width, so each embedded
/// unit is a coherent span the embedder's context window can hold and a citation
/// points at a real, jumpable chapter. Small enough that the vector captures one
/// idea, large enough that a passage isn't a fragment.
const PASSAGE_CHARS: usize = 1_200;

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

/// Split a chapter's prose into passages of at most [`PASSAGE_CHARS`], breaking at
/// paragraph boundaries (blank lines) so each passage is a coherent span. A
/// paragraph longer than the cap becomes its own passage (rather than being split
/// mid-sentence) — the embedder truncates to its window if need be. Empty/whitespace
/// chapters yield no passages.
pub fn chunk_chapter(title: &str, file: &str, body: &str) -> Vec<Passage> {
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
        if !current.is_empty() && current.len() + para.len() + 2 > PASSAGE_CHARS {
            flush(&mut current);
        }
        if !current.is_empty() {
            current.push_str("\n\n");
        }
        current.push_str(para);
        if current.len() >= PASSAGE_CHARS {
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

    /// Refresh the index against `chapters` using `embedder`, re-embedding only what
    /// changed (#90): a chapter whose digest already matches keeps its passages
    /// untouched; a changed/new chapter is re-chunked and re-embedded; a chapter no
    /// longer present is dropped. A changed embedder model (tag mismatch) rebuilds
    /// everything. Returns how many chapters were (re-)embedded, so the caller can
    /// report the cost. The embedder is called **once** for all changed passages, so
    /// a big refresh is one batched inference.
    pub fn refresh(
        &mut self,
        chapters: &[Chapter],
        embedder: &dyn Embedder,
    ) -> Result<usize, AiError> {
        let tag = embedder.model_tag();
        if self.model_tag != tag {
            // A different embedder model → the old vectors aren't comparable; rebuild.
            self.passages.clear();
            self.model_tag = tag;
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
            for passage in chunk_chapter(&chapter.title, &chapter.file, &chapter.body) {
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

/// The candle text-embedding backend (#831), behind `#[cfg(feature = "local")]` so
/// its heavy deps stay out of a default build — the same gating the generative
/// [`crate::ai::local`] backend uses. A small BERT-family sentence-transformers
/// encoder (`all-MiniLM-L6-v2`) loaded from safetensors, mean-pooled and
/// L2-normalized to one vector per passage. It only ever reads text to vectorize
/// it, so it's inside the AI boundary by construction.
#[cfg(feature = "local")]
mod local {
    use candle_core::{Device, Tensor};
    use candle_nn::VarBuilder;
    use candle_transformers::models::bert::{BertModel, Config, DTYPE};
    use tokenizers::Tokenizer;

    use super::Embedder;
    use crate::ai::model_select::{self, EmbedModel};
    use crate::ai::AiError;

    /// A loaded local embedding model — the encoder, its tokenizer, and the device.
    pub struct LocalEmbedder {
        model: BertModel,
        tokenizer: Tokenizer,
        device: Device,
        tag: String,
    }

    impl LocalEmbedder {
        /// Load an [`EmbedModel`] from the `hf-hub` cache (downloading if absent,
        /// resumable), matching the generative backend's `for_model` path. The
        /// repo carries `model.safetensors`, `config.json`, and `tokenizer.json` —
        /// the sentence-transformers layout candle's `bert` reads.
        pub fn for_model(model: &EmbedModel) -> Result<Self, AiError> {
            let hub = hf_hub::api::sync::ApiBuilder::new()
                .with_progress(false)
                .build()
                .map_err(|e| download_error(model, e))?;
            let repo = hub.model(model.repo.to_string());
            let weights = repo
                .get(model.weights_file)
                .map_err(|e| download_error(model, e))?;
            let config = repo
                .get("config.json")
                .map_err(|e| download_error(model, e))?;
            let tokenizer_path = repo
                .get("tokenizer.json")
                .map_err(|e| download_error(model, e))?;
            Self::load(
                weights.to_str().unwrap_or_default(),
                config.to_str().unwrap_or_default(),
                tokenizer_path.to_str().unwrap_or_default(),
                model.id,
            )
        }

        /// Load a BERT embedding model from a safetensors weights file, its
        /// `config.json`, and its `tokenizer.json`. Uses the best device candle
        /// offers (Metal on Apple Silicon, else CPU).
        pub fn load(
            weights_path: &str,
            config_path: &str,
            tokenizer_path: &str,
            tag: &str,
        ) -> Result<Self, AiError> {
            let device = model_select::best_device();
            let config: Config = serde_json::from_str(
                &std::fs::read_to_string(config_path)
                    .map_err(|e| internal(format!("read config: {e}")))?,
            )
            .map_err(|e| internal(format!("parse config: {e}")))?;
            let tokenizer = Tokenizer::from_file(tokenizer_path)
                .map_err(|e| internal(format!("tokenizer: {e}")))?;
            // SAFETY: mmap of a read-only weights file we just fetched; candle's
            // documented load path (the same one the bert example uses).
            let vb = unsafe {
                VarBuilder::from_mmaped_safetensors(&[weights_path], DTYPE, &device)
                    .map_err(|e| internal(format!("load weights: {e}")))?
            };
            let model =
                BertModel::load(vb, &config).map_err(|e| internal(format!("build model: {e}")))?;
            Ok(Self {
                model,
                tokenizer,
                device,
                tag: tag.to_string(),
            })
        }

        /// Embed one text into a mean-pooled sentence vector.
        fn embed_one(&self, text: &str) -> Result<Vec<f32>, AiError> {
            let encoding = self
                .tokenizer
                .encode(text, true)
                .map_err(|e| internal(format!("encode: {e}")))?;
            let ids = encoding.get_ids().to_vec();
            let n = ids.len();
            let token_ids = Tensor::new(ids.as_slice(), &self.device)
                .and_then(|t| t.unsqueeze(0))
                .map_err(|e| internal(format!("input tensor: {e}")))?;
            let token_type_ids = token_ids
                .zeros_like()
                .map_err(|e| internal(format!("type ids: {e}")))?;
            let out = self
                .model
                .forward(&token_ids, &token_type_ids, None)
                .map_err(|e| internal(format!("forward: {e}")))?;
            // Mean-pool over the token axis → one sentence vector. `out` is
            // (1, n_tokens, hidden); sum over tokens and divide by the count.
            let pooled = out
                .sum(1)
                .and_then(|t| t / n.max(1) as f64)
                .and_then(|t| t.squeeze(0))
                .and_then(|t| t.to_dtype(candle_core::DType::F32))
                .and_then(|t| t.to_vec1::<f32>())
                .map_err(|e| internal(format!("pool: {e}")))?;
            Ok(pooled)
        }
    }

    impl Embedder for LocalEmbedder {
        fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AiError> {
            texts.iter().map(|t| self.embed_one(t)).collect()
        }

        fn model_tag(&self) -> String {
            self.tag.clone()
        }
    }

    const fn internal(msg: String) -> AiError {
        AiError::Internal(msg)
    }

    /// An actionable error when an embedding model can't be fetched (#726), naming
    /// the model + repo + file so a genuine failure points at the concrete resource.
    fn download_error(model: &EmbedModel, cause: impl std::fmt::Display) -> AiError {
        AiError::NotConfigured(format!(
            "couldn't obtain the embedding model {} (~{} MB, {}/{}) — download it while \
             online, or Ask falls back to sending the whole book. ({cause})",
            model.id, model.size_mb, model.repo, model.weights_file,
        ))
    }
}

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
        let passages = chunk_chapter("One", "one.md", body);
        // Both short paragraphs pack into a single passage.
        assert_eq!(passages.len(), 1);
        assert!(passages[0].text.contains("First") && passages[0].text.contains("Second"));
        assert_eq!(passages[0].title, "One");
        assert_eq!(passages[0].file, "one.md");

        // A body far over the cap yields multiple passages, none empty.
        let big = "word ".repeat(PASSAGE_CHARS); // ~5x the cap
        let paras = format!("{big}\n\n{big}\n\n{big}");
        let many = chunk_chapter("Big", "big.md", &paras);
        assert!(many.len() >= 3, "oversized chapter splits: {}", many.len());
        assert!(many.iter().all(|p| !p.text.trim().is_empty()));
    }

    #[test]
    fn empty_chapter_yields_no_passages() {
        assert!(chunk_chapter("Empty", "e.md", "   \n\n  ").is_empty());
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
        let embedded = index.refresh(&chapters, &embedder).unwrap();
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
        index.refresh(&chapters, &embedder).unwrap();
        assert_eq!(embedder.calls.get(), 2, "both chapters embedded first time");

        // No change → nothing re-embedded.
        let n = index.refresh(&chapters, &embedder).unwrap();
        assert_eq!(n, 0);
        assert_eq!(
            embedder.calls.get(),
            2,
            "unchanged manuscript re-embeds nothing"
        );

        // Change one chapter → only it re-embeds.
        chapters[1].body = "beta content, revised".into();
        let n = index.refresh(&chapters, &embedder).unwrap();
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
        index.refresh(&chapters, &embedder).unwrap();
        assert_eq!(index.passage_count(), 2);

        // Drop chapter B → its passage leaves the index.
        let fewer = vec![chapter("A", "a.md", "aaa")];
        index.refresh(&fewer, &embedder).unwrap();
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
        index.refresh(&chapters, &a).unwrap();
        let tag_before = index.model_tag.clone();

        // A different embedder (different tag) forces a full rebuild.
        let n = index.refresh(&chapters, &Other).unwrap();
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
        index.refresh(&chapters, &embedder).unwrap();
        index.save(&dir).unwrap();

        let loaded = Index::load(&dir);
        assert_eq!(loaded.passage_count(), 1);
        let hits = loaded.search("gold", 1, &embedder).unwrap();
        assert_eq!(hits[0].passage.title, "Gold");
        std::fs::remove_dir_all(&dir).ok();
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
        let embedded = index.refresh(&chapters, &embedder).expect("index");
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
        assert_eq!(index.refresh(&chapters, &embedder).expect("refresh"), 0);
    }
}
