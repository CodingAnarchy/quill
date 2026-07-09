//! The fuzzy chapter/scene quick-switcher (#976): a `Ctrl-P`-style overlay that
//! lists every chapter *and* every scene heading and narrows them live by a typed
//! substring, jumping to the chosen target on Enter. It reuses the existing go-to
//! machinery — the same `activate_chapter` + jumplist path the corkboard and
//! outline jumps take — so a jump lands in a normal single-chapter buffer and
//! `` `` `` returns.
//!
//! Speed is the whole point on large / series projects, so the target index is
//! built **once** when the switcher opens (from the manifest's cached titles plus
//! a one-time read of each non-active chapter's scene headings — the same bounded,
//! off-keystroke work the manuscript-wide corkboard does). Filtering as the author
//! types then runs purely over that in-memory index: no per-keystroke file reads,
//! and independent of project size. The active chapter's headings come from its
//! live buffer so pending edits are reflected without a re-read.

use crossterm::event::{KeyCode, KeyEvent};

use crate::mode::{Mode, QuickSwitch, QuickTarget};
use crate::syntax::Syntax;

use super::App;

impl App {
    /// Open the quick-switcher (#976): build the chapter + scene-heading index and
    /// enter [`Mode::QuickSwitch`]. Reached from an editing mode; leaves the
    /// continuous view first so a jump lands in a single-chapter buffer, and flushes
    /// the active chapter so the index reads its latest saved headings for the
    /// off-buffer chapters. There is always at least one chapter, so the index is
    /// never empty.
    pub(crate) fn open_quick_switch(&mut self) {
        self.leave_continuous();
        let _ = self.flush_active();
        let targets = self.quick_switch_targets();
        self.mode = Mode::QuickSwitch(Box::new(QuickSwitch::new(targets)));
    }

    /// Build the quick-switcher's jump targets (#976): for every chapter, a target
    /// for the chapter itself (jumps to its top) followed by one per scene heading
    /// (level ≥ 2 — the `#` chapter title isn't a separate scene), each carrying the
    /// heading's byte offset. Built once when the switcher opens: the active
    /// chapter's headings come from its live buffer; the rest are read from disk one
    /// file at a time, off the keystroke path. The match `haystack` is pre-lowercased
    /// so the live filter never re-lowercases or allocates.
    fn quick_switch_targets(&self) -> Vec<QuickTarget> {
        let active = self.project.active_index();
        let mut targets = Vec::new();
        for ch in 0..self.chapter_count() {
            let title = self.project.chapter_title(ch).unwrap_or("").to_string();
            // The chapter itself — jumps to its top (byte 0).
            targets.push(QuickTarget {
                chapter: ch,
                byte: 0,
                haystack: title.to_lowercase(),
                title: title.clone(),
                chapter_title: String::new(),
                is_scene: false,
            });
            // Its scene headings. The active chapter is already parsed in the live
            // buffer; other chapters read + parse their body once here.
            let headings = if ch == active {
                self.edit_buffer().syntax().headings().to_vec()
            } else {
                Syntax::new(&self.project.chapter_body(ch))
                    .headings()
                    .to_vec()
            };
            for h in headings.iter().filter(|h| h.level >= 2) {
                let haystack = format!("{} {}", h.title, title).to_lowercase();
                targets.push(QuickTarget {
                    chapter: ch,
                    byte: h.start,
                    haystack,
                    title: h.title.clone(),
                    chapter_title: title.clone(),
                    is_scene: true,
                });
            }
        }
        targets
    }

    /// Key handling for the quick-switcher (#976): printable characters build the
    /// filter query (live), Backspace edits it, the arrows / Ctrl-N,P move within
    /// the narrowed list, Enter jumps to the highlighted target, and Esc closes
    /// without moving. Modeled on the chapter browser's filter sub-mode, but always
    /// in type-to-filter mode — the switcher is a single fuzzy field.
    pub(crate) fn on_key_quick_switch(&mut self, key: KeyEvent) {
        let Mode::QuickSwitch(state) = &mut self.mode else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.mode = self.editing_mode(),
            KeyCode::Enter => self.quick_switch_jump(),
            KeyCode::Up => state.step(-1),
            KeyCode::Down => state.step(1),
            KeyCode::Backspace => state.pop(),
            KeyCode::Char(c) => state.push(c),
            _ => {}
        }
    }

    /// Jump to the quick-switcher's selected target (#976) and close it: record the
    /// jump-back position, open the target chapter (a no-op when it's already
    /// active), then move the cursor to the target byte. Reuses the corkboard's
    /// jump shape so `` `` `` returns here. A no-op close when the filter left
    /// nothing selected.
    fn quick_switch_jump(&mut self) {
        let Mode::QuickSwitch(state) = &self.mode else {
            return;
        };
        let Some(target) = state.current().cloned() else {
            self.mode = self.editing_mode();
            return;
        };
        // Record where we jumped from before the cursor moves (possibly to another
        // chapter), so the jump-back list can return here.
        self.push_jump();
        self.mode = self.editing_mode();
        self.activate_chapter(target.chapter);
        let buf = self.edit_buffer_mut();
        let at = target.byte.min(buf.len_bytes());
        buf.cursor = buf.cursor_at_byte(at);
    }
}
