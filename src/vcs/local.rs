//! The local, offline half of version control (#659): everything that reads or
//! writes the working tree and this repository's own history — init/status/log,
//! reading a file out of a past commit, the diff helpers, restore, and commit.
//! Nothing here touches the network; the remote/auth side lives in
//! [`super::remote`].

use std::fs;
use std::path::{Path, PathBuf};

/// Editor state and scratch that should never be committed — Quill's own
/// metadata and OS cruft. Everything else (chapter `.md` files, `book.toml`, the
/// `bible/` and `submission/` artifacts) is tracked.
const GITIGNORE: &str = "\
# Quill editor state — not part of the manuscript
/.quill/
/unused/
/bible/.state.toml
/bible/findings.toml

# OS / editor cruft
.DS_Store
*.swp
*.tmp
";

/// Whether `root` is already the top of a Git repository.
pub fn is_repo(root: &Path) -> bool {
    root.join(".git").exists()
}

/// Initialize a Git repository at `root` and write a default `.gitignore` (only
/// if one isn't already present, so a hand-tuned file is never clobbered).
/// Idempotent: re-initializing an existing repository is safe.
pub fn init(root: &Path) -> Result<(), String> {
    git2::Repository::init(root).map_err(|e| format!("git init: {e}"))?;
    let gitignore = root.join(".gitignore");
    if !gitignore.exists() {
        fs::write(&gitignore, GITIGNORE).map_err(|e| format!("write .gitignore: {e}"))?;
    }
    Ok(())
}

/// One commit for the history view (#133): short id, first line of the message,
/// and the date it was made (in the commit's recorded timezone). `rev` is the full
/// object id — the unambiguous handle the diff/restore-from-a-commit actions (#587)
/// resolve, so a chosen row addresses exactly one commit even in a large history.
pub struct CommitMeta {
    pub id: String,
    pub rev: String,
    pub summary: String,
    pub when: String,
}

/// The most recent commits, newest first (up to `limit`). Empty for a repository
/// with no commits yet.
pub fn log(root: &Path, limit: usize) -> Result<Vec<CommitMeta>, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let mut walk = repo.revwalk().map_err(|e| e.to_string())?;
    if walk.push_head().is_err() {
        return Ok(Vec::new()); // unborn HEAD — no commits yet
    }
    walk.set_sorting(git2::Sort::TIME)
        .map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for oid in walk.take(limit) {
        let oid = oid.map_err(|e| e.to_string())?;
        let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
        out.push(CommitMeta {
            id: oid.to_string().chars().take(7).collect(),
            rev: oid.to_string(),
            summary: commit.summary().unwrap_or("(no message)").to_string(),
            when: format_date(commit.time()),
        });
    }
    Ok(out)
}

/// One named milestone (#833) — an annotated tag marking a whole-manuscript
/// version ("first draft", "sent to beta readers"). `rev` is the full object id
/// the tag points at, the handle the whole-project diff/restore resolve; `message`
/// is the annotation the author gave it, and `when` the date it was created.
pub struct TagMeta {
    pub name: String,
    pub rev: String,
    pub message: String,
    pub when: String,
}

/// Create an annotated tag `name` on the current `HEAD` (#833), recording
/// `message` as its annotation — a named milestone for the whole manuscript.
/// Fails when there's no commit to tag yet, or a tag by that name already exists,
/// so an author never silently overwrites an earlier milestone.
pub fn create_tag(root: &Path, name: &str, message: &str) -> Result<(), String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let head = repo
        .head()
        .and_then(|h| h.peel(git2::ObjectType::Commit))
        .map_err(|_| "No commit to tag yet — commit first.".to_string())?;
    let sig = signature(&repo)?;
    let msg = if message.trim().is_empty() {
        name
    } else {
        message
    };
    repo.tag(name, &head, &sig, msg, false)
        .map_err(|e| format!("create milestone: {e}"))?;
    Ok(())
}

/// Re-annotate an existing milestone (#966) with a new note, leaving it pointing
/// at the same commit — the edit path for a milestone's free-text note. Recreates
/// the annotated tag in place (`force`) on its current target, so the name, the
/// revision, and the whole-project diff/restore that resolve through it are all
/// unchanged; only the annotation the author reads changes. Errors when no
/// milestone by that name exists, rather than creating one silently.
pub fn set_tag_message(root: &Path, name: &str, message: &str) -> Result<(), String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    // Resolve the tag to the commit it marks, so the re-tag keeps the same target.
    let target = repo
        .revparse_single(&format!("refs/tags/{name}"))
        .and_then(|obj| obj.peel(git2::ObjectType::Commit))
        .map_err(|_| format!("No milestone named '{name}'."))?;
    let sig = signature(&repo)?;
    // An empty note falls back to the name, matching create — a milestone always
    // carries an annotation, so `list_tags` keeps listing it.
    let msg = if message.trim().is_empty() {
        name
    } else {
        message
    };
    repo.tag(name, &target, &sig, msg, true)
        .map_err(|e| format!("edit milestone note: {e}"))?;
    Ok(())
}

