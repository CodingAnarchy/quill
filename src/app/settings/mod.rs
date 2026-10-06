//! The plain, offline "Settings…" menu (#592): set the everyday `[editor]` /
//! `[theme]` knobs — theme, column width, smart typography, Vim mode, autosave
//! interval, typewriter / focus defaults — and the `[analysis]` stopwords, all
//! from a value-showing picker menu, no AI (and no hand-editing config) required.
//!
//! It mirrors the export-style setter (#569): each row shows its current value and
//! opens a small picker (theme name, on/off toggles) or a text prompt (column
//! width, autosave secs, stopwords). The `[editor]` / `[theme]` knobs are the same
//! write targets the AI-gated "Configure by description" already uses — this reuses
//! that machinery directly: [`validate_change`] checks the value, `config::set_values`
//! splices it into the home config file, and [`apply_config_live`](App::apply_config_live)
//! applies it to the running editor with no restart. The `[analysis]` stopwords
//! live in the project's `book.toml`, so they splice there (like the goals / style
//! setters), standalone novels only.

use super::App;
use crate::app::configure::validate_change;
use crate::app::dispatch::vim_remap_actions;
use crate::config;
use crate::mode::{
    Command, Mode, Palette, PaletteItem, Pending, SettingBool, SettingPicker, SettingTextKnob,
};
use crate::project::{read_existing, Project};
use crate::theme::{Theme, COLOR_FIELDS, THEME_NAMES};
use ratatui::style::Color;

/// Guidance shown when an author tries to set analysis stopwords on a multi-book
/// series (#1105). A series manifest carries no per-book `[analysis]`, so the
/// stopwords can't apply to the active book; this points at the path that does
/// work (a standalone novel's own project) instead of a hand-edit that wouldn't
/// take effect.
const SERIES_STOPWORDS_GUIDANCE: &str =
    "In-app stopwords apply to a standalone novel — a multi-book series doesn't \
     carry per-book [analysis] yet. Manage the book as its own project to set its \
     stopwords.";

