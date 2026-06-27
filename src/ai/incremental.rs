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

use std::collections::BTreeMap;
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

/// Build a "revise this bible" request (#90): the existing bible plus only the
/// changed chapters as cached context, with the same schema as the full pass so
/// the model returns a complete, replacement bible.
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

// --- The per-project state file (bible/.state.toml) -------------------------

/// What one tracker was last built from.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq)]
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
    pub fn load(root: &Path) -> State {
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
        let mut state = State::load(root);
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
    /// (A book's deleted chapters leave a stale entry until a full rebuild; that's
    /// the series trade-off for not re-scanning every book.)
    pub fn record_merge(
        root: &Path,
        tracker: &str,
        built: &str,
        chapters: Digests,
    ) -> std::io::Result<()> {
        let mut state = State::load(root);
        let entry = state.trackers.entry(tracker.to_string()).or_default();
        entry.built = built.to_string();
        entry.chapters.extend(chapters);
        Self::save(root, &state)
    }

    fn save(root: &Path, state: &State) -> std::io::Result<()> {
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
    use super::*;

    fn digests(pairs: &[(&str, &str)]) -> Digests {
        pairs
            .iter()
            .map(|(f, d)| (f.to_string(), d.to_string()))
            .collect()
    }

    #[test]
    fn record_merge_keeps_the_other_books_entries() {
        let dir = std::env::temp_dir().join(format!("quill-merge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        // Book one records; book two folds in via merge — book one survives (#339).
        State::record(&dir, "characters", "d", digests(&[("b1/one.md", "a")])).unwrap();
        State::record_merge(&dir, "characters", "e", digests(&[("b2/one.md", "b")])).unwrap();
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