/// Every annotated milestone (#833), newest first by the tag's own creation time.
/// Empty when the project has no milestones. Lightweight tags (created outside
/// Quill) are skipped — a milestone always carries a name and annotation.
pub fn list_tags(root: &Path) -> Result<Vec<TagMeta>, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let mut out = Vec::new();
    let names = repo.tag_names(None).map_err(|e| e.to_string())?;
    for name in names.iter().flatten() {
        // Resolve the tag *ref* to its object without peeling through it: an
        // annotated tag is a tag object carrying the message + tagger, whereas a
        // lightweight tag resolves straight to a commit. `revparse_single` on the
        // tag ref gives the tag object itself, which we can then narrow.
        let Ok(obj) = repo.revparse_single(&format!("refs/tags/{name}")) else {
            continue;
        };
        // Skip lightweight tags (created outside Quill) — they carry no annotation.
        let Some(tag) = obj.as_tag() else {
            continue;
        };
        let Ok(target) = tag.target() else {
            continue;
        };
        out.push(TagMeta {
            name: name.to_string(),
            rev: target.id().to_string(),
            message: tag.message().unwrap_or("").trim().to_string(),
            when: format_date(
                tag.tagger()
                    .map_or_else(|| git2::Time::new(0, 0), |t| t.when()),
            ),
        });
    }
    // Newest first, by the tag's own creation date, so a fresh milestone leads.
    out.sort_by(|a, b| b.when.cmp(&a.when));
    Ok(out)
}

/// Restore the *entire* working tree to the state at `rev` (#833) — a whole-project
/// restore to a milestone or past commit, the counterpart to the per-chapter
/// [`restore`]. Forces every tracked path to that revision's version and resets the
/// index to match, so the working tree is exactly that snapshot. Destructive to
/// uncommitted changes (the palette flow confirms first); `.gitignore`d editor
/// state is untouched, since it was never part of the tree.
pub fn restore_all(root: &Path, rev: &str) -> Result<(), String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let tree = repo
        .revparse_single(rev)
        .and_then(|obj| obj.peel_to_tree())
        .map_err(|e| format!("resolve {rev}: {e}"))?;
    let mut checkout = git2::build::CheckoutBuilder::new();
    // Force overwrites working-tree edits; remove_untracked drops files added since
    // the target so the tree matches the snapshot exactly, not a superset of it.
    checkout.force().remove_untracked(true);
    repo.checkout_tree(tree.as_object(), Some(&mut checkout))
        .map_err(|e| format!("restore project: {e}"))?;
    // Point the index at the restored tree so status reads clean against it.
    let mut index = repo.index().map_err(|e| e.to_string())?;
    index
        .read_tree(&tree)
        .map_err(|e| format!("reset index: {e}"))?;
    index.write().map_err(|e| e.to_string())?;
    Ok(())
}

/// A unified diff of the *whole* working tree against `rev` (#833) — what has
/// changed across the entire manuscript since that milestone or commit, the
/// counterpart to the per-chapter [`diff_file_to_worktree`]. Working-tree edits
/// included; `.gitignore`d paths excluded. Empty string when the tree matches the
/// revision.
pub fn diff_tree_to_worktree(root: &Path, rev: &str) -> Result<String, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let tree = repo
        .revparse_single(rev)
        .and_then(|obj| obj.peel_to_tree())
        .map_err(|e| format!("resolve {rev}: {e}"))?;
    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    let diff = repo
        .diff_tree_to_workdir_with_index(Some(&tree), Some(&mut opts))
        .map_err(|e| e.to_string())?;

    let mut out = String::new();
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        if matches!(line.origin(), '+' | '-' | ' ') {
            out.push(line.origin());
        }
        out.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
        true
    })
    .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Format a commit's time as `YYYY-MM-DD` in the timezone it was committed in.
fn format_date(t: git2::Time) -> String {
    let offset = time::UtcOffset::from_whole_seconds(t.offset_minutes() * 60)
        .unwrap_or(time::UtcOffset::UTC);
    time::OffsetDateTime::from_unix_timestamp(t.seconds()).map_or_else(
        |_| "????-??-??".to_string(),
        |dt| {
            let (y, m, d) = dt.to_offset(offset).to_calendar_date();
            format!("{y:04}-{:02}-{d:02}", u8::from(m))
        },
    )
}

