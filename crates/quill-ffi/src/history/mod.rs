//! Starting a book on the phone, and its version history (#1297, #1299). A
//! book created here is an ordinary Quill project, laid out by the same core
//! code the desktop uses, and gets the same automatic history (#1296): a local
//! Git repository from the first word, with snapshots at the phone's save
//! points. A chapter's earlier versions can be listed, read, and compared with
//! the chapter as it is now. Nothing here touches the network.

use std::path::Path;

use quill_core::frontmatter;
use quill_core::project::{sanitize_dir_name, Project, ProjectTemplate};
use quill_core::vcs::{self, AutoHistory, Change};

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

/// The most versions of one chapter the history list shows.
const VERSION_LIMIT: usize = 500;

/// One earlier version of a chapter: a commit that changed it.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct VersionInfo {
    /// The commit, to read the chapter back at this version.
    pub rev: String,
    /// Where it came from, in the commit's words: "Snapshot, 2 Nov 2026 at
    /// 14:05", "Edits on iPhone", or the author's own message.
    pub summary: String,
    /// When, in seconds since the Unix epoch.
    pub seconds: i64,
}

/// How a run of text changed between two versions.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ChangeKind {
    Same,
    Added,
    Removed,
}

/// A run of text in a comparison, in reading order.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ChangeRun {
    pub kind: ChangeKind,
    pub text: String,
}

/// Compare an earlier version of a chapter (`old`) with the chapter now
/// (`new`), word by word: what was kept, added since, and cut since.
#[uniffi::export]
pub fn compare_versions(old: &str, new: &str) -> Vec<ChangeRun> {
    vcs::compare_prose(old, new)
        .into_iter()
        .map(|(change, text)| ChangeRun {
            kind: match change {
                Change::Same => ChangeKind::Same,
                Change::Added => ChangeKind::Added,
                Change::Removed => ChangeKind::Removed,
            },
            text,
        })
        .collect()
}

#[uniffi::export]
impl QuillProject {
    /// Chapter `index`'s earlier versions, newest first (#1299): every commit
    /// that changed its file. Empty for a book with no history of its own.
    pub fn chapter_versions(&self, index: u32) -> Result<Vec<VersionInfo>, QuillError> {
        let (root, file) = self.chapter_file(index)?;
        if !vcs::is_repo(&root) {
            return Ok(Vec::new());
        }
        let versions = vcs::file_log(&root, &file, VERSION_LIMIT)
            .map_err(|message| QuillError::Io { message })?;
        Ok(versions
            .into_iter()
            .map(|v| VersionInfo {
                rev: v.rev,
                summary: v.summary,
                seconds: v.seconds,
            })
            .collect())
    }

    /// Chapter `index`'s prose as it was at version `rev`, with its front
    /// matter split off as the editor shows it.
    pub fn chapter_text_at(&self, index: u32, rev: &str) -> Result<String, QuillError> {
        let (root, file) = self.chapter_file(index)?;
        let text = vcs::show_file_at(&root, rev, &file)
            .map_err(|message| QuillError::Io { message })?
            .ok_or_else(|| QuillError::NotFound {
                message: "That version doesn't have this chapter.".into(),
            })?;
        Ok(frontmatter::split(&text).1.to_string())
    }

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

impl QuillProject {
    /// The project's folder and chapter `index`'s file, relative to it.
    fn chapter_file(&self, index: u32) -> Result<(std::path::PathBuf, String), QuillError> {
        let guard = self.lock();
        let p = guard.get()?;
        let index = index as usize;
        if index >= p.manifest.chapters.len() {
            return Err(QuillError::NotFound {
                message: format!("There's no chapter {}.", index + 1),
            });
        }
        Ok((p.root.clone(), p.chapter_file(index).to_string()))
    }
}

#[cfg(test)]
mod tests;
