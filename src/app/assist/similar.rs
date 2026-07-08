//! Semantic "find similar scenes" (#924) and echo / near-duplicate detection
//! (#920): the app-side glue between the embedding index ([`crate::ai::embed`])
//! and two retrieval-shaped author tools.
//!
//! Both read the manuscript's prose to *rank* and *cite* passages — one surfaces
//! the scenes most like a selection or a query, the other flags near-duplicate
//! passage pairs — and hand the author jumpable citations to act on. Neither ever
//! writes or edits prose, so both stay squarely inside Quill's AI boundary.
//!
//! They share the one on-device embedding index (the same one grounded-Ask built,
//! kept fresh content-addressed) and the `local`-gated [`LocalEmbedder`]; with no
//! embedding model on disk (no `--features local`, or the weights aren't
//! downloaded) each degrades gracefully to a clear message, exactly as Ask does.

use super::super::*;

use crate::ai::embed::{Chapter, EchoPair, Hit};
use crate::ai::model_select::EmbedModel;
use crate::mode::AiAction;
use crate::text::ellipsize;

#[cfg(feature = "local")]
use crate::ai::embed::{Breadth, Index, LocalEmbedder};

/// How many similar passages a semantic find returns (#924) — enough to see the
/// resonant scenes, few enough to scan at a glance. Only the `local` retrieval
/// path reads it; the fallback build has no embedder to rank with.
#[cfg_attr(not(feature = "local"), allow(dead_code))]
const FIND_TOP_K: usize = 10;

/// Near-duplicate threshold (#920): a cosine similarity at/above this marks two
/// passages as *near-duplicates* — very likely an accidental repeat, or a scene
/// pasted twice. High enough that only strong semantic overlap clears it.
const NEAR_DUPLICATE: f32 = 0.90;

/// Soft-echo threshold (#920): the lower bound a pair must clear to surface at
/// all. Between this and [`NEAR_DUPLICATE`] is a *soft echo* — the same beat or
/// image reworded — which the author may not realize they're creating. Set so an
/// ordinary pair of unrelated scenes (which the embedder scores well below this)
/// never trips it. Only the `local` scan reads it; the fallback build has no
/// embedder to pair with.
#[cfg_attr(not(feature = "local"), allow(dead_code))]
const SOFT_ECHO: f32 = 0.75;

impl App {
    /// Start a semantic "find similar scenes" (#924). With an active Visual
    /// selection, run straight from the selected prose — "find scenes like *this*".
    /// Otherwise open a prompt (prefilled with the word under the cursor) for a
    /// typed query. A clear message, no crash, when no embedding model is available.
    pub(crate) fn start_semantic_find(&mut self) {
        if let Mode::Visual { anchor, linewise } = self.mode {
            let (start, end) = self.selection_bytes(anchor, linewise);
            let selection = self.edit_buffer().byte_slice(start, end).trim().to_string();
            if !selection.is_empty() {
                self.mode = self.editing_mode();
                self.semantic_find(&selection);
                return;
            }
        }
        let prefill = self
            .edit_buffer()
            .text_object(crate::buffer::TextObject::Word, false)
            .map(|(lo, hi)| self.edit_buffer().byte_slice(lo, hi).trim().to_string())
            .filter(|w| !w.is_empty())
            .unwrap_or_default();
        self.open_prompt(
            Pending::SemanticFind,
            "Find similar scenes — a passage or a description: ",
            &prefill,
            None,
        );
    }

    /// Run a semantic find for `query` (#924): (re-)index the active book off the
    /// editor thread, rank the passages most similar to the query, and deliver them
    /// as ranked, jumpable citations. A no-op with a message for an empty query or
    /// when no embedding model is on disk (graceful degradation).
    pub(crate) fn semantic_find(&mut self, query: &str) {
        let query = query.trim().to_string();
        if query.is_empty() {
            self.message = Some("Find similar: nothing to search for.".into());
            return;
        }
        let Some(model) = self.ask_embed_model() else {
            self.message = Some(EMBED_UNAVAILABLE.into());
            return;
        };
        if self.ai_task.is_busy() {
            self.message = Some("An AI request is already in flight…".into());
            return;
        }
        let chapters = self.ask_index_chapters();
        let root = self.project.root.clone();
        self.ai_task.spawn(move || {
            Outcome::SemanticFind(find_similar_passages(&root, &chapters, model, &query))
        });
        self.message = Some("⟳ Finding similar scenes…".into());
    }