/// The working-tree status: which tracked-or-untracked (non-ignored) files differ
/// from the last commit. The basis for the uncommitted-changes indicator (#132),
/// and the per-file change list the diff/restore (#135) and AI incremental
/// scoping (#90) reuse.
pub struct Status {
    pub changed: Vec<PathBuf>,
}

impl Status {
    pub const fn count(&self) -> usize {
        self.changed.len()
    }
}

/// Compute the working-tree status for `root` — the non-ignored paths that differ
/// from `HEAD` (added, modified, deleted, or untracked). Stat-based via libgit2,
/// so it's cheap enough to refresh on idle, not per keystroke.
pub fn status(root: &Path) -> Result<Status, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let changed = changed_paths(&repo)?;
    Ok(Status { changed })
}

/// The non-ignored paths that differ from `HEAD` (added, modified, deleted, or
/// untracked). Shared by [`status`] and the dirty-tree guard in
/// [`super::remote::pull`].
fn changed_paths(repo: &git2::Repository) -> Result<Vec<PathBuf>, String> {
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true)
        .recurse_untracked_dirs(true)
        .include_ignored(false);
    let statuses = repo.statuses(Some(&mut opts)).map_err(|e| e.to_string())?;
    Ok(statuses
        .iter()
        .filter(|e| !e.status().is_ignored())
        .filter_map(|e| e.path().map(PathBuf::from))
        .collect())
}

/// Whether the working tree has uncommitted changes (added, modified, deleted, or
/// untracked non-ignored files). The guard [`super::remote::pull`] uses to avoid
/// clobbering edits.
pub(super) fn is_dirty(repo: &git2::Repository) -> Result<bool, String> {
    Ok(!changed_paths(repo)?.is_empty())
}

/// The working-tree diff against `HEAD` as a unified patch (added/modified/
/// deleted/untracked). Empty string when clean. For a repo with no commits yet,
/// everything reads as newly added.
pub fn diff(root: &Path) -> Result<String, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true);
    let head_tree = repo.head().ok().and_then(|h| h.peel_to_tree().ok());
    let diff = repo
        .diff_tree_to_workdir_with_index(head_tree.as_ref(), Some(&mut opts))
        .map_err(|e| e.to_string())?;

    let mut out = String::new();
    diff.print(git2::DiffFormat::Patch, |_delta, _hunk, line| {
        // Content lines need their origin marker (+/-/space) prepended; file and
        // hunk headers already carry their own text.
        if matches!(line.origin(), '+' | '-' | ' ') {
            out.push(line.origin());
        }
        out.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
        true
    })
    .map_err(|e| e.to_string())?;
    Ok(out)
}

/// The contents of `rel_path` as they were at commit `rev` (a short or full id,
/// or any revision spec `git2` can resolve). `Ok(None)` when that revision has no
/// such file — it was added later, or lived under a different name. The basis for
/// the in-app "diff a chapter against a past commit" and "restore a chapter to a
/// past commit" (#587), which read a single file out of history without touching
/// the working tree.
pub fn show_file_at(root: &Path, rev: &str, rel_path: &str) -> Result<Option<String>, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let commit = repo
        .revparse_single(rev)
        .and_then(|obj| obj.peel_to_commit())
        .map_err(|e| format!("resolve {rev}: {e}"))?;
    let tree = commit.tree().map_err(|e| e.to_string())?;
    // The file isn't in that commit.
    let Ok(entry) = tree.get_path(Path::new(rel_path)) else {
        return Ok(None);
    };
    let blob = entry
        .to_object(&repo)
        .map_err(|e| e.to_string())?
        .peel_to_blob()
        .map_err(|e| e.to_string())?;
    Ok(Some(String::from_utf8_lossy(blob.content()).into_owned()))
}

/// A unified diff of `rel_path` between two commits (#587): `rev_a` → `rev_b`.
/// Empty string when the file is identical (or absent) in both. Reads only the two
/// blobs from history, never the working tree, so it answers "this chapter then vs.
/// then" against any past pair. Pass `"HEAD"` as one side to anchor on the last
/// commit; the working-tree comparison the history view runs is
/// [`diff_file_to_worktree`].
// The commit-to-commit primitive of the pair (#587); the UI wires the
// working-tree variant, so in this bin crate this one is reached only from tests.
#[cfg_attr(not(test), allow(dead_code))]
pub fn diff_file(root: &Path, rev_a: &str, rev_b: &str, rel_path: &str) -> Result<String, String> {
    let old = show_file_at(root, rev_a, rel_path)?.unwrap_or_default();
    let new = show_file_at(root, rev_b, rel_path)?.unwrap_or_default();
    diff_bytes(old.as_bytes(), new.as_bytes(), rel_path)
}

