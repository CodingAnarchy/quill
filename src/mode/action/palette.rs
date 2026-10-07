//! The collapsible palette types: [`Palette`] state for an open
//! [`Mode::Palette`](crate::mode::Mode::Palette), its [`PaletteItem`] entries,
//! and the rendered [`PaletteRow`]s (headers plus items). Split out of
//! `action.rs` to keep each file under the ~1,000-line convention; re-exported
//! from the module root, so callers are unaffected. Pure move.

use std::collections::HashSet;

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
    pub(crate) collapsed: HashSet<&'static str>,
}

/// One palette entry: a label, a one-line description, the command it runs, an
/// optional group it belongs to (for the collapsible run-action menu, #514), and
/// the dedicated keyboard shortcut that also runs it, if any (#1103).
pub struct PaletteItem {
    pub(crate) label: String,
    pub(crate) desc: String,
    pub(crate) command: Command,
    /// The collapsible group this item sits under, or `None` for a flat (header-
    /// less) palette.
    pub(crate) group: Option<&'static str>,
    /// The action's dedicated key binding shown alongside the label so the palette
    /// doubles as a live cheat sheet (#1103), or `None` for the palette-only actions
    /// (which then show no shortcut — no clutter). Set from [`Action::shortcut`], the
    /// single source of truth shared with the key dispatch, so it can't drift.
    pub(crate) shortcut: Option<&'static str>,
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
            shortcut: None,
        }
    }

    /// Place this entry under a collapsible group heading (the run-action menu).
    pub(crate) const fn in_group(mut self, group: &'static str) -> Self {
        self.group = Some(group);
        self
    }

    /// Show this entry's dedicated keyboard shortcut beside its label (#1103) — the
    /// run-action menu sets it from [`Action::shortcut`](super::Action::shortcut) so
    /// the palette teaches the real binding. `None` (the default) shows nothing.
    pub(crate) const fn with_shortcut(mut self, shortcut: Option<&'static str>) -> Self {
        self.shortcut = shortcut;
        self
    }
}

impl Palette {
    /// A palette titled `title` over `items`, opening on the first row with every
    /// group expanded — the shape nearly every palette opens with.
    pub(crate) fn new(title: &'static str, items: Vec<PaletteItem>) -> Self {
        Self {
            title,
            items,
            selected: 0,
            collapsed: HashSet::default(),
        }
    }

    /// Open on visible row `selected` instead of the first (e.g. the active book).
    pub(crate) const fn with_selected(mut self, selected: usize) -> Self {
        self.selected = selected;
        self
    }

    /// Open with these group headings already collapsed (#943).
    pub(crate) fn with_collapsed(mut self, collapsed: HashSet<&'static str>) -> Self {
        self.collapsed = collapsed;
        self
    }

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
