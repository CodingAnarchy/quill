//! The remote/auth half of version control (#659): pointing the project at a
//! hosted `origin`, pushing and pulling, resolving credentials, and translating
//! libgit2's network/auth errors into author-facing guidance. The offline,
//! working-tree side (including the [`is_dirty`](super::local::is_dirty) guard
//! `pull` relies on) lives in [`super::local`].

use std::collections::BTreeMap;
use std::path::Path;

use super::local::{is_dirty, signature};

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
        .map_err(|e| friendly(&e))?;

    // A rejection that came back via the callback rather than as an error.
    if rejected.borrow_mut().take().is_some() {
        return Err(BEHIND_REMOTE.to_string());
    }
    Ok(branch)
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
        .map_err(|e| friendly(&e))?;

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

/// One chapter changed on both machines since the shared ancestor — a per-chapter
/// conflict two-way [`sync`] detects but never resolves on its own (#962). Chapters
/// are atomic files, so a conflict is per-chapter, not per-line: the same chapter
/// was edited differently on each side. All three versions are carried so the
/// resolution UI (#963) can show base-vs-local-vs-remote and let the author choose,
/// and so sync itself never has to pick a side or clobber one.
///
/// The three version strings are the data the resolution UI (#963) renders and acts
/// on; this ticket only detects and exposes them, so today they're read by name only
/// from the tests — annotated so the not-yet-wired bin build stays warning-clean.
#[derive(Debug, Clone)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct ChapterConflict {
    /// The chapter's path relative to the repo root (its file name).
    pub path: String,
    /// The version currently on this machine (left on disk, untouched by sync).
    pub local: String,
    /// The version fetched from the remote (preserved here, not written to disk).
    pub remote: String,
    /// The shared-ancestor version both sides diverged from (empty when the chapter
    /// was added independently on both sides).
    pub base: String,
}

/// What a two-way [`sync`] did (#962). A sync fetches the remote and reconciles it
/// with local work at chapter granularity: chapters only one side touched merge
/// cleanly, chapters both sides changed differently become [conflicts](ChapterConflict).
#[derive(Debug)]
pub struct SyncOutcome {
    /// The author-facing summary of what happened.
    pub summary: String,
    /// Paths of chapters that merged cleanly from the remote onto this machine
    /// (remote-only edits adopted). Empty on a pure fast-forward, which merges the
    /// whole tree at once rather than chapter-by-chapter. The summary already folds
    /// the count in, so today the list itself is read only by the tests (and the
    /// #963 UI to come) — annotated so the bin build stays warning-clean.
    #[cfg_attr(not(test), allow(dead_code))]
    pub merged: Vec<String>,
    /// Chapters changed on both sides — surfaced for resolution (#963), never
    /// silently clobbered. Empty on a clean sync.
    pub conflicts: Vec<ChapterConflict>,
    /// Whether the working tree on disk changed, so the editor should reload the
    /// active chapter (mirrors [`pull`]'s reload signal).
    pub reload: bool,
}

/// Two-way sync with the remote (#962): fetch `origin`, then reconcile its chapter
/// edits with the local working copy so the same project can be written on two
/// machines (laptop → remote → desktop) without a manual merge.
///
/// Chapters are atomic files, so reconciliation is a **chapter-level three-way**
/// (base / local / remote), never a per-line merge:
///
/// * **Up to date** — nothing to do.
/// * **Fast-forward** — local has no commits the remote lacks; the branch and tree
///   advance to the fetched commit, exactly like [`pull`].
/// * **Diverged** — both sides committed. For each chapter, sync compares its blob
///   in the merge base, local `HEAD`, and the fetched commit: a chapter only the
///   remote changed is adopted; a chapter only local changed is kept; a chapter
///   both changed *identically* needs nothing; a chapter both changed *differently*
///   is a [conflict](ChapterConflict). When there are no conflicts the merged tree
///   is written as a two-parent merge commit (both histories preserved) and checked
///   out. When any chapter conflicts, **nothing is committed or written** — the repo
///   is left exactly as it was and every conflict is returned with all three
///   versions, so neither side is ever clobbered and #963 can resolve them.
///
/// Like [`pull`], sync refuses up front when the working tree is dirty (#639), so an
/// uncommitted edit can never be overwritten by the merge checkout.
pub fn sync(root: &Path) -> Result<SyncOutcome, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;

    // Same guard as pull: the merge/fast-forward checkout force-writes the tree, so
    // refuse before touching the network if there are uncommitted edits to lose.
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
        .map_err(|e| friendly(&e))?;

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
        return Ok(SyncOutcome {
            summary: "Already in sync with the remote.".to_string(),
            merged: Vec::new(),
            conflicts: Vec::new(),
            reload: false,
        });
    }
    if analysis.is_fast_forward() {
        fast_forward(&repo, &branch, &fetched)?;
        return Ok(SyncOutcome {
            summary: "Pulled the remote's changes — you were behind.".to_string(),
            merged: Vec::new(),
            conflicts: Vec::new(),
            reload: true,
        });
    }

    sync_diverged(&repo, &branch, &fetched)
}

