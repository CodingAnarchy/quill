//! Incremental pass scoping (#90): keep the story bible up to date for the price
//! of the chapters that changed, not the whole novel.
//!
//! Each persistent-bible pass (character, setting, timeline) records the set of
//! chapters it was built from, as a `file → content-digest` map in a per-project
//! state file (`bible/.state.toml`). The next run hashes the current chapters and
//! diffs against that record:
//!
//! - nothing changed → the bible is current, skip the request entirely;
//! - a few chapters changed → an **incremental** pass: feed the existing bible
//!   plus only the changed chapters and ask the model to *revise* it;
//! - many chapters changed (past a configurable threshold), a chapter was
//!   removed, or there's no prior record → a **full** whole-manuscript rebuild.
//!
//! The digest is content-addressed, so it needs no version control and is immune
//! to commit timing — a chapter counts as changed exactly when its text differs.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{bible, Request};

/// A `chapter file → content digest` map: the manuscript state a pass was built
/// from. `BTreeMap` so the serialized state file is stable (ordered).
pub type Digests = BTreeMap<String, String>;

/// Stable, non-cryptographic content hash of a chapter's text — the same persisted
/// hash the bible uses, so the two can't drift (collisions only ever cost an
/// extra re-analysis).
pub fn digest(text: &str) -> String {
    bible::digest(text)
}

/// How a pass should run, given what changed since it was last built.
#[derive(Debug, PartialEq, Eq)]
pub enum Scope {
    /// Re-send the whole manuscript (first run, too much changed, or a removal).
    Full,
    /// Revise the bible from only these changed/added chapter files.
    Incremental(Vec<String>),
    /// Nothing changed — the bible already reflects the manuscript.
    UpToDate,
}

/// Decide how to run a pass. `prior` is the state it was last built from (`None`
/// for a first run); `current` is the manuscript now; `threshold` is the number
/// of changed chapters at or above which a full rebuild is preferred over an
/// incremental update (0 disables incremental).
pub fn decide(prior: Option<&Digests>, current: &Digests, threshold: usize) -> Scope {
    let Some(prior) = prior else {
        return Scope::Full;
    };
    let changed: Vec<String> = current
        .iter()
        .filter(|(file, dig)| prior.get(*file) != Some(dig))
        .map(|(file, _)| file.clone())
        .collect();
    let removed = prior.keys().any(|f| !current.contains_key(f));
    if changed.is_empty() && !removed {
        Scope::UpToDate
    } else if removed || changed.len() >= threshold {
        // A removed chapter can leave stale bible entries an incremental revise
        // wouldn't catch, so rebuild from scratch to stay correct.
        Scope::Full
    } else {
        Scope::Incremental(changed)
    }
}

/// The heading the retrieval-backed context (#929) is filed under in a revise
/// request, so a test can assert the relevant prior passages made it into the
/// context and the model knows they're reference, not the changed material.
pub const PRIOR_PASSAGES_HEADING: &str = "## Relevant prior passages (for reference)";

/// Fold the retrieved relevant prior passages (#929) into a changed-chapters block
/// for a revise request. `retrieved` is [`crate::ai::retrieved_context`]'s output —
/// the top prior passages the changed chapters bear on, each already under its
/// `# Title` heading so its chapter stays a jumpable citation. It's prepended as a
/// clearly-labeled reference section *before* the changed chapters, so the model can
/// check the change against the scenes that matter (where a character last appeared,
/// a rule was set) instead of being blind to everything outside the diff — while
/// still knowing which text is the changed material to revise from. An empty
/// `retrieved` (no index / no hits) returns the changed chapters unchanged, so the
/// caller degrades to the plain incremental path exactly as before.
pub fn with_prior_passages(retrieved: &str, changed_chapters: &str) -> String {
    if retrieved.trim().is_empty() {
        return changed_chapters.to_string();
    }
    format!(
        "{PRIOR_PASSAGES_HEADING}\n\n{}\n\n\
         ## Chapters that changed\n\n{changed_chapters}",
        retrieved.trim_end()
    )
}

