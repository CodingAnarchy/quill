//! TODO markers on the phone (#1237): the plain-text notes (`xxx` by default,
//! or the project's `todo_marker`) an author drops where something needs work.
//! They're ordinary text in the chapter, so they sync through git and count
//! toward the desktop's `xxx:` line, and the phone finds them with the same
//! case-insensitive match the desktop counts with.

use quill_core::buffer::Buffer;
use quill_core::text::{ellipsize, match_ranges};

use crate::{ChapterSession, QuillError, QuillProject};

/// One marker: where it is, and the line around it for the list.
#[derive(Clone, Debug, PartialEq, Eq, uniffi::Record)]
pub struct MarkerInfo {
    pub chapter_index: u32,
    pub chapter_title: String,
    /// The marker's position in the chapter's prose, in UTF-16 code units.
    pub start: u32,
    pub length: u32,
    /// The marker's line, shortened for a list row.
    pub line: String,
}

#[uniffi::export]
impl QuillProject {
    /// The project's TODO marker.
    pub fn todo_marker(&self) -> Result<String, QuillError> {
        Ok(self.lock().get()?.manifest.todo_marker().to_string())
    }

    /// Every marker in the active book, in manuscript order. Only chapters
    /// whose cached marker count (kept at open and on each save) is nonzero
    /// are read, so the list costs nothing for a chapter without markers.
    pub fn markers(&self) -> Result<Vec<MarkerInfo>, QuillError> {
        let guard = self.lock();
        let p = guard.get()?;
        let marker = p.manifest.todo_marker();
        let mut out = Vec::new();
        for i in (0..p.manifest.chapters.len()).filter(|&i| p.chapter_todo(i) > 0) {
            let Ok(body) = p.try_chapter_body(i) else {
                continue;
            };
            let title = p.chapter_title(i).unwrap_or_default();
            out.extend(markers_in(&Buffer::from_text(&body), marker, i, title));
        }
        Ok(out)
    }
}

#[uniffi::export]
impl ChapterSession {
    /// The markers in this chapter as it stands, saved or not.
    pub fn markers(&self) -> Vec<MarkerInfo> {
        let (index, marker) = (self.chapter_index() as usize, self.todo());
        let title = self.title();
        self.with_buffer(|b| markers_in(b, &marker, index, &title))
    }
}

fn markers_in(b: &Buffer, marker: &str, index: usize, title: &str) -> Vec<MarkerInfo> {
    let text = b.text();
    match_ranges(&text, marker)
        .into_iter()
        .map(|(start, end)| {
            let line = b.line(b.line_of_byte(start));
            let (s, e) = (b.utf16_of_byte(start), b.utf16_of_byte(end));
            MarkerInfo {
                chapter_index: u32::try_from(index).unwrap_or(u32::MAX),
                chapter_title: title.to_string(),
                start: u32::try_from(s).unwrap_or(u32::MAX),
                length: u32::try_from(e - s).unwrap_or(u32::MAX),
                line: ellipsize(line.trim(), 90),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
