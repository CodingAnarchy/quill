//! Starting a book on the phone, and its version history (#1297). A book
//! created here is an ordinary Quill project, laid out by the same core code
//! the desktop uses, and gets the same automatic history (#1296): a local Git
//! repository from the first word, with snapshots at the phone's save points.
//! Nothing here touches the network.

use std::path::Path;

use quill_core::project::{sanitize_dir_name, Project, ProjectTemplate};
use quill_core::vcs::{self, AutoHistory};

use crate::{ProjectForm, QuillError, QuillProject};

/// Create a new book called `title` in `projects_dir`, as a novel (one empty
/// chapter) or a short story, with version history started, and return its
/// folder. Refuses a title that names no folder, or a folder that's already
/// there, so an existing book is never overwritten.
#[uniffi::export]
pub fn create_project(
    title: &str,
    form: ProjectForm,
    projects_dir: &str,
) -> Result<String, QuillError> {
    let title = title.trim();
    let folder = sanitize_dir_name(title);
    if folder.is_empty() {
        return Err(QuillError::Invalid {
            message: "Give the book a title.".into(),
        });
    }
    let root = Path::new(projects_dir).join(&folder);
    if root.exists() {
        return Err(QuillError::Invalid {
            message: format!(
                "There's already a book called \u{201c}{folder}\u{201d} on this iPhone. Choose \
                 another title."
            ),
        });
    }
    let template = match form {
        ProjectForm::Novel => ProjectTemplate::at(0),
        ProjectForm::ShortStory => ProjectTemplate::short_story(),
        ProjectForm::Collection => {
            return Err(QuillError::Invalid {
                message: "A collection gathers short stories you've already written. Start a \
                          short story instead."
                    .into(),
            })
        }
    };
    let mut project = Project::create_with_template(&root, template)?;
    // The folder name stands in for the title until here; a title with a
    // slash in it keeps the slash in the book, if not in the folder.
    if project.manifest.title != title {
        project.manifest.title = title.to_string();
        project.manifest.sync_story_title();
        project.save_manifest()?;
    }
    drop(project); // release the lock before the first commit reads the folder
    vcs::start_auto_history(&root).map_err(|message| QuillError::Io { message })?;
    Ok(root.to_string_lossy().into_owned())
}

#[uniffi::export]
impl QuillProject {
    /// Give the book version history if it has none (#1296): the same as the
    /// desktop does on open. Returns whether a history was started just now.
    /// It commits the whole book the first time, so call it off the main
    /// thread. A book the author manages with Git, or one inside a larger
    /// repository, is left alone.
    pub fn start_history(&self) -> Result<bool, QuillError> {
        let root = self.lock().get()?.root.clone();
        let found = vcs::start_auto_history(&root).map_err(|message| QuillError::Io { message })?;
        Ok(found == AutoHistory::Started)
    }

    /// Snapshot the book's saved chapters into its history, if it's a history
    /// Quill started. Returns whether a snapshot was taken (`false` when nothing
    /// changed, or the history isn't Quill's). The caller saves the open
    /// chapter first; call it off the main thread.
    pub fn snapshot(&self) -> Result<bool, QuillError> {
        let root = self.lock().get()?.root.clone();
        if !vcs::is_auto_history(&root) {
            return Ok(false);
        }
        let commit = vcs::snapshot(&root).map_err(|message| QuillError::Io { message })?;
        Ok(commit.is_some())
    }
}

#[cfg(test)]
mod tests;
