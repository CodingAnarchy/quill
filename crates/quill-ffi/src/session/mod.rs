//! Editing a chapter on the phone (#1232). The on-screen text lives in a
//! `UITextView`, and every change the author makes there is sent here as a
//! UTF-16 range and its replacement. The core keeps doing what it does on the
//! desktop's edit path: the rope edit, the incremental word count, the parse
//! (only for what Swift asks to style), the annotation re-anchoring, and the
//! atomic save that re-attaches the chapter's front matter byte for byte.

use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use quill_core::annotation::{Live, Remapper};
use quill_core::buffer::Buffer;
use quill_core::syntax::HighlightKind;
use quill_core::text::match_ranges;

use crate::{QuillError, QuillProject};

/// What a styled span of the chapter is.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Enum)]
pub enum StyleKind {
    Heading,
    Strong,
    Emphasis,
    Code,
    SceneBreak,
    /// An emphasis delimiter (`*`, `_`, `**`), which the desktop hides and the
    /// phone shows dimmed so the author can still edit it.
    Marker,
    /// The project's TODO marker (`xxx` by default, #1237), made to stand out.
    Todo,
}

/// A styled span, in UTF-16 code units.
#[derive(Clone, Copy, Debug, PartialEq, Eq, uniffi::Record)]
pub struct StyleSpan {
    pub start: u32,
    pub length: u32,
    pub kind: StyleKind,
}

/// One chapter open for editing.
#[derive(uniffi::Object)]
pub struct ChapterSession {
    project: Arc<QuillProject>,
    state: Mutex<Session>,
}

struct Session {
    buffer: Buffer,
    index: usize,
    file: String,
    /// The chapter's annotations, re-anchored through each burst of edits and
    /// written back on save so each note stays on its phrase.
    annotations: Vec<Live>,
    /// The project's TODO marker (#1237).
    todo: String,
}

#[uniffi::export]
impl QuillProject {
    /// Open chapter `index` for editing.
    pub fn edit_chapter(self: Arc<Self>, index: u32) -> Result<Arc<ChapterSession>, QuillError> {
        let index = index as usize;
        let (buffer, file, annotations, todo) = {
            let guard = self.lock();
            let p = guard.get()?;
            let Some(entry) = p.manifest.chapters.get(index) else {
                return Err(QuillError::NotFound {
                    message: format!("There's no chapter {}.", index + 1),
                });
            };
            let file = entry.file.clone();
            let buffer = Buffer::from_path(p.chapter_path(index))?;
            (
                buffer,
                file.clone(),
                p.annotations_for(&file),
                p.manifest.todo_marker().to_string(),
            )
        };
        Ok(Arc::new(ChapterSession {
            project: self,
            state: Mutex::new(Session {
                buffer,
                index,
                file,
                annotations,
                todo,
            }),
        }))
    }
}

#[uniffi::export]
impl ChapterSession {
    /// The chapter this session edits.
    pub fn chapter_index(&self) -> u32 {
        u32::try_from(self.lock().index).unwrap_or(u32::MAX)
    }

    /// The prose, without front matter.
    pub fn text(&self) -> String {
        self.lock().buffer.text()
    }

    /// The prose's length in UTF-16 code units, for Swift to check that its
    /// text view and the core agree.
    pub fn utf16_length(&self) -> u32 {
        u32::try_from(self.lock().buffer.len_utf16()).unwrap_or(u32::MAX)
    }

    /// Replace the UTF-16 range `[start, end)` with `replacement`: the edit the
    /// text view just made. Out-of-range positions clamp to the end.
    pub fn apply_edit(&self, start: u32, end: u32, replacement: &str) {
        let mut s = self.lock();
        let b = &mut s.buffer;
        let from = b.byte_of_utf16(start as usize);
        let to = b.byte_of_utf16(end as usize).max(from);
        b.replace_byte_range(from, to, replacement);
    }

    /// The live word count, counted the way the desktop counts.
    pub fn word_count(&self) -> u64 {
        self.lock().buffer.word_count() as u64
    }

    /// Whether there are edits not yet saved.
    pub fn is_dirty(&self) -> bool {
        self.lock().buffer.dirty
    }

