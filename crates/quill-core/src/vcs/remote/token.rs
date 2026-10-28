//! An HTTPS token handed to the core by its front end (#1231). The iPhone app
//! keeps the author's personal access token in the iOS Keychain and passes it
//! here before a clone, pull, or sync. A phone has no ssh-agent, environment, or
//! git credential helper to fall back on. The token lives only in memory, never
//! in `.git/config` or any other file the core writes.

use std::sync::Mutex;

static TOKEN: Mutex<Option<String>> = Mutex::new(None);

/// Use `token` for HTTPS remotes from now on, or stop with `None`. An empty
/// token counts as none.
pub fn set_token(token: Option<String>) {
    let token = token.filter(|t| !t.trim().is_empty());
    *TOKEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner) = token;
}

/// The token the front end supplied, if any.
pub(super) fn supplied() -> Option<String> {
    TOKEN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}