/// Build a "revise this bible" request (#90): the existing bible plus only the
/// changed chapters as cached context, with the same schema as the full pass so
/// the model returns a complete, replacement bible.
///
/// `changed_chapters` is the changed material to revise from — either the changed
/// chapters alone (the classic path) or, when a semantic index is available, those
/// chapters preceded by a labeled block of the relevant prior passages retrieval
/// pulled up ([`with_prior_passages`], #929). Either way the request shape and the
/// schema are identical, so retrieval changes only *what context* the revise sees.
pub fn revise_request(
    system: &str,
    schema: Value,
    max_tokens: u32,
    prior_bible: &str,
    changed_chapters: &str,
    model: &str,
) -> Request {
    let context = format!(
        "## Existing story bible (revise this)\n\n{prior_bible}\n\n\
         ## Chapters that changed since it was built\n\n{changed_chapters}\n"
    );
    Request {
        system: Some(system.to_string()),
        cached_context: context,
        user: "Revise the existing bible to reflect the changed chapters. Keep \
            everything that is still accurate, update what the changes affect, add \
            anything new, and remove what the changes contradict. Return the \
            complete revised bible in the required schema."
            .to_string(),
        model: model.to_string(),
        max_tokens,
        json_schema: Some(schema),
    }
}

// --- Full-by-folding for the offline-oversized case (#849) ------------------

/// One chapter's size for the fold planner: its file (a chunk boundary always
/// falls on a chapter seam) and its estimated input-token count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChapterSize<'a> {
    pub file: &'a str,
    pub tokens: u32,
}

/// The next fold step, from [`next_fold_chunk`]. A pure decision over injected
/// sizes so it's unit-testable without a model.
#[derive(Debug, PartialEq, Eq)]
pub enum FoldStep {
    /// Revise the running bible with these whole chapters (in manuscript order);
    /// the count consumed from the head of the remaining list.
    Chunk(Vec<String>),
    /// The running bible plus even the single smallest next chapter can't fit the
    /// window — the genuine hard limit. The caller falls through to the graceful
    /// report rather than truncating or looping forever.
    HardLimit,
}

/// Plan the next fold chunk for an offline oversized rebuild (#849): as many WHOLE
/// chapters from the head of `remaining` (already in manuscript order) as fit the
/// window alongside the running bible. `window` is the model's context; `overhead`
/// is `est(system prompt + running bible + MAX_OUTPUT_TOKENS)` — everything the
/// revise request carries besides the chapter chunk. So the chunk budget is
/// `window − overhead`, and each chapter is admitted while it still fits.
///
/// If the very first (smallest available, since order is fixed we take the head)
/// chapter can't fit even alone, the running bible has grown past what leaves room
/// for progress → [`FoldStep::HardLimit`]. Otherwise at least one chapter is always
/// taken, so the fold strictly advances and terminates.
pub fn next_fold_chunk(remaining: &[ChapterSize], window: u32, overhead: u32) -> FoldStep {
    let budget = window.saturating_sub(overhead);
    let mut chunk = Vec::new();
    let mut used = 0u32;
    for ch in remaining {
        // A single chapter can exceed the whole budget; admit the first one anyway
        // so the fold can't stall — the pre-flight check still guards the request,
        // and a lone over-window chapter is the true hard limit handled below.
        if chunk.is_empty() {
            if ch.tokens > budget {
                break; // even one chapter doesn't fit → hard limit
            }
            chunk.push(ch.file.to_string());
            used = ch.tokens;
            continue;
        }
        if used.saturating_add(ch.tokens) > budget {
            break;
        }
        chunk.push(ch.file.to_string());
        used += ch.tokens;
    }
    if chunk.is_empty() {
        FoldStep::HardLimit
    } else {
        FoldStep::Chunk(chunk)
    }
}

// --- The per-project state file (bible/.state.toml) -------------------------

/// What one tracker was last built from.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
pub struct PassState {
    /// Generation date of the last build (informational).
    #[serde(default)]
    pub built: String,
    /// The chapters (file → digest) the bible currently reflects.
    #[serde(default)]
    pub chapters: Digests,
}

/// The incremental-scoping state for a project: one [`PassState`] per tracker
/// (`"characters"`, `"setting"`, `"timeline"`), persisted as `bible/.state.toml`.
#[derive(Serialize, Deserialize, Default, Debug)]
pub struct State {
    #[serde(default)]
    pub trackers: BTreeMap<String, PassState>,
}

/// The state file path for a project.
fn state_path(root: &Path) -> PathBuf {
    root.join("bible").join(".state.toml")
}

