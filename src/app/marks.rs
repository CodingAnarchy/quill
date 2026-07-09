//! Vim marks and the jump-back list (#825).
//!
//! `m<x>` records the cursor's position under a mark letter; `` `<x> `` / `'<x>`
//! jumps to it. The big navigation jumps — the outline / corkboard, find, and
//! go-to-chapter — push the pre-jump position onto a bounded jumplist, and
//! `` `` `` / `''` pops it to return to where the author was before the jump.
//! Together they give a large manuscript the standard "get back to where I was"
//! return path.
//!
//! Marks and the jumplist are **global**: a [`JumpPos`] carries its chapter's
//! manifest index alongside the byte offset, so a mark set in one chapter (or a
//! jump that crossed chapters) returns to the right chapter first, then restores
//! the cursor. Recording a position is O(1) — two `usize`s, no rope work — so it
//! stays cheap on the edit / nav path.

use crate::mode::JumpPos;

use super::App;

/// The jumplist's cap: older entries drop past this, so pushing on the nav path
/// can never grow unbounded (Vim's own jumplist is bounded at 100).
const JUMPLIST_CAP: usize = 100;

impl App {
    /// The cursor's current position as a global [`JumpPos`] (#825): the active
    /// chapter's manifest index plus the cursor's byte offset in the edit buffer.
    /// O(1) — no rope serialization.
    pub(crate) fn current_position(&self) -> JumpPos {
        JumpPos {
            chapter: self.project.active_index(),
            byte: self.edit_buffer().cursor_byte(),
        }
    }

    /// Record the current cursor position onto the jump-back list (#825): what a
    /// big jump pushes so `` `` `` can return.
    pub(crate) fn push_jump(&mut self) {
        self.push_jump_from(self.edit_buffer().cursor_byte());
    }

    /// Record a jump-back target at byte `byte` in the active chapter (#825), for
    /// jumps whose starting point isn't the cursor's *current* byte — a search
    /// pushes the position it began from (`origin_byte`), not where navigating
    /// matches has since moved the cursor. Bounded: the oldest entry drops once the
    /// list is full, so pushing on the nav path can never grow unbounded.
    pub(crate) fn push_jump_from(&mut self, byte: usize) {
        let pos = JumpPos {
            chapter: self.project.active_index(),
            byte,
        };
        let list = &mut self.vim_state.jumplist;
        if list.len() >= JUMPLIST_CAP {
            list.pop_front();
        }
        list.push_back(pos);
    }

    /// `m<x>`: record the cursor's position under mark `name` (#825). A later
    /// `` `<x> `` / `'<x>` returns here. Marks are global, so re-setting the same
    /// letter overwrites the previous position, as in Vim.
    pub(crate) fn set_mark(&mut self, name: char) {
        let pos = self.current_position();
        self.vim_state.marks.insert(name, pos);
    }

    /// `` `<x> `` / `'<x>`: jump to mark `name` (#825). Pushes the pre-jump
    /// position onto the jumplist first (so `` `` `` returns), then restores the
    /// mark's chapter and cursor. A no-op with a notice when the mark is unset.
    pub(crate) fn jump_to_mark(&mut self, name: char) {
        let Some(pos) = self.vim_state.marks.get(&name).copied() else {
            self.message = Some(format!("Mark not set: {name}"));
            return;
        };
        self.push_jump();
        self.restore_jump(pos);
    }

    /// `` `` `` / `''`: back-jump — pop the jumplist to the position the last jump
    /// left from (#825), returning the author to where they were before an
    /// outline / corkboard / find / go-to / mark jump. A no-op with a notice when
    /// the list is empty.
    pub(crate) fn jump_back(&mut self) {
        let Some(pos) = self.vim_state.jumplist.pop_back() else {
            self.message = Some("No jump to go back to.".into());
            return;
        };
        self.restore_jump(pos);
    }

    /// Move the editor to a recorded [`JumpPos`] (#825): open its chapter (leaving
    /// the continuous view first, so the byte offset addresses that chapter's own
    /// buffer), then place the cursor at the offset, clamped to the buffer in case
    /// the chapter shrank since the position was recorded. A stale chapter index
    /// (deleted / reordered away) is dropped with a notice rather than jumping
    /// somewhere wrong.
    fn restore_jump(&mut self, pos: JumpPos) {
        if pos.chapter >= self.project.manifest.chapters.len() {
            self.message = Some("That position's chapter is gone.".into());
            return;
        }
        // A recorded byte offset addresses a single chapter's buffer, so return to
        // the single-chapter view before honoring it.
        self.leave_continuous();
        if pos.chapter != self.project.active_index() {
            self.activate_chapter(pos.chapter);
        }
        let at = pos.byte.min(self.buffer.len_bytes());
        self.buffer.cursor = self.buffer.cursor_at_byte(at);
        // Returning to a mark / jumplist position in the *same* chapter doesn't clear
        // folds, so the restored byte can sit inside a collapsed section. Reveal the
        // fold so the cursor lands on a visible line (#975); a cross-chapter restore
        // already cleared folds, so this no-ops.
        self.reveal_fold_at_cursor();
    }
}
