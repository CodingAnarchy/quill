//! Durable "Ask the manuscript" Q&A recall (#654): the grounded-ask history
//! (#609) — each question, its answer prose, and the chapters it cited —
//! persisted per project as `bible/qa-log.toml` so a past lookup and its jumpable
//! citations survive a restart. The session log was rebuilt empty each launch;
//! this reloads it on open and appends each fresh ask.
//!
//! This stays within the AI boundary: it persists analysis recall (the author's
//! own questions, the grounded answers, and the chapters cited), never manuscript
//! prose. The answer text is the AI's off-manuscript reply about the book, the
//! same content #609 already held in memory — nothing here is written back into a
//! chapter.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::assist::QaEntry;
use crate::ai::Answer;

/// The most recent asks to keep on disk. Q&A recall is a "return to a past
/// lookup" convenience, not an archive, so cap the log to keep the file small and
/// the reloaded history browsable; older asks age out oldest-first.
const MAX_ENTRIES: usize = 200;

/// One persisted ask: the question, the grounded answer prose, and the cited
/// chapter titles — everything [`QaEntry`] carries, in a shape that round-trips
/// through TOML. Kept separate from [`Answer`] (which only derives `Deserialize`)
/// so the store owns its own serialization.
#[derive(Serialize, Deserialize, Clone, Default)]
struct Record {
    #[serde(default)]
    question: String,
    #[serde(default)]
    answer: String,
    #[serde(default)]
    citations: Vec<String>,
}

impl Record {
    fn from(entry: &QaEntry) -> Self {
        Record {
            question: entry.question.clone(),
            answer: entry.answer.answer.clone(),
            citations: entry.answer.citations.clone(),
        }
    }

    fn entry(&self) -> QaEntry {
        QaEntry {
            question: self.question.clone(),
            answer: Answer {
                answer: self.answer.clone(),
                citations: self.citations.clone(),
            },
        }
    }
}

/// The on-disk Q&A log: every remembered ask, oldest first (matching the
/// in-session `qa_log` order). Serialized as `bible/qa-log.toml`.
#[derive(Serialize, Deserialize, Default)]
pub(super) struct QaStore {
    #[serde(default)]
    entries: Vec<Record>,
}

fn store_path(root: &Path) -> PathBuf {
    root.join("bible").join("qa-log.toml")
}

impl QaStore {
    /// Load the store, or the empty default when it's missing/unreadable (best
    /// effort — a corrupt file just starts recall fresh, never blocks the editor).
    pub(super) fn load(root: &Path) -> QaStore {
        std::fs::read_to_string(store_path(root))
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    fn save(&self, root: &Path) {
        let path = store_path(root);
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        if let Ok(body) = toml::to_string(self) {
            let _ = std::fs::write(path, body);
        }
    }

    /// The remembered asks as session [`QaEntry`]s, oldest first — the Q&A log to
    /// restore at startup.
    pub(super) fn entries(&self) -> Vec<QaEntry> {
        self.entries.iter().map(Record::entry).collect()
    }

    /// Append a fresh ask and persist, capping the log to the most recent
    /// [`MAX_ENTRIES`] so the file stays small (older asks age out oldest-first).
    pub(super) fn append(&mut self, root: &Path, entry: &QaEntry) {
        self.entries.push(Record::from(entry));
        let overflow = self.entries.len().saturating_sub(MAX_ENTRIES);
        if overflow > 0 {
            self.entries.drain(0..overflow);
        }
        self.save(root);
    }
}

#[cfg(test)]
mod tests {
    use super::super::assist::QaEntry;
    use super::{QaStore, MAX_ENTRIES};
    use crate::ai::Answer;

    fn entry(question: &str, answer: &str, citations: &[&str]) -> QaEntry {
        QaEntry {
            question: question.into(),
            answer: Answer {
                answer: answer.into(),
                citations: citations.iter().map(|c| c.to_string()).collect(),
            },
        }
    }

    #[test]
    fn ask_persists_with_citations_across_reload() {
        let dir = crate::testutil::scratch_dir("qa-store", "reload");
        let mut store = QaStore::load(&dir);
        store.append(
            &dir,
            &entry(
                "what did I establish about the locket?",
                "It changes color.",
                &["One", "Three"],
            ),
        );

        // Reload: the ask, its answer prose, and its citations all come back.
        let reloaded = QaStore::load(&dir);
        let entries = reloaded.entries();
        assert_eq!(entries.len(), 1);
        assert_eq!(
            entries[0].question,
            "what did I establish about the locket?"
        );
        assert_eq!(entries[0].answer.answer, "It changes color.");
        assert_eq!(entries[0].answer.citations, vec!["One", "Three"]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn order_is_preserved_oldest_first() {
        let dir = crate::testutil::scratch_dir("qa-store", "order");
        let mut store = QaStore::load(&dir);
        store.append(&dir, &entry("first", "a1", &[]));
        store.append(&dir, &entry("second", "a2", &[]));

        let entries = QaStore::load(&dir).entries();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].question, "first");
        assert_eq!(entries[1].question, "second");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn log_is_capped_to_the_most_recent_entries() {
        let dir = crate::testutil::scratch_dir("qa-store", "cap");
        let mut store = QaStore::load(&dir);
        for i in 0..(MAX_ENTRIES + 5) {
            store.append(&dir, &entry(&format!("q{i}"), "a", &[]));
        }

        let entries = QaStore::load(&dir).entries();
        assert_eq!(
            entries.len(),
            MAX_ENTRIES,
            "log caps at the most recent asks"
        );
        // The oldest five aged out; the newest is still last.
        assert_eq!(entries[0].question, "q5");
        assert_eq!(
            entries.last().unwrap().question,
            format!("q{}", MAX_ENTRIES + 4)
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