    /// The styled spans overlapping the UTF-16 range `[start, end)`. Reparses
    /// first if the text changed, and only the edited blocks are reparsed.
    pub fn styles(&self, start: u32, end: u32) -> Vec<StyleSpan> {
        let mut guard = self.lock();
        let s = &mut *guard;
        let b = &mut s.buffer;
        b.refresh_syntax();
        let from = b.byte_of_utf16(start as usize);
        let to = b.byte_of_utf16(end as usize);
        let mut spans: Vec<StyleSpan> = b
            .syntax()
            .highlights_in(from, to)
            .map(|h| span(b, h.start, h.end, style_kind(h.kind)))
            .collect();
        let markers = b.syntax().conceal_ranges(None);
        let first = markers.partition_point(|&(_, e)| e <= from);
        spans.extend(
            markers[first..]
                .iter()
                .take_while(|&&(s, _)| s < to)
                .map(|&(s, e)| span(b, s, e, StyleKind::Marker)),
        );
        // TODO markers in the requested range, matched the way the desktop
        // counts them (case-insensitive), so both agree on what's a marker.
        let slice = b.byte_slice(from, to);
        spans.extend(
            match_ranges(&slice, &s.todo)
                .into_iter()
                .map(|(ms, me)| span(b, from + ms, from + me, StyleKind::Todo)),
        );
        spans
    }

    /// Save the chapter: the atomic write that re-attaches its front matter,
    /// then its annotations (re-anchored to the edited text) and the project's
    /// cached word count. A no-op when nothing changed.
    pub fn save(&self) -> Result<(), QuillError> {
        let mut s = self.lock();
        if !s.buffer.dirty {
            return Ok(());
        }
        s.buffer.save()?;
        let edits = s.buffer.drain_anchor_edits();
        if !edits.is_empty() && !s.annotations.is_empty() {
            let mapper = Remapper::new(&edits);
            s.annotations
                .retain_mut(|a| match mapper.remap_range(a.start, a.end) {
                    Some((start, end)) => {
                        a.start = start;
                        a.end = end;
                        true
                    }
                    None => false,
                });
        }
        let words = s.buffer.word_count();
        let todos = s.buffer.count_matches(&s.todo);
        let mut guard = self.project.lock();
        let p = guard.get_mut()?;
        if !edits.is_empty() {
            p.set_annotations([(s.file.as_str(), s.annotations.as_slice())])?;
        }
        p.set_chapter_words(s.index, words);
        // Keep the project-wide marker list's cache current (#1237).
        p.set_chapter_todo(s.index, todos);
        Ok(())
    }
}

impl ChapterSession {
    fn lock(&self) -> MutexGuard<'_, Session> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Run `f` over the buffer as it stands.
    pub fn with_buffer<T>(&self, f: impl FnOnce(&Buffer) -> T) -> T {
        f(&self.lock().buffer)
    }

    /// Run `f` over the buffer, which it may reparse.
    pub fn with_buffer_mut<T>(&self, f: impl FnOnce(&mut Buffer) -> T) -> T {
        f(&mut self.lock().buffer)
    }

    /// The project's TODO marker.
    pub fn todo(&self) -> String {
        self.lock().todo.clone()
    }

    /// The chapter's title.
    pub fn title(&self) -> String {
        let index = self.lock().index;
        self.project
            .lock()
            .get()
            .ok()
            .and_then(|p| p.chapter_title(index).map(str::to_string))
            .unwrap_or_default()
    }
}

const fn style_kind(kind: HighlightKind) -> StyleKind {
    match kind {
        HighlightKind::Heading => StyleKind::Heading,
        HighlightKind::Strong => StyleKind::Strong,
        HighlightKind::Emphasis => StyleKind::Emphasis,
        HighlightKind::Code => StyleKind::Code,
        HighlightKind::SceneBreak => StyleKind::SceneBreak,
    }
}

/// The byte range `[start, end)` as a UTF-16 span.
fn span(b: &Buffer, start: usize, end: usize, kind: StyleKind) -> StyleSpan {
    let s = b.utf16_of_byte(start);
    let e = b.utf16_of_byte(end);
    StyleSpan {
        start: u32::try_from(s).unwrap_or(u32::MAX),
        length: u32::try_from(e.saturating_sub(s)).unwrap_or(u32::MAX),
        kind,
    }
}

#[cfg(test)]
mod tests;
