//! The remote-error taxonomy (#967): libgit2 reports dozens of distinct error
//! codes, but for triage they collapse into a small set of buckets — offline,
//! auth, a rejected (non-fast-forward) push, or anything else — each with its own
//! actionable message and recovery hint. [`classify`] maps a raw `git2::Error`
//! into a [`RemoteError`], leaning on libgit2's own class/code first and falling
//! back to the message wording, so the sync/push/pull flow can say *which kind* of
//! failure it was rather than a single generic line.

use std::fmt;

use git2::{ErrorClass, ErrorCode};

/// How a remote operation failed, at the granularity the author can act on (#967).
/// libgit2 reports dozens of distinct error codes, but for triage they collapse to
/// four buckets — each with its own recovery move — so the sync/push/pull flow can
/// say *which kind* of failure it was rather than a single generic line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteErrorKind {
    /// The remote couldn't be reached at all — no network, DNS failure, connection
    /// refused/timed out, host down. The work is fine; the link isn't.
    Offline,
    /// The remote was reached but rejected the credentials — no/expired token, an
    /// SSH key the ssh-agent doesn't hold, a 401/403. Nothing to pull or push until
    /// auth is fixed.
    Auth,
    /// The push was refused as a non-fast-forward: the remote has commits the local
    /// branch doesn't, so pushing would drop them. Pull/sync first, then push.
    Rejected,
    /// Anything else — a real failure worth surfacing verbatim, but not one of the
    /// three actionable categories above.
    Other,
}

/// A categorized remote failure (#967): a [kind](RemoteErrorKind) plus the raw
/// libgit2 detail, from which [`message`](RemoteError::message) builds the
/// author-facing string (actionable message + recovery hint). Kept as a distinct
/// type — rather than folding straight to a `String` — so callers (and tests) can
/// branch on the category, and so the message/hint wording lives in one place.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteError {
    /// The triage category the failure falls into.
    pub kind: RemoteErrorKind,
    /// The underlying libgit2 message in its original case, carried so an `Other`
    /// failure can still show the author what actually went wrong.
    detail: String,
}

impl RemoteError {
    /// A rejected (non-fast-forward) push — the branch has diverged from the remote.
    /// Named because [`push`](super::push) reaches this both from a push error and
    /// from the update-reference callback, and both want the identical guidance.
    pub(in crate::vcs) const fn rejected() -> Self {
        Self {
            kind: RemoteErrorKind::Rejected,
            detail: String::new(),
        }
    }

    /// The author-facing string: an actionable statement of what went wrong followed
    /// by the concrete recovery move. This is what push/pull/sync return as their
    /// error, so the category shapes the message the editor shows.
    pub fn message(&self) -> String {
        match self.kind {
            RemoteErrorKind::Offline => {
                "Can't reach the remote — check your internet connection, then try again."
                    .to_string()
            }
            RemoteErrorKind::Auth => "Authentication failed — the remote rejected your \
                 credentials. Set up an SSH key, a credential helper, or a QUILL_GIT_TOKEN, \
                 then try again."
                .to_string(),
            RemoteErrorKind::Rejected => {
                "The remote has changes you don't — pull first, then push again.".to_string()
            }
            RemoteErrorKind::Other => {
                format!(
                    "Remote operation failed: {} Try again, or check the remote.",
                    {
                        let d = self.detail.trim();
                        if d.is_empty() {
                            "the remote operation failed"
                        } else {
                            d
                        }
                    }
                )
            }
        }
    }
}

impl fmt::Display for RemoteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

