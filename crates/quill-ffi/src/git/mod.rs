//! Getting a project onto the phone and keeping it current (#1231): clone it
//! from the author's HTTPS remote, then pull the desktop's pushes. The access
//! token comes from the iOS Keychain through [`set_git_token`] and stays in
//! memory; nothing here writes it to disk.

use std::path::Path;
use std::sync::Arc;

use quill_core::vcs;

use crate::{QuillError, QuillProject};

/// Use `token` (a personal access token) for HTTPS remotes, or clear it.
#[uniffi::export]
pub fn set_git_token(token: Option<String>) {
    vcs::set_token(token);
}

/// The folder a clone of `url` lands in, inside `projects_dir`.
#[uniffi::export]
pub fn clone_destination(url: &str, projects_dir: &str) -> String {
    Path::new(projects_dir)
        .join(vcs::folder_name(url))
        .to_string_lossy()
        .into_owned()
}

/// Clone the project at `url` into `projects_dir` and return its folder. On
/// any failure nothing is left behind, and the message says what to fix.
#[uniffi::export]
pub fn clone_project(url: &str, projects_dir: &str) -> Result<String, QuillError> {
    let url = url.trim();
    if !url.starts_with("https://") {
        return Err(QuillError::Remote {
            message: "Enter the repository's HTTPS address (it starts with https://).".into(),
        });
    }
    let dest = clone_destination(url, projects_dir);
    vcs::clone(url, Path::new(&dest)).map_err(|message| QuillError::Remote { message })?;
    Ok(dest)
}

/// What a sync did, for the app to tell the author.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct SyncReport {
    /// The sentence to show.
    pub message: String,
    /// The draft this phone's conflicting work was parked on, if it was.
    pub draft: Option<String>,
    /// Whether chapters on disk changed, so an open chapter must reload.
    pub reload: bool,
}

#[uniffi::export]
impl QuillProject {
    /// Sync with the remote (#1233): commit this phone's edits (as
    /// `commit_message`, which names the device), merge the remote's, and
    /// push. When a chapter changed on both sides, this phone's work goes to a
    /// `phone/<date>` draft for the desktop to merge, and the phone takes the
    /// remote's version. The project is reopened after, so cached counts are
    /// current. The caller saves any open chapter first.
    pub fn sync(self: Arc<Self>, commit_message: &str) -> Result<SyncReport, QuillError> {
        let root = self.lock().get()?.root.clone();
        let outcome = vcs::sync_or_divert(&root, commit_message)
            .map_err(|message| QuillError::Remote { message })?;
        self.reopen()?;
        Ok(match outcome {
            vcs::DeviceSync::Synced { summary, reload } => SyncReport {
                message: summary,
                draft: None,
                reload,
            },
            vcs::DeviceSync::Diverted { draft } => SyncReport {
                message: format!(
                    "This phone and the remote both changed the same chapter, so your edits \
                     were saved on the draft \u{201c}{draft}\u{201d}. Merge it on the desktop; \
                     this phone now has the remote's version."
                ),
                draft: Some(draft),
                reload: true,
            },
        })
    }

    /// The `origin` this project syncs with, if it has one.
    pub fn remote_url(&self) -> Result<Option<String>, QuillError> {
        Ok(vcs::remote_url(&self.lock().get()?.root))
    }

    /// Pull the remote's changes, then reopen the project so its chapter list
    /// and word counts reflect them. Returns a message for the author.
    pub fn pull(self: Arc<Self>) -> Result<String, QuillError> {
        let root = self.lock().get()?.root.clone();
        let message = vcs::pull(&root).map_err(|message| QuillError::Remote { message })?;
        self.reopen()?;
        Ok(message)
    }
}

#[cfg(test)]
mod tests;