    /// Present a completed semantic find (#924): the ranked passages as a jumpable
    /// [`Mode::Findings`] list — one row per hit, labelled with its similarity score
    /// and a snippet, targeting the chapter the passage lives in so Enter opens it
    /// there. Unlike an analysis pass these are retrieval for *this* query, not
    /// standing revision findings, so they never enter durable triage (like a Q&A
    /// citation). `None`/empty → a clear message; an error surfaces as `AI: …`.
    pub(crate) fn apply_semantic_find(&mut self, result: Result<Option<Vec<Hit>>, ai::AiError>) {
        let Some(hits) = self.ok_or_report(result) else {
            return;
        };
        let Some(hits) = hits else {
            self.message = Some(EMBED_UNAVAILABLE.into());
            return;
        };
        if hits.is_empty() {
            self.message = Some("No similar passages found.".into());
            return;
        }
        let items: Vec<Finding> = hits.iter().map(hit_finding).collect();
        self.mode = Mode::Findings(Box::new(Findings {
            items,
            ..Default::default()
        }));
    }

    /// Start echo / near-duplicate detection (#920): scan the embedding index for
    /// near-duplicate and echoed passage pairs and surface them as findings. Honors
    /// the `[ai] echo-detection` off-switch, and reports clearly when no embedding
    /// model is available (graceful degradation, no crash).
    pub(crate) fn start_echo_detection(&mut self) {
        if !self.ai.echo_detection {
            self.message =
                Some("Echo detection is off — set `[ai] echo-detection = true` to enable.".into());
            return;
        }
        let Some(model) = self.ask_embed_model() else {
            self.message = Some(EMBED_UNAVAILABLE.into());
            return;
        };
        if self.ai_task.is_busy() {
            self.message = Some("An AI request is already in flight…".into());
            return;
        }
        let chapters = self.ask_index_chapters();
        let root = self.project.root.clone();
        self.ai_task
            .spawn(move || Outcome::Echo(detect_echoes(&root, &chapters, model)));
        self.message = Some("⟳ Scanning for echoes & near-duplicates…".into());
    }

    /// Present a completed echo scan (#920): each near-duplicate / echo pair as a
    /// [`Finding`] carrying both citations + the score, routed through the shared
    /// findings flow so they accumulate into revision triage and the health
    /// dashboard (grouped under [`FindingKind::Echo`]). A pair the author decides is
    /// a deliberate reprise is dismissed there ("mark intentional reprise"), which
    /// the durable store keeps from re-surfacing. `None` → the unavailable message;
    /// an empty result reports the clean bill; an error surfaces as `AI: …`.
    pub(crate) fn apply_echo(&mut self, result: Result<Option<Vec<EchoPair>>, ai::AiError>) {
        let Some(pairs) = self.ok_or_report(result) else {
            return;
        };
        let Some(pairs) = pairs else {
            self.message = Some(EMBED_UNAVAILABLE.into());
            return;
        };
        if pairs.is_empty() {
            self.message = Some("No echoes or near-duplicates found.".into());
            return;
        }
        let findings: Vec<Finding> = pairs.iter().map(echo_finding).collect();
        self.show_findings(findings);
    }
}

/// The message shown when semantic find / echo detection can't run because no
/// embedding model is on disk (no `--features local`, or the weights aren't
/// downloaded) — the same graceful-degradation path grounded-Ask takes.
const EMBED_UNAVAILABLE: &str = "Semantic search needs the on-device embedding model \
    — build with `--features local` and download it (Local AI models), or it stays unavailable.";

/// A ranked similar-passage hit as a jumpable finding (#924): the score and a
/// snippet in the summary, the passage's chapter as the (single) jump target. Not
/// an analysis-pass finding — [`FindingKind::Other`], no re-runnable `action` —
/// so it lists and jumps like a Q&A citation without entering durable triage.
fn hit_finding(hit: &Hit) -> Finding {
    Finding {
        summary: format!(
            "{} — {} \u{2014} \u{201c}{}\u{201d}",
            score_label(hit.score),
            hit.passage.title,
            ellipsize(hit.passage.text.trim(), 60),
        ),
        kind: FindingKind::Other,
        chapters: vec![hit.passage.title.clone()],
        book: None,
        action: None,
        corroborators: Vec::new(),
    }
}

