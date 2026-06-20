//! Version control (#54), the single seam between Quill and Git. Everything that
//! touches `git2` (libgit2) lives here, so the backend stays swappable (the spike
//! #129 chose git2 over the CLI and gitoxide; see `docs/git-integration-spike.md`).
//!
//! Version control is opt-in: a project is only a repository once the author runs
//! [`init`]. This first slice (#130) covers detecting and initializing a
//! repository with a sensible `.gitignore`; commit / status / log / diff land in
//! the later tickets through this same module.

use std::fs;
use std::path::{Path, PathBuf};

/// Editor state and scratch that should never be committed — Quill's own
/// metadata and OS cruft. Everything else (chapter `.md` files, `book.toml`, the
/// `bible/` and `submission/` artifacts) is tracked.
const GITIGNORE: &str = "\
# Quill editor state — not part of the manuscript
/.quill/
/unused/

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
/// and the date it was made (in the commit's recorded timezone).
pub struct CommitMeta {
    pub id: String,
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
    walk.set_sorting(git2::Sort::TIME).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for oid in walk.take(limit) {
        let oid = oid.map_err(|e| e.to_string())?;
        let commit = repo.find_commit(oid).map_err(|e| e.to_string())?;
        out.push(CommitMeta {
            id: oid.to_string().chars().take(7).collect(),
            summary: commit.summary().unwrap_or("(no message)").to_string(),
            when: format_date(commit.time()),
        });
    }
    Ok(out)
}

/// Format a commit's time as `YYYY-MM-DD` in the timezone it was committed in.
fn format_date(t: git2::Time) -> String {
    let offset = time::UtcOffset::from_whole_seconds(t.offset_minutes() * 60)
        .unwrap_or(time::UtcOffset::UTC);
    match time::OffsetDateTime::from_unix_timestamp(t.seconds()) {
        Ok(dt) => {
            let (y, m, d) = dt.to_offset(offset).to_calendar_date();
            format!("{y:04}-{:02}-{d:02}", u8::from(m))
        }
        Err(_) => "????-??-??".to_string(),
    }
}

/// The working-tree status: which tracked-or-untracked (non-ignored) files differ
/// from the last commit. The basis for the uncommitted-changes indicator (#132),
/// and the per-file change list the diff/restore (#135) and AI incremental
/// scoping (#90) reuse.
pub struct Status {
    pub changed: Vec<PathBuf>,
}

impl Status {
    pub fn count(&self) -> usize {
        self.changed.len()
    }
}

/// Compute the working-tree status for `root` — the non-ignored paths that differ
/// from `HEAD` (added, modified, deleted, or untracked). Stat-based via libgit2,
/// so it's cheap enough to refresh on idle, not per keystroke.
pub fn status(root: &Path) -> Result<Status, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let mut opts = git2::StatusOptions::new();
    opts.include_untracked(true).recurse_untracked_dirs(true).include_ignored(false);
    let statuses = repo.statuses(Some(&mut opts)).map_err(|e| e.to_string())?;
    let changed = statuses
        .iter()
        .filter(|e| !e.status().is_ignored())
        .filter_map(|e| e.path().map(PathBuf::from))
        .collect();
    Ok(Status { changed })
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
    repo.checkout_head(Some(&mut checkout)).map_err(|e| format!("restore: {e}"))?;
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
    let sig = repo
        .signature()
        .or_else(|_| git2::Signature::now("Quill Author", "author@quill.local"))
        .map_err(|e| format!("signature: {e}"))?;

    let mut index = repo.index().map_err(|e| e.to_string())?;
    index.add_all(["*"].iter(), git2::IndexAddOption::DEFAULT, None).map_err(|e| e.to_string())?;
    index.update_all(["*"].iter(), None).map_err(|e| e.to_string())?; // stage deletions of tracked files
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("quill-vcs-{}-{}", std::process::id(), name));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn init_creates_a_repo_and_gitignore() {
        let dir = temp_dir("init");
        assert!(!is_repo(&dir));
        init(&dir).unwrap();
        assert!(is_repo(&dir), "a .git directory should exist");
        let ignore = fs::read_to_string(dir.join(".gitignore")).unwrap();
        assert!(ignore.contains("/.quill/") && ignore.contains("/unused/"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn commit_captures_changes_and_reports_nothing_when_clean() {
        let dir = temp_dir("commit");
        init(&dir).unwrap();
        fs::write(dir.join("one.md"), "# One\n\nThe night was dark.\n").unwrap();

        let id = commit(&dir, "first").unwrap();
        assert!(id.is_some(), "the first commit should be created");
        // Nothing changed since → clean, no commit.
        assert_eq!(commit(&dir, "again").unwrap(), None);
        // A change → a new commit.
        fs::write(dir.join("one.md"), "# One\n\nThe night was long.\n").unwrap();
        assert!(commit(&dir, "edit").unwrap().is_some());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn log_lists_commits_newest_first() {
        let dir = temp_dir("log");
        init(&dir).unwrap();
        fs::write(dir.join("a.md"), "1\n").unwrap();
        commit(&dir, "first").unwrap();
        fs::write(dir.join("a.md"), "2\n").unwrap();
        commit(&dir, "second").unwrap();

        let entries = log(&dir, 10).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].summary, "second"); // newest first
        assert_eq!(entries[1].summary, "first");
        assert_eq!(entries[0].when.len(), 10, "YYYY-MM-DD"); // "{}", entries[0].when
        assert_eq!(entries[0].id.len(), 7, "short id");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn log_is_empty_before_any_commit() {
        let dir = temp_dir("log-empty");
        init(&dir).unwrap();
        assert!(log(&dir, 10).unwrap().is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn commit_ignores_editor_state_and_unused() {
        let dir = temp_dir("commit-ignore");
        init(&dir).unwrap();
        fs::write(dir.join("one.md"), "# One\n").unwrap();
        commit(&dir, "first").unwrap();
        // Editor state under ignored paths must not count as a change.
        fs::create_dir_all(dir.join(".quill")).unwrap();
        fs::write(dir.join(".quill/history.jsonl"), "{}\n").unwrap();
        fs::create_dir_all(dir.join("unused")).unwrap();
        fs::write(dir.join("unused/old.md"), "# Old\n").unwrap();
        assert_eq!(commit(&dir, "noise").unwrap(), None, "ignored paths aren't committed");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn init_is_idempotent_and_preserves_an_existing_gitignore() {
        let dir = temp_dir("idempotent");
        fs::write(dir.join(".gitignore"), "# hand-tuned\n/custom/\n").unwrap();
        init(&dir).unwrap();
        init(&dir).unwrap(); // second time is harmless
        assert!(is_repo(&dir));
        // The author's existing .gitignore is left untouched.
        assert_eq!(fs::read_to_string(dir.join(".gitignore")).unwrap(), "# hand-tuned\n/custom/\n");
        fs::remove_dir_all(&dir).ok();
    }
}
