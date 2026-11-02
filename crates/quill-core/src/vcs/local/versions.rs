//! One file's history, for reading back through a chapter's earlier versions
//! on a small screen (#1299): the commits that changed the file, newest first,
//! and a word-level comparison of two versions of prose. The desktop's history
//! view lists the whole project's commits; the phone asks about one chapter.

use super::open_repo;
use std::path::Path;

use similar::{ChangeTag, TextDiff};

/// One version of a file: a commit that changed it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileVersion {
    /// The commit's full id, to read the file back at this version.
    pub rev: String,
    /// The commit's message, which says where it came from: a snapshot, a
    /// sync from the phone, or the author's own commit.
    pub summary: String,
    /// When the commit was made, in seconds since the Unix epoch.
    pub seconds: i64,
}

/// The commits on the current branch that changed `rel_path`, newest first,
/// at most `limit`. A commit counts when the file's content differs from its
/// first parent's (or it has no parent and contains the file). Renames aren't
/// followed: a chapter's history starts where its file name did.
pub fn file_log(root: &Path, rel_path: &str, limit: usize) -> Result<Vec<FileVersion>, String> {
    let repo = open_repo(root)?;
    let mut walk = repo.revwalk().map_err(|e| e.to_string())?;
    if walk.push_head().is_err() {
        return Ok(Vec::new()); // no commits yet
    }
    walk.simplify_first_parent().map_err(|e| e.to_string())?;
    let path = Path::new(rel_path);
    let blob_at = |commit: &git2::Commit| {
        commit
            .tree()
            .ok()
            .and_then(|tree| tree.get_path(path).ok())
            .map(|entry| entry.id())
    };
    let mut out = Vec::new();
    for oid in walk {
        if out.len() >= limit {
            break;
        }
        let commit = repo
            .find_commit(oid.map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        let here = blob_at(&commit);
        let before = commit.parent(0).ok().and_then(|parent| blob_at(&parent));
        if here.is_some() && here != before {
            out.push(FileVersion {
                rev: commit.id().to_string(),
                summary: commit.summary().unwrap_or("(no message)").to_string(),
                seconds: commit.time().seconds(),
            });
        }
    }
    Ok(out)
}

/// How a run of text in a comparison changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// In both versions.
    Same,
    /// Only in the newer version.
    Added,
    /// Only in the older version.
    Removed,
}

/// Compare two versions of prose word by word: runs of text marked as kept,
/// added, or removed, in reading order, so the old version can be shown with
/// its cuts struck through and the new words highlighted.
///
/// A rewritten phrase reads as one cut followed by one addition ("came in."
/// out, "went out." in), not word by word: changes separated only by spaces
/// are gathered into a single removed run and a single added run.
pub fn compare_prose(old: &str, new: &str) -> Vec<(Change, String)> {
    let diff = TextDiff::from_words(old, new);
    let mut runs: Vec<(Change, String)> = Vec::new();
    // The changed block being gathered: its removed and added text, and the
    // spaces seen since its last change (kept only if another change follows).
    let mut removed = String::new();
    let mut added = String::new();
    let mut gap = String::new();
    let flush = |runs: &mut Vec<(Change, String)>, removed: &mut String, added: &mut String| {
        for (kind, text) in [(Change::Removed, removed), (Change::Added, added)] {
            if !text.is_empty() {
                runs.push((kind, std::mem::take(text)));
            }
        }
    };
    for change in diff.iter_all_changes() {
        let text = change.value();
        match change.tag() {
            ChangeTag::Equal if !removed.is_empty() || !added.is_empty() => {
                if text.trim().is_empty() {
                    gap.push_str(text);
                } else {
                    flush(&mut runs, &mut removed, &mut added);
                    push_run(&mut runs, Change::Same, &std::mem::take(&mut gap));
                    push_run(&mut runs, Change::Same, text);
                }
            }
            ChangeTag::Equal => push_run(&mut runs, Change::Same, text),
            ChangeTag::Delete | ChangeTag::Insert => {
                // Spaces between two changes belong to both sides of the block.
                if !gap.is_empty() {
                    if !removed.is_empty() {
                        removed.push_str(&gap);
                    }
                    if !added.is_empty() {
                        added.push_str(&gap);
                    }
                    gap.clear();
                }
                if change.tag() == ChangeTag::Delete {
                    removed.push_str(text);
                } else {
                    added.push_str(text);
                }
            }
        }
    }
    flush(&mut runs, &mut removed, &mut added);
    push_run(&mut runs, Change::Same, &gap);
    runs
}

/// Append `text` as a `kind` run, merging it into the last run when that's
/// the same kind.
fn push_run(runs: &mut Vec<(Change, String)>, kind: Change, text: &str) {
    if text.is_empty() {
        return;
    }
    match runs.last_mut() {
        Some((last, run)) if *last == kind => run.push_str(text),
        _ => runs.push((kind, text.to_string())),
    }
}