impl App {
    /// Open the plain Settings menu (`settings`, #592): one row per editable knob,
    /// each showing its current value and opening a picker / prompt. Enter on a row
    /// sets that knob; the menu re-opens after a change so several can be tweaked in
    /// one sitting.
    pub(crate) fn open_settings_menu(&mut self) {
        let items = vec![
            PaletteItem::new(
                "Theme",
                format!("Currently: {}", Self::current_theme_name()),
                Command::OpenSettingPicker(SettingPicker::Theme),
            ),
            PaletteItem::new(
                "Column width",
                format!("Currently: {} characters", self.focus_width),
                Command::EditSettingText(SettingTextKnob::ColumnWidth),
            ),
            bool_row(SettingBool::SmartTypography, self.smart_typography),
            bool_row(SettingBool::AutoPair, self.auto_pair),
            bool_row(SettingBool::Vim, self.vim),
            PaletteItem::new(
                "Autosave interval",
                format!("Currently: {}", autosave_label(self.autosave_interval_secs)),
                Command::EditSettingText(SettingTextKnob::AutosaveSecs),
            ),
            bool_row(SettingBool::Typewriter, self.typewriter),
            bool_row(SettingBool::FocusMode, self.focus_mode),
            bool_row(SettingBool::ParagraphIndent, self.paragraph_indent),
            bool_row(SettingBool::AnnotationMargin, self.annotation_margin),
            PaletteItem::new(
                "Analysis stopwords",
                format!("Currently: {}", self.stopwords_label()),
                Command::EditSettingText(SettingTextKnob::Stopwords),
            ),
            PaletteItem::new(
                "Theme colors…",
                "Override individual [theme] colors, applied live",
                Command::OpenSettingPicker(SettingPicker::ThemeColors),
            ),
            PaletteItem::new(
                "Key bindings…",
                "View and remap the Vim [editor.keys] bindings",
                Command::OpenSettingPicker(SettingPicker::KeyBindings),
            ),
        ];
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Settings",
            items,
            selected: 0,
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// Open the Settings sub-picker the menu row chose (#592).
    pub(crate) fn open_setting_picker(&mut self, picker: SettingPicker) {
        match picker {
            SettingPicker::Theme => self.open_setting_theme_picker(),
            SettingPicker::Bool(knob) => self.open_setting_bool_picker(knob),
            SettingPicker::ThemeColors => self.open_setting_theme_colors(),
            SettingPicker::KeyBindings => self.open_setting_key_bindings(),
        }
    }

    /// Open the theme picker (#592): the built-in themes, the active one noted.
    fn open_setting_theme_picker(&mut self) {
        let current = Self::current_theme_name();
        let items = THEME_NAMES
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let here = if *name == current { " (current)" } else { "" };
                PaletteItem::new(
                    *name,
                    format!("Base color theme{here}"),
                    Command::SetSettingTheme(i),
                )
            })
            .collect();
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Theme",
            items,
            selected: 0,
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// Open the on/off picker for a boolean `[editor]` knob (#592).
    fn open_setting_bool_picker(&mut self, knob: SettingBool) {
        let items = vec![
            PaletteItem::new("On", "", Command::SetSettingBool(knob, true)),
            PaletteItem::new("Off", "", Command::SetSettingBool(knob, false)),
        ];
        self.mode = Mode::Palette(Box::new(Palette {
            title: knob.label(),
            items,
            selected: 0,
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// Open the theme-color editor (#1109): one row per overridable `[theme]`
    /// color field, each showing its current value (the author's override, or the
    /// base theme's value) and opening a text prompt to set / clear it.
    fn open_setting_theme_colors(&mut self) {
        let overrides = config::theme_color_overrides();
        let items = COLOR_FIELDS
            .iter()
            .map(|&field| {
                PaletteItem::new(
                    field,
                    format!(
                        "Currently: {} — {}",
                        color_value_label(&overrides, field, &self.theme),
                        crate::theme::color_role(field)
                    ),
                    Command::EditSettingText(SettingTextKnob::ThemeColor(field)),
                )
            })
            .collect();
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Theme colors",
            items,
            selected: 0,
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// Open the key-bindings editor (#1109): one row per remappable Vim action,
    /// each showing its current key (the author's remap, or the built-in default)
    /// and opening a prompt to remap it. Satisfies the "view the bindings"
    /// acceptance directly, and each row is a live remap.
    fn open_setting_key_bindings(&mut self) {
        let remaps = config::EditorConfig::load().keys;
        let items = vim_remap_actions()
            .iter()
            .map(|&(action, default_key)| {
                let current = remaps.get(action).cloned();
                let shown = current.clone().unwrap_or_else(|| default_key.to_string());
                let origin = if current.is_some() {
                    "remapped"
                } else {
                    "default"
                };
                PaletteItem::new(
                    action,
                    format!("Currently: {shown} ({origin})"),
                    Command::EditSettingText(SettingTextKnob::KeyRemap(action)),
                )
            })
            .collect();
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Key bindings",
            items,
            selected: 0,
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// Set a boolean `[editor]` knob (#592): splice `true` / `false` through the
    /// shared config machinery, apply it live, and re-open the menu. A bool always
    /// validates, so there's no rejection path.
    pub(crate) fn set_setting_bool(&mut self, knob: SettingBool, on: bool) {
        let field = knob
            .key()
            .strip_prefix("editor.")
            .unwrap_or_else(|| knob.key());
        // A bool always validates, so the raw `true`/`false` is the fallback the
        // validator would return anyway — no panic path needed.
        let raw = if on { "true" } else { "false" };
        let literal = validate_change(knob.key(), raw).unwrap_or_else(|_| raw.to_string());
        self.persist_config_setting(
            "editor",
            field,
            &literal,
            &format!("{} {}.", knob.label(), on_off(on)),
        );
    }

    /// Set the `[theme] name` (#592): `index` indexes [`THEME_NAMES`]. Spliced +
    /// applied live through the shared config machinery.
    pub(crate) fn set_setting_theme(&mut self, index: usize) {
        let Some(name) = THEME_NAMES.get(index).copied() else {
            self.mode = self.editing_mode();
            return;
        };
        match validate_change("theme.name", name) {
            Ok(literal) => self.persist_config_setting(
                "theme",
                "name",
                &literal,
                &format!("Theme set to {name}."),
            ),
            Err(why) => {
                self.message = Some(format!("Couldn't set theme: {why}"));
                self.mode = self.editing_mode();
            }
        }
    }

    /// Open the text prompt for a numeric / list Settings knob (#592), prefilled
    /// with its current value. Enter validates + saves it.
    pub(crate) fn prompt_setting_text(&mut self, knob: SettingTextKnob) {
        let (current, hint) = match knob {
            SettingTextKnob::ColumnWidth => (self.focus_width.to_string(), "32–160".to_string()),
            SettingTextKnob::AutosaveSecs => (
                self.autosave_interval_secs.to_string(),
                "seconds; 0 turns it off".to_string(),
            ),
            SettingTextKnob::Stopwords => (
                self.current_stopwords().join(" "),
                "space-separated words; blank clears them".to_string(),
            ),
            SettingTextKnob::ThemeColor(field) => (
                config::theme_color_overrides()
                    .get(field)
                    .cloned()
                    .unwrap_or_default(),
                "ANSI name, #hex, r,g,b, or 0–255; blank resets".to_string(),
            ),
            SettingTextKnob::KeyRemap(action) => (
                config::EditorConfig::load()
                    .keys
                    .get(action)
                    .cloned()
                    .unwrap_or_default(),
                "one key; blank resets to the default".to_string(),
            ),
        };
        let label = format!("{} ({hint}): ", knob.label());
        self.open_prompt(Pending::SetSettingText(knob), label, &current, None);
    }

    /// Apply a numeric / list Settings prompt's input (#592). The `[editor]` knobs
    /// validate through the shared vocabulary and splice into the home config; the
    /// stopwords splice into `book.toml`'s `[analysis]`. On success the menu
    /// re-opens; on a bad value / failed write the message reports it.
    pub(crate) fn apply_setting_text(&mut self, knob: SettingTextKnob, text: &str) {
        match knob {
            SettingTextKnob::ColumnWidth => {
                self.apply_validated_editor(knob, "editor.column_width", "column_width", text);
            }
            SettingTextKnob::AutosaveSecs => self.apply_validated_editor(
                knob,
                "editor.autosave_interval_secs",
                "autosave_interval_secs",
                text,
            ),
            SettingTextKnob::Stopwords => self.apply_stopwords(text),
            SettingTextKnob::ThemeColor(field) => self.apply_theme_color(field, text),
            SettingTextKnob::KeyRemap(action) => self.apply_key_remap(action, text),
        }
    }

    /// Apply a `[theme]` color override (#1109): a blank entry strips the field
    /// (resetting it to the base theme's value), else the value is validated as a
    /// color and spliced into `[theme]`. Applied live through the shared config
    /// path (the theme reloads and repaints), then the color menu re-opens so
    /// several can be tweaked in one sitting. A bad color re-reports and re-prompts.
    fn apply_theme_color(&mut self, field: &'static str, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            match strip_config_field("theme", field) {
                Ok(()) => {
                    self.apply_config_live();
                    self.message = Some(format!("Reset {field} to the theme default."));
                    self.open_setting_theme_colors();
                }
                Err(e) => {
                    self.message = Some(format!("Couldn't reset {field}: {e}"));
                    self.mode = self.editing_mode();
                }
            }
            return;
        }
        match validate_change(&format!("theme.{field}"), text) {
            Ok(literal) => {
                self.persist_config_setting(
                    "theme",
                    field,
                    &literal,
                    &format!("{field} set to {text}."),
                );
                // persist_config_setting re-opens the flat Settings menu; re-open the
                // color submenu instead so the author stays in the color editor.
                if matches!(&self.mode, Mode::Palette(_)) {
                    self.open_setting_theme_colors();
                }
            }
            Err(why) => {
                self.message = Some(format!("Theme color {why}"));
                self.prompt_setting_text(SettingTextKnob::ThemeColor(field));
            }
        }
    }

    /// Apply a `[editor.keys]` remap (#1109): a blank entry strips the remap
    /// (resetting the action to its default key), else the value is validated as a
    /// single key and spliced into `[editor.keys]`. Applied live (the keymap
    /// rebuilds through `apply_config_live`), then the key-binding menu re-opens.
    /// A bad key re-reports and re-prompts.
    fn apply_key_remap(&mut self, action: &'static str, text: &str) {
        let text = text.trim();
        if text.is_empty() {
            match strip_config_field("editor.keys", action) {
                Ok(()) => {
                    self.apply_config_live();
                    self.message = Some(format!("Reset {action} to its default key."));
                    self.open_setting_key_bindings();
                }
                Err(e) => {
                    self.message = Some(format!("Couldn't reset {action}: {e}"));
                    self.mode = self.editing_mode();
                }
            }
            return;
        }
        if text.chars().count() != 1 {
            self.message = Some(format!("{action}: bind to a single key, got '{text}'"));
            self.prompt_setting_text(SettingTextKnob::KeyRemap(action));
            return;
        }
        let literal = toml::Value::String(text.to_string()).to_string();
        self.persist_config_setting(
            "editor.keys",
            action,
            &literal,
            &format!("Remapped {action} to '{text}'."),
        );
        if matches!(&self.mode, Mode::Palette(_)) {
            self.open_setting_key_bindings();
        }
    }

    /// Validate a numeric `[editor]` value through the shared vocabulary, then
    /// persist + apply it live. On a rejection, re-report and re-open the same
    /// prompt so the author can correct it (mirrors the goals setter). `vocab_key`
    /// is the dotted key the validator matches; `field` is the bare `[editor]` key.
    fn apply_validated_editor(
        &mut self,
        knob: SettingTextKnob,
        vocab_key: &str,
        field: &str,
        text: &str,
    ) {
        match validate_change(vocab_key, text.trim()) {
            Ok(literal) => self.persist_config_setting(
                "editor",
                field,
                &literal,
                &format!("{} set to {}.", capitalize(knob.label()), text.trim()),
            ),
            Err(why) => {
                self.message = Some(format!("{}: {why}", capitalize(knob.label())));
                self.prompt_setting_text(knob);
            }
        }
    }

    /// The shared home-config writer for the `[editor]` / `[theme]` knobs (#592):
    /// splice `section.field = literal` via [`config::set_values`], apply the config
    /// live (so the change shows with no restart), and re-open the Settings menu.
    fn persist_config_setting(&mut self, section: &str, field: &str, literal: &str, note: &str) {
        let changes = [(section.to_string(), field.to_string(), literal.to_string())];
        match config::set_values(&changes) {
            Ok(_) => {
                self.apply_config_live();
                self.message = Some(note.to_string());
                self.open_settings_menu();
            }
            Err(e) => {
                self.message = Some(format!("Couldn't write config: {e}"));
                self.mode = self.editing_mode();
            }
        }
    }

    /// Apply the analysis-stopwords prompt (#592): the whitespace-separated words
    /// replace `[analysis] stopwords_add` in `book.toml` (a blank entry clears the
    /// key), updating the in-memory manifest so the next analysis report reflects
    /// it. Standalone novels only, like the goals / style setters.
    fn apply_stopwords(&mut self, text: &str) {
        // A multi-book series carries no per-book [analysis] (#1105): show the clean
        // guidance up front rather than reporting the guard as a write failure.
        if self.project.book_count() > 1 {
            self.message = Some(SERIES_STOPWORDS_GUIDANCE.to_string());
            self.mode = self.editing_mode();
            return;
        }
        let words: Vec<String> = text.split_whitespace().map(str::to_string).collect();
        match self.splice_stopwords(&words) {
            Ok(()) => {
                self.message = Some(if words.is_empty() {
                    "Cleared the analysis stopwords.".into()
                } else {
                    format!("Set {} analysis stopword(s).", words.len())
                });
                self.open_settings_menu();
            }
            Err(e) => {
                self.message = Some(format!("Couldn't update [analysis]: {e}"));
                self.mode = self.editing_mode();
            }
        }
    }

    /// Splice `[analysis] stopwords_add` into `book.toml` (#592): update the
    /// in-memory manifest, then write a targeted splice (or strip, for an empty
    /// list) preserving the rest of the manifest. Standalone novels only — a
    /// multi-book series carries no per-book `[analysis]`, so `apply_stopwords`
    /// guides the author to a standalone project before ever reaching here; this
    /// guard is defense-in-depth (#1105).
    fn splice_stopwords(&mut self, words: &[String]) -> std::io::Result<()> {
        if self.project.book_count() > 1 {
            return Err(std::io::Error::other(SERIES_STOPWORDS_GUIDANCE));
        }
        let analysis = self
            .project
            .manifest
            .analysis
            .get_or_insert_with(Default::default);
        analysis.stopwords_add = words.to_vec();

        let path = Project::manifest_path(&self.project.root);
        // A failed read must abort: splicing onto an empty base would overwrite the
        // whole manifest while reporting success (#565).
        let text = read_existing(&path)?;
        let updated = if words.is_empty() {
            config::strip_value(&text, "analysis", "stopwords_add").0
        } else {
            let array: Vec<toml::Value> = words.iter().cloned().map(toml::Value::String).collect();
            let literal = toml::Value::Array(array).to_string();
            config::set_value(&text, "analysis", "stopwords_add", &literal)
        };
        crate::atomic::write(&path, updated.as_bytes())
    }

    /// The active theme name for the menu / picker — the configured `[theme] name`,
    /// or the default (`dark`) when unset.
    fn current_theme_name() -> String {
        config::theme_name().unwrap_or_else(|| "dark".to_string())
    }

    /// The current `[analysis] stopwords_add` words from the manifest.
    fn current_stopwords(&self) -> Vec<String> {
        self.project
            .manifest
            .analysis
            .as_ref()
            .map(|a| a.stopwords_add.clone())
            .unwrap_or_default()
    }

    /// A one-line label for the stopwords menu row.
    fn stopwords_label(&self) -> String {
        let words = self.current_stopwords();
        if words.is_empty() {
            "(none added)".to_string()
        } else {
            words.join(", ")
        }
    }
}

/// The value to show for a `[theme]` color row (#1109): the author's raw override
/// string if the field is overridden, else the base theme's color for that field
/// (marked "(default)"), so the menu shows what's actually in effect.
fn color_value_label(
    overrides: &std::collections::HashMap<String, String>,
    field: &str,
    theme: &Theme,
) -> String {
    overrides.get(field).map_or_else(
        || format!("{} (default)", describe_color(theme.field_color(field))),
        String::clone,
    )
}

/// Strip `section.field` from the home config, preserving the rest (#1109): the
/// reset path for a theme-color override or a key remap. A missing file / section
/// / key is a no-op success (nothing to reset). Follows the splice/atomic-write
/// convention (`config::strip_value` + `atomic::write`) the other config edits use.
fn strip_config_field(section: &str, field: &str) -> std::io::Result<()> {
    let Some(path) = config::config_path() else {
        return Ok(());
    };
    let text = match std::fs::read_to_string(&path) {
        Ok(t) => t,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };
    let (updated, removed) = config::strip_value(&text, section, field);
    if removed {
        crate::atomic::write(&path, updated.as_bytes())?;
    }
    Ok(())
}

/// A short, config-like description of a `Color` for the theme-color menu (#1109):
/// `#rrggbb` for an RGB color, the palette index for an indexed one, else the
/// ANSI name lowercased (`Cyan` → `cyan`) so it reads like a value you could type.
fn describe_color(color: Color) -> String {
    match color {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(i) => i.to_string(),
        other => format!("{other:?}").to_ascii_lowercase(),
    }
}

/// A menu row for a boolean `[editor]` knob, showing its current on/off state.
fn bool_row(knob: SettingBool, on: bool) -> PaletteItem {
    PaletteItem::new(
        knob.label(),
        format!("Currently: {}", on_off(on)),
        Command::OpenSettingPicker(SettingPicker::Bool(knob)),
    )
}

/// "on" / "off" for a toggle's label and status message.
const fn on_off(on: bool) -> &'static str {
    if on {
        "on"
    } else {
        "off"
    }
}

/// The autosave-interval label: "off" for 0, else "N s".
fn autosave_label(secs: u64) -> String {
    if secs == 0 {
        "off".to_string()
    } else {
        format!("{secs} s")
    }
}

/// Capitalize the first character of `s` — for a status / rejection message that
/// leads with a knob label ("Column width: expected a width 32–160").
fn capitalize(s: &str) -> String {
    let mut chars = s.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}

#[cfg(test)]
mod tests;