/// Triage a libgit2 error into the [`RemoteError`] taxonomy (#967). Classification
/// leans on libgit2's own `class`/`code` first — the authoritative signal — and
/// falls back to matching the message text, since many failures libgit2 reports
/// under a generic class (`Net`, `Http`) only distinguish offline-vs-auth-vs-rejected
/// in their wording. Order matters: auth and rejected are checked before the broad
/// network bucket so a "401 over HTTP" reads as auth, not merely "network".
pub(in crate::vcs) fn classify(e: &git2::Error) -> RemoteError {
    let msg = e.message().to_lowercase();
    let kind = if is_auth(e, &msg) {
        RemoteErrorKind::Auth
    } else if is_rejected(&msg) {
        RemoteErrorKind::Rejected
    } else if is_offline(e, &msg) {
        RemoteErrorKind::Offline
    } else {
        RemoteErrorKind::Other
    };
    RemoteError {
        kind,
        detail: e.message().to_string(),
    }
}

/// The remote rejected the author's credentials. libgit2 flags this as
/// [`ErrorCode::Auth`]/[`ErrorCode::Certificate`] or through the SSH/callback error
/// classes; HTTP transports instead surface a 401/403 or the word "authentication"
/// in the message, so both signals are checked.
fn is_auth(e: &git2::Error, msg: &str) -> bool {
    matches!(e.code(), ErrorCode::Auth | ErrorCode::Certificate)
        || matches!(e.class(), ErrorClass::Ssh)
        || msg.contains("authentication")
        || msg.contains("authorization")
        || msg.contains("credentials")
        || msg.contains("401")
        || msg.contains("403")
}

/// The push was refused as a non-fast-forward — the local branch is behind the
/// remote. libgit2 reports this only in the message (there's no dedicated code for
/// it on the push path), so it's matched on the wording git uses.
fn is_rejected(msg: &str) -> bool {
    msg.contains("fast-forward")
        || msg.contains("fast forward")
        || msg.contains("rejected")
        || msg.contains("cannot push")
        || msg.contains("non-fast-forward")
        || msg.contains("not present locally")
        || msg.contains("tip of your current branch is behind")
}

/// The remote couldn't be reached. Covers the network/TLS error classes plus the
/// message shapes for DNS/connection failures and timeouts that libgit2 sometimes
/// reports under a generic class.
fn is_offline(e: &git2::Error, msg: &str) -> bool {
    matches!(
        e.class(),
        ErrorClass::Net | ErrorClass::Ssl | ErrorClass::Http
    ) || msg.contains("could not resolve")
        || msg.contains("couldn't resolve")
        || msg.contains("failed to resolve")
        || msg.contains("connection refused")
        || msg.contains("could not connect")
        || msg.contains("couldn't connect")
        || msg.contains("failed to connect")
        || msg.contains("timed out")
        || msg.contains("timeout")
        || msg.contains("network is unreachable")
        || msg.contains("no route to host")
        || msg.contains("name or service not known")
        || msg.contains("temporary failure in name resolution")
}

#[cfg(test)]
mod error_taxonomy_tests {
    //! Hermetic (#531) tests over the remote-error taxonomy (#967): forge a
    //! `git2::Error` with a chosen class/code/message and assert `classify` buckets it
    //! into the expected category, and that each category renders an actionable
    //! message. No repository, network, or auth is involved — the mapping is pure.

    use super::*;

    /// Build a libgit2 error with a specific class/code/message, as the transports
    /// produce on a real failure.
    fn err(code: ErrorCode, class: ErrorClass, message: &str) -> git2::Error {
        git2::Error::new(code, class, message)
    }

    #[test]
    fn offline_is_categorized_from_the_network_class() {
        // A dropped/absent network surfaces under libgit2's Net class…
        let e = err(
            ErrorCode::GenericError,
            ErrorClass::Net,
            "failed to connect to github.com: Network is unreachable",
        );
        let mapped = classify(&e);
        assert_eq!(mapped.kind, RemoteErrorKind::Offline);
        let msg = mapped.message();
        assert!(msg.contains("Can't reach the remote"), "{msg}");
        assert!(msg.contains("connection"), "recovery hint present: {msg}");
    }

