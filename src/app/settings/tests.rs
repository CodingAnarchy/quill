//! Tests for the offline Settings menu (#592): that every knob is listed with its
//! current value, and that choosing one validates the input, persists it to the
//! right file (the home config for `[editor]` / `[theme]`, the project manifest
//! for `[analysis]` stopwords), and applies it to the running editor with no
//! restart. Covers the theme-color overrides and key remaps (#1109) — including
//! their reset-strips-the-key paths — and the series stopwords guidance (#1105).

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
    // Theme, width, smart typography, auto-pair, vim, autosave, typewriter,
    // focus, paragraph indent, annotation margin, daily goal cue, stopwords, theme
    // colors, key bindings.
    assert_eq!(p.items.len(), 14);
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

#[test]
fn overriding_a_theme_color_persists_applies_live_and_round_trips() {
    // #1109: an individual [theme] color set from the Settings menu splices into
    // the config, applies to the running palette live, and round-trips through
    // the loader.
    let (mut app, _dir) = settings_app("settings-theme-color");
    assert_eq!(app.theme.heading, crate::theme::Theme::dark().heading);
    app.apply_setting_text(SettingTextKnob::ThemeColor("heading"), "#ff8800");
    // Persisted under [theme], quoted so it round-trips.
    let toml = config_text();
    assert!(toml.contains("[theme]"), "{toml}");
    assert!(toml.contains("heading = \"#ff8800\""), "{toml}");
    // Applied live to the running theme.
    assert_eq!(
        app.theme.heading,
        ratatui::style::Color::Rgb(0xff, 0x88, 0x00),
        "override applied live"
    );
    // And it round-trips: reloading the theme reflects the override.
    let (reloaded, warnings) = config::load_theme();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        reloaded.heading,
        ratatui::style::Color::Rgb(0xff, 0x88, 0x00)
    );
}

#[test]
fn an_invalid_theme_color_is_rejected_and_re_prompts() {
    let (mut app, _dir) = settings_app("settings-theme-color-bad");
    app.apply_setting_text(SettingTextKnob::ThemeColor("heading"), "chartreuse");
    // Nothing written and the theme is unchanged.
    assert_eq!(app.theme.heading, crate::theme::Theme::dark().heading);
    let cfg = config::config_path().and_then(|p| std::fs::read_to_string(p).ok());
    assert!(
        cfg.is_none_or(|t| !t.contains("heading")),
        "nothing spliced"
    );
    // The prompt re-opens with the rejection reported.
    assert!(matches!(&app.mode, Mode::Prompt(_)), "re-prompts");
    assert!(
        app.message.as_deref().unwrap_or("").contains("color"),
        "{:?}",
        app.message
    );
}

#[test]
fn clearing_a_theme_color_override_strips_the_key_and_resets_live() {
    let (mut app, _dir) = settings_app("settings-theme-color-clear");
    app.apply_setting_text(SettingTextKnob::ThemeColor("heading"), "#ff8800");
    assert!(config_text().contains("heading = \"#ff8800\""));
    // A blank entry resets the field: the key is stripped and the live theme
    // returns to the base value.
    app.apply_setting_text(SettingTextKnob::ThemeColor("heading"), "");
    let toml = config_text();
    assert!(!toml.contains("heading ="), "override stripped: {toml}");
    assert_eq!(
        app.theme.heading,
        crate::theme::Theme::dark().heading,
        "reset to the base theme live"
    );
}

#[test]
fn the_theme_color_menu_lists_every_field_with_its_value() {
    let (mut app, _dir) = settings_app("settings-theme-color-menu");
    app.open_setting_theme_colors();
    let Mode::Palette(p) = &app.mode else {
        panic!("the theme-color editor opens a palette");
    };
    assert_eq!(p.title, "Theme colors");
    assert_eq!(
        p.items.len(),
        crate::theme::COLOR_FIELDS.len(),
        "a row per overridable color field"
    );
    // Each row is labeled by its field and shows its current value.
    assert!(p.items.iter().any(|i| i.label == "heading"));
    assert!(
        p.items.iter().all(|i| i.desc.contains("Currently:")),
        "every row shows its current value"
    );
}

#[test]
fn the_key_bindings_view_lists_the_current_bindings() {
    // #1109 (the "at minimum, VIEW them" acceptance): the key-bindings menu
    // lists every remappable action with its current key.
    let (mut app, _dir) = settings_app("settings-keys-view");
    app.open_setting_key_bindings();
    let Mode::Palette(p) = &app.mode else {
        panic!("the key-bindings editor opens a palette");
    };
    assert_eq!(p.title, "Key bindings");
    assert!(!p.items.is_empty(), "the bindings are listed");
    // The default 'j' → down binding is shown.
    let down = p
        .items
        .iter()
        .find(|i| i.label == "down")
        .expect("the down action is listed");
    assert!(
        down.desc.contains('j'),
        "shows the default key: {}",
        down.desc
    );
    assert!(down.desc.contains("default"), "{}", down.desc);
}

