//! Exporting a book from the phone (#1300): a submission manuscript (Word), an
//! EPUB, or a PDF, written to a file for the share sheet. The compiling and the
//! rendering are the core's (`export::assemble`), the same code the desktop's
//! export runs, so a book comes out the same from either.

use std::fs;
use std::path::Path;

use quill_core::export::assemble::{self, Assembly, Format};
use quill_core::project::sanitize_dir_name;

use crate::{QuillError, QuillProject};

/// What to export a book as.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum ExportKind {
    /// Word, in standard manuscript format.
    Manuscript,
    /// An EPUB ebook.
    Epub,
    /// A PDF to read on screen.
    Pdf,
    /// A print interior PDF at the book's trim size.
    PrintPdf,
}

impl From<ExportKind> for Format {
    fn from(kind: ExportKind) -> Self {
        match kind {
            ExportKind::Manuscript => Self::Manuscript,
            ExportKind::Epub => Self::Epub,
            ExportKind::Pdf => Self::Pdf,
            ExportKind::PrintPdf => Self::PrintPdf,
        }
    }
}

/// A finished export: the file, and anything about it the author should know.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct ExportResult {
    /// The exported file, named for the book.
    pub path: String,
    /// Warnings about the styling (an unreadable font, a low-resolution cover)
    /// or the book (a collection story that couldn't be read).
    pub warnings: Vec<String>,
}

#[uniffi::export]
impl QuillProject {
    /// Export the active book (a series' open book; a short story or a
    /// collection as itself) as `kind` into `dest_dir`, and return the file.
    /// The caller saves the open chapter first. Refuses, naming the chapters,
    /// when a chapter's file is missing or unreadable, rather than export a book
    /// with a hole in it. Call it off the main thread: a novel takes a moment.
    pub fn export(&self, kind: ExportKind, dest_dir: &str) -> Result<ExportResult, QuillError> {
        let guard = self.lock();
        let project = guard.get()?;
        let unreadable = assemble::unreadable_chapters(project);
        if !unreadable.is_empty() {
            let names: Vec<String> = unreadable
                .iter()
                .map(|(title, file)| format!("\u{201c}{title}\u{201d} ({file})"))
                .collect();
            return Err(QuillError::NotFound {
                message: format!(
                    "Quill can't read {}. Restore the file from the chapter's History, or remove \
                     the chapter from the book, then export again.",
                    names.join(", ")
                ),
            });
        }
        let (style, _) = assemble::resolve_style(project);
        let doc = Assembly::new(project, None, &style).document(None);
        let format = Format::from(kind);
        let (bytes, mut warnings) = assemble::render(project, &doc, format);
        let bytes = bytes?;
        drop(doc); // the compiled book, before the file is written
        let missing = assemble::missing_stories(project);
        if missing > 0 {
            warnings.push(format!(
                "{missing} of the collection's stories couldn't be read and were left out."
            ));
        }
        let name = match sanitize_dir_name(&project.manifest.title) {
            title if title.is_empty() => "Untitled".to_string(),
            title => title,
        };
        fs::create_dir_all(dest_dir)?;
        let path = Path::new(dest_dir).join(format!("{name}.{}", format.extension()));
        fs::write(&path, bytes)?;
        Ok(ExportResult {
            path: path.to_string_lossy().into_owned(),
            warnings,
        })
    }
}

#[cfg(test)]
mod tests;