impl State {
    /// Load the state file, or the empty default when it's missing/unreadable
    /// (best effort — a corrupt file just means the next pass rebuilds).
    pub fn load(root: &Path) -> Self {
        std::fs::read_to_string(state_path(root))
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// The chapters a tracker was last built from, if it has run.
    pub fn built_from(&self, tracker: &str) -> Option<&Digests> {
        self.trackers.get(tracker).map(|p| &p.chapters)
    }

    /// Record what a tracker was just built from and persist the file (creating
    /// `bible/`). Best effort: a write failure is returned for the caller to
    /// surface, but never blocks the pass result that already succeeded.
    pub fn record(
        root: &Path,
        tracker: &str,
        built: &str,
        chapters: Digests,
    ) -> std::io::Result<()> {
        let mut state = Self::load(root);
        state.trackers.insert(
            tracker.to_string(),
            PassState {
                built: built.to_string(),
                chapters,
            },
        );
        Self::save(root, &state)
    }

    /// Merge one book's chapter digests into a tracker's record (#339), leaving
    /// the other books' entries in place — so a series refresh updates the book it
    /// just folded in without forgetting what the earlier books were built from.
    ///
    /// `preserve` is the set of files owned by the *other* books; every recorded
    /// entry not in it belongs to the book being folded and is dropped before the
    /// fresh `chapters` go in. Extending alone would leave a deleted chapter's
    /// digest (and the tracked entries tied to it) stale until a full rebuild
    /// (#645); pruning this book's slice keeps the shared bible's record honest
    /// while still not re-scanning the other books.
    pub fn record_merge(
        root: &Path,
        tracker: &str,
        built: &str,
        chapters: Digests,
        preserve: &BTreeSet<String>,
    ) -> std::io::Result<()> {
        let mut state = Self::load(root);
        let entry = state.trackers.entry(tracker.to_string()).or_default();
        entry.built = built.to_string();
        // Drop this book's prior entries (anything not owned by another book), so a
        // deleted chapter doesn't linger, then fold in the book's current digests.
        entry.chapters.retain(|f, _| preserve.contains(f));
        entry.chapters.extend(chapters);
        Self::save(root, &state)
    }

    fn save(root: &Path, state: &Self) -> std::io::Result<()> {
        let path = state_path(root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = toml::to_string(state).map_err(std::io::Error::other)?;
        std::fs::write(path, body)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn digests(pairs: &[(&str, &str)]) -> Digests {
        pairs
            .iter()
            .map(|(f, d)| (f.to_string(), d.to_string()))
            .collect()
    }

    fn preserve(files: &[&str]) -> BTreeSet<String> {
        files.iter().map(std::string::ToString::to_string).collect()
    }

    #[test]
    fn record_merge_keeps_the_other_books_entries() {
        let dir = std::env::temp_dir().join(format!("quill-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Book one records; book two folds in via merge — book one survives (#339).
        // Book two's merge preserves book one's file ("b1/one.md").
        State::record(&dir, "characters", "d", digests(&[("b1/one.md", "a")])).unwrap();
        State::record_merge(
            &dir,
            "characters",
            "e",
            digests(&[("b2/one.md", "b")]),
            &preserve(&["b1/one.md"]),
        )
        .unwrap();
        let recorded = State::load(&dir).built_from("characters").unwrap().clone();
        assert!(
            recorded.contains_key("b1/one.md") && recorded.contains_key("b2/one.md"),
            "{recorded:?}"
        );
        // A plain record still replaces (single-book deletion handling, unchanged).
        State::record(&dir, "characters", "f", digests(&[("b1/one.md", "a")])).unwrap();
        let replaced = State::load(&dir).built_from("characters").unwrap().clone();
        assert!(
            !replaced.contains_key("b2/one.md"),
            "record replaces: {replaced:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn record_merge_prunes_this_books_deleted_chapters() {
        let dir = std::env::temp_dir().join(format!("quill-merge-prune-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Book one recorded two chapters; book two folded in beside it (#339).
        State::record(
            &dir,
            "characters",
            "d",
            digests(&[("b1/one.md", "a"), ("b1/two.md", "b")]),
        )
        .unwrap();
        State::record_merge(
            &dir,
            "characters",
            "e",
            digests(&[("b2/one.md", "c")]),
            &preserve(&["b1/one.md", "b1/two.md"]),
        )
        .unwrap();
        // The author deletes book two's chapter, then re-folds book two: its now-
        // empty digest set must drop the stale "b2/one.md" entry (#645), while book
        // one (preserved) stays intact.
        State::record_merge(
            &dir,
            "characters",
            "f",
            digests(&[]),
            &preserve(&["b1/one.md", "b1/two.md"]),
        )
        .unwrap();
        let recorded = State::load(&dir).built_from("characters").unwrap().clone();
        assert!(
            !recorded.contains_key("b2/one.md"),
            "book two's deleted chapter pruned: {recorded:?}"
        );
        assert!(
            recorded.contains_key("b1/one.md") && recorded.contains_key("b1/two.md"),
            "book one's entries survive the merge: {recorded:?}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    fn sizes<'a>(pairs: &[(&'a str, u32)]) -> Vec<ChapterSize<'a>> {
        pairs
            .iter()
            .map(|&(file, tokens)| ChapterSize { file, tokens })
            .collect()
    }

    #[test]
    fn fold_chunk_packs_whole_chapters_up_to_the_budget() {
        // window 100, overhead 40 → budget 60. Chapters 30,20,20,50: the first three
        // fit (70? no — 30+20=50, +20=70 > 60), so 30+20 fit, the third 20 overflows.
        let chapters = sizes(&[("a", 30), ("b", 20), ("c", 20), ("d", 50)]);
        assert_eq!(
            next_fold_chunk(&chapters, 100, 40),
            FoldStep::Chunk(vec!["a".into(), "b".into()]),
            "packs whole chapters only up to the budget, never splitting one"
        );
    }

    #[test]
    fn fold_chunk_boundary_is_a_whole_chapter() {
        // budget 60; a single 60-token chapter fills it exactly, the next is a new
        // chunk — the boundary lands on the chapter seam, never mid-chapter.
        let chapters = sizes(&[("a", 60), ("b", 10)]);
        assert_eq!(
            next_fold_chunk(&chapters, 100, 40),
            FoldStep::Chunk(vec!["a".into()])
        );
    }

    #[test]
    fn fold_takes_at_least_one_chapter_so_it_advances() {
        // A lone chapter larger than the budget is still admitted (so the fold can't
        // stall); the pre-flight window check guards the actual request.
        let chapters = sizes(&[("big", 55)]);
        assert_eq!(
            next_fold_chunk(&chapters, 100, 40), // budget 60, 55 fits
            FoldStep::Chunk(vec!["big".into()])
        );
    }

    #[test]
    fn fold_hard_limit_when_the_running_bible_leaves_no_room() {
        // The running bible has grown so overhead (95) nearly fills the window (100):
        // budget 5, and even the smallest next chapter (10) can't fit → hard limit,
        // which the caller turns into the graceful "too large" report.
        let chapters = sizes(&[("a", 10), ("b", 20)]);
        assert_eq!(next_fold_chunk(&chapters, 100, 95), FoldStep::HardLimit);
    }

    #[test]
    fn folding_the_whole_manuscript_covers_every_chapter_in_order() {
        // Drive the planner to convergence with a fixed overhead and confirm the
        // chunks partition the chapters, in order, on seams.
        let all = sizes(&[
            ("one", 20),
            ("two", 20),
            ("three", 20),
            ("four", 20),
            ("five", 20),
        ]);
        let mut remaining = all.clone();
        let mut folded: Vec<String> = Vec::new();
        while !remaining.is_empty() {
            match next_fold_chunk(&remaining, 100, 55) {
                // budget 45 → two 20-token chapters per chunk
                FoldStep::Chunk(files) => {
                    assert!(!files.is_empty());
                    remaining.drain(0..files.len());
                    folded.extend(files);
                }
                FoldStep::HardLimit => panic!("should fit at this overhead"),
            }
        }
        assert_eq!(
            folded,
            vec!["one", "two", "three", "four", "five"],
            "every chapter folded once, in manuscript order"
        );
    }

    #[test]
    fn with_prior_passages_prepends_a_labeled_reference_section() {
        // Retrieval-backed context (#929): the retrieved prior passages are folded in
        // ahead of the changed chapters, under a clearly-labeled reference heading, so
        // the model can check the change against them without mistaking them for the
        // material to revise from. The changed chapters still follow, intact.
        let retrieved = "# The Locket\n\nShe clutched the silver locket.\n";
        let changed = "# The Duel\n\nHe drew the gold locket.\n";
        let context = with_prior_passages(retrieved, changed);
        assert!(
            context.contains(PRIOR_PASSAGES_HEADING),
            "the prior passages are under their labeled heading: {context}"
        );
        assert!(
            context.contains("silver locket"),
            "the retrieved prior passage is present as context"
        );
        assert!(
            context.contains("gold locket"),
            "the changed chapter is still present"
        );
        // The prior passages come before the changed chapters (reference, then the
        // material to revise from).
        let prior_at = context.find("silver locket").unwrap();
        let changed_at = context.find("gold locket").unwrap();
        assert!(prior_at < changed_at, "prior passages lead: {context}");
    }

    #[test]
    fn with_prior_passages_empty_retrieval_is_the_plain_incremental_context() {
        // No index / no hits → the changed chapters alone, byte-identical to the
        // classic incremental path, so retrieval degrades gracefully (#929).
        let changed = "# The Duel\n\nHe drew the gold locket.\n";
        assert_eq!(with_prior_passages("", changed), changed);
        assert_eq!(with_prior_passages("   \n  ", changed), changed);
    }

    #[test]
    fn revise_request_carries_the_retrieved_prior_context() {
        // End to end at the request level: a contradiction only detectable against a
        // prior scene (a locket silver in ch.1, gold in the changed ch.2) — the plain
        // incremental request can't see ch.1, but the retrieval-backed one folds it in
        // as context, so the revise pass has the passage the contradiction needs.
        let prior_bible = "## Timeline\n- The locket is silver.\n";
        let changed = "# The Duel\n\nHe drew the gold locket.\n";
        let retrieved = "# The Locket\n\nShe clutched the silver locket.\n";

        // Plain incremental: the changed chapter only — no prior scene in context.
        let plain = revise_request(
            "sys",
            json!({}),
            256,
            prior_bible,
            changed,
            "claude-opus-4-8",
        );
        assert!(
            !plain.cached_context.contains("silver locket"),
            "the plain path is blind to the prior locket scene"
        );

        // Retrieval-backed: the same request shape, but the prior scene is now in the
        // context, labeled as reference — at least as much to check against as before.
        let context = with_prior_passages(retrieved, changed);
        let backed = revise_request(
            "sys",
            json!({}),
            256,
            prior_bible,
            &context,
            "claude-opus-4-8",
        );
        assert!(
            backed.cached_context.contains("silver locket"),
            "the retrieval-backed path carries the prior scene the contradiction needs"
        );
        assert!(
            backed.cached_context.contains("gold locket"),
            "and still carries the changed chapter"
        );
        assert!(
            backed.cached_context.contains(PRIOR_PASSAGES_HEADING),
            "the prior scene is labeled as reference, not changed material"
        );
    }

    #[test]
    fn first_run_with_no_prior_is_full() {
        let cur = digests(&[("one.md", "a"), ("two.md", "b")]);
        assert_eq!(decide(None, &cur, 5), Scope::Full);
    }

    #[test]
    fn no_change_is_up_to_date() {
        let prior = digests(&[("one.md", "a"), ("two.md", "b")]);
        let cur = prior.clone();
        assert_eq!(decide(Some(&prior), &cur, 5), Scope::UpToDate);
    }

    #[test]
    fn one_changed_chapter_is_incremental() {
        let prior = digests(&[("one.md", "a"), ("two.md", "b")]);
        let cur = digests(&[("one.md", "a"), ("two.md", "B!")]); // two changed
        assert_eq!(
            decide(Some(&prior), &cur, 5),
            Scope::Incremental(vec!["two.md".into()])
        );
    }

    #[test]
    fn a_new_chapter_counts_as_changed() {
        let prior = digests(&[("one.md", "a")]);
        let cur = digests(&[("one.md", "a"), ("two.md", "b")]); // two added
        assert_eq!(
            decide(Some(&prior), &cur, 5),
            Scope::Incremental(vec!["two.md".into()])
        );
    }

    #[test]
    fn past_the_threshold_rebuilds_fully() {
        let prior = digests(&[("a", "1"), ("b", "2"), ("c", "3")]);
        let cur = digests(&[("a", "x"), ("b", "y"), ("c", "z")]); // 3 changed
        assert_eq!(decide(Some(&prior), &cur, 3), Scope::Full);
        assert!(matches!(
            decide(Some(&prior), &cur, 4),
            Scope::Incremental(_)
        ));
    }

    #[test]
    fn a_removed_chapter_forces_full() {
        let prior = digests(&[("one.md", "a"), ("two.md", "b")]);
        let cur = digests(&[("one.md", "a")]); // two removed
        assert_eq!(decide(Some(&prior), &cur, 5), Scope::Full);
    }

    #[test]
    fn state_round_trips_through_the_file() {
        let mut root = std::env::temp_dir();
        root.push(format!("quill-incr-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        assert!(State::load(&root).built_from("characters").is_none());
        State::record(
            &root,
            "characters",
            "2026-06-21",
            digests(&[("one.md", "a")]),
        )
        .unwrap();
        let state = State::load(&root);
        assert_eq!(
            state.built_from("characters"),
            Some(&digests(&[("one.md", "a")]))
        );
        assert!(state.built_from("setting").is_none());

        std::fs::remove_dir_all(&root).ok();
    }
}