/// An echo pair as a triage finding (#920): both citations + the score in the
/// summary, both chapters in `chapters` (the first is the jump target, so Enter
/// opens one side; the summary names the other). [`FindingKind::Echo`] groups it
/// in the dashboard; `action: Some(Echo)` marks the pass that produced it.
fn echo_finding(pair: &EchoPair) -> Finding {
    let kind = if pair.score >= NEAR_DUPLICATE {
        "near-duplicate"
    } else {
        "echo"
    };
    Finding {
        summary: format!(
            "{kind} ({}): \u{201c}{}\u{201d} \u{2194} \u{201c}{}\u{201d}",
            score_label(pair.score),
            ellipsize(pair.a.text.trim(), 45),
            ellipsize(pair.b.text.trim(), 45),
        ),
        kind: FindingKind::Echo,
        // The first chapter is the jump target; both are recorded so the finding's
        // content key (kind + summary + chapters) is stable across re-runs and the
        // dashboard attributes it to both scenes.
        chapters: vec![pair.a.title.clone(), pair.b.title.clone()],
        book: None,
        action: Some(AiAction::Echo),
        corroborators: Vec::new(),
    }
}

/// A percentage similarity label (`92%`) for a finding's summary — a readable
/// stand-in for a raw cosine in `[0, 1]`, clamped so a tiny negative never shows.
fn score_label(score: f32) -> String {
    format!("{:.0}%", score.clamp(0.0, 1.0) * 100.0)
}

/// Rank the active book's passages most similar to `query` off the editor thread
/// (#924): load + incrementally refresh the persisted index (re-embedding only the
/// chapters whose text changed, #90), persist it, then return the top hits. `None`
/// — graceful degradation — when the embedder can't be built (model not on disk);
/// any transient embedding error propagates as `Err`. Reads prose to rank + cite
/// it, never writes it.
#[cfg(feature = "local")]
fn find_similar_passages(
    root: &std::path::Path,
    chapters: &[Chapter],
    model: &'static EmbedModel,
    query: &str,
) -> Result<Option<Vec<Hit>>, ai::AiError> {
    let Ok(embedder) = LocalEmbedder::for_model(model) else {
        return Ok(None);
    };
    let mut index = Index::load(root);
    // Find-similar retrieves at the default granularity (#924); breadth is a
    // grounded-Ask control (#931).
    index.refresh(chapters, Breadth::Normal, &embedder)?;
    let _ = index.save(root);
    Ok(Some(index.similar(query, FIND_TOP_K, &embedder)?))
}

/// Without the `local` feature there's no on-device embedder, so a semantic find
/// is unavailable (#924) — `None`, which the caller surfaces as a clear message.
#[cfg(not(feature = "local"))]
#[allow(
    clippy::unnecessary_wraps,
    reason = "the signature must match the `local` build's fallible variant, which \
              genuinely returns Err on an embedding failure"
)]
const fn find_similar_passages(
    _root: &std::path::Path,
    _chapters: &[Chapter],
    _model: &'static EmbedModel,
    _query: &str,
) -> Result<Option<Vec<Hit>>, ai::AiError> {
    Ok(None)
}

/// Detect echo / near-duplicate passage pairs off the editor thread (#920): load +
/// incrementally refresh the persisted index (#90), persist it, then pair every
/// passage against its peers above the soft-echo floor. `None` — graceful
/// degradation — when the embedder can't be built; embedding errors propagate as
/// `Err`. Reads prose to compare + cite it, never writes it.
#[cfg(feature = "local")]
fn detect_echoes(
    root: &std::path::Path,
    chapters: &[Chapter],
    model: &'static EmbedModel,
) -> Result<Option<Vec<EchoPair>>, ai::AiError> {
    let Ok(embedder) = LocalEmbedder::for_model(model) else {
        return Ok(None);
    };
    let mut index = Index::load(root);
    // Echo detection scans the whole index at the default granularity (#920).
    index.refresh(chapters, Breadth::Normal, &embedder)?;
    let _ = index.save(root);
    Ok(Some(index.echo_pairs(SOFT_ECHO)))
}

/// Without the `local` feature there's no on-device embedder, so echo detection is
/// unavailable (#920) — `None`, which the caller surfaces as a clear message.
#[cfg(not(feature = "local"))]
#[allow(
    clippy::unnecessary_wraps,
    reason = "the signature must match the `local` build's fallible variant, which \
              genuinely returns Err on an embedding failure"
)]
const fn detect_echoes(
    _root: &std::path::Path,
    _chapters: &[Chapter],
    _model: &'static EmbedModel,
) -> Result<Option<Vec<EchoPair>>, ai::AiError> {
    Ok(None)
}
