//! The remote/auth half of version control (#659): pointing the project at a
//! hosted `origin`, pushing and pulling, resolving credentials, and translating
//! libgit2's network/auth errors into author-facing guidance. The offline,
//! working-tree side (including the [`is_dirty`](super::local::is_dirty) guard
//! `pull` relies on) lives in [`super::local`].

use std::path::Path;

use super::local::is_dirty;

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
        .map_err(friendly)?;

    // A rejection that came back via the callback rather than as an error.
    if rejected.borrow_mut().take().is_some() {
        return Err(BEHIND_REMOTE.to_string());
    }
    Ok(branch)
}

/// Pull `origin`'s changes for the current branch (#134). Fast-forward only and
/// non-destructive: if local and remote have diverged (each has commits the other
/// lacks), it stops and explains rather than merging or resetting. Returns a human
/// message.
///
/// A fast-forward finishes with a force checkout of the fetched tree, which would
/// clobber uncommitted working-tree edits. Rather than trust the caller to keep the
/// tree clean, `pull` refuses up front when the tree is dirty (#639) — so a
/// save-but-don't-commit-then-pull can never silently discard the author's words.
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
        .map_err(friendly)?;

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
        return Err("Local and remote have diverged — resolve manually.".to_string());
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
fn friendly(e: git2::Error) -> String {
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
