//! The remote/auth half of version control (#659): pointing the project at a
//! hosted `origin`, pushing and pulling, resolving credentials, and translating
//! libgit2's network/auth errors into author-facing guidance. The offline,
//! working-tree side (including the [`is_dirty`](super::local::is_dirty) guard
//! `pull` relies on) lives in [`super::local`].
//!
//! Remote failures are triaged into a small taxonomy (#967) rather than a single
//! generic "operation failed" string: [`RemoteError`] carries a [category](RemoteErrorKind)
//! — offline/network, auth, a rejected (non-fast-forward) push, or anything else —
//! and each category renders an actionable message *plus* a recovery hint. A bare
//! "pull first" leaves the author stuck when the real cause was a dropped network
//! or a stale token; the category lets the sync/push/pull flow say which of those
//! it was, and what to do next. The push/pull/sync entry points still hand callers
//! a `String` (what the UI shows), but it's built from a category via
//! [`RemoteError::message`], not lifted from a raw libgit2 line.
//!
//! Three cohesive pieces sit under this half: [`error`] is the [`RemoteError`]
//! taxonomy, [`merge`] holds the chapter-level three-way primitives the sync and
//! the branch-merge (#965) share, and [`sync`] is the two-way sync (#962) plus its
//! hand-resolved [`finalize_sync`] (#963). This module keeps `set_remote`/`push`/
//! `pull` and the credential/branch plumbing, and re-exports the pieces so
//! `super::remote::…` (and the flat `crate::vcs::…` surface) see one facade.

mod clone;
mod divert;
mod error;
pub(in crate::vcs) mod merge;
mod sync;
mod token;

use std::path::Path;

use super::local::{is_dirty, signature};

pub use clone::{clone, folder_name};
pub use divert::{sync_or_divert, DeviceSync, PHONE_DRAFT_PREFIX};
pub use error::{RemoteError, RemoteErrorKind};
pub use sync::{finalize_sync, sync, ChapterConflict};
pub use token::set_token;

pub(in crate::vcs) use error::classify;

/// Point the project at a hosted remote (#134). Stores it as `origin`, replacing
/// any existing one, so reconfiguring is just calling this again. The URL can be
/// HTTPS (`https://…`), SSH (`git@…`), or a local path — auth is resolved at
/// push/pull time, not here.
pub fn set_remote(root: &Path, url: &str) -> Result<(), String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    // remote_set_url creates or updates; fall back to add for a brand-new remote.
    if repo.find_remote("origin").is_ok() {
        repo.remote_set_url("origin", url)
            .map_err(|e| format!("set remote: {e}"))?;
    } else {
        repo.remote("origin", url)
            .map_err(|e| format!("add remote: {e}"))?;
    }
    Ok(())
}

/// The configured `origin` URL, if any — for showing the author where their work
/// backs up to.
pub fn remote_url(root: &Path) -> Option<String> {
    let repo = git2::Repository::open(root).ok()?;
    let remote = repo.find_remote("origin").ok()?;
    remote.url().map(str::to_string)
}

/// Push the current branch's commits to `origin` (#134). Never forces: if the
/// remote has commits the local branch doesn't, libgit2 rejects the
/// non-fast-forward push and we surface that ("pull first") rather than
/// clobbering remote history. Returns the branch name pushed.
pub fn push(root: &Path) -> Result<String, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let branch = current_branch(&repo)?;
    push_branch(&repo, &branch)?;
    Ok(branch)
}

/// Push the local branch `branch` to the branch of the same name on `origin`,
/// never forcing. Shared by [`push`] and the phone's draft diversion (#1233).
fn push_branch(repo: &git2::Repository, branch: &str) -> Result<(), String> {
    let mut remote = repo
        .find_remote("origin")
        .map_err(|_| NO_REMOTE.to_string())?;

    // A rejected ref (e.g. non-fast-forward) is reported through this callback,
    // not always as an error from push() — capture it so we never silently
    // "succeed" at a push the remote refused.
    let rejected = std::rc::Rc::new(std::cell::RefCell::new(None::<String>));
    let captured = rejected.clone();
    let mut cbs = git2::RemoteCallbacks::new();
    cbs.credentials(credentials);
    cbs.push_update_reference(move |_refname, status| {
        if let Some(reason) = status {
            *captured.borrow_mut() = Some(reason.to_string());
        }
        Ok(())
    });
    let mut opts = git2::PushOptions::new();
    opts.remote_callbacks(cbs);

    let refspec = format!("refs/heads/{branch}:refs/heads/{branch}");
    remote
        .push(&[&refspec], Some(&mut opts))
        .map_err(|e| classify(&e).message())?;

    // A rejection that came back via the callback rather than as an error.
    if rejected.borrow_mut().take().is_some() {
        return Err(RemoteError::rejected().message());
    }
    Ok(())
}

