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

use crate::atomic;
use crate::mode::{Finding, FindingKind};

/// A content key for a finding — stable across sessions and re-runs, derived from
/// what the finding *says* (kind + summary + chapters + book), not its list
/// position. A re-produced finding hashes to the same key, so a prior dismissal
/// still applies; the `kind` prefix (#827) keeps two passes that flag the same
/// span with coincidentally identical text from colliding.
fn key(f: &Finding) -> String {
    // Kind, then book index, then summary, then chapters — joined with
    // control-character separators that can't occur in the fields' own text, so
    // distinct findings never collide.
    format!("{}\u{1f}{}", f.kind.slug(), span_key(f))
}

/// The *span* key — book, summary, and chapters, **without** the kind (#921). A
/// dismissal is anchored to the span, so dismissing a finding one pass raised also
/// suppresses the same span another pass (continuity + plot on one dangling
/// thread) re-raises. It's still derived from what the finding *says* — the summary
/// carries the flagged detail and the chapters the location — so it stays keyed to
/// the underlying text: change the prose and the summary/chapters shift, the span
/// key changes, and the finding legitimately re-surfaces.
fn span_key(f: &Finding) -> String {
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
    /// The finding's category slug (#827) — persisted so a restored finding keeps
    /// its pass even though `action` is lost. Absent in a pre-#827 file, defaulting
    /// to `"other"` there (unknown slugs resolve to [`FindingKind::Other`]).
    #[serde(default)]
    kind: String,
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
        Self {
            summary: f.summary.clone(),
            kind: f.kind.slug().to_string(),
            chapters: f.chapters.clone(),
            book: f.book,
            dismissed,
        }
    }

    fn finding(&self) -> Finding {
        Finding {
            summary: self.summary.clone(),
            kind: FindingKind::from_slug(&self.kind),
            chapters: self.chapters.clone(),
            book: self.book,
            // The store records content, not the originating pass, so a restored
            // finding can't be rechecked until a fresh pass re-attributes it (#608).
            action: None,
            // Cross-pass consensus (#921) is recomputed by the health view from the
            // live set each run, not persisted; a restored finding starts lone.
            corroborators: Vec::new(),
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

/// The project-relative path of the findings store, so a caller can read the same
/// file out of git history (a past milestone or branch) via
/// [`crate::vcs::show_file_at`] — the basis for the findings diff (#1051).
pub(super) const fn store_rel_path() -> &'static str {
    "bible/findings.toml"
}

/// How a finding changed between two points in history (#1051). Each finding —
/// keyed by its content, the same span/kind identity the store dedups on — lands
/// in exactly one bucket relative to the two chosen points.
#[derive(Default, Debug)]
pub(super) struct FindingsDiff {
    /// Open at A, gone at B — the revision closed these out.
    pub(super) resolved: Vec<Finding>,
    /// New at B, absent at A — the work surfaced (or created) these.
    pub(super) introduced: Vec<Finding>,
    /// Open at both points — carried through unchanged.
    pub(super) still_open: Vec<Finding>,
}

impl FindingsDiff {
    /// No change at all between the two points — nothing resolved, introduced, or
    /// even carried over (both stores empty, or identical *and* empty).
    pub(super) const fn is_empty(&self) -> bool {
        self.resolved.is_empty() && self.introduced.is_empty() && self.still_open.is_empty()
    }
}

/// Classify the open findings of two history points (#1051): purely local, over
/// two already-generated stores — no AI request, no model call, no prose. A
/// finding open at A but not B is **resolved**; open at B but not A is
/// **introduced**; open at both is **still open**. Identity is the store's own
/// content key (kind + span), so a re-produced finding matches across the two
/// points exactly as a re-run matches a prior dismissal. Dismissed records don't
/// count as open on either side, so they never appear in the diff.
pub(super) fn diff_findings(before: &FindingsStore, after: &FindingsStore) -> FindingsDiff {
    let a: std::collections::HashSet<String> = before.open().iter().map(key).collect();
    let b: std::collections::HashSet<String> = after.open().iter().map(key).collect();
    let mut diff = FindingsDiff::default();
    for f in before.open() {
        // Resolved: open at the earlier point, gone by the later one.
        if !b.contains(&key(&f)) {
            diff.resolved.push(f);
        }
    }
    for f in after.open() {
        if a.contains(&key(&f)) {
            diff.still_open.push(f);
        } else {
            diff.introduced.push(f);
        }
    }
    diff
}

impl FindingsStore {
    /// Load the store, or the empty default when it's missing/unreadable (best
    /// effort — a corrupt file just starts triage fresh, never blocks the editor).
    pub(super) fn load(root: &Path) -> Self {
        std::fs::read_to_string(store_path(root))
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Parse a store from raw TOML — the store as it existed at a past point, read
    /// out of git history (the blob at a milestone or branch) rather than off disk
    /// (#1051). Unparseable or empty text yields the empty store, matching
    /// [`load`](Self::load)'s best-effort behavior, so a point that predates the
    /// store (or has a corrupt one) simply diffs as "no findings there".
    pub(super) fn from_toml(text: &str) -> Self {
        toml::from_str(text).unwrap_or_default()
    }

    fn save(&self, root: &Path) {
        let path = store_path(root);
        if let Some(parent) = path.parent() {
            if std::fs::create_dir_all(parent).is_err() {
                return;
            }
        }
        if let Ok(body) = toml::to_string(self) {
            let _ = atomic::write(&path, body.as_bytes());
        }
    }

    fn position(&self, f: &Finding) -> Option<usize> {
        let k = key(f);
        self.findings.iter().position(|r| key(&r.finding()) == k)
    }

    /// The still-open (not dismissed) findings, as UI findings — the triage list
    /// to restore at startup.
    ///
    /// A record whose `chapters` is empty is skipped: `bible/findings.toml` is
    /// hand-editable and the deserializer accepts any Vec, but the triage UI's
    /// "first chapter is the jump target" logic presupposes a non-empty list. An
    /// empty-chapters record has no jump target and can't be acted on, so we drop
    /// it here rather than restore an un-jumpable (and potentially panic-inducing
    /// on a `.first()`-less path) finding.
    pub(super) fn open(&self) -> Vec<Finding> {
        self.findings
            .iter()
            .filter(|r| !r.dismissed && !r.chapters.is_empty())
            .map(Record::finding)
            .collect()
    }

    /// Whether a finding has been dismissed (so a re-run shouldn't re-surface it).
    /// Matched by *span*, not the exact pass (#921): a dismissal the author made on
    /// one pass's finding also suppresses the same span raised by a corroborating
    /// pass, since the two describe one issue. The span key still tracks the
    /// underlying text, so a prose edit that changes the summary/chapters lets the
    /// finding re-surface.
    pub(super) fn is_dismissed(&self, f: &Finding) -> bool {
        let span = span_key(f);
        self.findings
            .iter()
            .any(|r| r.dismissed && span_key(&r.finding()) == span)
    }

    /// Record a finding as seen (open) if it's new, persisting. A finding already
    /// on file — open or dismissed — keeps its state untouched, so this never
    /// resurrects a dismissal. Returns whether the store was actually written, so
    /// the caller can auto-commit the store (#1060) only when a pass changed it.
    pub(super) fn remember(&mut self, root: &Path, f: &Finding) -> bool {
        if self.position(f).is_none() {
            self.findings.push(Record::from(f, false));
            self.save(root);
            return true;
        }
        false
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
    use super::{diff_findings, store_path, FindingsStore};
    use crate::atomic;
    use crate::mode::{Finding, FindingKind};

    fn finding(summary: &str) -> Finding {
        finding_of(FindingKind::Continuity, summary)
    }

    /// A finding of a specific pass on chapter "One" — for the cross-pass dismissal
    /// tests, where the pass (kind) varies but the span stays the same.
    fn finding_of(kind: FindingKind, summary: &str) -> Finding {
        Finding {
            summary: summary.into(),
            kind,
            chapters: vec!["One".into()],
            book: None,
            action: None,
            corroborators: Vec::new(),
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

    // A dismissal is anchored to the *span* (#921): the author dismisses a dangling
    // thread the continuity pass raised, and when the plot pass later flags the same
    // span it's suppressed too — one triage decision covers the issue, whichever
    // pass re-surfaces it.
    #[test]
    fn dismissal_suppresses_the_same_span_from_another_pass() {
        let dir = crate::testutil::scratch_dir("findings-store", "cross-pass-dismiss");
        let same = "a dangling thread";
        let mut store = FindingsStore::load(&dir);
        let continuity = finding_of(FindingKind::Continuity, same);
        store.remember(&dir, &continuity);
        store.dismiss(&dir, &continuity);

        // A different pass raises the identical span: still recognized as dismissed.
        let plot = finding_of(FindingKind::Plot, same);
        let reloaded = FindingsStore::load(&dir);
        assert!(
            reloaded.is_dismissed(&plot),
            "a dismissal on one pass suppresses the same span on another"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    // The span key tracks the underlying text, so an edit that changes the finding's
    // summary (the flagged detail) re-surfaces it — a dismissal doesn't outlive the
    // prose it was about (#921).
    #[test]
    fn a_changed_span_is_not_suppressed_by_an_old_dismissal() {
        let dir = crate::testutil::scratch_dir("findings-store", "resurface-on-change");
        let mut store = FindingsStore::load(&dir);
        let before = finding("the locket is silver in ch.1");
        store.remember(&dir, &before);
        store.dismiss(&dir, &before);

        // After a revision the pass flags a different detail: a new span, so the old
        // dismissal doesn't reach it.
        let after = finding("the locket is gold in ch.1");
        let reloaded = FindingsStore::load(&dir);
        assert!(
            !reloaded.is_dismissed(&after),
            "a dismissal keyed to the old text doesn't suppress the revised finding"
        );
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

    // Routing the store through the atomic layer (#694) means an interrupted save
    // can't truncate it: a write faulted before its rename leaves the prior good
    // file byte-for-byte intact. We arm the pre-rename fault seam, force a save
    // (via `remember`), then confirm the on-disk store still holds the earlier
    // finding rather than an empty or half-written file.
    #[test]
    fn interrupted_save_preserves_the_prior_store() {
        let dir = crate::testutil::scratch_dir("findings-store", "atomic-preserve");
        let mut store = FindingsStore::load(&dir);
        store.remember(&dir, &finding("the good version")); // writes the seed store
        let good = std::fs::read_to_string(store_path(&dir)).unwrap();
        assert!(good.contains("the good version"));

        // Arm a crash before the next write's rename, then trigger a save.
        atomic::fail_point::arm_pre_rename_failure();
        store.remember(&dir, &finding("a doomed addition"));

        // The seed store survives unchanged; the doomed addition never landed.
        assert_eq!(
            std::fs::read_to_string(store_path(&dir)).unwrap(),
            good,
            "the previous store must survive an interrupted save"
        );
        let reloaded = FindingsStore::load(&dir);
        assert!(!reloaded.is_dismissed(&finding("a doomed addition")));
        assert_eq!(reloaded.open().len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // --- Findings diff between two history points (#1051) --------------------

    /// Build a store holding the given open findings — the store as it stood at one
    /// point in history, for the diff tests. Round-trips through TOML so it's exactly
    /// what [`FindingsStore::from_toml`] parses out of a git blob.
    fn store_with(findings: &[Finding]) -> FindingsStore {
        let mut store = FindingsStore::default();
        for f in findings {
            store.findings.push(super::Record::from(f, false));
        }
        FindingsStore::from_toml(&toml::to_string(&store).unwrap())
    }

    // The core classification (#1051): a finding open at A but gone at B is resolved,
    // one new at B is introduced, one at both stays open — a pure local comparison of
    // two already-generated stores, no AI request anywhere in reach.
    #[test]
    fn diff_classifies_resolved_introduced_and_still_open() {
        let carried = finding("the locket keeps changing color");
        let before = store_with(&[finding("a plot hole in ch.1"), carried.clone()]);
        let after = store_with(&[carried, finding("a new dangling thread")]);

        let diff = diff_findings(&before, &after);

        let summaries = |fs: &[Finding]| fs.iter().map(|f| f.summary.clone()).collect::<Vec<_>>();
        assert_eq!(
            summaries(&diff.resolved),
            vec!["a plot hole in ch.1".to_string()],
            "gone at B → resolved"
        );
        assert_eq!(
            summaries(&diff.introduced),
            vec!["a new dangling thread".to_string()],
            "new at B → introduced"
        );
        assert_eq!(
            summaries(&diff.still_open),
            vec!["the locket keeps changing color".to_string()],
            "present at both → still open"
        );
    }

    // Two empty stores diff cleanly (#1051): no findings on either side means every
    // bucket is empty and the diff reports itself as empty — the "nothing to compare"
    // case the UI turns into a message rather than an overlay.
    #[test]
    fn diff_of_two_empty_stores_is_empty() {
        let diff = diff_findings(&FindingsStore::default(), &FindingsStore::default());
        assert!(diff.resolved.is_empty());
        assert!(diff.introduced.is_empty());
        assert!(diff.still_open.is_empty());
        assert!(
            diff.is_empty(),
            "no findings at either point → an empty diff"
        );
    }

    // A dismissed finding isn't "open" on either side, so it never shows in the diff
    // (#1051) — the diff measures the open triage list, matching what the author acts
    // on, not the full history of every flag ever seen.
    #[test]
    fn dismissed_findings_are_excluded_from_the_diff() {
        let dir = crate::testutil::scratch_dir("findings-store", "diff-dismissed");
        let mut before = FindingsStore::load(&dir);
        let dismissed = finding("dismissed noise");
        before.remember(&dir, &finding("real open finding"));
        before.remember(&dir, &dismissed);
        before.dismiss(&dir, &dismissed);

        // Point B keeps only the real finding open (the dismissed one is gone).
        let after = store_with(&[finding("real open finding")]);
        let diff = diff_findings(&before, &after);

        assert!(
            diff.resolved.is_empty(),
            "a dismissed finding was never open, so dropping it isn't a resolution"
        );
        assert_eq!(diff.still_open.len(), 1, "the real finding carries through");
        assert!(diff.introduced.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }

    // `bible/findings.toml` is hand-editable, and the deserializer accepts a record
    // with an empty `chapters` list — but the triage UI presupposes a non-empty list
    // (the first chapter is the jump target). Loading such a store must not panic, and
    // `open()` skips the un-jumpable record while keeping the well-formed ones.
    #[test]
    fn empty_chapters_record_loads_without_panicking_and_is_skipped() {
        let body = "\
[[findings]]
summary = \"a finding with no chapters\"
kind = \"continuity\"
chapters = []

[[findings]]
summary = \"a well-formed finding\"
kind = \"continuity\"
chapters = [\"One\"]
";
        let store = FindingsStore::from_toml(body);
        let open = store.open();
        assert_eq!(
            open.len(),
            1,
            "the empty-chapters record is skipped; the well-formed one survives"
        );
        assert_eq!(open[0].summary, "a well-formed finding");
        assert_eq!(open[0].chapters, vec!["One".to_string()]);
    }

    // from_toml is the store-at-a-ref reader (#1051): it parses a real store body and
    // shrugs off a corrupt or empty one as the empty store, exactly like the on-disk
    // load — so a point that predates the findings file diffs as "nothing there".
    #[test]
    fn from_toml_parses_a_store_and_tolerates_junk() {
        let store = store_with(&[finding("a finding")]);
        let body = toml::to_string(&store).unwrap();
        assert_eq!(FindingsStore::from_toml(&body).open().len(), 1);
        assert!(FindingsStore::from_toml("not valid toml @#$")
            .open()
            .is_empty());
        assert!(FindingsStore::from_toml("").open().is_empty());
    }
}
