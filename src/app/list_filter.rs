//! The shared type-to-filter handling for the filterable list modes (#410/#612/
//! #822): the chapter browser, the focused outline, the annotation list, the
//! history view, the findings overlay, and the project browser.
//!
//! Each mode carries a [`ListFilter`] beside its `selected` row and implements
//! [`FilteredList`] on a marker type: how to reach that state in [`App::mode`],
//! how many rows there are, and its own `matches` predicate. Everything else —
//! the filter-input key handler, the shared command keys (`/`, the arrows /
//! `j`/`k`, Esc-clears-the-filter), stepping over hidden rows, and snapping the
//! selection back onto a visible one when the query changes — is written once
//! here. A mode overrides a default only where it genuinely differs: the
//! annotation list steps in its sorted display order and its filter also covers
//! the per-reviewer filter (#933); the chapter and project browsers' Enter opens
//! the highlighted match straight from the query line (#410/#612).

use crossterm::event::{KeyCode, KeyEvent};

use super::App;
use crate::mode::ListFilter;

/// A list mode with a type-to-filter query (see the module docs). The methods
/// take the [`App`] because the predicates read live state (the buffer, the
/// project's cached chapter metadata); off the mode, the state accessors return
/// `None` and every default is a no-op.
pub(super) trait FilteredList {
    /// The mode's selected row and filter, `None` off the mode.
    fn state(app: &App) -> Option<(usize, &ListFilter)>;

    /// The mode's selected row and filter, mutably; `None` off the mode.
    fn state_mut(app: &mut App) -> Option<(&mut usize, &mut ListFilter)>;

    /// How many rows the list has (filtered or not).
    fn len(app: &App) -> usize;

    /// Whether row `index` passes the already-lowercased query `q` (empty = every
    /// row matches, unless the mode filters on something besides the query).
    fn matches(app: &App, index: usize, q: &str) -> bool;

    /// The active query, lowercased (empty off the mode or unfiltered).
    fn query(app: &App) -> String {
        Self::state(app).map_or_else(String::new, |(_, f)| f.query())
    }

    /// Whether the selected row passes the active filter — so a command never
    /// acts on a row the query has hidden.
    fn selected_matches(app: &App) -> bool {
        Self::state(app).is_some_and(|(sel, f)| Self::matches(app, sel, &f.query()))
    }

    /// Whether any narrowing filter is active; the first Esc clears it before a
    /// second closes the list.
    fn has_filter(app: &App) -> bool {
        Self::state(app).is_some_and(|(_, f)| f.is_active())
    }

    /// Clear the narrowing filter and leave the input sub-mode.
    fn clear(app: &mut App) {
        if let Some((_, f)) = Self::state_mut(app) {
            f.clear();
        }
    }

    /// Move the selection to `index`.
    fn select(app: &mut App, index: usize) {
        if let Some((s, _)) = Self::state_mut(app) {
            *s = index;
        }
    }

    /// Enter or leave the filter-input sub-mode.
    fn set_typing(app: &mut App, on: bool) {
        if let Some((_, f)) = Self::state_mut(app) {
            f.typing = on;
        }
    }

    /// The next (or previous, by `forward`) row that passes the filter, or the
    /// current selection unchanged when there's no further match that way.
    fn step(app: &App, forward: bool) -> usize {
        let Some((from, f)) = Self::state(app) else {
            return 0;
        };
        let q = f.query();
        step_matching(from, Self::len(app), forward, |i| Self::matches(app, i, &q))
    }

    /// After the filter changes, keep the selection on a visible row: if the
    /// current one no longer matches, jump to the first that does.
    fn snap(app: &mut App) {
        let Some((sel, f)) = Self::state(app) else {
            return;
        };
        let q = f.query();
        let to = snap_matching(sel, Self::len(app), |i| Self::matches(app, i, &q));
        Self::select(app, to);
    }

    /// Enter in the filter-input sub-mode: by default it just leaves input,
    /// keeping the filter so the list's commands act on the narrowed set.
    fn submit(app: &mut App) {
        Self::set_typing(app, false);
    }
}

