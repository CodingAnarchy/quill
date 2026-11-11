//! The in-editor scansion marks (#1326): with the toggle on, each line of a
//! chapter that has a target meter or rhyme scheme, and of each fenced verse
//! block in a chapter (#1337), shows its stress reading,
//! syllable count, and rhyme letter in a gutter beside it, and the words that
//! break the meter or the scheme are marked in the text (licences apart from
//! faults). `]m` / `[m` step between the lines that break it.
//!
//! The marks are computed on the idle tick over the edit buffer (one chapter, or
//! the continuous view's window) and cached by buffer revision, the targets, and
//! the project's pronunciations, never per keystroke or per draw; the renderer
//! reads the cache. Like the other revision aids, Quill only marks. It never
//! changes the verse.

use std::fmt::Write;
use std::thread;

use super::*;

use crate::verse::lexicon::Lexicon;
use crate::verse::marks::{self as verse_marks, Marks};
use crate::verse::{pronounce, Targets};

/// What the marks were computed from: the buffer's content revision, each
/// chapter in view with its targets, and the pronunciations.
pub(super) struct ScansionKey {
    rev: u64,
    targets: Vec<(usize, usize, Option<Meter>, Option<RhymeScheme>)>,
    lexicon: Lexicon,
}

impl App {
    /// Recompute the scansion marks for the edit buffer, off the keystroke path.
    /// Empty when the marks are off, when the focused pane isn't a chapter, or
    /// when no chapter in view has a target or a fenced verse block. Skips the
    /// scan when nothing it reads has changed since the last one.
    pub(crate) fn refresh_scansion(&mut self) {
        if !self.scansion_on || !self.focus_shows_active_chapter() {
            self.scansion = Marks::default();
            self.scansion_key = None;
            return;
        }
        let rev = self.edit_buffer().revision();
        let spans = match &self.manuscript {
            Some(m) => m.segment_spans(),
            None => vec![(
                self.project.active_index(),
                0,
                self.edit_buffer().len_bytes(),
            )],
        };
        let targets: Vec<_> = spans
            .into_iter()
            .map(|(i, start, end)| {
                (
                    start,
                    end,
                    self.project.chapter_meter(i).cloned(),
                    self.project.chapter_rhyme(i).cloned(),
                )
            })
            .collect();
        let lexicon = self.project.lexicon();
        if self
            .scansion_key
            .as_ref()
            .is_some_and(|k| k.rev == rev && k.targets == targets && &k.lexicon == lexicon)
        {
            return;
        }
        let mut marks = Marks::default();
        // Only on a changed revision, with the marks on: one copy of the window.
        let text = self.edit_buffer().text();
        if text.contains("```")
            || targets
                .iter()
                .any(|(_, _, m, r)| m.is_some() || r.is_some())
        {
            for (start, end, meter, rhyme) in &targets {
                let Some(span) = text.get(*start..*end) else {
                    continue;
                };
                let chapter = Targets {
                    meter: meter.clone(),
                    rhyme: rhyme.clone(),
                };
                marks.extend(verse::chapter_marks(span, *start, &chapter, lexicon));
            }
        }
        self.scansion = marks;
        self.scansion_key = Some(ScansionKey {
            rev,
            targets,
            lexicon: lexicon.clone(),
        });
    }

    /// Turn the scansion marks on or off, and remember the choice for the
    /// project. Turning them on parses the pronouncing dictionary off the UI
    /// thread first, then marks the view straight away.
    pub(crate) fn toggle_scansion(&mut self) {
        self.scansion_on = !self.scansion_on;
        let remembered = verse_marks::set_shown(&self.project.root, self.scansion_on);
        if !self.scansion_on {
            self.scansion = Marks::default();
            self.scansion_key = None;
            self.message = Some("Scansion marks off.".into());
            return;
        }
        thread::spawn(pronounce::warm);
        self.refresh_scansion();
        let lines = self.scansion.lines.len();
        let faults = self.scansion.fault_lines().len();
        let mut message = if lines == 0 {
            "Scansion marks on. Nothing here has a target meter or rhyme scheme (V or H in the chapter browser, or a ```verse block).".to_string()
        } else if faults == 0 {
            format!("Scansion marks on: {lines} lines, all keeping to the target.")
        } else {
            format!("Scansion marks on: {lines} lines, {faults} off the target (]m / [m to step).")
        };
        if let Err(e) = remembered {
            let _ = write!(message, " (Couldn't remember the choice: {e})");
        }
        self.message = Some(message);
    }

    /// Move to the next (`dir` > 0) or previous line that breaks the meter or
    /// the scheme. Wraps within the buffer in the continuous view; in the single
    /// view, crosses into the next chapter with a target and a fault.
    pub(crate) fn navigate_scansion(&mut self, dir: i32) {
        if !self.scansion_on {
            self.message = Some("Scansion marks are off. Turn them on with F3.".into());
            return;
        }
        self.refresh_scansion();
        let faults = self.scansion.fault_lines();
        let cur = self.edit_buffer().cursor_byte();
        let buffer = self.edit_buffer();
        let line_start = buffer.line_start_byte(buffer.line_of_byte(cur));
        let within = if dir >= 0 {
            faults.iter().copied().find(|&s| s > cur)
        } else {
            faults.iter().rev().copied().find(|&s| s < line_start)
        };
        if let Some(target) = within {
            self.land_on_line(target);
            return;
        }
        if self.manuscript.is_some() {
            let wrap = if dir >= 0 {
                faults.first()
            } else {
                faults.last()
            };
            match wrap {
                Some(&target) => {
                    self.land_on_line(target);
                    self.message = Some("Scansion: wrapped around.".into());
                }
                None => {
                    self.message = Some("Scansion: every line in view keeps to the target.".into());
                }
            }
            return;
        }
        self.scansion_across_chapters(dir);
    }

    /// Put the cursor at `byte` and reveal any fold hiding it.
    fn land_on_line(&mut self, byte: usize) {
        let buffer = self.edit_buffer_mut();
        buffer.cursor = buffer.cursor_at_byte(byte);
        self.reveal_fold_at_cursor();
    }

    /// Single view: walk to the next / previous chapter with a line off its
    /// target (its own, or a fenced verse block's), open it, and land on its
    /// first (forward) or last (backward) one.
    fn scansion_across_chapters(&mut self, dir: i32) {
        let active = self.project.active_index();
        let count = self.chapter_count();
        let mut idx = active;
        loop {
            idx = if dir >= 0 {
                if idx + 1 >= count {
                    self.message = Some("Scansion: end of the manuscript.".into());
                    return;
                }
                idx + 1
            } else {
                if idx == 0 {
                    self.message = Some("Scansion: start of the manuscript.".into());
                    return;
                }
                idx - 1
            };
            let chapter = self.chapter_targets(idx);
            let text = self.chapter_text_with_active(idx, active);
            if chapter.is_empty() && !text.contains("```") {
                continue;
            }
            let faults =
                verse::chapter_marks(&text, 0, &chapter, self.project.lexicon()).fault_lines();
            let landing = if dir >= 0 {
                faults.first()
            } else {
                faults.last()
            };
            if let Some(&target) = landing {
                self.activate_chapter(idx);
                let at = target.min(self.buffer.len_bytes());
                self.buffer.cursor = self.buffer.cursor_at_byte(at);
                self.reveal_fold_at_cursor();
                if let Some(title) = self.project.active_title() {
                    self.message = Some(format!("Scansion: {title}"));
                }
                return;
            }
        }
    }
}
