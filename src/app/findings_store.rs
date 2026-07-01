//! Durable revision-triage state (#591): the accumulated findings list and the
//! author's dismiss/resolve decisions, persisted per project as
//! `bible/findings.toml` so triage works as a standing to-do list across
//! sessions.
//!
//! Findings are content-keyed by summary + chapters (+ book, for a series), so a
//! re-run that re-produces a finding the author already dismissed doesn't
//! re-surface it, and a finding the author neither marked nor fixed survives a
//! restart. This stays within the AI boundary: it persists flags and the
//! author's triage decisions, never prose.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::mode::Finding;

/// A content key for a finding — stable across sessions and re-runs, derived from
/// what the finding *says* (summary + chapters + book), not its list position. A
/// re-produced finding hashes to the same key, so a prior dismissal still
/// applies.
fn key(f: &Finding) -> String {
    // Book index, then summary, then chapters — joined with control-character
    // separators that can't occur in the fields' own text, so distinct findings
    // never collide.
    let book = f.book.map(|b| b.to_string()).unwrap_or_default();
    format!(
        "{book}\u{1f}{}\u{1f}{}",
        f.summary,
        f.chapters.join("\u{1e}")
    )
}

/// One persisted finding: the display fields plus whether the author has
/// dismissed it. Resolved findings (marked `xxx`) are dropped from the file
/// entirely, so presence-with-`dismissed=false` means "still open triage".
#[derive(Serialize, Deserialize, Clone)]
struct Record {
    summary: String,
    #[serde(default)]
    chapters: Vec<String>,
    #[serde(default)]
    book: Option<usize>,
    /// The author dismissed this finding; keep the record so a re-run doesn't
    /// re-surface it, but don't show it in triage.
    #[serde(default)]
    dismissed: bool,
}

impl Record {
    fn from(f: &Finding, dismissed: bool) -> Self {
        Record {
            summary: f.summary.clone(),
            chapters: f.chapters.clone(),
            book: f.book,
            dismissed,
        }
    }

    fn finding(&self) -> Finding {
        Finding {
            summary: self.summary.clone(),
            chapters: self.chapters.clone(),
            book: self.book,
            // The store records content, not the originating pass, so a restored
            // finding can't be rechecked until a fresh pass re-attributes it (#608).
            action: None,
        }
    }
}

/// The on-disk triage store: every finding the project has seen and its dismiss
/// state, keyed by content. Serialized as `bible/findings.toml`.
#[derive(Serialize, Deserialize, Default)]
pub(super) struct FindingsStore {
    #[serde(default)]
    findings: Vec<Record>,
}

fn store_path(root: &Path) -> PathBuf {
    root.join("bible").join("findings.toml")
}

impl FindingsStore {
    /// Load the store, or the empty default when it's missing/unreadable (best
    /// effort — a corrupt file just starts triage fresh, never blocks the editor).
    pub(super) fn load(root: &Path) -> FindingsStore {
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

    fn position(&self, f: &Finding) -> Option<usize> {
        let k = key(f);
        self.findings.iter().position(|r| key(&r.finding()) == k)
    }

    /// The still-open (not dismissed) findings, as UI findings — the triage list
    /// to restore at startup.
    pub(super) fn open(&self) -> Vec<Finding> {
        self.findings
            .iter()
            .filter(|r| !r.dismissed)
            .map(Record::finding)
            .collect()
    }

    /// Whether a finding has been dismissed (so a re-run shouldn't re-surface it).
    pub(super) fn is_dismissed(&self, f: &Finding) -> bool {
        self.position(f).is_some_and(|i| self.findings[i].dismissed)
    }

    /// Record a finding as seen (open) if it's new, persisting. A finding already
    /// on file — open or dismissed — keeps its state untouched, so this never
    /// resurrects a dismissal.
    pub(super) fn remember(&mut self, root: &Path, f: &Finding) {
        if self.position(f).is_none() {
            self.findings.push(Record::from(f, false));
            self.save(root);
        }
    }

    /// Mark a finding dismissed and persist. The record is kept (not removed) so a
    /// later re-run of the same pass doesn't re-surface it.
    pub(super) fn dismiss(&mut self, root: &Path, f: &Finding) {
        match self.position(f) {
            Some(i) => self.findings[i].dismissed = true,
            None => self.findings.push(Record::from(f, true)),
        }
        self.save(root);
    }

    /// Drop a resolved finding (the author marked it `xxx`) and persist. Unlike a
    /// dismissal, a resolved finding leaves no record — if the underlying issue
    /// recurs in a later manuscript, a fresh pass *should* surface it again.
    pub(super) fn resolve(&mut self, root: &Path, f: &Finding) {
        if let Some(i) = self.position(f) {
            self.findings.remove(i);
            self.save(root);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FindingsStore;
    use crate::mode::Finding;

    fn finding(summary: &str) -> Finding {
        Finding {
            summary: summary.into(),
            chapters: vec!["One".into()],
            book: None,
            action: None,
        }
    }

    #[test]
    fn dismiss_persists_content_keyed_across_reload() {
        let dir = crate::testutil::scratch_dir("findings-store", "dismiss");
        let f = finding("the locket changes color");
        let mut store = FindingsStore::load(&dir);
        store.remember(&dir, &f);
        store.dismiss(&dir, &f);

        // Reload: a distinct `Finding` value that re-produces the same content is
        // recognized as dismissed, and doesn't come back as open.
        let reloaded = FindingsStore::load(&dir);
        assert!(reloaded.is_dismissed(&finding("the locket changes color")));
        assert!(reloaded.open().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn open_findings_survive_reload_but_resolved_ones_drop() {
        let dir = crate::testutil::scratch_dir("findings-store", "open-resolve");
        let open = finding("still open");
        let done = finding("resolved");
        let mut store = FindingsStore::load(&dir);
        store.remember(&dir, &open);
        store.remember(&dir, &done);
        store.resolve(&dir, &done); // marked xxx → drops off entirely

        let reloaded = FindingsStore::load(&dir);
        let restored = reloaded.open();
        assert_eq!(restored.len(), 1, "only the un-triaged finding survives");
        assert_eq!(restored[0].summary, "still open");
        // A resolved finding leaves no record, so a re-run may surface it again.
        assert!(!reloaded.is_dismissed(&done));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
