//! Sync for a device that never resolves a prose conflict (#1233): the iPhone.
//! Resolving two versions of a chapter on a small touch screen is where words
//! get lost, so the phone doesn't try. When its work conflicts with the
//! desktop's, the work goes onto a draft named for the day (`phone/2026-10-26`),
//! pushed to the remote. The phone goes back to the remote's version, and the
//! desktop merges the draft later with its conflict resolver (#963). Every word
//! from both sides stays in history the whole time.

use std::path::Path;

use super::super::local::commit;
use super::sync::sync_fetched;
use super::{current_branch, push_branch};

/// Drafts a device parked its conflicting work on start with this.
pub const PHONE_DRAFT_PREFIX: &str = "phone/";

/// What a device sync did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DeviceSync {
    /// Both sides' work is now on both sides. `reload` is true when files on
    /// disk changed (the remote's edits came in), so open chapters must reload.
    Synced { summary: String, reload: bool },
    /// This device's work conflicted with the remote's, so it was pushed to
    /// `draft`, and this device is back on the remote's version.
    Diverted { draft: String },
}

/// Commit this device's work (as `message`), then sync it with the remote:
/// merge what doesn't conflict and push, or divert to a draft when chapters
/// conflict. On any failure (no network, a rejected token) the work stays
/// committed here and goes out on the next successful sync; nothing is lost.
pub fn sync_or_divert(root: &Path, message: &str) -> Result<DeviceSync, String> {
    let had_work = commit(root, message)?.is_some();
    let (outcome, fetched) = sync_fetched(root)?;
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    if !outcome.conflicts.is_empty() {
        return divert(&repo, fetched).map(|draft| DeviceSync::Diverted { draft });
    }
    let branch = current_branch(&repo)?;
    let head = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|e| e.to_string())?
        .id();
    let (ahead, _) = repo
        .graph_ahead_behind(head, fetched)
        .map_err(|e| e.to_string())?;
    if ahead > 0 {
        push_branch(&repo, &branch)?;
    }
    let summary = match (ahead > 0 || had_work, outcome.reload) {
        (true, true) => "Synced: sent your edits and brought in the remote's.",
        (true, false) => "Synced: sent your edits to the remote.",
        (false, true) => "Synced: brought in the remote's edits.",
        (false, false) => "Already in sync with the remote.",
    };
    Ok(DeviceSync::Synced {
        summary: summary.to_string(),
        reload: outcome.reload,
    })
}

/// Park this device's committed work on a new `phone/<date>` draft, push it,
/// then move the current branch to the remote's commit `fetched`. The branch
/// only moves once the draft is safely on the remote; if the push fails, the
/// draft is removed again and the work stays where it was.
fn divert(repo: &git2::Repository, fetched: git2::Oid) -> Result<String, String> {
    let head = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|e| e.to_string())?;
    let draft = draft_name(repo, &crate::date::iso(crate::date::today()));
    let mut created = repo
        .branch(&draft, &head, false)
        .map_err(|e| format!("create draft: {e}"))?;
    if let Err(e) = push_branch(repo, &draft) {
        let _ = created.delete();
        return Err(e);
    }

    let branch = current_branch(repo)?;
    repo.find_reference(&format!("refs/heads/{branch}"))
        .and_then(|mut r| {
            r.set_target(fetched, "sync: work diverted to a draft")
                .map(|_| ())
        })
        .map_err(|e| e.to_string())?;
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .map_err(|e| e.to_string())?;
    Ok(draft)
}

/// `phone/<day>`, or `phone/<day>-2`, `-3`… when the day already has one.
fn draft_name(repo: &git2::Repository, day: &str) -> String {
    let taken = |name: &str| repo.find_branch(name, git2::BranchType::Local).is_ok();
    let base = format!("{PHONE_DRAFT_PREFIX}{day}");
    if !taken(&base) {
        return base;
    }
    // A thousand conflicts in a day is no real day; past that, the timestamp.
    (2..1000)
        .map(|n| format!("{base}-{n}"))
        .find(|name| !taken(name))
        .unwrap_or_else(|| format!("{base}-{}", crate::date::file_stamp()))
}
