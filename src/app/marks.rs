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
//!
//! The **recent-edit-locations ring** (#1010) builds on the same [`JumpPos`]
//! machinery: it fills automatically on the edit path — no manual mark needed —
//! so the author can step back through the trail of where they were just editing,
//! with a direct `gi`-style jump to the last insertion. It coexists with the
//! manual marks and the nav jumplist (all three are independent), and a return
//! through it pushes the jumplist like any big jump, so `` `` `` still comes back.

use crate::mode::{JumpPos, Mode};
use crate::syntax::Syntax;

use super::App;

/// The jumplist's cap: older entries drop past this, so pushing on the nav path
/// can never grow unbounded (Vim's own jumplist is bounded at 100).
const JUMPLIST_CAP: usize = 100;

/// The recent-edit-locations ring's cap (#1010): the trail keeps this many recent
/// edit spots, the oldest dropping past it. Small — it's a breadcrumb of "where
/// was I just editing," not a full history — so cycling stays a short walk and the
/// ring can't grow on the edit path.
const EDIT_RING_CAP: usize = 20;

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
    pub(super) fn restore_jump(&mut self, pos: JumpPos) {
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

    // --- Recent edit locations ring (#1010) -------------------------------

    /// Whether the active buffer is manuscript prose, so an edit to it belongs in
    /// the recent-edit ring (#1010). The metadata / bible-review editors share the
    /// edit chokepoints but edit a *different* buffer, whose byte offsets don't
    /// address a chapter — recording those would put a meaningless spot in the
    /// ring, so they're excluded.
    const fn records_edits(&self) -> bool {
        !matches!(
            self.mode,
            Mode::EditMeta(_) | Mode::BibleReview(_) | Mode::ConflictReview(_)
        )
    }

    /// Record where an edit just happened onto the recent-edit ring (#1010) — the
    /// automatic breadcrumb the go-to-recent commands walk back through. Called
    /// from the edit path (insert / delete), so it stays cheap: O(1), no rope work.
    ///
    /// Consecutive edits on the **same line of the same chapter** coalesce into the
    /// newest entry (its byte is refreshed) rather than pushing a slot per
    /// keystroke, so cycling steps between *places*, not characters. Any fresh edit
    /// re-anchors the cycle at the newest spot (clears the walk cursor). Bounded —
    /// the oldest entry drops once [`EDIT_RING_CAP`] is reached.
    pub(crate) fn record_edit_location(&mut self) {
        if !self.records_edits() {
            return;
        }
        let pos = self.current_position();
        let line = self.edit_buffer().cursor.line;
        // Whether the newest spot sits on the same line as this edit — only
        // decidable when that spot is in the *active* chapter (its byte addresses
        // this buffer). A spot in another chapter never coalesces, so it doesn't
        // matter that we can't cheaply resolve its line.
        let coalesce = match self.vim_state.edits.spots.back() {
            Some(last) if last.chapter == pos.chapter => {
                self.edit_buffer().cursor_at_byte(last.byte).line == line
            }
            _ => false,
        };
        let ring = &mut self.vim_state.edits;
        // A fresh edit restarts the walk from the newest spot.
        ring.cursor = None;
        // Coalesce consecutive same-line edits into the newest entry — the common
        // case while typing a sentence — rather than a slot per keystroke.
        if coalesce {
            if let Some(last) = ring.spots.back_mut() {
                last.byte = pos.byte;
            }
            return;
        }
        if ring.spots.len() >= EDIT_RING_CAP {
            ring.spots.pop_front();
        }
        ring.spots.push_back(pos);
    }

    /// Record where text was last *inserted* (#1010) — the `gi` target — in
    /// addition to the ring spot. Insertions specifically (not deletions) update
    /// this, so "jump to last insertion" lands where the author last added prose.
    pub(crate) fn record_insertion(&mut self) {
        if !self.records_edits() {
            return;
        }
        self.vim_state.edits.last_insertion = Some(self.current_position());
        self.record_edit_location();
    }

    /// Cycle one step **older** through the recent-edit ring (#1010): move the
    /// cursor to the previous edit spot, pushing the pre-jump position onto the
    /// jumplist first (so `` `` `` returns) — the same big-jump contract marks
    /// honor. Repeated calls walk further back, bounded by the ring; a no-op with a
    /// notice when the ring is empty or already at the oldest spot.
    pub(crate) fn jump_to_older_edit(&mut self) {
        if self.vim_state.edits.spots.is_empty() {
            self.message = Some("No recent edits to step back to.".into());
            return;
        }
        // Start from the newest spot, then step back one each call. `len` (not
        // `len - 1`) as the resting cursor means the first step lands on the newest.
        let len = self.vim_state.edits.spots.len();
        let cur = self.vim_state.edits.cursor.unwrap_or(len);
        if cur == 0 {
            self.message = Some("Oldest edit — no earlier one.".into());
            return;
        }
        let next = cur - 1;
        let Some(pos) = self.vim_state.edits.spots.get(next).copied() else {
            return;
        };
        self.vim_state.edits.cursor = Some(next);
        self.push_jump();
        self.restore_jump(pos);
    }

    /// Cycle one step **newer** through the recent-edit ring (#1010): the inverse
    /// of [`jump_to_older_edit`], walking back toward the most recent spot. Only
    /// meaningful mid-cycle; a no-op with a notice when already at the newest (or
    /// not cycling).
    ///
    /// [`jump_to_older_edit`]: Self::jump_to_older_edit
    pub(crate) fn jump_to_newer_edit(&mut self) {
        let len = self.vim_state.edits.spots.len();
        let Some(cur) = self.vim_state.edits.cursor else {
            self.message = Some("Not stepping through edits.".into());
            return;
        };
        if cur + 1 >= len {
            self.message = Some("Newest edit — no later one.".into());
            return;
        }
        let next = cur + 1;
        let Some(pos) = self.vim_state.edits.spots.get(next).copied() else {
            return;
        };
        self.vim_state.edits.cursor = Some(next);
        self.push_jump();
        self.restore_jump(pos);
    }

    /// `gi`-style jump to the last insertion point (#1010): move the cursor to
    /// where text was most recently inserted, pushing the jumplist first so
    /// `` `` `` returns. A no-op with a notice before any insertion has happened.
    pub(crate) fn jump_to_last_insertion(&mut self) {
        let Some(pos) = self.vim_state.edits.last_insertion else {
            self.message = Some("No insertion to jump to yet.".into());
            return;
        };
        self.push_jump();
        self.restore_jump(pos);
    }

    // --- Scene-heading motion (#1009) -------------------------------------

    /// `]]` / `[[`: step the cursor to the next (`forward`) or previous scene
    /// heading (#1009), so the author walks a chapter's beats with one keystroke
    /// instead of a trip through the outline. Works from any cursor position and
    /// in either view — the headings come from the loaded buffer's live parse.
    ///
    /// Within the loaded buffer it's a plain cursor move (the jumplist isn't flooded
    /// by beat-stepping). When there's no heading left in the current direction it
    /// crosses into the nearest adjacent chapter that *has* one — the first heading
    /// forward, the last heading backward — as a big jump: it pushes the jumplist
    /// (so `` `` `` returns) and activates that chapter through the shared
    /// [`restore_jump`](Self::restore_jump) machinery. At the very ends of the
    /// manuscript it reports that there's no more rather than moving silently.
    pub(crate) fn jump_to_scene_heading(&mut self, forward: bool) {
        let cur = self.edit_buffer().cursor_byte();
        // The next heading within the loaded buffer, in the requested direction.
        let within = {
            let headings = self.edit_buffer().syntax().headings();
            if forward {
                headings.iter().find(|h| h.start > cur).map(|h| h.start)
            } else {
                headings
                    .iter()
                    .rev()
                    .find(|h| h.start < cur)
                    .map(|h| h.start)
            }
        };
        if let Some(at) = within {
            let buffer = self.edit_buffer_mut();
            buffer.cursor = buffer.cursor_at_byte(at);
            self.reveal_fold_at_cursor();
            return;
        }
        // No heading left this way in the current chapter — cross into the nearest
        // adjacent chapter that has one. The scan reads each candidate chapter's
        // body off disk once (off the keystroke path) rather than loading it.
        let active = self.project.active_index();
        let count = self.chapter_count();
        let target = if forward {
            (active + 1..count).find_map(|ch| self.first_heading_in_chapter(ch).map(|b| (ch, b)))
        } else {
            (0..active)
                .rev()
                .find_map(|ch| self.last_heading_in_chapter(ch).map(|b| (ch, b)))
        };
        match target {
            Some((chapter, byte)) => {
                self.push_jump();
                self.restore_jump(JumpPos { chapter, byte });
            }
            None => {
                self.message = Some(if forward {
                    "No next scene heading.".into()
                } else {
                    "No previous scene heading.".into()
                });
            }
        }
    }

    /// The byte offset of chapter `ch`'s first heading, or `None` when it has none
    /// (#1009). Reads the chapter body off disk and parses its headings — one read
    /// per scanned chapter, off the keystroke path — so a heading in an unloaded
    /// chapter is a valid cross-chapter jump target.
    fn first_heading_in_chapter(&self, ch: usize) -> Option<usize> {
        let body = self.project.chapter_body(ch);
        Syntax::new(&body).headings().first().map(|h| h.start)
    }

    /// The byte offset of chapter `ch`'s last heading, or `None` when it has none
    /// (#1009) — the backward-crossing counterpart of
    /// [`first_heading_in_chapter`](Self::first_heading_in_chapter).
    fn last_heading_in_chapter(&self, ch: usize) -> Option<usize> {
        let body = self.project.chapter_body(ch);
        Syntax::new(&body).headings().last().map(|h| h.start)
    }
}
