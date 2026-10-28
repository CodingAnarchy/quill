//! An open project as Swift sees it: its title and form, its books, and its
//! chapters with the same cached word counts and statuses the desktop shows.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use quill_core::frontmatter::Band;
use quill_core::project::{length_category, Form, Project};
use quill_core::vcs;

use crate::QuillError;

/// What kind of project this is (#1239, #1244).
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ProjectForm {
    Novel,
    ShortStory,
    Collection,
}

impl From<Form> for ProjectForm {
    fn from(form: Form) -> Self {
        match form {
            Form::Novel => Self::Novel,
            Form::ShortStory => Self::ShortStory,
            Form::Collection => Self::Collection,
        }
    }
}

/// The project as a whole.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ProjectInfo {
    pub title: String,
    pub author: Option<String>,
    pub form: ProjectForm,
    /// Every book of a series, in order; one entry for a standalone novel.
    pub books: Vec<BookInfo>,
    /// The book whose chapters [`QuillProject::chapters`] lists.
    pub active_book: u32,
    /// Words across the active book's chapters.
    pub words: u64,
    /// The short fiction length category ("flash fiction", "short story",
    /// "novelette", "novella length") of a short story's story chapter, as the
    /// desktop status bar names it; `None` for a novel or a collection.
    pub length_category: Option<String>,
}

/// One book of a series.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct BookInfo {
    pub index: u32,
    pub title: String,
    pub words: u64,
}

/// One chapter of the active book, in manuscript order.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ChapterInfo {
    pub index: u32,
    pub title: String,
    pub words: u64,
    /// The revision status label ("Draft", "Revised", "Final"), if one is set.
    pub status: Option<String>,
    /// The chapter's synopsis, if one is set.
    pub summary: Option<String>,
    /// Front matter (a dedication, say), body, or back matter.
    pub matter: Matter,
}

/// Where a chapter sits in the book.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum Matter {
    Front,
    Body,
    Back,
}

impl From<Band> for Matter {
    fn from(band: Band) -> Self {
        match band {
            Band::Front => Self::Front,
            Band::Body => Self::Body,
            Band::Back => Self::Back,
        }
    }
}

/// An open Quill project (#1230). Opening takes the same project lock the
/// desktop does, so a second app instance opens it read-only rather than
/// racing its saves; dropping the object releases the lock.
#[derive(uniffi::Object)]
pub struct QuillProject {
    /// `None` only for the moment between dropping a project and reopening it
    /// ([`reopen`](Self::reopen)), so the reopen doesn't find its own lock.
    inner: Mutex<Option<Project>>,
}

#[uniffi::export]
impl QuillProject {
    /// Open the project in the folder `path`.
    #[uniffi::constructor]
    pub fn open(path: &str) -> Result<Arc<Self>, QuillError> {
        let project = Project::open(path)?;
        // Opening writes the project lock under `.quill/`; keep it out of git in
        // a repository set up without Quill's `.gitignore` (#1231), so a pull
        // doesn't see it as uncommitted work. Best-effort: a read-only repo
        // still opens.
        let _ = vcs::exclude_editor_state(&project.root);
        Ok(Arc::new(Self {
            inner: Mutex::new(Some(project)),
        }))
    }

    /// The project's title, form, books, and totals.
    pub fn info(&self) -> Result<ProjectInfo, QuillError> {
        let guard = self.lock();
        let p = guard.get()?;
        let m = &p.manifest;
        let words: usize = (0..m.chapters.len()).map(|i| p.chapter_words(i)).sum();
        let books = (0..p.book_count())
            .map(|b| BookInfo {
                index: to_u32(b),
                title: p.book_title(b).unwrap_or(&m.title).to_string(),
                words: p.book_word_count(b) as u64,
            })
            .collect();
        Ok(ProjectInfo {
            title: p.series_title().unwrap_or(&m.title).to_string(),
            author: m.author.clone(),
            form: m.form.into(),
            books,
            active_book: to_u32(p.active_book()),
            words: words as u64,
            // Named from the story chapter alone, as the desktop status bar does
            // (a dedication or afterword doesn't change the story's length).
            length_category: m
                .story_chapter()
                .map(|i| length_category(p.chapter_words(i)).to_string()),
        })
    }

    /// The active book's chapters, in manuscript order.
    pub fn chapters(&self) -> Result<Vec<ChapterInfo>, QuillError> {
        let guard = self.lock();
        let p = guard.get()?;
        Ok(p.manifest
            .chapters
            .iter()
            .enumerate()
            .map(|(i, c)| ChapterInfo {
                index: to_u32(i),
                title: c.title.clone(),
                words: p.chapter_words(i) as u64,
                status: c.status.map(|s| s.label().to_string()),
                summary: c.summary.clone(),
                matter: p.chapter_band(i).into(),
            })
            .collect())
    }

    /// Chapter `index`'s prose, with its front matter split off exactly as the
    /// desktop editor shows it.
    pub fn chapter_text(&self, index: u32) -> Result<String, QuillError> {
        let guard = self.lock();
        let p = guard.get()?;
        let index = index as usize;
        if index >= p.manifest.chapters.len() {
            return Err(QuillError::NotFound {
                message: format!("There's no chapter {}.", index + 1),
            });
        }
        Ok(p.try_chapter_body(index)?)
    }

    /// Switch a series to book `index`; its chapters become the ones listed.
    pub fn set_active_book(&self, index: u32) -> Result<(), QuillError> {
        self.lock().get_mut()?.set_active_book(index as usize);
        Ok(())
    }

    /// The project's folder.
    pub fn path(&self) -> Result<String, QuillError> {
        Ok(self.lock().get()?.root.to_string_lossy().into_owned())
    }
}

impl QuillProject {
    /// The open project. Recovers from a poisoned lock: a panic on another
    /// thread mid-call must not make the author's open project unusable.
    pub fn lock(&self) -> ProjectGuard<'_> {
        ProjectGuard(self.inner.lock().unwrap_or_else(PoisonError::into_inner))
    }

    /// Close and reopen the project, so everything cached at open (word
    /// counts, statuses, the chapter list) reflects what's on disk now, after
    /// a pull rewrote it. The old project is dropped first to release its lock.
    pub fn reopen(&self) -> Result<(), QuillError> {
        let mut slot = self.inner.lock().unwrap_or_else(PoisonError::into_inner);
        let root = slot.as_ref().map(|p| p.root.clone());
        let Some(root) = root else {
            return Err(QuillError::Io {
                message: "The project isn't open.".into(),
            });
        };
        *slot = None;
        *slot = Some(Project::open(&root)?);
        Ok(())
    }
}

/// A locked, open project. A failed [`QuillProject::reopen`] can leave the
/// slot empty; every call after that reports the project as closed rather
/// than panicking.
pub struct ProjectGuard<'a>(MutexGuard<'a, Option<Project>>);

impl ProjectGuard<'_> {
    pub fn get(&self) -> Result<&Project, QuillError> {
        self.0.as_ref().ok_or_else(closed)
    }

    pub fn get_mut(&mut self) -> Result<&mut Project, QuillError> {
        self.0.as_mut().ok_or_else(closed)
    }
}

fn closed() -> QuillError {
    QuillError::Io {
        message: "The project couldn't be reopened. Close it and open it again.".into(),
    }
}

/// A count or index for Swift. Projects never approach `u32::MAX` chapters, so
/// this saturates rather than failing.
fn to_u32(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests;