    #[test]
    fn offline_is_categorized_from_the_message_when_the_class_is_generic() {
        // …and even when libgit2 reports it under a generic class, the DNS/connection
        // wording still reads as offline.
        for detail in [
            "could not resolve host: origin",
            "Connection refused",
            "operation timed out",
        ] {
            let e = err(ErrorCode::GenericError, ErrorClass::None, detail);
            assert_eq!(
                classify(&e).kind,
                RemoteErrorKind::Offline,
                "should read as offline: {detail}"
            );
        }
    }

    #[test]
    fn auth_is_categorized_from_the_code_and_from_http_status() {
        // libgit2's own auth code…
        let by_code = classify(&err(
            ErrorCode::Auth,
            ErrorClass::None,
            "too many redirects or authentication replays",
        ));
        assert_eq!(by_code.kind, RemoteErrorKind::Auth);
        // …a certificate rejection…
        let by_cert = classify(&err(
            ErrorCode::Certificate,
            ErrorClass::Ssl,
            "the SSL certificate is invalid",
        ));
        assert_eq!(by_cert.kind, RemoteErrorKind::Auth);
        // …and an HTTP 401 the transport reports only in the message.
        let by_status = classify(&err(
            ErrorCode::GenericError,
            ErrorClass::Http,
            "request failed with status code: 401",
        ));
        assert_eq!(by_status.kind, RemoteErrorKind::Auth);

        let msg = by_code.message();
        assert!(msg.contains("Authentication failed"), "{msg}");
        assert!(
            msg.contains("SSH key") || msg.contains("QUILL_GIT_TOKEN"),
            "recovery hint present: {msg}"
        );
        // The multi-line literal joins on single spaces (no double-space seam).
        assert!(!msg.contains("  "), "no doubled spaces: {msg:?}");
    }

    #[test]
    fn rejected_non_fast_forward_is_categorized() {
        // The classic "pull first" push refusal, in git's own wording.
        for detail in [
            "cannot push non-fast-forward",
            "failed to push some refs: Updates were rejected",
            "the tip of your current branch is behind its remote counterpart",
        ] {
            let mapped = classify(&err(ErrorCode::GenericError, ErrorClass::None, detail));
            assert_eq!(
                mapped.kind,
                RemoteErrorKind::Rejected,
                "should read as rejected: {detail}"
            );
        }
        let msg = RemoteError::rejected().message();
        assert!(msg.contains("pull first"), "{msg}");
    }

    #[test]
    fn auth_wins_over_the_network_class() {
        // A 401 arriving over an HTTP transport (Net/Http class) is auth, not merely
        // "offline" — the ordering in `classify` guarantees this.
        let e = err(
            ErrorCode::GenericError,
            ErrorClass::Http,
            "remote authentication required but no callback set: 401",
        );
        assert_eq!(classify(&e).kind, RemoteErrorKind::Auth);
    }

    #[test]
    fn unknown_failures_fall_through_to_other_and_show_the_detail() {
        // Anything outside the three actionable buckets stays verbatim so the author
        // sees what actually went wrong.
        let e = err(
            ErrorCode::GenericError,
            ErrorClass::None,
            "unexpected packfile object",
        );
        let mapped = classify(&e);
        assert_eq!(mapped.kind, RemoteErrorKind::Other);
        let msg = mapped.message();
        assert!(msg.contains("Remote operation failed"), "{msg}");
        assert!(
            msg.contains("unexpected packfile object"),
            "the raw detail is preserved: {msg}"
        );
    }

    #[test]
    fn every_category_renders_a_nonempty_actionable_message() {
        for kind in [
            RemoteErrorKind::Offline,
            RemoteErrorKind::Auth,
            RemoteErrorKind::Rejected,
            RemoteErrorKind::Other,
        ] {
            let e = RemoteError {
                kind,
                detail: "detail".to_string(),
            };
            assert!(!e.message().trim().is_empty(), "{kind:?} has a message");
            // Display mirrors message() so `{e}` at a call site is the same string.
            assert_eq!(e.to_string(), e.message());
        }
    }
}
