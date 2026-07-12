//! Vim Normal-mode transient state (#202–#206, #811, #825): the pending count /
//! operator / register / find that drive multi-key commands, the mark and
//! jump-back positions, and the yank registers. Lives on `App` (not inside
//! [`Mode::Normal`](super::Mode)) so it survives a trip through Insert, the way
//! Vim's does. Split out of `mode.rs` to keep the core enums under the ~1,000-line
//! convention; re-exported from there, so callers (`crate::mode::{VimState, …}`)
//! are unaffected. Pure move.

use std::collections::{HashMap, VecDeque};

use crossterm::event::KeyEvent;

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
    /// Dot-repeat state (#1004): the keys of the change command currently being
    /// composed, and the completed last change `.` replays. The foundation the
    /// later mechanics tickets (#1005 join/reflow, #1006 transpose/case) reuse —
    /// any command that edits is recorded here automatically, with no per-command
    /// bookkeeping, because recording keys off whether the buffer changed.
    pub(crate) dot: DotRepeat,
    /// The recent-edit-locations ring (#1010): where edits recently happened, so
    /// the author can step back through their trail without dropping marks by
    /// hand. Updated automatically on the edit path; independent of [`marks`] and
    /// the [`jumplist`], which it lives alongside. See [`EditRing`].
    ///
    /// [`marks`]: Self::marks
    /// [`jumplist`]: Self::jumplist
    pub(crate) edits: EditRing,
}

/// Vim `.` (repeat) state (#1004). The design records the raw key sequence of a
/// change command and replays it, the way Vim itself does — so *every* change,
/// including operators, text objects, insert-then-escape, and the change
/// commands later tickets add, becomes repeatable without listing them here.
///
/// A command is "a change" if it mutated the buffer (checked via the buffer's
/// edit counter); a pure motion or a mode switch that edited nothing never
/// overwrites [`last`](DotRepeat::last), so `.` keeps repeating the previous real
/// edit — exactly Vim's rule.
#[derive(Default)]
pub struct DotRepeat {
    /// Keys of the change command being composed, accumulated across the
    /// keystrokes that make it up (an operator and its motion, or the whole
    /// Insert session from `i`/`a`/`c…` through `Esc`). Committed to `last` when
    /// the command completes having changed the buffer; discarded otherwise.
    pub(crate) pending: Vec<KeyEvent>,
    /// True while a change command is mid-flight and its keys should be recorded:
    /// set when a potentially-changing Normal-mode key starts, cleared when the
    /// command resolves. Keeps stray keys (a bare motion, a `.` replay itself) out
    /// of the pending buffer.
    pub(crate) recording: bool,
    /// The buffer's edit counter when the current recording began, so completing
    /// the command can tell whether it actually edited (commit) or was a pure
    /// motion / cancelled operator (discard). Meaningful only while `recording`.
    pub(crate) start_seq: u64,
    /// True while `.` is replaying, so the replayed keystrokes aren't themselves
    /// re-recorded (which would rewrite `last` with an identical copy — harmless,
    /// but the guard keeps the replay side-effect-free and the intent clear).
    pub(crate) replaying: bool,
    /// The last completed change, replayed by `.`. `None` until the first edit.
    pub(crate) last: Option<LastChange>,
}

/// A completed change command (#1004), stored as the key sequence that produced
/// it so `.` can replay it verbatim through the normal dispatch. Keeping the
/// record as replayable keystrokes — rather than a parsed operator/motion —
/// means the later mechanics tickets get repeat "for free": a new change command
/// is recorded and replayed with no change here.
#[derive(Clone)]
pub struct LastChange {
    /// The keys that made up the change (e.g. `d`,`w` for `dw`; `c`,`i`,`"`,…,
    /// `Esc` for `ci"…<Esc>`), replayed in order at the cursor.
    pub(crate) keys: Vec<KeyEvent>,
}

/// The bounded ring of recent edit locations (#1010): a breadcrumb of where the
/// author has been editing, cycled back through with the go-to-recent commands
/// and used for the `gi`-style "jump to last insertion." Filled automatically on
/// the edit path — the author never manages it — and kept independent of the
/// manually-set Vim marks and the nav jumplist, which it coexists with.
///
/// Entries are **coalesced by line**: consecutive edits on the same line of the
/// same chapter fold into one entry (moved to the newest slot) rather than
/// flooding the ring with a slot per keystroke, so cycling steps between *places*
/// the author edited, not individual characters. Bounded (older entries drop), so
/// it can never grow without limit on the hot path. `last_insertion` records the
/// most recent *insertion* specifically, for the direct "jump to last insertion."
#[derive(Default)]
pub struct EditRing {
    /// Recent edit spots, oldest first / newest last. Each entry is one line the
    /// author edited (coalesced); bounded — the oldest drops once full.
    pub(crate) spots: VecDeque<JumpPos>,
    /// A cursor into `spots` while cycling back through them: `None` when not
    /// mid-cycle (the next "go to older edit" starts from the newest). Reset the
    /// moment a fresh edit lands, so a new edit re-anchors the walk at the newest.
    pub(crate) cursor: Option<usize>,
    /// Where text was last *inserted* (#1010) — the `gi` target. Distinct from the
    /// ring's newest spot, which also advances on deletions.
    pub(crate) last_insertion: Option<JumpPos>,
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
    /// `r` (or `{count}r`) seen — awaiting the replacement character (#1106).
    /// Replaces `count` graphemes under the cursor with that character; the pending
    /// state keeps the dot-repeat recorder composing across the two keystrokes, so
    /// `.` replays the whole `{count}r<char>` (#1004).
    Replace { count: usize },
    /// `>` / `<` seen — awaiting the doubled key for the linewise indent / dedent
    /// (#1106): `>>` indents `count` lines a level, `<<` dedents them. `dedent` is
    /// the `<` form. Any other second key cancels, as Vim's `>`+non-motion does.
    Indent { dedent: bool, count: usize },
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
