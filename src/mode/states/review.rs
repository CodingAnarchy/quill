//! Bible- and conflict-review mode state (#963): the queues of conflicting bible
//! files and two-way-sync chapters resolved one at a time. Split out of
//! `states.rs` (pure move).

use std::path::PathBuf;

use crate::buffer::Buffer;

/// State for an open [`Mode::BibleReview`]: the queue of conflicting bible files
/// (front of the queue is the one shown), and a scratch buffer while the author
/// edits the proposed version before saving.
pub struct BibleReview {
    pub(crate) conflicts: Vec<BibleConflict>,
    /// `Some` while editing the proposed version of the front conflict.
    pub(crate) editing: Option<Buffer>,
}

/// State for an open [`Mode::ConflictReview`] (#963): the queue of chapters a
/// two-way sync (#962) found changed on both machines, resolved one at a time.
/// The front of the queue is the chapter shown; each carries both versions
/// (never lost) plus a pre-rendered local↔remote diff. `resolved` accumulates the
/// author's chosen text per already-decided chapter, keyed by path, which
/// [`crate::vcs::finalize_sync`] lands as one merge commit once the queue drains.
/// A safety milestone was tagged before the flow opened, so both sides survive
/// even a mid-resolution abort.
pub struct ConflictReview {
    pub(crate) conflicts: Vec<ChapterConflictItem>,
    /// Resolutions chosen so far: chapter path → final text. Filled as each front
    /// conflict is decided; handed to `finalize_sync` when the queue empties.
    pub(crate) resolved: std::collections::BTreeMap<String, String>,
    /// `Some` while hand-merging the front conflict — a scratch buffer seeded with
    /// both versions the author edits into the final text, saved with Ctrl-S.
    pub(crate) editing: Option<Buffer>,
}

/// One chapter a sync found changed on both machines (#963), carrying both full
/// versions so neither is lost and a pre-rendered diff for the overlay.
pub struct ChapterConflictItem {
    /// The chapter's path relative to the repo root (its file name).
    pub(crate) path: String,
    /// The version on this machine (kept as-is on disk).
    pub(crate) local: String,
    /// The version fetched from the remote.
    pub(crate) remote: String,
    /// Pre-rendered unified diff (local → remote), each line prefixed with
    /// `+`/`-`/` `. Computed once so the overlay isn't re-diffing every frame.
    pub(crate) diff: Vec<String>,
}

/// One bible file whose on-disk generated block differs from what a fresh pass
/// would write.
pub struct BibleConflict {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) proposed: String,
    /// Pre-rendered unified diff (current → proposed), each line prefixed with
    /// `+`/`-`/` `. Computed once when the conflict is staged so the overlay
    /// isn't re-diffing every frame.
    pub(crate) diff: Vec<String>,
    /// The on-disk entry's readable provenance (#832) — where it was generated
    /// from and that you've edited it — shown above the diff. `None` for a
    /// pre-#832 / foreign file that carries no provenance line.
    pub(crate) provenance: Option<String>,
}
