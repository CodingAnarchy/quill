//! Removing inline annotations from the editor: the note(s) under the cursor (or
//! overlapping the Visual selection), or every note on the loaded chapter(s) after
//! a y/N confirm. The editor-side counterparts to the review list's `d` (#302),
//! split out of `annotations.rs` to keep it under the ~1,000-line convention.

use super::App;
use crate::mode::{Mode, Pending};

impl App {
    /// Remove the annotation(s) anchored under the cursor — or, in Visual mode,
    /// every one overlapping the selection. A cursor sitting just past an annotated
    /// word still counts as on it, matching how annotating picks the word there.
    pub(crate) fn remove_annotation_at_cursor(&mut self) {
        let (from, to) = if let Mode::Visual { anchor, linewise } = self.mode {
            self.selection_bytes(anchor, linewise)
        } else {
            let at = self.edit_buffer().cursor_byte();
            (at, at)
        };
        // A bare cursor hits a note it sits inside or at either edge of; a selection
        // hits any note it overlaps.
        let hit = |start: usize, end: usize| {
            if from == to {
                start <= from && from <= end
            } else {
                start < to && from < end
            }
        };
        let before = self.annotations.len();
        self.annotations.retain(|a| !hit(a.start, a.end));
        let removed = before - self.annotations.len();
        if removed == 0 {
            self.message = Some("No annotation at the cursor.".into());
            return;
        }
        self.persist_annotations();
        self.message = Some(removed_message(removed));
    }

    /// Ask before removing every annotation on the loaded chapter(s): the active
    /// chapter, or the whole continuous window.
    pub(crate) fn start_clear_annotations(&mut self) {
        let count = self.annotations.len();
        if count == 0 {
            self.message = Some("No annotations here.".into());
            return;
        }
        let noun = if count == 1 {
            "annotation"
        } else {
            "annotations"
        };
        self.open_prompt(
            Pending::ClearAnnotations,
            format!("Remove all {count} {noun} here? (y/N)"),
            "",
            None,
        );
    }

    /// Remove every annotation on the loaded chapter(s) — the confirmed
    /// [`start_clear_annotations`](Self::start_clear_annotations).
    pub(crate) fn clear_annotations(&mut self) {
        let removed = self.annotations.len();
        self.annotations.clear();
        self.persist_annotations();
        self.message = Some(removed_message(removed));
    }
}

/// The status message after removing `n` annotations.
fn removed_message(n: usize) -> String {
    if n == 1 {
        "Annotation removed.".into()
    } else {
        format!("{n} annotations removed.")
    }
}
