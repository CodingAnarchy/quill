//! The in-app export-style setter (#569): pick a built-in `[style]` preset and
//! the common knobs (scene-break glyph, drop caps, heading alignment, title/part
//! ornaments) and the print-interior `[print] trim` size — all from a menu, no
//! hand-editing of `book.toml`.
//!
//! It mirrors the font browser (#494) and the goals setter (#550): each choice is
//! a *targeted splice* into the `[style]` / `[print]` table via the shared
//! `config::set_value` / `strip_value` splicer, leaving the rest of the manifest
//! (other sections, comments, formatting) byte-for-byte. The in-memory manifest is
//! updated first, so the next export and "Preview style…" reflect the change with
//! no restart; the existing hand-edit / file-path config keeps round-tripping.
//!
//! Standalone novels only (#1105): a multi-book series does not carry per-book
//! `[style]` / `[print]` design — the on-disk `[[books]]` form has no style/print
//! table, and a top-level one is dropped when a series manifest loads — so the
//! in-app setter can't target the active book without new manifest fields. Rather
//! than pretend the feature is merely "unfinished" and point at a hand-edit that
//! wouldn't take effect, the series branch surfaces the path that actually works:
//! design lives in a standalone novel's `book.toml`, so manage the book as its own
//! project to set its style / print design.

use super::App;
use crate::config;
use crate::export::pdf::{self, PrintConfig};
use crate::export::style::{preset_blurb, HEADING_ALIGNS, PRESET_NAMES};
use crate::export::ExportStyle;
use crate::mode::{Command, Mode, Palette, PaletteItem, Pending, StylePicker, StyleTextKnob};
use crate::project::{read_existing, Project};

/// Guidance shown when an author tries to set `[style]` design on a multi-book
/// series (#1105). A series manifest carries no per-book `[style]` table, so the
/// setter can't apply to the active book; this points at the path that does work
/// (a standalone novel's own `book.toml`) instead of a hand-edit that wouldn't
/// take effect.
const SERIES_STYLE_GUIDANCE: &str =
    "In-app style design applies to a standalone novel — a multi-book series \
     doesn't carry per-book [style] yet. Manage the book as its own project to \
     set its style.";

/// Guidance shown when an author tries to set `[print]` trim on a multi-book
/// series (#1105); the print counterpart to [`SERIES_STYLE_GUIDANCE`].
const SERIES_PRINT_GUIDANCE: &str =
    "In-app print design applies to a standalone novel — a multi-book series \
     doesn't carry per-book [print] yet. Manage the book as its own project to \
     set its trim.";