/// Pull `origin`'s changes for the current branch (#134), resolving a diverged
/// branch in-app (#703). Non-destructive throughout — it never resets, force-pushes,
/// or discards a commit:
///
/// * **Up to date / behind** — a fast-forward moves the branch to the fetched commit.
/// * **Diverged, no conflicts** — both sides committed. The fetched tree is merged
///   into local and, when the merge is clean, committed as an ordinary two-parent
///   merge commit, so both authors' commits stay reachable.
/// * **Diverged, with conflicts** — the same files were edited on both machines in a
///   way git can't reconcile. Rather than write half-merged files or leave the repo
///   mid-merge, the attempt is rolled back and the conflicted paths are reported so
///   the author knows exactly what to reconcile.
///
/// A fast-forward (and the merge checkout) finishes with a force checkout of the
/// resulting tree, which would clobber uncommitted working-tree edits. Rather than
/// trust the caller to keep the tree clean, `pull` refuses up front when the tree is
/// dirty (#639) — so a save-but-don't-commit-then-pull can never silently discard the
/// author's words. Returns a human message.
pub fn pull(root: &Path) -> Result<String, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;

    // Guard against silent data loss: the fast-forward below force-checks-out the
    // fetched tree, overwriting any uncommitted edits. Refuse before touching the
    // network so the author commits (or restores) those changes first.
    if is_dirty(&repo)? {
        return Err(DIRTY_PULL.to_string());
    }

    let branch = current_branch(&repo)?;
    let mut remote = repo
        .find_remote("origin")
        .map_err(|_| NO_REMOTE.to_string())?;

    let mut cbs = git2::RemoteCallbacks::new();
    cbs.credentials(credentials);
    let mut opts = git2::FetchOptions::new();
    opts.remote_callbacks(cbs);
    remote
        .fetch(&[&branch], Some(&mut opts), None)
        .map_err(|e| classify(&e).message())?;

    let fetch_head = repo
        .find_reference("FETCH_HEAD")
        .map_err(|e| e.to_string())?;
    let fetched = repo
        .reference_to_annotated_commit(&fetch_head)
        .map_err(|e| e.to_string())?;
    let (analysis, _) = repo
        .merge_analysis(&[&fetched])
        .map_err(|e| e.to_string())?;

    if analysis.is_up_to_date() {
        return Ok("Already up to date with the remote.".to_string());
    }
    if !analysis.is_fast_forward() {
        // Local and remote both moved (each has commits the other lacks). Merge the
        // fetched commit into local rather than dead-ending (#703).
        return merge_diverged(&repo, &branch, &fetched);
    }

    // Fast-forward: move the branch ref to the fetched commit and update the tree.
    let refname = format!("refs/heads/{branch}");
    match repo.find_reference(&refname) {
        Ok(mut r) => {
            r.set_target(fetched.id(), "pull: fast-forward")
                .map_err(|e| e.to_string())?;
        }
        Err(_) => {
            // Unborn branch (no local commits yet) — create it at the fetched commit.
            repo.reference(&refname, fetched.id(), true, "pull: initial")
                .map_err(|e| e.to_string())?;
        }
    }
    repo.set_head(&refname).map_err(|e| e.to_string())?;
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .map_err(|e| e.to_string())?;
    Ok("Pulled the latest changes from the remote.".to_string())
}

