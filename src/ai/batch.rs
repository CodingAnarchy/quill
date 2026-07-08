//! The durable in-flight batch-update record (#950): what the opt-in background
//! bible update submitted to the Message Batches API and still has to reconcile.
//!
//! The background update (#950) is the async counterpart to the interactive
//! all-bibles refresh (#655/#955). Where that runs synchronously and lands before
//! the author's eyes, this *submits* a full-manuscript rebuild per bible tracker to
//! the batch endpoint (half price, but a window of up to ~24h), persists the
//! returned batch ids here as `bible/.batch.toml`, and returns immediately. A
//! later launch or idle tick polls each batch and applies the ones that have
//! finished — so a batch that takes hours survives app restarts.
//!
//! This module is the *pure* data + policy: the persisted record, load/save/prune,
//! and the manuscript-staleness check. It stays provider-agnostic (batches are
//! stored by their bible-tracker slug, the same way the pass queue does #923); the
//! app layer resolves the slug, submits, polls the backend, and writes the bible.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::incremental::Digests;

/// One in-flight batch: the provider's batch id, the bible tracker + book it
/// rebuilds, and the manuscript state (chapter `file → digest`) captured at submit
/// time. The digests let the reconcile detect a **stale** pass — the manuscript was
/// edited while the batch was in flight — so the result is warned about / re-scoped
/// rather than blindly overwriting a bible the author has since moved past (#950).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PendingBatch {
    /// The provider's batch id, polled to reconcile the result.
    pub batch_id: String,
    /// The bible tracker's stable run-action slug (e.g. `"ai:continuity"`), stored
    /// by slug so this module stays provider-agnostic (the app resolves it).
    pub slug: String,
    /// The book index this pass rebuilds (0 for a standalone novel).
    pub book: usize,
    /// The chapter `file → content-digest` map the manuscript had when the batch was
    /// submitted — the staleness baseline.
    #[serde(default)]
    pub digests: Digests,
    /// Submission date (informational), so a very old batch is recognizable.
    #[serde(default)]
    pub submitted: String,
}

/// Whether a batch built against `built_from` is stale for the `current`
/// manuscript (#950): the digests differ, so a chapter changed / was added /
/// removed while the batch was in flight. A stale pass shouldn't blindly overwrite
/// the bible — the reconcile warns / re-scopes instead. The one comparison the
/// app's reconcile check goes through, so staleness is defined in exactly one place.
pub fn is_stale(built_from: &Digests, current: &Digests) -> bool {
    built_from != current
}

/// The durable record of every batch the background update has in flight (#950),
/// persisted as `bible/.batch.toml`. Submissions are appended; each is removed once
/// reconciled (applied, failed, or discarded as stale), so the file empties itself
/// as batches land and is cleared when the last one is gone.
#[derive(Serialize, Deserialize, Default, Clone, Debug, PartialEq, Eq)]
pub struct BatchUpdate {
    /// The in-flight batches, one per submitted bible-tracker pass.
    #[serde(default)]
    pub batches: Vec<PendingBatch>,
}

/// The record file path for a project.
fn batch_path(root: &Path) -> PathBuf {
    root.join("bible").join(".batch.toml")
}

impl BatchUpdate {
    /// Load the persisted record, or the empty default when there's no (readable)
    /// file — best effort, so a corrupt file is treated as "nothing in flight".
    pub fn load(root: &Path) -> Self {
        std::fs::read_to_string(batch_path(root))
            .ok()
            .and_then(|t| toml::from_str(&t).ok())
            .unwrap_or_default()
    }

    /// Whether any batch is still in flight (a reconcile pass is worth running).
    pub const fn is_empty(&self) -> bool {
        self.batches.is_empty()
    }

    /// The `(slug, book)` pairs already in flight, so the queue command doesn't
    /// double-submit a tracker that's still pending from an earlier queueing.
    pub fn pending_keys(&self) -> BTreeSet<(String, usize)> {
        self.batches
            .iter()
            .map(|b| (b.slug.clone(), b.book))
            .collect()
    }