/// Advance the branch ref and working tree to the fetched commit — the shared
/// fast-forward step of [`pull`] and [`sync`]. The caller has confirmed the tree is
/// clean, so the force checkout is safe.
fn fast_forward(
    repo: &git2::Repository,
    branch: &str,
    fetched: &git2::AnnotatedCommit,
) -> Result<(), String> {
    let refname = format!("refs/heads/{branch}");
    match repo.find_reference(&refname) {
        Ok(mut r) => {
            r.set_target(fetched.id(), "sync: fast-forward")
                .map_err(|e| e.to_string())?;
        }
        Err(_) => {
            repo.reference(&refname, fetched.id(), true, "sync: initial")
                .map_err(|e| e.to_string())?;
        }
    }
    repo.set_head(&refname).map_err(|e| e.to_string())?;
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Reconcile a diverged branch chapter-by-chapter (#962). See [`sync`] for the
/// three-way model. On a clean reconciliation this records a two-parent merge commit
/// and checks it out; on any conflict it changes nothing and returns the conflicts.
fn sync_diverged(
    repo: &git2::Repository,
    branch: &str,
    fetched: &git2::AnnotatedCommit,
) -> Result<SyncOutcome, String> {
    let local = repo
        .head()
        .and_then(|h| h.peel_to_commit())
        .map_err(|e| e.to_string())?;
    let theirs = repo.find_commit(fetched.id()).map_err(|e| e.to_string())?;

    // The merge base is the shared ancestor both sides diverged from — the "base"
    // leg of every chapter's three-way. Absent (unrelated histories) is treated as
    // an empty ancestor, so every touched chapter reads as added on both sides.
    let base_tree = match repo.merge_base(local.id(), theirs.id()) {
        Ok(base) => Some(
            repo.find_commit(base)
                .and_then(|c| c.tree())
                .map_err(|e| e.to_string())?,
        ),
        Err(_) => None,
    };
    let local_tree = local.tree().map_err(|e| e.to_string())?;
    let their_tree = theirs.tree().map_err(|e| e.to_string())?;

    // Blob id per path in each of the three trees. `None` means the path is absent
    // in that tree (added on one side, or deleted). Only blobs (chapter files) are
    // reconciled here; trees (subdirectories) are compared by their own entry id, so
    // a changed subtree still reads as a conflict/adoption rather than being missed.
    let mut base = BTreeMap::new();
    if let Some(t) = &base_tree {
        collect_blobs(t, &mut base);
    }
    let mut ours = BTreeMap::new();
    collect_blobs(&local_tree, &mut ours);
    let mut theirs_blobs = BTreeMap::new();
    collect_blobs(&their_tree, &mut theirs_blobs);

    let mut paths: Vec<String> = base
        .keys()
        .chain(ours.keys())
        .chain(theirs_blobs.keys())
        .cloned()
        .collect();
    paths.sort();
    paths.dedup();

    let mut merged = Vec::new();
    let mut conflicts = Vec::new();
    // Start the merged tree from local; clean remote-only edits are layered on.
    let mut builder = repo
        .treebuilder(Some(&local_tree))
        .map_err(|e| e.to_string())?;

    for path in paths {
        let b = base.get(&path).copied();
        let o = ours.get(&path).copied();
        let t = theirs_blobs.get(&path).copied();

        if o == t {
            continue; // both sides agree (including both absent) — nothing to do
        }
        if o == b {
            // Only the remote changed this chapter — adopt its version.
            adopt(&mut builder, &path, t)?;
            merged.push(path);
        } else if t == b {
            // Only local changed it — keep ours (already in the builder base).
        } else {
            // Both changed it, differently — a per-chapter conflict (#962).
            conflicts.push(ChapterConflict {
                path: path.clone(),
                local: blob_text(repo, o),
                remote: blob_text(repo, t),
                base: blob_text(repo, b),
            });
        }
    }

    if !conflicts.is_empty() {
        // Never write a half-merged tree: leave the repo exactly as it was and hand
        // the conflicts back for resolution (#963). Both versions are preserved.
        let merged_note = if merged.is_empty() {
            String::new()
        } else {
            format!(
                " {} other chapter(s) are ready to merge once resolved.",
                merged.len()
            )
        };
        return Ok(SyncOutcome {
            summary: format!(
                "{} chapter(s) changed on both machines — resolve them, then sync again.{merged_note}",
                conflicts.len()
            ),
            merged,
            conflicts,
            reload: false,
        });
    }

    // Clean reconciliation: commit the merged tree as a two-parent merge so both
    // histories stay reachable, then bring the working tree up to it.
    let tree_id = builder.write().map_err(|e| e.to_string())?;
    let tree = repo.find_tree(tree_id).map_err(|e| e.to_string())?;
    let sig = signature(repo)?;
    let message = format!("Sync remote changes into {branch}");
    repo.commit(
        Some("HEAD"),
        &sig,
        &sig,
        &message,
        &tree,
        &[&local, &theirs],
    )
    .map_err(|e| format!("sync commit: {e}"))?;
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force()))
        .map_err(|e| e.to_string())?;

    let summary = if merged.is_empty() {
        "Synced with the remote — your local edits lead.".to_string()
    } else {
        format!(
            "Synced with the remote — merged {} chapter(s) cleanly.",
            merged.len()
        )
    };
    Ok(SyncOutcome {
        summary,
        merged,
        conflicts: Vec::new(),
        reload: true,
    })
}

