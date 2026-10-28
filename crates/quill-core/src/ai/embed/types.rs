//! The retrieval value types and the model seam (#831, #531): passages, ranked
//! hits, echo pairs, a live chapter, and the [`Embedder`] trait the index ranks
//! through. All pure data — the trait is the only place a model plugs in, so the
//! whole retrieval path runs against an injected fake with no model download.

use serde::{Deserialize, Serialize};

use crate::ai::AiError;

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
/// Persisted in the [`Index`](super::Index); the digest lets a refresh drop a
/// changed chapter's stale passages and re-embed only what moved.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(super) struct IndexedPassage {
    pub(super) passage: Passage,
    /// The chapter's content digest when this passage was embedded.
    pub(super) digest: String,
    /// The (L2-normalized) embedding vector.
    pub(super) vector: Vec<f32>,
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

/// One chapter's live text as the index sees it: its title, file, and body. The app
/// assembles these from the active book so the index never holds more than it needs.
#[derive(Clone, Debug)]
pub struct Chapter {
    pub title: String,
    pub file: String,
    pub body: String,
}

/// The seam between the index and a model: turn texts into fixed-width embedding
/// vectors. `LocalEmbedder` is the candle implementation; tests inject a fake so
/// the whole retrieval path runs with no model download (#531).
///
/// An embedder **only reads** text to vectorize it — it never generates prose, so
/// it's inside the AI boundary by construction.
pub trait Embedder {
    /// Embed each text into a vector. The returned vectors should be L2-normalized
    /// (so a dot product is cosine similarity); [`Index`](super::Index) normalizes
    /// defensively regardless. One vector per input, in order.
    fn embed(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, AiError>;

    /// A stable tag identifying this embedder's model, stored in the index so a
    /// model swap invalidates a stale index built by a different embedder.
    fn model_tag(&self) -> String;
}