    /// Append a submitted batch and persist.
    pub fn push(&mut self, root: &Path, batch: PendingBatch) -> std::io::Result<()> {
        self.batches.push(batch);
        self.save(root)
    }

    /// Drop the batch with `batch_id` (it reconciled — applied, failed, or stale)
    /// and persist; the file is removed once the last batch is gone, so a fully
    /// reconciled update leaves nothing behind.
    pub fn remove(&mut self, root: &Path, batch_id: &str) -> std::io::Result<()> {
        self.batches.retain(|b| b.batch_id != batch_id);
        if self.batches.is_empty() {
            Self::clear(root);
            Ok(())
        } else {
            self.save(root)
        }
    }

    /// Persist the record (creating `bible/`). Best effort — a write failure is
    /// returned for the caller to surface, but never blocks the update.
    pub fn save(&self, root: &Path) -> std::io::Result<()> {
        let path = batch_path(root);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let body = toml::to_string(self).map_err(std::io::Error::other)?;
        std::fs::write(path, body)
    }

    /// Remove the record file (nothing left to reconcile). Best effort.
    pub fn clear(root: &Path) {
        let _ = std::fs::remove_file(batch_path(root));
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

    fn batch(id: &str, slug: &str, book: usize, dig: Digests) -> PendingBatch {
        PendingBatch {
            batch_id: id.to_string(),
            slug: slug.to_string(),
            book,
            digests: dig,
            submitted: "2026-06-17".to_string(),
        }
    }

    #[test]
    fn a_pending_batch_is_stale_only_when_the_manuscript_drifts() {
        let built = digests(&[("one.md", "a"), ("two.md", "b")]);
        // Unchanged manuscript → not stale, safe to apply.
        assert!(!is_stale(&built, &built));
        // A chapter edited since submit → stale, the result mustn't blindly land.
        assert!(is_stale(
            &built,
            &digests(&[("one.md", "a"), ("two.md", "B!")])
        ));
        // A chapter added since submit → stale.
        assert!(is_stale(
            &built,
            &digests(&[("one.md", "a"), ("two.md", "b"), ("three.md", "c")])
        ));
        // A chapter removed since submit → stale.
        assert!(is_stale(&built, &digests(&[("one.md", "a")])));
    }

    #[test]
    fn record_round_trips_and_prunes_reconciled_batches() {
        let dir = std::env::temp_dir().join(format!("quill-batch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let mut rec = BatchUpdate::default();
        assert!(rec.is_empty());
        rec.push(
            &dir,
            batch("id1", "ai:characters", 0, digests(&[("one.md", "a")])),
        )
        .unwrap();
        rec.push(
            &dir,
            batch("id2", "ai:continuity", 0, digests(&[("one.md", "a")])),
        )
        .unwrap();

        // Reloads with both batches in flight.
        let loaded = BatchUpdate::load(&dir);
        assert_eq!(loaded.batches.len(), 2);
        assert_eq!(
            loaded.pending_keys(),
            [
                ("ai:characters".to_string(), 0),
                ("ai:continuity".to_string(), 0)
            ]
            .into_iter()
            .collect()
        );

        // Reconciling one leaves the other; the file survives.
        rec.remove(&dir, "id1").unwrap();
        let after = BatchUpdate::load(&dir);
        assert_eq!(after.batches.len(), 1);
        assert_eq!(after.batches[0].batch_id, "id2");

        // Reconciling the last clears the file — nothing left to reconcile.
        rec.remove(&dir, "id2").unwrap();
        assert!(BatchUpdate::load(&dir).is_empty());
        assert!(
            !batch_path(&dir).exists(),
            "a fully reconciled update leaves no file"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_corrupt_record_loads_as_empty() {
        let dir = std::env::temp_dir().join(format!("quill-batch-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("bible")).unwrap();
        std::fs::write(batch_path(&dir), "not = valid = toml =").unwrap();
        assert!(BatchUpdate::load(&dir).is_empty());
        std::fs::remove_dir_all(&dir).ok();
    }
}
