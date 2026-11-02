//! Automatic version history (#1296): every book gets a history without the
//! author setting anything up. Opening a book with no repository starts one, and
//! the editor then takes snapshots on its own as the author works, so the history
//! view, restore, and milestones have something to show for every writer, not
//! only the ones who know git.
//!
//! Quill only snapshots a history it started. A repository Quill creates here is
//! marked with the `quill.autohistory` key in its own `.git/config`, so a book the
//! author already manages with git is left alone: no commits appear in their
//! history unless they ask for them. A book that sits inside some larger
//! repository (a folder of books under one repo, a dotfiles repo at `$HOME`) is
//! never given a second, nested repository either.
//!
//! Nothing here touches the network. Pushing anywhere stays the separate,
//! deliberate step of connecting a remote and turning backups on.

use std::path::Path;

use time::macros::format_description;
use time::OffsetDateTime;

use super::{commit, exclude_editor_state, init, is_repo};

/// The git config key that marks a repository as one Quill started for automatic
/// history, in the repository's own `.git/config` (never committed).
const MARKER: &str = "quill.autohistory";

/// The message on the first commit of a history Quill starts.
const FIRST_MESSAGE: &str = "Start of version history";

/// What [`start_auto_history`] found, or did, for a book.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoHistory {
    /// The book had no history, so Quill started one and committed the book as
    /// it stands. The editor tells the author, once.
    Started,
    /// The book already has a history Quill started; snapshots carry on.
    On,
    /// The book is a repository Quill didn't start (the author manages it with
    /// git), so Quill takes no snapshots unless the author turns them on.
    HandManaged,
    /// The book is inside a larger repository, so it gets no history of its own.
    Nested,
}

impl AutoHistory {
    /// Whether the editor should take automatic snapshots of this book.
    /// `existing` is the author's choice to snapshot a history Quill didn't start.
    pub const fn snapshots(self, existing: bool) -> bool {
        match self {
            Self::Started | Self::On => true,
            Self::HandManaged => existing,
            Self::Nested => false,
        }
    }
}

/// Make sure the book at `root` has version history, starting one when it has
/// none: initialize a repository (with Quill's `.gitignore`), mark it as Quill's,
/// and commit the book as it stands. A book that's already a repository, or sits
/// inside one, is reported and left as it is.
///
/// This stages and hashes the whole book on first run, so call it off the UI
/// thread.
pub fn start_auto_history(root: &Path) -> Result<AutoHistory, String> {
    if is_repo(root) {
        return Ok(if is_auto_history(root) {
            AutoHistory::On
        } else {
            AutoHistory::HandManaged
        });
    }
    if git2::Repository::discover(root).is_ok() {
        return Ok(AutoHistory::Nested);
    }
    init(root)?;
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    repo.config()
        .and_then(|mut config| config.set_bool(MARKER, true))
        .map_err(|e| format!("mark the history as Quill's: {e}"))?;
    // `init` writes Quill's `.gitignore` only when there isn't one; an older
    // one of the author's may not list the editor state.
    exclude_editor_state(root)?;
    commit(root, FIRST_MESSAGE)?;
    Ok(AutoHistory::Started)
}

/// Whether the book at `root` is a repository Quill started for automatic history.
pub fn is_auto_history(root: &Path) -> bool {
    git2::Repository::open(root)
        .and_then(|repo| repo.config())
        .and_then(|config| config.get_bool(MARKER))
        .unwrap_or(false)
}

/// Commit the book at `root` as an automatic snapshot, with a plain dated
/// message. Returns the new commit's short id, or `None` when nothing changed
/// since the last commit. The caller decides whether this book takes snapshots
/// (see [`AutoHistory::snapshots`]).
pub fn snapshot(root: &Path) -> Result<Option<String>, String> {
    commit(root, &snapshot_message(local_now()))
}

/// The author's local time, or UTC when the offset can't be determined.
fn local_now() -> OffsetDateTime {
    OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc())
}

/// "Snapshot, 2 Nov 2026 at 14:05": the commit message for an automatic snapshot
/// taken at `at`. The commit records its own time too; the message makes the
/// history readable at a glance and tells snapshots apart from the author's own
/// commits and milestones.
pub fn snapshot_message(at: OffsetDateTime) -> String {
    let when = at
        .format(format_description!(
            "[day padding:none] [month repr:short] [year] at [hour]:[minute]"
        ))
        .unwrap_or_default();
    format!("Snapshot, {when}")
}