/// Merge the fetched commit into the local branch when the two have diverged (#703).
/// The tree is already known clean (the caller's #639 dirty guard runs first), so a
/// clean merge can safely land on disk. On a clean merge this writes a two-parent
/// merge commit (both authors' history preserved) and checks it out; on conflicts it
/// rolls the attempt back and reports the conflicted paths without changing a commit
/// or leaving a half-written tree.
fn merge_diverged(
    repo: &git2::Repository,
    branch: &str,
    fetched: &git2::AnnotatedCommit,
) -> Result<String, String> {
    let local = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|e| e.to_string())?;
    let their_commit = repo.find_commit(fetched.id()).map_err(|e| e.to_string())?;

    // Merge the two trees in memory first, so a conflict never touches the working
    // tree or the index — nothing to unwind on the conflict path.
    let mut merged = repo
        .merge_commits(&local, &their_commit, None)
        .map_err(|e| format!("merge: {e}"))?;

    if merged.has_conflicts() {
        let files = conflicted_paths(&merged);
        return Err(format!(
            "Local and remote have diverged and the same text was edited on both — \
             Quill can't merge {files} automatically. Restore or reconcile {them} \
             (the History view shows each side), then pull again.",
            files = if files.is_empty() {
                "these changes".to_string()
            } else {
                files.clone()
            },
            them = if files.is_empty() { "them" } else { "it" },
        ));
    }

    // Clean merge: write the merged tree and record a two-parent merge commit so both
    // sides' commits stay in history.
    let tree_id = merged
        .write_tree_to(repo)
        .map_err(|e| format!("merge tree: {e}"))?;
    let tree = repo.find_tree(tree_id).map_err(|e| e.to_string())?;
    let sig = signature(repo)?;
    let message = format!("Merge remote changes into {branch}");
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        &message,
        &tree,
        &[&local, &their_commit],
    )
    .map_err(|e| format!("merge commit: {e}"))?;

    // Bring the working tree up to the merged result.
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .map_err(|e| e.to_string())?;
    Ok("Merged the remote's changes with yours — push to share the merge.".to_string())
}

/// The paths libgit2 reports as conflicting in a merged index, joined for display.
/// Reads the "our" side's path (present for edit/edit conflicts, the common case for
/// two authors touching the same chapter).
fn conflicted_paths(index: &git2::Index) -> String {
    let Ok(conflicts) = index.conflicts() else {
        return String::new();
    };
    let mut names: Vec<String> = conflicts
        .filter_map(std::result::Result::ok)
        .filter_map(|c| c.our.or(c.their).or(c.ancestor))
        .filter_map(|entry| String::from_utf8(entry.path).ok())
        .collect();
    names.sort();
    names.dedup();
    names.join(", ")
}

const NO_REMOTE: &str = "No remote configured — set one first.";

/// Shown when a pull is refused because the working tree has uncommitted edits a
/// fast-forward checkout would overwrite (#639). Commit (or restore) them first.
const DIRTY_PULL: &str = "You have uncommitted changes — commit them before pulling.";

/// The short name of the branch `HEAD` points at (e.g. `main`), or an error for a
/// detached or unborn HEAD (nothing to push/pull).
fn current_branch(repo: &git2::Repository) -> Result<String, String> {
    let head = repo
        .head()
        .map_err(|_| "No commits yet — commit before syncing.".to_string())?;
    head.shorthand()
        .map(str::to_string)
        .ok_or_else(|| "Not on a branch.".to_string())
}

/// git2 credential callback: reuse whatever the author already has, rather than
/// inventing key management. SSH URLs use the ssh-agent; HTTPS uses a token the
/// front end supplied ([`set_token`], the iPhone app's Keychain, #1231), then one
/// from the environment, otherwise the system git credential helper (e.g. the
/// macOS keychain or `gh`). Documented in `docs/git-integration-spike.md`.
fn credentials(
    url: &str,
    username: Option<&str>,
    allowed: git2::CredentialType,
) -> Result<git2::Cred, git2::Error> {
    use git2::CredentialType as Type;
    if allowed.contains(Type::SSH_KEY) {
        return git2::Cred::ssh_key_from_agent(username.unwrap_or("git"));
    }
    if allowed.contains(Type::USER_PASS_PLAINTEXT) {
        if let Some(token) = token::supplied().or_else(token_from_env) {
            return git2::Cred::userpass_plaintext(username.unwrap_or("git"), &token);
        }
        let config = git2::Config::open_default()?;
        return git2::Cred::credential_helper(&config, url, username);
    }
    if allowed.contains(Type::USERNAME) {
        return git2::Cred::username(username.unwrap_or("git"));
    }
    Err(git2::Error::from_str(
        "no supported authentication method available",
    ))
}

/// A personal-access token from the environment, for HTTPS auth without a
/// configured credential helper. Checks Quill's own var first, then the
/// conventional GitHub ones.
fn token_from_env() -> Option<String> {
    ["QUILL_GIT_TOKEN", "GITHUB_TOKEN", "GH_TOKEN"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.is_empty()))
}
