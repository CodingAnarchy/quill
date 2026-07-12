//! Vim dot-repeat (`.`) recording and replay (#1004): the choke point every
//! editing key passes through so a change is captured, the compose/settle rule,
//! and the replay. Split out of `vim.rs` (pure move).
use crossterm::event::KeyEvent;

use crate::mode::{LastChange, Mode, VimPending};

use super::App;

impl App {
    /// Run one editing-mode key through `handle`, recording it into the
    /// dot-repeat buffer when it's part of a change command (#1004). The single
    /// choke point every Normal / Insert / Visual key passes through, so a change
    /// is captured no matter how many keystrokes (and mode hops) compose it.
    ///
    /// The rule mirrors Vim's: while a change is being *composed* (an operator or
    /// its motion is pending, or we're in Insert mode after `i`/`a`/`o`/`c…`)
    /// every key is appended; the moment the command settles back into a resting
    /// Normal state, the composed keys are committed as the last change *iff* they
    /// edited the buffer. A pure motion or a cancelled operator edits nothing, so
    /// it's discarded and the previous `.` target survives.
    pub(crate) fn dot_record_around(
        &mut self,
        key: KeyEvent,
        handle: impl FnOnce(&mut Self, KeyEvent),
    ) {
        // A replay drives the same handlers; don't record the keys it feeds back.
        if self.vim_state.dot.replaying {
            handle(self, key);
            return;
        }

        let was_composing = self.dot_composing();
        // Begin a fresh recording when a resting Normal-mode key might start a
        // change; continue an in-progress one. A key pressed while at rest that
        // turns out to be a bare motion is recorded then discarded on settle.
        if !was_composing && matches!(self.mode, Mode::Normal) {
            self.vim_state.dot.pending.clear();
            self.vim_state.dot.recording = true;
            self.vim_state.dot.start_seq = self.edit_buffer().edit_seq();
        }
        if self.vim_state.dot.recording {
            self.vim_state.dot.pending.push(key);
        }

        handle(self, key);

        // Still composing (operator/pending awaiting a key, or now in Insert):
        // keep collecting until the command settles.
        if self.dot_composing() {
            return;
        }
        self.dot_settle();
    }
    /// Whether a change command is mid-composition: an operator or a multi-key
    /// command is awaiting its next key, we're in Insert mode (an `i`/`c…` session
    /// runs until `Esc`), or we're in Visual mode building a selection toward an
    /// operator (#1034). A visual command (`vlld`, `V~`, `viwc…<Esc>`) composes
    /// from the `v`/`V` entry — itself recorded, since it starts at rest in Normal
    /// — through the terminating operator that drops back to Normal (or Insert, for
    /// `c`). An `Esc` that cancels the selection edits nothing, so `dot_settle`
    /// discards the recording and the previous `.` target survives, exactly as a
    /// bare Normal-mode motion does.
    fn dot_composing(&self) -> bool {
        matches!(self.mode, Mode::Insert | Mode::Visual { .. })
            || self.vim_state.op.is_some()
            || self.case_op.is_some() // a `g~`/`gu`/`gU` awaiting its motion (#1006)
            || self.vim_state.pending != VimPending::None
            // A pending count prefix (`3` of `3x`, `2` of `2dw`) is still composing
            // the command (#1142): its digit keys edit nothing, so without this the
            // recorder would settle and discard them, then start a fresh recording
            // at the operator key — capturing `x`/`dw`/`r` without the count, so `.`
            // replayed a count of 1. Keeping the count "composing" folds the digits
            // into the same recording, so `.` replays the counted command.
            || self.vim_state.count.is_some()
    }
    /// A composed command has settled back into resting Normal mode: commit its
    /// keys as the last change if it edited the buffer, else drop them.
    fn dot_settle(&mut self) {
        if !self.vim_state.dot.recording {
            return;
        }
        self.vim_state.dot.recording = false;
        let edited = self.edit_buffer().edit_seq() != self.vim_state.dot.start_seq;
        let keys = std::mem::take(&mut self.vim_state.dot.pending);
        if edited && !keys.is_empty() {
            self.vim_state.dot.last = Some(LastChange { keys });
        }
    }
    /// Vim `.`: replay the last recorded change at the cursor as one undo step.
    /// Re-feeds the recorded keystrokes through the normal dispatch, so every
    /// change command repeats through exactly the code that first ran it. A no-op
    /// when nothing has been changed yet. `count` replays it that many times
    /// (Vim's `{count}.`), all folded into the single undo step.
    pub(crate) fn dot_repeat(&mut self, count: usize) {
        let Some(change) = self.vim_state.dot.last.clone() else {
            return;
        };
        // Seal the current undo group so the replay's edits open a fresh one; it
        // stays open across the whole (synchronous, no idle tick) replay, so the
        // repeated change undoes in a single step — like the original edit.
        self.edit_buffer_mut().seal_undo_group();
        self.note_annotation_undo_point();

        self.vim_state.dot.replaying = true;
        for _ in 0..count.max(1) {
            for key in change.keys.iter().copied() {
                match self.mode {
                    Mode::Insert => self.on_key_insert(key),
                    Mode::Normal => self.on_key_normal(key),
                    Mode::Visual { .. } => self.on_key_visual(key),
                    _ => {}
                }
            }
        }
        // A replayed insert-session change (`i…`/`c…`) leaves Insert mode only if
        // the recorded `Esc` was replayed; it was, since recording ran through the
        // settling `Esc`. Clear any transient Vim state the replay left pending.
        self.vim_state.dot.replaying = false;
        self.vim_state.op = None;
        self.case_op = None;
        self.vim_state.pending = VimPending::None;
        // Close the replay's undo group so a following edit (a keystroke or a
        // further `.`) is its own step, not folded into this repeat.
        self.edit_buffer_mut().seal_undo_group();
    }
}