/// Write the remote's version of `path` into the merged tree builder (or stage its
/// removal when the remote deleted the chapter). The chapter is adopted wholesale —
/// chapters are atomic files, so there's no sub-file merge to do.
fn adopt(
    builder: &mut git2::TreeBuilder,
    path: &str,
    theirs: Option<git2::Oid>,
) -> Result<(), String> {
    match theirs {
        Some(oid) => builder
            .insert(path, oid, git2::FileMode::Blob.into())
            .map(|_| ())
            .map_err(|e| e.to_string()),
        None => builder.remove(path).map_err(|e| e.to_string()),
    }
}

/// Collect every blob (file) in `tree`, keyed by its full path, as `path -> blob id`.
/// Recurses into subtrees so nested paths are compared too; non-blob entries (the
/// subtree objects themselves) are skipped, since only files carry chapter text.
fn collect_blobs(tree: &git2::Tree, out: &mut BTreeMap<String, git2::Oid>) {
    let _ = tree.walk(git2::TreeWalkMode::PreOrder, |dir, entry| {
        if entry.kind() == Some(git2::ObjectType::Blob) {
            if let Some(name) = entry.name() {
                out.insert(format!("{dir}{name}"), entry.id());
            }
        }
        git2::TreeWalkResult::Ok
    });
}

/// The text of a blob by id, lossily decoded as UTF-8 (chapters are Markdown).
/// Empty string when the blob is absent (the chapter didn't exist on that side) or
/// can't be read — a missing ancestor reads as "added", the caller's intent.
fn blob_text(repo: &git2::Repository, oid: Option<git2::Oid>) -> String {
    oid.and_then(|oid| repo.find_blob(oid).ok())
        .map(|blob| String::from_utf8_lossy(blob.content()).into_owned())
        .unwrap_or_default()
}

const NO_REMOTE: &str = "No remote configured — set one first.";

/// Shown when a pull is refused because the working tree has uncommitted edits a
/// fast-forward checkout would overwrite (#639). Commit (or restore) them first.
const DIRTY_PULL: &str = "You have uncommitted changes — commit them before pulling.";

/// Shown when a push is refused because the remote has commits the local branch
/// doesn't. The single source for this guidance — both the push error path
/// ([`friendly`]) and the update-reference callback in [`push`] use it.
const BEHIND_REMOTE: &str = "The remote has changes you don't — pull first, then push again.";

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
/// inventing key management. SSH URLs use the ssh-agent; HTTPS uses a token from
/// the environment if present, otherwise the system git credential helper (e.g.
/// the macOS keychain or `gh`). Documented in `docs/git-integration-spike.md`.
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
        if let Some(token) = token_from_env() {
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

/// Turn a libgit2 network/auth error into author-facing guidance instead of a
/// cryptic message.
fn friendly(e: &git2::Error) -> String {
    let msg = e.message().to_lowercase();
    if msg.contains("authentication")
        || msg.contains("401")
        || msg.contains("403")
        || msg.contains("credentials")
    {
        "Authentication failed — set up an SSH key, a credential helper, or a \
         QUILL_GIT_TOKEN for the remote."
            .to_string()
    } else if msg.contains("fast-forward")
        || msg.contains("rejected")
        || msg.contains("cannot push")
        || msg.contains("not present locally")
    {
        BEHIND_REMOTE.to_string()
    } else {
        format!("Remote operation failed: {}", e.message())
    }
}
