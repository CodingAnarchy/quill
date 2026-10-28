//! Loading the terminal color theme from `[theme]` (#300). Apart from the rest
//! of `config` because it builds the editor's ratatui [`Theme`] (#1229).

use std::collections::HashMap;

use crate::theme::Theme;

use super::load_section;

/// Load the color theme from the `[theme]` section (#300): an optional `name`
/// (`dark` / `light`) plus any per-field color overrides, all string values.
/// Returns the theme and any warnings (bad name / field / color) for the caller
/// to surface. A missing section (or a malformed one) yields the default theme.
pub fn load_theme() -> (Theme, Vec<String>) {
    // The whole `[theme]` table as string keys; `name` is pulled out, the rest
    // are color-field overrides. Untyped (string values) so an unknown key is a
    // reported warning, not a parse failure.
    let mut table: HashMap<String, String> = load_section("theme");
    let name = table.remove("name");
    Theme::from_config(name.as_deref(), &table)
}
