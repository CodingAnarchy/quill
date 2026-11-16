//! Reaching `origin` over HTTPS when its URL names a user but no password
//! (`https://matt@host/…`). libgit2 answers the server's first challenge with
//! that user and an empty password, before it asks Quill's credential callback.
//! Forgejo and Gitea refuse that with a bare 401 that carries no new challenge,
//! so libgit2 gives up and the push fails as an authentication error, even
//! though `git push` (which asks its credential helper first) gets through.
//!
//! For such a URL, remote work goes through an in-memory remote at the same URL
//! without the user, and the user is handed to the credential helper instead.
//! The named remote isn't there to keep `refs/remotes/origin/*` current, so
//! fetches name their tracking ref and a push moves it itself.

use super::NO_REMOTE;

/// The remote to fetch from and push to: `origin`, or an in-memory copy of it
/// without a bare URL user (see the module docs).
pub(super) fn origin(repo: &git2::Repository) -> Result<git2::Remote<'_>, String> {
    let remote = repo
        .find_remote("origin")
        .map_err(|_| NO_REMOTE.to_string())?;
    remote
        .url()
        .and_then(without_bare_user)
        .map_or(Ok(remote), |url| {
            repo.remote_anonymous(&url)
                .map_err(|e| format!("Couldn't reach the remote: {e}"))
        })
}

/// The user an HTTP(S) `origin` URL names without a password, which the
/// credential helper is asked for in libgit2's place.
pub(super) fn origin_user(repo: &git2::Repository) -> Option<String> {
    let remote = repo.find_remote("origin").ok()?;
    let (user, _) = bare_user(remote.url()?)?;
    Some(user.to_string())
}

/// The fetch refspec that brings `branch` into its tracking ref, so a fetch
/// through either kind of remote leaves `refs/remotes/origin/<branch>` current.
pub(super) fn fetch_spec(branch: &str) -> String {
    format!("+refs/heads/{branch}:refs/remotes/origin/{branch}")
}

/// After `branch` was pushed, move its tracking ref to what was pushed, which a
/// push through the named remote does on its own. Best-effort: a stale tracking
/// ref only means the next fetch brings it up to date.
pub(super) fn note_pushed(repo: &git2::Repository, branch: &str) {
    let Ok(local) = repo.refname_to_id(&format!("refs/heads/{branch}")) else {
        return;
    };
    let _ = repo.reference(
        &format!("refs/remotes/origin/{branch}"),
        local,
        true,
        "push",
    );
}

/// `url` without its user, when it's HTTP(S) and names a user but no password.
pub(super) fn without_bare_user(url: &str) -> Option<String> {
    let (user, scheme_len) = bare_user(url)?;
    let rest = &url[scheme_len + user.len() + 1..];
    Some(format!("{}{rest}", &url[..scheme_len]))
}

/// The bare user in an HTTP(S) URL, with the length of the scheme before it.
pub(super) fn bare_user(url: &str) -> Option<(&str, usize)> {
    let scheme_len = ["https://", "http://"]
        .iter()
        .find(|scheme| {
            url.get(..scheme.len())
                .is_some_and(|s| s.eq_ignore_ascii_case(scheme))
        })?
        .len();
    let after = &url[scheme_len..];
    let authority = &after[..after.find('/').unwrap_or(after.len())];
    let (userinfo, _) = authority.rsplit_once('@')?;
    (!userinfo.is_empty() && !userinfo.contains(':')).then_some((userinfo, scheme_len))
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::{origin, origin_user, without_bare_user};
    use crate::testutil::scratch_dir;

    #[test]
    fn a_bare_user_is_dropped_from_an_https_url() {
        assert_eq!(
            without_bare_user("https://matt@host.example/matt/book.git").as_deref(),
            Some("https://host.example/matt/book.git")
        );
        assert_eq!(
            without_bare_user("HTTP://a@host:3000/x.git").as_deref(),
            Some("HTTP://host:3000/x.git")
        );
    }

    #[test]
    fn other_urls_are_left_alone() {
        for url in [
            "https://host.example/matt/book.git",
            "https://matt:secret@host.example/book.git",
            "https://host.example/path@with-at/book.git",
            "git@github.com:matt/book.git",
            "ssh://git@host/book.git",
            "/srv/git/book.git",
            "https://@host/book.git",
        ] {
            assert_eq!(without_bare_user(url), None, "{url}");
        }
    }

    #[test]
    fn origin_with_a_bare_user_is_reached_without_it() {
        let dir = scratch_dir("vcs", "origin-bare-user");
        let repo = git2::Repository::init(&dir).expect("init");
        repo.remote("origin", "https://matt@host.example/matt/book.git")
            .expect("add origin");

        let remote = origin(&repo).expect("origin");
        assert_eq!(remote.url(), Some("https://host.example/matt/book.git"));
        assert_eq!(origin_user(&repo).as_deref(), Some("matt"));
        // The configured URL itself is untouched.
        assert_eq!(
            repo.find_remote("origin").expect("named").url(),
            Some("https://matt@host.example/matt/book.git")
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn any_other_origin_is_the_named_remote() {
        let dir = scratch_dir("vcs", "origin-plain");
        let repo = git2::Repository::init(&dir).expect("init");
        repo.remote("origin", "git@host.example:matt/book.git")
            .expect("add origin");
        let remote = origin(&repo).expect("origin");
        assert_eq!(remote.name(), Some("origin"));
        assert_eq!(origin_user(&repo), None);
        fs::remove_dir_all(&dir).ok();
    }
}
