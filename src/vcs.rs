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
/bible/.state.toml

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

/// Point the project at a hosted remote (#134). Stores it as `origin`, replacing
/// any existing one, so reconfiguring is just calling this again. The URL can be
/// HTTPS (`https://…`), SSH (`git@…`), or a local path — auth is resolved at
/// push/pull time, not here.
pub fn set_remote(root: &Path, url: &str) -> Result<(), String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    // remote_set_url creates or updates; fall back to add for a brand-new remote.
    if repo.find_remote("origin").is_ok() {
        repo.remote_set_url("origin", url).map_err(|e| format!("set remote: {e}"))?;
    } else {
        repo.remote("origin", url).map_err(|e| format!("add remote: {e}"))?;
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
    let mut remote = repo.find_remote("origin").map_err(|_| NO_REMOTE.to_string())?;

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
    remote.push(&[&refspec], Some(&mut opts)).map_err(friendly)?;

    // A rejection that came back via the callback rather than as an error.
    if rejected.borrow_mut().take().is_some() {
        return Err(BEHIND_REMOTE.to_string());
    }
    Ok(branch)
}

/// Pull `origin`'s changes for the current branch (#134). Fast-forward only and
/// non-destructive: if local and remote have diverged (each has commits the other
/// lacks), it stops and explains rather than merging or resetting. The caller is
/// expected to ensure the working tree is clean first. Returns a human message.
pub fn pull(root: &Path) -> Result<String, String> {
    let repo = git2::Repository::open(root).map_err(|e| format!("open repo: {e}"))?;
    let branch = current_branch(&repo)?;
    let mut remote = repo.find_remote("origin").map_err(|_| NO_REMOTE.to_string())?;

    let mut cbs = git2::RemoteCallbacks::new();
    cbs.credentials(credentials);
    let mut opts = git2::FetchOptions::new();
    opts.remote_callbacks(cbs);
    remote.fetch(&[&branch], Some(&mut opts), None).map_err(friendly)?;

    let fetch_head = repo.find_reference("FETCH_HEAD").map_err(|e| e.to_string())?;
    let fetched = repo.reference_to_annotated_commit(&fetch_head).map_err(|e| e.to_string())?;
    let (analysis, _) = repo.merge_analysis(&[&fetched]).map_err(|e| e.to_string())?;

    if analysis.is_up_to_date() {
        return Ok("Already up to date with the remote.".to_string());
    }
    if !analysis.is_fast_forward() {
        return Err("Local and remote have diverged — resolve manually.".to_string());
    }

    // Fast-forward: move the branch ref to the fetched commit and update the tree.
    let refname = format!("refs/heads/{branch}");
    match repo.find_reference(&refname) {
        Ok(mut r) => {
            r.set_target(fetched.id(), "pull: fast-forward").map_err(|e| e.to_string())?;
        }
        Err(_) => {
            // Unborn branch (no local commits yet) — create it at the fetched commit.
            repo.reference(&refname, fetched.id(), true, "pull: initial").map_err(|e| e.to_string())?;
        }
    }
    repo.set_head(&refname).map_err(|e| e.to_string())?;
    repo.checkout_head(Some(git2::build::CheckoutBuilder::new().force())).map_err(|e| e.to_string())?;
    Ok("Pulled the latest changes from the remote.".to_string())
}

const NO_REMOTE: &str = "No remote configured — set one first.";

/// Shown when a push is refused because the remote has commits the local branch
/// doesn't. The single source for this guidance — both the push error path
/// ([`friendly`]) and the update-reference callback in [`push`] use it.
const BEHIND_REMOTE: &str = "The remote has changes you don't — pull first, then push again.";