#[test]
fn remapping_a_key_persists_applies_live_and_round_trips() {
    // #1109 (full remap): binding an action to a new key splices into
    // [editor.keys], rebuilds the running keymap live, and round-trips.
    let (mut app, _dir) = settings_app("settings-keys-remap");
    assert!(app.vim_keys.is_empty(), "no remaps by default");
    app.apply_setting_text(SettingTextKnob::KeyRemap("down"), "n");
    // Persisted under [editor.keys], quoted.
    let toml = config_text();
    assert!(toml.contains("[editor.keys]"), "{toml}");
    assert!(toml.contains("down = \"n\""), "{toml}");
    // Applied live: pressing 'n' now runs the action bound to the default 'j'.
    assert_eq!(app.vim_keys.get(&'n'), Some(&'j'), "remap applied live");
    // Round-trips through the loader.
    assert_eq!(
        config::EditorConfig::load()
            .keys
            .get("down")
            .map(String::as_str),
        Some("n")
    );
}

#[test]
fn a_key_remap_coexists_with_scalar_editor_keys_and_round_trips() {
    // A remap writes the `[editor.keys]` sub-table; it must not collide with a
    // scalar `[editor]` key set earlier (e.g. Vim mode), and both must load.
    let (mut app, _dir) = settings_app("settings-keys-coexist");
    app.set_setting_bool(SettingBool::Vim, true);
    app.apply_setting_text(SettingTextKnob::KeyRemap("down"), "n");
    let toml = config_text();
    assert!(
        toml.contains("vim = true"),
        "scalar editor key kept: {toml}"
    );
    assert!(toml.contains("[editor.keys]"), "{toml}");
    // Both round-trip through the one loader.
    let editor = config::EditorConfig::load();
    assert!(editor.vim, "vim still loads");
    assert_eq!(editor.keys.get("down").map(String::as_str), Some("n"));
}

#[test]
fn clearing_a_key_remap_strips_the_key_and_resets_live() {
    let (mut app, _dir) = settings_app("settings-keys-reset");
    app.apply_setting_text(SettingTextKnob::KeyRemap("down"), "n");
    assert_eq!(app.vim_keys.get(&'n'), Some(&'j'));
    // Blank resets to the default: the key is stripped and the live keymap clears.
    app.apply_setting_text(SettingTextKnob::KeyRemap("down"), "");
    let toml = config_text();
    assert!(!toml.contains("down ="), "remap stripped: {toml}");
    assert!(app.vim_keys.is_empty(), "keymap reset to defaults live");
}

#[test]
fn a_multi_character_key_remap_is_rejected_and_re_prompts() {
    let (mut app, _dir) = settings_app("settings-keys-bad");
    app.apply_setting_text(SettingTextKnob::KeyRemap("down"), "nn");
    assert!(app.vim_keys.is_empty(), "nothing bound");
    let cfg = config::config_path().and_then(|p| std::fs::read_to_string(p).ok());
    assert!(
        cfg.is_none_or(|t| !t.contains("[editor.keys]")),
        "nothing spliced"
    );
    assert!(matches!(&app.mode, Mode::Prompt(_)), "re-prompts");
    assert!(
        app.message.as_deref().unwrap_or("").contains("single key"),
        "{:?}",
        app.message
    );
}

/// A multi-book series project on disk, for the series-guidance test.
fn series_settings_app(name: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    std::fs::create_dir_all(dir.join("b1")).unwrap();
    std::fs::create_dir_all(dir.join("b2")).unwrap();
    std::fs::write(dir.join("b1/one.md"), "alpha\n").unwrap();
    std::fs::write(dir.join("b2/one.md"), "beta\n").unwrap();
    std::fs::write(
        Project::manifest_path(&dir),
        "title = \"The Saga\"\n\n\
         [[books]]\ntitle = \"Book One\"\n\n\
         [[books.chapters]]\ntitle = \"Dawn\"\nfile = \"b1/one.md\"\n\n\
         [[books]]\ntitle = \"Book Two\"\n\n\
         [[books.chapters]]\ntitle = \"Dusk\"\nfile = \"b2/one.md\"\n",
    )
    .unwrap();
    config::set_config_dir(Some(app_scratch_dir(&format!("{name}-cfg"))));
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

#[test]
fn series_stopwords_guide_to_a_standalone_and_leave_the_manifest_untouched() {
    // A multi-book series carries no per-book [analysis] (#1105): setting
    // stopwords must guide to a standalone project rather than reading as
    // "unfinished" or corrupting the series manifest.
    let (mut app, dir) = series_settings_app("settings-stopwords-series");
    let before = manifest_text(&dir);
    app.apply_setting_text(SettingTextKnob::Stopwords, "locket said");
    let msg = app.message.as_deref().unwrap_or("");
    assert!(msg.contains("standalone"), "{msg:?}");
    assert!(!msg.contains("isn't supported yet"), "{msg:?}");
    assert!(!msg.contains("per book by hand"), "{msg:?}");
    assert_eq!(manifest_text(&dir), before, "series manifest untouched");
}