impl App {
    /// Open the export-style setter menu (`style:set`, #569): one row per editable
    /// knob, each showing its current value and opening a picker / prompt. Enter on
    /// a row sets that knob; the menu re-opens after a change so several can be
    /// tweaked in one sitting.
    pub(crate) fn open_export_style_menu(&mut self) {
        let style = self.project.manifest.style.clone().unwrap_or_default();
        let trim = self.current_trim();
        let items = vec![
            PaletteItem::new(
                "Preset",
                format!("Currently: {}", opt_label(style.preset.as_deref())),
                Command::OpenStylePicker(StylePicker::Preset),
            ),
            PaletteItem::new(
                "Scene-break glyph",
                format!("Currently: {}", opt_label(style.scene_break.as_deref())),
                Command::EditStyleText(StyleTextKnob::SceneBreak),
            ),
            PaletteItem::new(
                "Heading alignment",
                format!("Currently: {}", opt_label(style.heading_align.as_deref())),
                Command::OpenStylePicker(StylePicker::Align),
            ),
            PaletteItem::new(
                "Drop caps",
                format!("Currently: {}", drop_cap_label(style.drop_cap)),
                Command::OpenStylePicker(StylePicker::DropCap),
            ),
            PaletteItem::new(
                "Title ornament",
                format!("Currently: {}", opt_label(style.title_ornament.as_deref())),
                Command::EditStyleText(StyleTextKnob::TitleOrnament),
            ),
            PaletteItem::new(
                "Part ornament",
                format!("Currently: {}", opt_label(style.part_ornament.as_deref())),
                Command::EditStyleText(StyleTextKnob::PartOrnament),
            ),
            PaletteItem::new(
                "Cover image",
                format!("Currently: {}", opt_label(style.cover_image.as_deref())),
                Command::EditStyleText(StyleTextKnob::CoverImage),
            ),
            PaletteItem::new(
                "Cover description (alt text)",
                format!("Currently: {}", opt_label(style.cover_image_alt.as_deref())),
                Command::EditStyleText(StyleTextKnob::CoverImageAlt),
            ),
            PaletteItem::new(
                "Print trim size",
                format!("Currently: {trim}"),
                Command::OpenStylePicker(StylePicker::Trim),
            ),
        ];
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Export style",
            items,
            selected: 0,
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// Open the export-style sub-picker the menu row chose (#569).
    pub(crate) fn open_style_picker(&mut self, picker: StylePicker) {
        match picker {
            StylePicker::Preset => self.open_style_preset_picker(),
            StylePicker::Align => self.open_style_align_picker(),
            StylePicker::DropCap => self.open_style_drop_cap_picker(),
            StylePicker::Trim => self.open_style_trim_picker(),
        }
    }

    /// Open the preset picker (#569): the built-in library, each row showing its
    /// look, plus a "(none)" row to clear the preset. Enter sets it and returns to
    /// the style menu.
    pub(crate) fn open_style_preset_picker(&mut self) {
        let mut items = vec![PaletteItem::new(
            "(none)",
            "Clear the preset — fall back to the plain default + any explicit keys",
            Command::SetStylePreset(None),
        )];
        items.extend(PRESET_NAMES.iter().enumerate().map(|(i, name)| {
            PaletteItem::new(*name, preset_blurb(name), Command::SetStylePreset(Some(i)))
        }));
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Style preset",
            items,
            selected: 0,
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// Open the heading-alignment picker (#569): the four allowed values plus a
    /// "(default)" row that clears the key (centered).
    pub(crate) fn open_style_align_picker(&mut self) {
        let mut items = vec![PaletteItem::new(
            "(default — centered)",
            "Clear heading_align",
            Command::SetStyleAlign(None),
        )];
        items.extend(
            HEADING_ALIGNS
                .iter()
                .enumerate()
                .map(|(i, a)| PaletteItem::new(*a, "", Command::SetStyleAlign(Some(i)))),
        );
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Heading alignment",
            items,
            selected: 0,
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// Open the drop-cap picker (#569): on, off, or "(default)" which clears the key.
    pub(crate) fn open_style_drop_cap_picker(&mut self) {
        let items = vec![
            PaletteItem::new(
                "On",
                "Open each chapter's first paragraph with a drop cap",
                Command::SetStyleDropCap(Some(true)),
            ),
            PaletteItem::new("Off", "No drop cap", Command::SetStyleDropCap(Some(false))),
            PaletteItem::new(
                "(default — off)",
                "Clear drop_cap so a preset's value shows through",
                Command::SetStyleDropCap(None),
            ),
        ];
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Drop caps",
            items,
            selected: 0,
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// Open the print-trim picker (#569): the supported trim sizes, the active one
    /// noted in its description.
    pub(crate) fn open_style_trim_picker(&mut self) {
        let current = self.current_trim();
        let items = pdf::TRIM_NAMES
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let here = if *name == current { " (current)" } else { "" };
                PaletteItem::new(
                    *name,
                    format!("Print interior trim{here}"),
                    Command::SetStyleTrim(i),
                )
            })
            .collect();
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Print trim size",
            items,
            selected: 0,
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// Open the text prompt for a free-text `[style]` knob (#569), prefilled with
    /// its current value (blank when unset). Enter saves it; a blank entry clears
    /// the key.
    pub(crate) fn prompt_style_text(&mut self, knob: StyleTextKnob) {
        let current = self.style_text_value(knob).unwrap_or_default();
        let label = format!("{} (blank clears it): ", knob.label());
        self.open_prompt(Pending::SetStyleText(knob), label, &current, None);
    }

    /// Apply a free-text knob prompt's input (#569): clear the key on blank, else
    /// splice it. On success the style menu re-opens; on a failed write the message
    /// reports it and the editor resumes.
    pub(crate) fn apply_style_text(&mut self, knob: StyleTextKnob, text: &str) {
        let text = text.trim();
        let value = (!text.is_empty()).then(|| text.to_string());
        match self.set_style_value(knob.key(), value.clone(), |style, v| {
            *match knob {
                StyleTextKnob::SceneBreak => &mut style.scene_break,
                StyleTextKnob::TitleOrnament => &mut style.title_ornament,
                StyleTextKnob::PartOrnament => &mut style.part_ornament,
                StyleTextKnob::CoverImage => &mut style.cover_image,
                StyleTextKnob::CoverImageAlt => &mut style.cover_image_alt,
            } = v;
        }) {
            Ok(()) => {
                self.message = Some(value.map_or_else(
                    || format!("Cleared {}.", knob.label()),
                    |v| format!("Set {} to {v}.", knob.label()),
                ));
                self.open_export_style_menu();
            }
            Err(e) => {
                self.message = Some(format!("Couldn't update [style]: {e}"));
                self.mode = self.editing_mode();
            }
        }
    }

    /// Set (or clear) the `[style]` preset (#569). `index` indexes [`PRESET_NAMES`];
    /// `None` clears it.
    pub(crate) fn set_style_preset(&mut self, index: Option<usize>) {
        let value = index
            .and_then(|i| PRESET_NAMES.get(i))
            .map(std::string::ToString::to_string);
        let outcome = self.set_style_value("preset", value.clone(), |style, v| style.preset = v);
        self.report_style_set(outcome, "preset", value.as_deref());
    }

    /// Set (or clear) `[style] heading_align` (#569). `index` indexes
    /// [`HEADING_ALIGNS`]; `None` clears it (centered).
    pub(crate) fn set_style_align(&mut self, index: Option<usize>) {
        let value = index
            .and_then(|i| HEADING_ALIGNS.get(i))
            .map(std::string::ToString::to_string);
        let outcome = self.set_style_value("heading_align", value.clone(), |style, v| {
            style.heading_align = v;
        });
        self.report_style_set(outcome, "heading alignment", value.as_deref());
    }

    /// Set (or clear) `[style] drop_cap` (#569). `Some` writes the bool; `None`
    /// clears the key (default off).
    pub(crate) fn set_style_drop_cap(&mut self, value: Option<bool>) {
        // A bool is a bare TOML literal (`true` / `false`), not a quoted string.
        let outcome = self.splice_style("drop_cap", value.map(|b| b.to_string()), |style| {
            style.drop_cap = value;
        });
        let label = value.map(|b| if b { "on" } else { "off" });
        self.report_style_set(outcome, "drop caps", label);
    }

    /// Set the `[print] trim` size (#569). `index` indexes [`pdf::TRIM_NAMES`].
    pub(crate) fn set_style_trim(&mut self, index: usize) {
        let Some(name) = pdf::TRIM_NAMES.get(index).copied() else {
            self.mode = self.editing_mode();
            return;
        };
        match self.splice_print_trim(name) {
            Ok(()) => {
                self.message = Some(format!("Print trim set to {name}."));
                self.open_export_style_menu();
            }
            Err(e) => {
                self.message = Some(format!("Couldn't update [print]: {e}"));
                self.mode = self.editing_mode();
            }
        }
    }

    /// Report the outcome of a string-valued `[style]` set and re-open the menu, or
    /// surface the write error.
    fn report_style_set(&mut self, outcome: std::io::Result<()>, label: &str, value: Option<&str>) {
        match outcome {
            Ok(()) => {
                self.message = Some(value.map_or_else(
                    || format!("Cleared {label}."),
                    |v| format!("Set {label} to {v}."),
                ));
                self.open_export_style_menu();
            }
            Err(e) => {
                self.message = Some(format!("Couldn't update [style]: {e}"));
                self.mode = self.editing_mode();
            }
        }
    }

    /// The current value of a free-text `[style]` knob, for prefilling its prompt.
    fn style_text_value(&self, knob: StyleTextKnob) -> Option<String> {
        let style = self.project.manifest.style.as_ref()?;
        match knob {
            StyleTextKnob::SceneBreak => style.scene_break.clone(),
            StyleTextKnob::TitleOrnament => style.title_ornament.clone(),
            StyleTextKnob::PartOrnament => style.part_ornament.clone(),
            StyleTextKnob::CoverImage => style.cover_image.clone(),
            StyleTextKnob::CoverImageAlt => style.cover_image_alt.clone(),
        }
    }

    /// The active print trim — the manifest `[print] trim`, or the default.
    fn current_trim(&self) -> String {
        self.project
            .manifest
            .print
            .as_ref()
            .and_then(|p| p.trim.clone())
            .unwrap_or_else(|| pdf::DEFAULT_TRIM.to_string())
    }

    /// Splice a string-valued `[style]` key (#569): update the in-memory style via
    /// `set_field` so a preview reflects it immediately, then persist a targeted
    /// splice into `[style]`. A `None` value clears the key. Convenience over
    /// [`Self::splice_style`] for the `Option<String>` fields — it TOML-quotes the
    /// value so a glyph / path can't break the syntax.
    fn set_style_value(
        &mut self,
        key: &str,
        value: Option<String>,
        set_field: impl FnOnce(&mut ExportStyle, Option<String>),
    ) -> std::io::Result<()> {
        let v = value.clone();
        let literal = value.map(|s| toml::Value::String(s).to_string());
        self.splice_style(key, literal, move |style| set_field(style, v))
    }

    /// The shared `[style]` splicer (#569): standalone novels only — a multi-book
    /// series carries no per-book `[style]` table, so it degrades to a clear
    /// [`SERIES_STYLE_GUIDANCE`] message (#1105). Updates the in-memory style via
    /// `set_field`,
    /// then writes a targeted splice (or strip, for `None`) into `book.toml`,
    /// preserving the rest of the manifest (mirrors #494 / #550). `literal` is the
    /// already-formatted TOML value to write (a quoted string, or a bare `true` /
    /// `false`), so each knob type serializes correctly; `None` strips the key.
    fn splice_style(
        &mut self,
        key: &str,
        literal: Option<String>,
        set_field: impl FnOnce(&mut ExportStyle),
    ) -> std::io::Result<()> {
        if self.project.book_count() > 1 {
            return Err(std::io::Error::other(SERIES_STYLE_GUIDANCE));
        }
        let style = self
            .project
            .manifest
            .style
            .get_or_insert_with(Default::default);
        set_field(style);

        let path = Project::manifest_path(&self.project.root);
        // A failed read must abort: splicing onto an empty base would overwrite the
        // whole manifest while reporting success (#565).
        let text = read_existing(&path)?;
        let updated = literal.map_or_else(
            || config::strip_value(&text, "style", key).0,
            |v| config::set_value(&text, "style", key, &v),
        );
        crate::atomic::write(&path, updated.as_bytes())
    }

    /// Splice `[print] trim` (#569). Same standalone-only constraint and targeted
    /// splice as [`Self::splice_style`], for the print table.
    fn splice_print_trim(&mut self, name: &str) -> std::io::Result<()> {
        if self.project.book_count() > 1 {
            return Err(std::io::Error::other(SERIES_PRINT_GUIDANCE));
        }
        self.project
            .manifest
            .print
            .get_or_insert_with(PrintConfig::default)
            .trim = Some(name.to_string());

        let path = Project::manifest_path(&self.project.root);
        let text = read_existing(&path)?;
        let quoted = toml::Value::String(name.to_string()).to_string();
        let updated = config::set_value(&text, "print", "trim", &quoted);
        crate::atomic::write(&path, updated.as_bytes())
    }
}

/// A bool's drop-cap label for the menu (`Some(true)` → "on", `None` → "default").
const fn drop_cap_label(value: Option<bool>) -> &'static str {
    match value {
        Some(true) => "on",
        Some(false) => "off",
        None => "(default — off)",
    }
}

/// An optional string's menu label — the value, or "(default)" when unset.
fn opt_label(value: Option<&str>) -> String {
    value.map_or_else(|| "(default)".to_string(), ToString::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::tests::{app_scratch_dir, open_test_app, TempProject};
    use crate::export::epub::to_epub;
    use crate::export::ImageRole;
    use crate::mode::Action;
    use std::io::Read;

    /// A one-chapter standalone project on disk whose manifest a style test inspects.
    fn style_app(name: &str) -> (App, TempProject) {
        let dir = app_scratch_dir(name);
        std::fs::write(
            Project::manifest_path(&dir),
            "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("one.md"), "first chapter\n").unwrap();
        let app = open_test_app(&dir);
        (app, TempProject::new(dir))
    }

    fn manifest_text(dir: &std::path::Path) -> String {
        std::fs::read_to_string(Project::manifest_path(dir)).unwrap()
    }

    #[test]
    fn the_set_style_action_is_registered() {
        assert!(
            Action::ALL.iter().any(|a| a.name() == "style:set"),
            "style:set is in the registry"
        );
        assert!(matches!(
            Action::from_name("style:set"),
            Some(Action::SetExportStyle)
        ));
        // It sits in the Export group, with the other export actions.
        assert_eq!(Action::SetExportStyle.group(), "Export");
    }

    #[test]
    fn every_trim_name_resolves_to_points() {
        // The picker offers exactly TRIM_NAMES; each must be a real trim size.
        for name in pdf::TRIM_NAMES {
            assert!(
                pdf::trim_points(name).is_some(),
                "trim '{name}' should resolve"
            );
        }
    }

    #[test]
    fn the_menu_lists_every_knob() {
        let (mut app, _dir) = style_app("style-menu");
        app.open_export_style_menu();
        let Mode::Palette(p) = &app.mode else {
            panic!("the setter opens a palette");
        };
        // Preset, scene break, alignment, drop caps, two ornaments, cover, cover
        // description, trim.
        assert_eq!(p.items.len(), 9);
        assert_eq!(p.title, "Export style");
    }

    #[test]
    fn the_preset_picker_offers_none_plus_the_library() {
        let (mut app, _dir) = style_app("style-presets");
        app.open_style_preset_picker();
        let Mode::Palette(p) = &app.mode else {
            panic!("preset picker opens a palette");
        };
        assert_eq!(
            p.items.len(),
            PRESET_NAMES.len() + 1,
            "(none) + the library"
        );
        assert_eq!(p.items[0].label, "(none)");
        for (item, name) in p.items[1..].iter().zip(PRESET_NAMES) {
            assert_eq!(item.label, name);
        }
    }

    #[test]
    fn picking_a_preset_splices_style_and_round_trips() {
        let (mut app, dir) = style_app("style-pick-preset");
        // `classic` is index 0 in PRESET_NAMES.
        app.set_style_preset(Some(0));

        // In-memory style carries it immediately (for the preview / next export).
        assert_eq!(
            app.project
                .manifest
                .style
                .as_ref()
                .unwrap()
                .preset
                .as_deref(),
            Some("classic")
        );
        // Persisted as a `[style]` table, re-opening identically.
        let toml = manifest_text(&dir);
        assert!(toml.contains("[style]"), "{toml}");
        assert!(toml.contains("preset = \"classic\""), "{toml}");
        let reopened = Project::open(&*dir).unwrap();
        assert_eq!(
            reopened.manifest.style.unwrap().preset.as_deref(),
            Some("classic")
        );
    }

    #[test]
    fn an_app_set_preset_reads_back_identical_to_a_hand_typed_one() {
        let (mut app, dir) = style_app("style-roundtrip-app");
        app.set_style_preset(Some(1)); // "modern"
        let from_app = Project::open(&*dir).unwrap().manifest.style.unwrap();

        let hand = app_scratch_dir("style-roundtrip-hand");
        std::fs::write(
            Project::manifest_path(&hand),
            "title = \"Book\"\n\n[style]\npreset = \"modern\"\n\n\
             [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
        )
        .unwrap();
        std::fs::write(hand.join("one.md"), "x\n").unwrap();
        let from_hand = Project::open(&hand).unwrap().manifest.style.unwrap();
        let _hand = TempProject::new(hand);

        assert_eq!(from_app, from_hand, "app-set preset == hand-typed preset");
    }

    #[test]
    fn setting_trim_splices_the_print_table() {
        let (mut app, dir) = style_app("style-trim");
        // "5x8" is index 1 in TRIM_NAMES.
        app.set_style_trim(1);
        assert_eq!(
            app.project
                .manifest
                .print
                .as_ref()
                .and_then(|p| p.trim.as_deref()),
            Some("5x8")
        );
        let toml = manifest_text(&dir);
        assert!(toml.contains("[print]"), "{toml}");
        assert!(toml.contains("trim = \"5x8\""), "{toml}");
        let reopened = Project::open(&*dir).unwrap();
        assert_eq!(
            reopened.manifest.print.unwrap().trim.as_deref(),
            Some("5x8")
        );
    }

    #[test]
    fn setting_the_knobs_preserves_the_rest_of_the_manifest() {
        // Seed a manifest with another section + a comment to guard untouched,
        // then open the app on it.
        let dir = app_scratch_dir("style-preserve");
        std::fs::write(
            Project::manifest_path(&dir),
            "title = \"Book\"\n# keep me\n\n[goals]\nwords = 1000\n\n\
             [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
        )
        .unwrap();
        std::fs::write(dir.join("one.md"), "first chapter\n").unwrap();
        let mut app = open_test_app(&dir);
        let dir = TempProject::new(dir);

        app.set_style_preset(Some(0));
        app.apply_style_text(StyleTextKnob::SceneBreak, "@@@");
        app.set_style_align(Some(0)); // "left"
        app.set_style_drop_cap(Some(true));
        app.set_style_trim(0); // "6x9"

        let toml = manifest_text(&dir);
        // The pre-existing content survives.
        assert!(toml.contains("# keep me"), "{toml}");
        assert!(toml.contains("[goals]"), "{toml}");
        assert!(toml.contains("words = 1000"), "{toml}");
        // The new keys all landed.
        assert!(toml.contains("preset = \"classic\""), "{toml}");
        assert!(toml.contains("scene_break = \"@@@\""), "{toml}");
        assert!(toml.contains("heading_align = \"left\""), "{toml}");
        assert!(toml.contains("drop_cap = true"), "{toml}");
        assert!(toml.contains("trim = \"6x9\""), "{toml}");

        // And the project still opens and parses every section.
        let reopened = Project::open(&*dir).unwrap();
        let style = reopened.manifest.style.unwrap();
        assert_eq!(style.preset.as_deref(), Some("classic"));
        assert_eq!(style.scene_break.as_deref(), Some("@@@"));
        assert!(style.drop_cap.unwrap());
        assert_eq!(reopened.manifest.goals.unwrap().words, 1000);
    }

    #[test]
    fn clearing_a_knob_strips_its_key() {
        let (mut app, dir) = style_app("style-clear");
        app.apply_style_text(StyleTextKnob::SceneBreak, "* * *");
        assert!(manifest_text(&dir).contains("scene_break"));
        // A blank entry clears it.
        app.apply_style_text(StyleTextKnob::SceneBreak, "");
        let toml = manifest_text(&dir);
        assert!(!toml.contains("scene_break"), "{toml}");
        assert!(app
            .project
            .manifest
            .style
            .as_ref()
            .unwrap()
            .scene_break
            .is_none());
    }

    #[test]
    fn setting_the_cover_image_splices_the_path_and_exports_it() {
        // Setting the cover via the in-app style flow (#582) splices `cover_image`
        // into `[style]` and the resolved cover embeds in the EPUB. Hermetic: a
        // scratch project plus a tiny PNG written to disk.
        let (mut app, dir) = style_app("style-cover");
        // A small real PNG the export pipeline reads from the project root.
        let png = image::RgbaImage::from_pixel(4, 6, image::Rgba([10, 20, 30, 255]));
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(png)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        std::fs::write(dir.join("cover.png"), buf.into_inner()).unwrap();

        app.apply_style_text(StyleTextKnob::CoverImage, "cover.png");

        // In-memory + persisted into [style].
        assert_eq!(
            app.project
                .manifest
                .style
                .as_ref()
                .unwrap()
                .cover_image
                .as_deref(),
            Some("cover.png")
        );
        let toml = manifest_text(&dir);
        assert!(
            toml.contains("[style]") && toml.contains("cover_image = \"cover.png\""),
            "{toml}"
        );

        // Resolve it from disk (as the exporter does) and embed in an EPUB.
        let mut style = app.project.manifest.style.clone().unwrap();
        let warnings = style.resolve_images(|p| std::fs::read(dir.join(p)));
        assert!(warnings.is_empty(), "cover resolves: {warnings:?}");
        assert!(style.image(ImageRole::Cover).is_some(), "cover resolved");
        let bytes = to_epub(
            &crate::export::Document {
                title: "Book".into(),
                subtitle: None,
                author: None,
                series: None,
                publishing: crate::export::PubMeta::default(),
                epigraph: None,
                half_title: false,
                series_books: Vec::new(),
                front: vec![],
                parts: vec![crate::export::Part {
                    name: None,
                    chapters: vec![crate::export::Chapter {
                        title: "One".into(),
                        epigraph: None,
                        body: "Prose.".into(),
                        role: None,
                    }],
                }],
                back: vec![],
            },
            "2026-06-30T00:00:00Z",
            &style,
        )
        .unwrap();
        let opf = read_zip_entry(&bytes, "OEBPS/content.opf");
        assert!(opf.contains("properties=\"cover-image\""), "{opf}");
        assert!(opf.contains("<meta name=\"cover\""), "{opf}");
    }

    #[test]
    fn setting_the_cover_description_splices_the_alt_text_and_round_trips() {
        // The in-app "Cover description" knob (#818) splices `cover_image_alt` into
        // `[style]`, the seam authors use to supply real alt text.
        let (mut app, dir) = style_app("style-cover-alt");
        app.apply_style_text(StyleTextKnob::CoverImageAlt, "A lighthouse at dusk");

        assert_eq!(
            app.project
                .manifest
                .style
                .as_ref()
                .unwrap()
                .cover_image_alt
                .as_deref(),
            Some("A lighthouse at dusk")
        );
        let toml = manifest_text(&dir);
        assert!(
            toml.contains("cover_image_alt = \"A lighthouse at dusk\""),
            "{toml}"
        );
        // Re-opening the project reads the same alt text back.
        let reopened = Project::open(&*dir).unwrap();
        assert_eq!(
            reopened.manifest.style.unwrap().cover_image_alt.as_deref(),
            Some("A lighthouse at dusk")
        );
    }

    /// Read one zip entry's text by name — for inspecting the EPUB the cover test
    /// builds.
    fn read_zip_entry(bytes: &[u8], name: &str) -> String {
        let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
        let mut file = zip.by_name(name).unwrap();
        let mut s = String::new();
        file.read_to_string(&mut s).unwrap();
        s
    }

    /// Write a two-book series manifest (and its chapter files) to `dir`, for the
    /// series-guidance tests.
    fn write_series(dir: &std::path::Path) {
        std::fs::create_dir_all(dir.join("b1")).unwrap();
        std::fs::create_dir_all(dir.join("b2")).unwrap();
        std::fs::write(dir.join("b1/one.md"), "alpha\n").unwrap();
        std::fs::write(dir.join("b2/one.md"), "beta\n").unwrap();
        std::fs::write(
            Project::manifest_path(dir),
            "title = \"The Saga\"\n\n\
             [[books]]\ntitle = \"Book One\"\n\n\
             [[books.chapters]]\ntitle = \"Dawn\"\nfile = \"b1/one.md\"\n\n\
             [[books]]\ntitle = \"Book Two\"\n\n\
             [[books.chapters]]\ntitle = \"Dusk\"\nfile = \"b2/one.md\"\n",
        )
        .unwrap();
    }

    #[test]
    fn a_series_style_set_guides_to_a_standalone_and_leaves_the_manifest_untouched() {
        // A multi-book series carries no per-book [style]; the setter degrades to a
        // clear message rather than corrupting the manifest (#1105). The message
        // must not read as merely "unfinished" — it names the path that works
        // (standalone) and does NOT send the author to a hand-edit that wouldn't
        // take effect.
        let dir = app_scratch_dir("style-series");
        write_series(&dir);
        let before = manifest_text(&dir);
        let mut app = open_test_app(&dir);
        let _dir = TempProject::new(dir);

        app.set_style_preset(Some(0));
        let msg = app.message.as_deref().unwrap_or("");
        assert!(msg.contains("standalone"), "{msg:?}");
        assert!(
            !msg.contains("isn't supported yet"),
            "must not read as unfinished: {msg:?}"
        );
        assert!(
            !msg.contains("per book by hand"),
            "must not point at a hand-edit that wouldn't take effect: {msg:?}"
        );
        // Nothing was written to the series manifest.
        assert_eq!(manifest_text(&_dir), before, "series manifest untouched");
    }

    #[test]
    fn a_series_trim_set_guides_to_a_standalone() {
        // The [print] trim counterpart to the [style] guidance (#1105).
        let dir = app_scratch_dir("style-series-trim");
        write_series(&dir);
        let before = manifest_text(&dir);
        let mut app = open_test_app(&dir);
        let _dir = TempProject::new(dir);

        app.set_style_trim(0);
        let msg = app.message.as_deref().unwrap_or("");
        assert!(msg.contains("standalone"), "{msg:?}");
        assert!(!msg.contains("isn't supported yet"), "{msg:?}");
        assert_eq!(manifest_text(&_dir), before, "series manifest untouched");
    }

    /// A standalone novel is unchanged by the series guidance — its style still
    /// splices in-app (#1105 guards single-book behavior).
    #[test]
    fn a_standalone_still_splices_style_after_the_series_reword() {
        let (mut app, dir) = style_app("style-standalone-unchanged");
        app.set_style_preset(Some(0));
        assert_eq!(
            app.message.as_deref(),
            Some("Set preset to classic."),
            "single-book behavior unchanged"
        );
        assert!(manifest_text(&dir).contains("preset = \"classic\""));
    }
}
