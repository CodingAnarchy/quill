//! Type-to-filter keys for navigable list overlays (#822).
//!
//! Each overlay still owns Enter, dismiss, and its extra commands; this module
//! classifies the shared filter-input sub-mode so a new list does not copy the
//! `/`-loop by hand.

use crossterm::event::{KeyCode, KeyEvent};

/// One key in a list overlay's `/` filter-input sub-mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FilterInput {
    /// Esc: leave typing, keep the query. A second Esc (in command mode) is the
    /// overlay's own clear-or-close, not classified here.
    Leave,
    /// Enter: overlay-specific — leave typing, or jump/open the highlighted row.
    Enter,
    /// Arrow: `true` is down / next match.
    Step(bool),
    /// Backspace: drop the last query character.
    Pop,
    /// A printable character appended to the query (including `j`/`k`/`/`).
    Push(char),
}

/// Classify a key for the filter-input sub-mode. `None` means the overlay ignores
/// it (function keys, modifiers-only, …).
pub const fn filter_input(key: KeyEvent) -> Option<FilterInput> {
    match key.code {
        KeyCode::Esc => Some(FilterInput::Leave),
        KeyCode::Enter => Some(FilterInput::Enter),
        KeyCode::Up => Some(FilterInput::Step(false)),
        KeyCode::Down => Some(FilterInput::Step(true)),
        KeyCode::Backspace => Some(FilterInput::Pop),
        KeyCode::Char(c) => Some(FilterInput::Push(c)),
        _ => None,
    }
}

/// Classify a filter-input key, mapping Enter to [`FilterInput::Leave`] when
/// `enter_leaves` is set — the findings / outline / annotations / history
/// pattern, where Enter just ends typing. The chapter and project browsers pass
/// `false` so Enter stays a jump/open.
pub const fn on_filter_key(key: KeyEvent, enter_leaves: bool) -> Option<FilterInput> {
    match filter_input(key) {
        Some(FilterInput::Enter) if enter_leaves => Some(FilterInput::Leave),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::{filter_input, on_filter_key, FilterInput};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn filter_input_classifies_the_shared_keys() {
        assert_eq!(filter_input(key(KeyCode::Esc)), Some(FilterInput::Leave));
        assert_eq!(filter_input(key(KeyCode::Enter)), Some(FilterInput::Enter));
        assert_eq!(
            filter_input(key(KeyCode::Up)),
            Some(FilterInput::Step(false))
        );
        assert_eq!(
            filter_input(key(KeyCode::Down)),
            Some(FilterInput::Step(true))
        );
        assert_eq!(
            filter_input(key(KeyCode::Backspace)),
            Some(FilterInput::Pop)
        );
        assert_eq!(
            filter_input(key(KeyCode::Char('j'))),
            Some(FilterInput::Push('j'))
        );
        assert_eq!(filter_input(key(KeyCode::F(1))), None);
    }

    #[test]
    fn on_filter_key_maps_enter_to_leave_when_asked() {
        assert_eq!(
            on_filter_key(key(KeyCode::Enter), true),
            Some(FilterInput::Leave)
        );
        assert_eq!(
            on_filter_key(key(KeyCode::Enter), false),
            Some(FilterInput::Enter)
        );
        assert_eq!(
            on_filter_key(key(KeyCode::Esc), true),
            Some(FilterInput::Leave)
        );
    }
}