/// A unified diff of `rel_path` from commit `rev` to its *current* on-disk version
/// (#587) — what has changed in this one chapter since that commit, working-tree
/// edits included. Empty when unchanged. The comparison the history view's "diff
/// this chapter" runs, so a chosen commit is measured against what the author is
/// looking at now, not the last commit. The between-two-commits variant is
/// [`diff_file`].
pub fn diff_file_to_worktree(root: &Path, rev: &str, rel_path: &str) -> Result<String, String> {
    let old = show_file_at(root, rev, rel_path)?.unwrap_or_default();
    let new = fs::read(root.join(rel_path)).unwrap_or_default();
    diff_bytes(old.as_bytes(), &new, rel_path)
}

/// Render a unified patch for one path from `old` to `new` bytes. The shared body
/// of the file-diff helpers, so the origin-marker handling lives in one place.
fn diff_bytes(old: &[u8], new: &[u8], rel_path: &str) -> Result<String, String> {
    let mut out = String::new();
    let path = Some(Path::new(rel_path));
    git2::Patch::from_buffers(old, path, new, path, None)
        .map_err(|e| e.to_string())?
        .print(&mut |_delta, _hunk, line| {
            // Content lines need their origin marker (+/-/space) prepended; the
            // file and hunk headers already carry their own text.
            if matches!(line.origin(), '+' | '-' | ' ') {
                out.push(line.origin());
            }
            out.push_str(std::str::from_utf8(line.content()).unwrap_or(""));
            true
        })
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Restore the file at `rel_path` (relative to `root`) to its last committed
/// version, discarding working-tree changes to it. Returns `false` if there's no
/// committed version to restore to (no commits yet, or the file is new). Touches
/// only that one file.
pub fn restore(root: &Path, rel_path: &str) -> Result<bool, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let Some(head_tree) = repo.head().ok().and_then(|h| h.peel_to_tree().ok()) else {
        return Ok(false); // no commits → nothing to restore to
    };
    if head_tree.get_path(Path::new(rel_path)).is_err() {
        return Ok(false); // the file isn't in the last commit
    }
    let mut checkout = git2::build::CheckoutBuilder::new();
    checkout.force().path(rel_path);
    repo.checkout_head(Some(&mut checkout))
        .map_err(|e| format!("restore: {e}"))?;
    Ok(true)
}

/// Stage the whole project and commit it with `message`. Returns the new commit's
/// short id, or `None` when there's nothing to commit (a clean working tree).
///
/// Staging respects `.gitignore` and captures additions, modifications, and
/// deletions (e.g. a chapter moved to `unused/`). Identity comes from the git
/// config when set, else a generic default — so an author who has never used git
/// can still commit without configuring anything.
pub fn commit(root: &Path, message: &str) -> Result<Option<String>, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let sig = signature(&repo)?;

    let mut index = repo.index().map_err(|e| e.to_string())?;
    index
        .add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None)
        .map_err(|e| e.to_string())?;
    index
        .update_all(["*"].iter(), None)
        .map_err(|e| e.to_string())?; // stage deletions of tracked files
    index.write().map_err(|e| e.to_string())?;
    let tree_id = index.write_tree().map_err(|e| e.to_string())?;
    let tree = repo.find_tree(tree_id).map_err(|e| e.to_string())?;

    // Parent = current HEAD, unless this is the first (unborn HEAD) commit.
    let parent = match repo.head() {
        Ok(head) => Some(head.peel_to_commit().map_err(|e| e.to_string())?),
        Err(_) => None,
    };
    // Nothing changed since the last commit → don't create an empty commit.
    if let Some(p) = &parent {
        if p.tree_id() == tree_id {
            return Ok(None);
        }
    }

    let parents: Vec<&git2::Commit> = parent.iter().collect();
    let oid = repo
        .commit(Some("HEAD"), &sig, &sig, message, &tree, &parents)
        .map_err(|e| format!("commit: {e}"))?;
    Ok(Some(oid.to_string().chars().take(7).collect()))
}

/// The identity to record on a commit — the author's git config when set, else a
/// generic Quill default, so someone who has never configured git can still commit
/// (and merge). Shared by [`commit`] and the merge commit [`super::remote::pull`]
/// creates when a diverged branch merges cleanly.
pub(super) fn signature(repo: &git2::Repository) -> Result<git2::Signature<'static>, String> {
    repo.signature()
        .or_else(|_| git2::Signature::now("Quill Author", "author@quill.local"))
        .map_err(|e| format!("signature: {e}"))
}
