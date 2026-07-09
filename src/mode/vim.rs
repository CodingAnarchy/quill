//! Vim Normal-mode transient state (#202–#206, #811, #825): the pending count /
//! operator / register / find that drive multi-key commands, the mark and
//! jump-back positions, and the yank registers. Lives on `App` (not inside
//! [`Mode::Normal`](super::Mode)) so it survives a trip through Insert, the way
//! Vim's does. Split out of `mode.rs` to keep the core enums under the ~1,000-line
//! convention; re-exported from there, so callers (`crate::mode::{VimState, …}`)
//! are unaffected. Pure move.

use std::collections::{HashMap, VecDeque};

use crate::buffer::FindKind;

/// Transient Vim Normal-mode state (#202): a pending numeric count, a multi-key
/// command awaiting its next key, and the last find for `;` / `,`. Lives on
/// `App` (not inside [`Mode::Normal`](super::Mode)) so `last_find` survives a trip
/// through Insert mode, the way Vim's does.
#[derive(Default)]
pub struct VimState {
    /// Digits typed as a count prefix (`5` in `5j`); `None` when none is pending.
    pub(crate) count: Option<usize>,
    /// A multi-key command awaiting another key (`g…`, an `f`/`t` target, or an
    /// `i`/`a` text-object id while an operator is pending).
    pub(crate) pending: VimPending,
    /// The last `f`/`t`/`F`/`T` motion, replayed by `;` / `,`.
    pub(crate) last_find: Option<Find>,
    /// A pending operator (`d`/`c`/`y`) and its count, awaiting a motion or text
    /// object (`dw`, `ci"`, `yi(`, …).
    pub(crate) op: Option<(Op, usize)>,
    /// The yank/delete registers (#204): the unnamed register plus the named
    /// `"a`–`"z`.
    pub(crate) registers: Registers,
    /// The register `"x` chose for the next yank / delete / put; `None` is the
    /// unnamed register.
    pub(crate) selected_register: Option<char>,
    /// The system-clipboard register `"+` / `"*` was chosen for the next put
    /// (#811): `p` / `P` then paste the OS clipboard instead of an internal
    /// register. Cleared when the command that uses it completes, like
    /// `selected_register`. (Yank / delete already mirror to the clipboard, so the
    /// flag only needs to redirect the *read* side.)
    pub(crate) clipboard_register: bool,
    /// The last `/` / `?` / `*` search, replayed by `n` / `N` (#206).
    pub(crate) last_search: Option<VimSearch>,
    /// Vim marks (#825): a mark letter → the position `m<x>` recorded, jumped back
    /// to by `` `<x> `` / `'<x>`. Marks are **global** — a position carries its
    /// chapter, so a mark set in one chapter jumps back there even after moving
    /// to another. Survives a trip through Insert (it lives on `App`, like the
    /// other Vim state).
    pub(crate) marks: HashMap<char, JumpPos>,
    /// The jump-back list (#825): the positions the big jumps (outline / corkboard
    /// / find / go-to-chapter, and `` `<x> ``) leave, newest last. `` `` `` /
    /// `''` pops it to return to where a jump started. Bounded (older entries drop)
    /// so it can't grow without limit on the nav path.
    pub(crate) jumplist: VecDeque<JumpPos>,
}

/// A recorded editor position for a Vim mark or the jump-back list (#825): which
/// chapter (by its manifest index) and the byte offset within that chapter's
/// buffer. Storing the chapter makes marks and jumps **global** — returning to a
/// mark or popping the jumplist re-opens the right chapter first, then restores
/// the cursor. `Copy` (two `usize`s), so recording one is O(1) with no allocation
/// and no rope work.
#[derive(Clone, Copy)]
pub struct JumpPos {
    /// The chapter's manifest index at the time the position was recorded.
    pub(crate) chapter: usize,
    /// The cursor's byte offset within that chapter's buffer.
    pub(crate) byte: usize,
}

/// A recorded Vim search, so `n` / `N` can repeat it (#206).
#[derive(Clone)]
pub struct VimSearch {
    pub(crate) query: String,
    /// The search direction; `n` repeats it, `N` inverts it.
    pub(crate) backward: bool,
}

/// The Vim register store (#204): the unnamed register, written by every yank /
/// delete / change, and the named registers `"a`–`"z`. Numbered registers and
/// the system clipboard are out of scope.
#[derive(Default)]
pub struct Registers {
    pub(crate) unnamed: Option<Yank>,
    pub(crate) named: HashMap<char, Yank>,
}

/// A Vim Normal-mode command waiting on its next keystroke.
#[derive(Default, Clone, Copy, PartialEq, Eq)]
pub enum VimPending {
    #[default]
    None,
    /// `g` seen — awaiting the second `g` (for `gg`).
    G,
    /// `f`/`t`/`F`/`T` seen — awaiting the target character.
    Find(FindKind),
    /// `i` / `a` seen with an operator pending — awaiting the text-object id
    /// (`w`, `"`, `(`, …); `around` is the `a` form.
    Object { around: bool },
    /// `"` seen — awaiting the register name (`a`–`z`).
    Register,
    /// `]` / `[` seen — awaiting `s` for the spell-flag jump (#359): `]s` next
    /// misspelling, `[s` previous. `forward` is the `]` form.
    Bracket { forward: bool },
    /// `z` seen — awaiting `=` for the spelling-suggestions pick-list (#647),
    /// Vim's own `z=` spell-suggest binding; any other second key cancels.
    Z,
    /// `m` / `` ` `` / `'` seen — awaiting the mark letter (#825). `set` is the
    /// `m` form (record a mark); otherwise a jump — `` `` `` / `''` back-jumps
    /// through the jumplist, and a letter jumps to that mark.
    Mark { set: bool },
}

/// A Vim operator (#203): delete leaves Normal mode, change enters Insert, yank
/// (#204) copies and stays put.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Delete,
    Change,
    Yank,
}

/// Text captured by a delete / change / yank, for putting back with `p` / `P`.
#[derive(Clone)]
pub struct Yank {
    pub(crate) text: String,
    /// Whole-line text (from `dd`/`cc`/`yy`/linewise motions), put on its own
    /// lines rather than inline.
    pub(crate) linewise: bool,
}

/// A recorded find-char motion, so `;` / `,` can replay it.
#[derive(Clone, Copy)]
pub struct Find {
    pub(crate) kind: FindKind,
    pub(crate) target: char,
}
