//! The type-to-filter state shared by the filterable list modes (#410/#612/#822):
//! the chapter browser, the focused outline, the annotation list, the history
//! view, the findings overlay, and the project browser. Each mode carries one
//! [`ListFilter`] beside its `selected` row; the shared key handling and the
//! step/snap helpers that move that selection over the rows the mode's own
//! predicate admits live in `crate::app::list_filter`.

/// A list's type-to-filter query and its input sub-mode. `/` sets `typing` so
/// keystrokes build `text` instead of running the list's single-key commands; Esc
/// or Enter leaves input (keeping the filter) and a second Esc clears it.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListFilter {
    /// The query as typed (case preserved for display).
    pub(crate) text: String,
    /// Whether keystrokes are building the query (the filter-input sub-mode).
    pub(crate) typing: bool,
}

impl ListFilter {
    /// An empty, not-typing filter — the state every list opens in.
    pub const fn new() -> Self {
        Self {
            text: String::new(),
            typing: false,
        }
    }

    /// The query lowercased, ready for a case-insensitive substring match.
    pub(crate) fn query(&self) -> String {
        self.text.to_lowercase()
    }

    /// Whether a query is narrowing the list.
    pub(crate) const fn is_active(&self) -> bool {
        !self.text.is_empty()
    }

    /// Drop the query and leave the input sub-mode.
    pub(crate) fn clear(&mut self) {
        self.text.clear();
        self.typing = false;
    }
}
