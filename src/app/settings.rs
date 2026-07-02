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
use crate::config;
use crate::mode::{
    Command, Mode, Palette, PaletteItem, Pending, SettingBool, SettingPicker, SettingTextKnob,
};
use crate::project::{read_existing, Project};
use crate::theme::THEME_NAMES;

impl App {
    /// Open the plain Settings menu (`settings`, #592): one row per editable knob,
    /// each showing its current value and opening a picker / prompt. Enter on a row
    /// sets that knob; the menu re-opens after a change so several can be tweaked in
    /// one sitting.
    pub(crate) fn open_settings_menu(&mut self) {
        let items = vec![
            PaletteItem::new(
                "Theme",
                format!("Currently: {}", self.current_theme_name()),
                Command::OpenSettingPicker(SettingPicker::Theme),
            ),
            PaletteItem::new(
                "Column width",
                format!("Currently: {} characters", self.focus_width),
                Command::EditSettingText(SettingTextKnob::ColumnWidth),
            ),
            bool_row(SettingBool::SmartTypography, self.smart_typography),
            bool_row(SettingBool::Vim, self.vim),
            PaletteItem::new(
                "Autosave interval",
                format!("Currently: {}", autosave_label(self.autosave_interval_secs)),
                Command::EditSettingText(SettingTextKnob::AutosaveSecs),
            ),
            bool_row(SettingBool::Typewriter, self.typewriter),
            bool_row(SettingBool::FocusMode, self.focus_mode),
            bool_row(SettingBool::ParagraphIndent, self.paragraph_indent),
            PaletteItem::new(
                "Analysis stopwords",
                format!("Currently: {}", self.stopwords_label()),
                Command::EditSettingText(SettingTextKnob::Stopwords),
            ),
        ];
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Settings",
            items,
            selected: 0,
            collapsed: Default::default(),
        }));
    }

    /// Open the Settings sub-picker the menu row chose (#592).
    pub(crate) fn open_setting_picker(&mut self, picker: SettingPicker) {
        match picker {
            SettingPicker::Theme => self.open_setting_theme_picker(),
            SettingPicker::Bool(knob) => self.open_setting_bool_picker(knob),
        }
    }

    /// Open the theme picker (#592): the built-in themes, the active one noted.
    fn open_setting_theme_picker(&mut self) {
        let current = self.current_theme_name();
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
            collapsed: Default::default(),
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
            collapsed: Default::default(),
        }));
    }

    /// Set a boolean `[editor]` knob (#592): splice `true` / `false` through the
    /// shared config machinery, apply it live, and re-open the menu. A bool always
    /// validates, so there's no rejection path.
    pub(crate) fn set_setting_bool(&mut self, knob: SettingBool, on: bool) {
        let field = knob.key().strip_prefix("editor.").unwrap_or(knob.key());
        let literal = validate_change(knob.key(), if on { "true" } else { "false" })
            .expect("true/false always validates");
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
            SettingTextKnob::ColumnWidth => (self.focus_width.to_string(), "32–160"),
            SettingTextKnob::AutosaveSecs => (
                self.autosave_interval_secs.to_string(),
                "seconds; 0 turns it off",
            ),
            SettingTextKnob::Stopwords => (
                self.current_stopwords().join(" "),
                "space-separated words; blank clears them",
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
                self.apply_validated_editor(knob, "editor.column_width", "column_width", text)
            }
            SettingTextKnob::AutosaveSecs => self.apply_validated_editor(
                knob,
                "editor.autosave_interval_secs",
                "autosave_interval_secs",
                text,
            ),
            SettingTextKnob::Stopwords => self.apply_stopwords(text),
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
    /// multi-book series has no single top-level `[analysis]`.
    fn splice_stopwords(&mut self, words: &[String]) -> std::io::Result<()> {
        if self.project.book_count() > 1 {
            return Err(std::io::Error::other(
                "setting series stopwords isn't supported yet — set [analysis] per book by hand",
            ));
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
    fn current_theme_name(&self) -> String {
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

/// A menu row for a boolean `[editor]` knob, showing its current on/off state.
fn bool_row(knob: SettingBool, on: bool) -> PaletteItem {
    PaletteItem::new(
        knob.label(),
        format!("Currently: {}", on_off(on)),
        Command::OpenSettingPicker(SettingPicker::Bool(knob)),
    )
}

/// "on" / "off" for a toggle's label and status message.
fn on_off(on: bool) -> &'static str {
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
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{app_scratch_dir, open_test_app, TempProject};
    use crate::mode::Action;

    /// A one-chapter standalone project on disk, with the config dir pointed at a
    /// fresh scratch dir (#528) so the `[editor]`/`[theme]` writes are hermetic.
    fn settings_app(name: &str) -> (App, TempProject) {
        let dir = app_scratch_dir(name);
        std::fs::write(
            Project::manifest_path(&dir),
            "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("one.md"), "first chapter\n").unwrap();
        // Point config resolution at a per-test scratch dir so config writes don't
        // touch the developer's real ~/.config (#528).
        config::set_config_dir(Some(app_scratch_dir(&format!("{name}-cfg"))));
        let app = open_test_app(&dir);
        (app, TempProject::new(dir))
    }

    fn manifest_text(dir: &std::path::Path) -> String {
        std::fs::read_to_string(Project::manifest_path(dir)).unwrap()
    }

    fn config_text() -> String {
        std::fs::read_to_string(config::config_path().unwrap()).unwrap()
    }

    #[test]
    fn the_settings_action_is_registered() {
        assert!(
            Action::ALL.iter().any(|a| a.name() == "settings"),
            "settings is in the registry"
        );
        assert!(matches!(
            Action::from_name("settings"),
            Some(Action::Settings)
        ));
        assert_eq!(Action::Settings.group(), "View");
    }

    #[test]
    fn the_menu_lists_every_knob() {
        let (mut app, _dir) = settings_app("settings-menu");
        app.open_settings_menu();
        let Mode::Palette(p) = &app.mode else {
            panic!("the setter opens a palette");
        };
        // Theme, width, smart typography, vim, autosave, typewriter, focus,
        // paragraph indent, stopwords.
        assert_eq!(p.items.len(), 9);
        assert_eq!(p.title, "Settings");
    }

    #[test]
    fn setting_a_theme_persists_and_applies_live() {
        let (mut app, _dir) = settings_app("settings-theme");
        // "light" is index 1 in THEME_NAMES.
        app.set_setting_theme(1);
        // Persisted to [theme] and round-trips through the loader.
        let toml = config_text();
        assert!(toml.contains("[theme]"), "{toml}");
        assert!(toml.contains("name = \"light\""), "{toml}");
        assert_eq!(config::theme_name().as_deref(), Some("light"));
        // Applied live: the running editor's theme is the light one.
        assert_eq!(app.theme, crate::theme::Theme::light());
    }

    #[test]
    fn setting_a_bool_persists_and_applies_live() {
        let (mut app, _dir) = settings_app("settings-bool");
        assert!(!app.vim);
        app.set_setting_bool(SettingBool::Vim, true);
        assert!(config_text().contains("vim = true"), "{}", config_text());
        // Applied live via apply_config_live.
        assert!(app.vim, "vim toggled on live");
        // And it round-trips through the loader.
        assert!(config::EditorConfig::load().vim);
    }

    #[test]
    fn setting_the_column_width_persists_and_applies_live() {
        let (mut app, _dir) = settings_app("settings-width");
        app.apply_setting_text(SettingTextKnob::ColumnWidth, "64");
        assert!(
            config_text().contains("column_width = 64"),
            "{}",
            config_text()
        );
        assert_eq!(app.focus_width, 64, "applied live");
        assert_eq!(config::EditorConfig::load().column_width, 64);
    }

    #[test]
    fn a_configured_column_width_applies_at_startup() {
        // Regression for #742: `App::new` must read `[editor] column_width` from
        // the loaded config, not seed a hardcoded default. Point config resolution
        // at a scratch dir (#528) and write the width *before* opening the app.
        let dir = app_scratch_dir("startup-width");
        std::fs::write(
            Project::manifest_path(&dir),
            "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("one.md"), "first chapter\n").unwrap();
        let cfg_dir = app_scratch_dir("startup-width-cfg");
        config::set_config_dir(Some(cfg_dir));
        std::fs::write(
            config::config_path().unwrap(),
            "[editor]\ncolumn_width = 96\n",
        )
        .unwrap();
        let app = open_test_app(&dir);
        assert_eq!(
            app.focus_width, 96,
            "startup width reflects the configured column_width, not the default"
        );
    }

    #[test]
    fn an_invalid_column_width_is_rejected_and_nothing_written() {
        let (mut app, _dir) = settings_app("settings-width-bad");
        // Out of the 32–160 range — rejected by the shared validator.
        app.apply_setting_text(SettingTextKnob::ColumnWidth, "9000");
        assert_eq!(app.focus_width, 72, "unchanged");
        // No config file written (or, if written, no column_width key).
        let cfg = config::config_path().and_then(|p| std::fs::read_to_string(p).ok());
        assert!(
            cfg.is_none_or(|t| !t.contains("column_width")),
            "nothing spliced"
        );
        // The prompt re-opens with the rejection reported.
        assert!(matches!(&app.mode, Mode::Prompt(_)), "re-prompts");
        assert!(
            app.message.as_deref().unwrap_or("").contains("32–160"),
            "{:?}",
            app.message
        );
    }

    #[test]
    fn an_invalid_autosave_is_rejected() {
        let (mut app, _dir) = settings_app("settings-autosave-bad");
        app.apply_setting_text(SettingTextKnob::AutosaveSecs, "nonsense");
        assert_eq!(app.autosave_interval_secs, 0, "unchanged");
        assert!(matches!(&app.mode, Mode::Prompt(_)), "re-prompts");
    }

    #[test]
    fn setting_autosave_persists_and_applies_live() {
        let (mut app, _dir) = settings_app("settings-autosave");
        app.apply_setting_text(SettingTextKnob::AutosaveSecs, "30");
        assert!(
            config_text().contains("autosave_interval_secs = 30"),
            "{}",
            config_text()
        );
        assert_eq!(app.autosave_interval_secs, 30, "applied live");
    }

    #[test]
    fn setting_stopwords_splices_the_analysis_table_and_round_trips() {
        let (mut app, dir) = settings_app("settings-stopwords");
        app.apply_setting_text(SettingTextKnob::Stopwords, "locket said");
        // In-memory manifest carries them immediately.
        assert_eq!(
            app.project
                .manifest
                .analysis
                .as_ref()
                .unwrap()
                .stopwords_add,
            vec!["locket".to_string(), "said".to_string()]
        );
        let toml = manifest_text(&dir);
        assert!(toml.contains("[analysis]"), "{toml}");
        assert!(
            toml.contains("stopwords_add = [\"locket\", \"said\"]"),
            "{toml}"
        );
        // Re-opening reads them back identically.
        let reopened = Project::open(&*dir).unwrap();
        assert_eq!(
            reopened.manifest.analysis.unwrap().stopwords_add,
            vec!["locket".to_string(), "said".to_string()]
        );
    }

    #[test]
    fn clearing_stopwords_strips_the_key() {
        let (mut app, dir) = settings_app("settings-stopwords-clear");
        app.apply_setting_text(SettingTextKnob::Stopwords, "locket");
        assert!(manifest_text(&dir).contains("stopwords_add"));
        app.apply_setting_text(SettingTextKnob::Stopwords, "");
        let toml = manifest_text(&dir);
        assert!(!toml.contains("stopwords_add"), "{toml}");
        assert!(app
            .project
            .manifest
            .analysis
            .as_ref()
            .unwrap()
            .stopwords_add
            .is_empty());
    }

    #[test]
    fn setting_a_knob_preserves_the_rest_of_the_config() {
        let (mut app, _dir) = settings_app("settings-preserve");
        // Seed a config file with another section + a comment to guard untouched.
        let path = config::config_path().unwrap();
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "# my settings\n[ai]\nenabled = true\n").unwrap();

        app.set_setting_bool(SettingBool::Vim, true);
        let toml = config_text();
        assert!(toml.contains("# my settings"), "comment kept: {toml}");
        assert!(
            toml.contains("[ai]\nenabled = true"),
            "section kept: {toml}"
        );
        assert!(toml.contains("vim = true"), "new key landed: {toml}");
    }
}