impl App {
    /// The command-mode keys every filterable list shares: `/` starts the query,
    /// Up/`k` and Down/`j` move over the visible rows, and Esc clears an active
    /// filter (restoring the full list). Returns whether the key was handled; an
    /// unhandled Esc (no filter) is the mode's own close.
    pub(super) fn on_key_list_command<L: FilteredList>(&mut self, key: KeyEvent) -> bool {
        match key.code {
            KeyCode::Char('/') => L::set_typing(self, true),
            KeyCode::Up | KeyCode::Char('k') => self.list_move::<L>(false),
            KeyCode::Down | KeyCode::Char('j') => self.list_move::<L>(true),
            KeyCode::Esc if L::has_filter(self) => L::clear(self),
            _ => return false,
        }
        true
    }

    /// Keys for a list's filter-input sub-mode: printable characters build the
    /// query (live), Backspace edits it, the arrows move within the matches, Esc
    /// leaves input — keeping the filter (a second Esc, in command mode, clears
    /// it) — and Enter is the mode's [`submit`](FilteredList::submit).
    pub(super) fn on_key_list_filter<L: FilteredList>(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => L::set_typing(self, false),
            KeyCode::Enter => L::submit(self),
            KeyCode::Up => self.list_move::<L>(false),
            KeyCode::Down => self.list_move::<L>(true),
            KeyCode::Backspace => self.list_edit::<L>(|f| {
                f.text.pop();
            }),
            KeyCode::Char(c) => self.list_edit::<L>(|f| f.text.push(c)),
            _ => {}
        }
    }

    /// Step the selection to the next / previous visible row.
    fn list_move<L: FilteredList>(&mut self, forward: bool) {
        let to = L::step(self, forward);
        L::select(self, to);
    }

    /// Edit the query, then snap the selection back onto a visible row.
    fn list_edit<L: FilteredList>(&mut self, edit: impl FnOnce(&mut ListFilter)) {
        if let Some((_, f)) = L::state_mut(self) {
            edit(f);
        }
        L::snap(self);
    }
}

/// The next (or previous, by `forward`) index in `0..n` after `from` that
/// `matches`, or `from` unchanged when there's no further match that way — the
/// list clamps at its ends rather than wrapping.
pub(super) fn step_matching(
    from: usize,
    n: usize,
    forward: bool,
    matches: impl Fn(usize) -> bool,
) -> usize {
    let mut i = from;
    loop {
        if forward {
            if i + 1 >= n {
                return from;
            }
            i += 1;
        } else {
            if i == 0 {
                return from;
            }
            i -= 1;
        }
        if matches(i) {
            return i;
        }
    }
}

/// Keep `selected` on a visible row after the filter changes: unchanged when it
/// still `matches`, else the first index in `0..n` that does, else unchanged.
pub(super) fn snap_matching(selected: usize, n: usize, matches: impl Fn(usize) -> bool) -> usize {
    if matches(selected) {
        return selected;
    }
    (0..n).find(|&i| matches(i)).unwrap_or(selected)
}

/// The next (or previous) row after `from` in a display `order` of visible
/// indices — for a list whose display order isn't index order (the sorted
/// annotation list, #933). `from` unchanged at the end that way; the first
/// visible row (or `from`, when none) when `from` isn't visible at all.
pub(super) fn step_in_order(order: &[usize], from: usize, forward: bool) -> usize {
    let Some(pos) = order.iter().position(|&i| i == from) else {
        return order.first().copied().unwrap_or(from);
    };
    let next = if forward {
        pos + 1
    } else {
        pos.wrapping_sub(1)
    };
    order.get(next).copied().unwrap_or(from)
}

/// [`snap_matching`] over a display `order`: `selected` when it's visible, else
/// the first visible row, else unchanged.
pub(super) fn snap_in_order(order: &[usize], selected: usize) -> usize {
    if order.contains(&selected) {
        return selected;
    }
    order.first().copied().unwrap_or(selected)
}
