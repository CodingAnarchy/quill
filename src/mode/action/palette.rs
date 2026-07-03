//! The collapsible palette types: [`Palette`] state for an open
//! [`Mode::Palette`](crate::mode::Mode::Palette), its [`PaletteItem`] entries,
//! and the rendered [`PaletteRow`]s (headers plus items). Split out of
//! `action.rs` to keep each file under the ~1,000-line convention; re-exported
//! from the module root, so callers are unaffected. Pure move.

use super::Command;

/// State for an open [`Mode::Palette`](crate::mode::Mode::Palette): the entries, the
/// selected row, and which group headings are collapsed. `selected` indexes the
/// *visible* rows ([`Palette::rows`]) — headers plus the items of expanded groups
/// — not `items`, so it tracks what's on screen as groups collapse (#514).
pub struct Palette {
    /// Title shown in the palette's bar (e.g. "AI", "Version control").
    pub(crate) title: &'static str,
    pub(crate) items: Vec<PaletteItem>,
    pub(crate) selected: usize,
    /// Collapsed group headings — their items are hidden until expanded. Empty for
    /// a flat palette (every item ungrouped), which then renders exactly as before.
    pub(crate) collapsed: std::collections::HashSet<&'static str>,
}

/// One palette entry: a label, a one-line description, the command it runs, and an
/// optional group it belongs to (for the collapsible run-action menu, #514).
pub struct PaletteItem {
    pub(crate) label: String,
    pub(crate) desc: String,
    pub(crate) command: Command,
    /// The collapsible group this item sits under, or `None` for a flat (header-
    /// less) palette.
    pub(crate) group: Option<&'static str>,
}

/// A rendered row of an open palette: a collapsible group header, or a runnable
/// item carrying its index into [`Palette::items`] (#514).
pub enum PaletteRow {
    Header {
        group: &'static str,
        collapsed: bool,
    },
    Item(usize),
}

impl PaletteItem {
    /// An ungrouped palette entry — the default; flat palettes (AI, export, git,
    /// snippets, …) use this and render exactly as before.
    pub(crate) fn new(label: impl Into<String>, desc: impl Into<String>, command: Command) -> Self {
        Self {
            label: label.into(),
            desc: desc.into(),
            command,
            group: None,
        }
    }

    /// Place this entry under a collapsible group heading (the run-action menu).
    pub(crate) const fn in_group(mut self, group: &'static str) -> Self {
        self.group = Some(group);
        self
    }
}

impl Palette {
    /// The rows to show: a header before each group (an item whose group differs
    /// from the one above), then that group's items unless it's collapsed.
    /// Ungrouped items render flat with no header, so a palette with no groups
    /// yields exactly its items — the pre-#514 behavior.
    pub(crate) fn rows(&self) -> Vec<PaletteRow> {
        let mut rows = Vec::with_capacity(self.items.len());
        let mut current: Option<&'static str> = None;
        let mut hidden = false;
        for (i, item) in self.items.iter().enumerate() {
            match item.group {
                Some(g) if current == Some(g) => {
                    if !hidden {
                        rows.push(PaletteRow::Item(i));
                    }
                }
                Some(g) => {
                    current = Some(g);
                    hidden = self.collapsed.contains(g);
                    rows.push(PaletteRow::Header {
                        group: g,
                        collapsed: hidden,
                    });
                    if !hidden {
                        rows.push(PaletteRow::Item(i));
                    }
                }
                None => {
                    current = None;
                    hidden = false;
                    rows.push(PaletteRow::Item(i));
                }
            }
        }
        rows
    }

    /// Collapse (`want = true`) or expand a group heading (#514).
    pub(crate) fn set_collapsed(&mut self, group: &'static str, want: bool) {
        if want {
            self.collapsed.insert(group);
        } else {
            self.collapsed.remove(group);
        }
    }
}
