//! Cloning a project from its remote (#1231): how a novel the desktop pushes
//! reaches a second device. The clone either lands whole or not at all. It's
//! written to a hidden sibling folder and renamed into place only once it has
//! checked out and proven to be a Quill project, so a wrong token, a dropped
//! connection, or the wrong repository never leaves a half-cloned folder for
//! the author to stumble over later.

use std::fs;
use std::path::{Path, PathBuf};

use super::super::local::exclude_editor_state;
use super::{classify, credentials};

/// Clone the repository at `url` into the new folder `dest`. Fails, leaving
/// nothing behind, when `dest` already exists, the remote can't be reached or
/// refuses the credentials, or the repository has no `book.toml` at its top.
pub fn clone(url: &str, dest: &Path) -> Result<(), String> {
    if dest.exists() {
        return Err(format!(
            "There's already a project at {} — remove it or choose another name.",
            dest.display()
        ));
    }
    let staging = staging_path(dest);
    // A crashed earlier attempt may have left its staging folder; start clean.
    let _ = fs::remove_dir_all(&staging);
    let result = clone_into(url, &staging).and_then(|()| {
        fs::rename(&staging, dest).map_err(|e| format!("Couldn't finish the clone: {e}"))
    });
    if result.is_err() {
        let _ = fs::remove_dir_all(&staging);
    }
    result
}

fn clone_into(url: &str, staging: &Path) -> Result<(), String> {
    let mut callbacks = git2::RemoteCallbacks::new();
    callbacks.credentials(credentials);
    let mut fetch = git2::FetchOptions::new();
    fetch.remote_callbacks(callbacks);
    git2::build::RepoBuilder::new()
        .fetch_options(fetch)
        .clone(url, staging)
        .map_err(|e| classify(&e).message())?;
    if !staging.join(crate::project::MANIFEST_NAME).exists() {
        return Err(format!(
            "That repository isn't a Quill project: it has no {} at its top level.",
            crate::project::MANIFEST_NAME
        ));
    }
    exclude_editor_state(staging)
}

/// The hidden sibling folder a clone into `dest` is staged in.
fn staging_path(dest: &Path) -> PathBuf {
    let name = dest
        .file_name()
        .map_or_else(|| "project".into(), |n| n.to_string_lossy().into_owned());
    dest.with_file_name(format!(".{name}.cloning"))
}

/// A folder name for the project cloned from `url`: the repository's name,
/// without a trailing `.git` ("https://host/me/my-novel.git" is "my-novel").
pub fn folder_name(url: &str) -> String {
    let trimmed = url.trim().trim_end_matches('/');
    let last = trimmed.rsplit(['/', ':']).next().unwrap_or(trimmed);
    let name = last.strip_suffix(".git").unwrap_or(last);
    if name.is_empty() {
        "project".to_string()
    } else {
        name.to_string()
    }
}
