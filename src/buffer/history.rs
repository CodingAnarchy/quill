//! Undo / redo (#145): whole-document snapshots grouped by typing burst. Split
//! out of `buffer.rs` (#150); these are `Buffer` methods reached via
//! `use super::*`.

use super::*;

/// One undo/redo entry: the whole document plus where the cursor was (#145).
/// ropey ropes clone cheaply — the node tree is shared via `Arc` — so keeping a
/// snapshot per edit group is light even on a large chapter.
#[derive(Clone)]
pub(super) struct Snapshot {
    rope: Rope,
    cursor: Cursor,
}

/// Cap on retained undo entries, so a long writing session doesn't grow the
/// stacks without bound. 256 groups (each a typing burst) is plenty of reach.
const MAX_UNDO: usize = 256;

impl Buffer {
    // --- Undo / redo (#145) ------------------------------------------------

    /// Open a new undo group if one isn't already open, snapshotting the current
    /// (pre-edit) state to return to. Any redo history is invalidated, since the
    /// document is about to diverge from it. Called from the edit choke points
    /// ([`edit_insert`]/[`edit_remove`]) so every mutator gets undo for free.
    pub(super) fn checkpoint(&mut self) {
        if self.group_open {
            return;
        }
        self.undo_stack.push_back(Snapshot {
            rope: self.rope.clone(),
            cursor: self.cursor,
        });
        if self.undo_stack.len() > MAX_UNDO {
            self.undo_stack.pop_front();
        }
        self.redo_stack.clear();
        self.group_open = true;
        self.checkpointed = true; // signal the app to snapshot annotations (#370)
    }

    /// The retained-undo-groups cap, so the app's parallel annotation history
    /// (#370) can stay the same depth as the buffer's and pop in lockstep.
    pub const MAX_UNDO_GROUPS: usize = MAX_UNDO;

    /// Whether a new undo group has opened since the last call (#370), clearing
    /// the flag. The app reads this to snapshot annotations alongside the buffer's
    /// undo stack, since undo/redo restores the rope without an edit delta.
    pub fn take_checkpointed(&mut self) -> bool {
        std::mem::take(&mut self.checkpointed)
    }

    /// Close the current edit group so the next edit starts a fresh undo entry.
    /// Called when typing goes idle, so one undo reverses a burst of typing.
    pub const fn seal_undo_group(&mut self) {
        self.group_open = false;
    }

    /// Whether an edit group is currently open. The continuous view reads this
    /// around each edit to snapshot its per-chapter bookkeeping in lockstep with
    /// the buffer's rope stack (#649): a `false → true` transition means a new
    /// group just opened and a fresh segment snapshot belongs on the stack.
    pub const fn group_open(&self) -> bool {
        self.group_open
    }

    /// Revert to the state before the most recent edit group. Returns `false`
    /// when there's nothing to undo.
    pub fn undo(&mut self) -> bool {
        self.step(true)
    }

    /// Re-apply the most recently undone group. Returns `false` when there's
    /// nothing to redo.
    pub fn redo(&mut self) -> bool {
        self.step(false)
    }

    /// Move one step through history: pop from one stack onto the other and
    /// restore. `undo` true pops the undo stack (pushing the current state for
    /// redo); false does the reverse. The two are exact mirrors.
    fn step(&mut self, undo: bool) -> bool {
        let current = Snapshot {
            rope: self.rope.clone(),
            cursor: self.cursor,
        };
        let snap = if undo {
            self.undo_stack.pop_back()
        } else {
            self.redo_stack.pop_back()
        };
        let Some(snap) = snap else { return false };
        if undo {
            self.redo_stack.push_back(current);
        } else {
            self.undo_stack.push_back(current);
        }
        self.restore(snap);
        true
    }

    /// Swap the document for a snapshot (undo/redo). The rope changes wholesale,
    /// so the incremental parse tree is stale — reparse fully rather than feeding
    /// the parser edits it never saw.
    fn restore(&mut self, snap: Snapshot) {
        self.rope = snap.rope;
        self.cursor = snap.cursor;
        self.goal_x = None;
        self.group_open = false; // the next edit opens a fresh group
        self.dirty = true;
        self.clamp_cursor();
        self.word_count_cache.set(None); // the whole document changed (#151)
        self.syntax = Syntax::new(&self.rope.to_string());
        self.syntax_dirty = false;
    }

    /// Keep the cursor inside the document after a snapshot restore.
    fn clamp_cursor(&mut self) {
        let last_line = self.line_count().saturating_sub(1);
        self.cursor.line = self.cursor.line.min(last_line);
        self.cursor.col = self.cursor.col.min(self.line_graphemes(self.cursor.line));
    }
}