/// The short name of the branch `HEAD` points at (e.g. `main`), or an error for a
/// detached or unborn HEAD (nothing to push/pull).
fn current_branch(repo: &git2::Repository) -> Result<String, String> {
    let head = repo.head().map_err(|_| "No commits yet — commit before syncing.".to_string())?;
    head.shorthand().map(str::to_string).ok_or_else(|| "Not on a branch.".to_string())
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
    Err(git2::Error::from_str("no supported authentication method available"))
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
fn friendly(e: git2::Error) -> String {
    let msg = e.message().to_lowercase();
    if msg.contains("authentication") || msg.contains("401") || msg.contains("403") || msg.contains("credentials") {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        crate::testutil::scratch_dir("vcs", name)
    }

    #[test]
    fn init_creates_a_repo_and_gitignore() {
        let dir = temp_dir("init");
        assert!(!is_repo(&dir));
        init(&dir).unwrap();
        assert!(is_repo(&dir), "a .git directory should exist");
        let ignore = fs::read_to_string(dir.join(".gitignore")).unwrap();
        assert!(ignore.contains("/.quill/") && ignore.contains("/unused/"));
        assert!(ignore.contains("/bible/.state.toml"), "the AI scoping cache is ignored (#90)");
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
    fn set_and_read_remote() {
        let dir = temp_dir("remote-set");
        init(&dir).unwrap();
        assert!(remote_url(&dir).is_none(), "no remote on a fresh repo");
        set_remote(&dir, "https://example.com/a.git").unwrap();
        assert_eq!(remote_url(&dir).as_deref(), Some("https://example.com/a.git"));
        // Reconfiguring just replaces the URL.
        set_remote(&dir, "https://example.com/b.git").unwrap();
        assert_eq!(remote_url(&dir).as_deref(), Some("https://example.com/b.git"));
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn push_and_pull_through_a_local_remote() {
        // A bare repo stands in for the hosted remote — this exercises the full
        // push/pull/divergence plumbing with no network or auth.
        let remote = temp_dir("remote-bare");
        git2::Repository::init_bare(&remote).unwrap();
        let url = remote.to_str().unwrap();

        let a = temp_dir("remote-a");
        init(&a).unwrap();
        fs::write(a.join("one.md"), "# One\n\nDraft.\n").unwrap();
        commit(&a, "first").unwrap();
        set_remote(&a, url).unwrap();
        assert!(!push(&a).unwrap().is_empty(), "first push reports the branch");

        // b is a second working copy (clone) — it has "first".
        let b = temp_dir("remote-b");
        fs::remove_dir_all(&b).ok();
        git2::Repository::clone(url, &b).unwrap();
        assert!(b.join("one.md").exists());

        // a commits again and pushes; b fast-forwards on pull.
        fs::write(a.join("one.md"), "# One\n\nRevised draft.\n").unwrap();
        commit(&a, "second").unwrap();
        push(&a).unwrap();
        let msg = pull(&b).unwrap();
        assert!(msg.contains("Pulled"), "{msg}");
        assert!(fs::read_to_string(b.join("one.md")).unwrap().contains("Revised"));
        // Pulling again with nothing new is a no-op, not an error.
        assert!(pull(&b).unwrap().contains("up to date"));

        // Divergence: b moves ahead and pushes; a (behind, with its own new
        // commit) is refused rather than overwriting b's work.
        fs::write(b.join("one.md"), "# One\n\nB's edit.\n").unwrap();
        commit(&b, "b-change").unwrap();
        push(&b).unwrap();
        fs::write(a.join("one.md"), "# One\n\nA's edit.\n").unwrap();
        commit(&a, "a-change").unwrap();
        let err = push(&a).unwrap_err();
        assert!(err.to_lowercase().contains("pull first"), "{err}");

        fs::remove_dir_all(&a).ok();
        fs::remove_dir_all(&b).ok();
        fs::remove_dir_all(&remote).ok();
    }

    #[test]
    fn push_without_a_remote_explains() {
        let dir = temp_dir("remote-none");
        init(&dir).unwrap();
        fs::write(dir.join("one.md"), "# One\n").unwrap();
        commit(&dir, "first").unwrap();
        let err = push(&dir).unwrap_err();
        assert!(err.contains("No remote"), "{err}");
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
