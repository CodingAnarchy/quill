//! Tests for the in-editor style preview (#483): an author opens a styled sample
//! EPUB of the current `[style]` settings (preset + overrides) before committing to
//! a full export. The preview reuses the real export's style resolution and EPUB
//! writer, renders a fixed sample (never the author's chapters), and touches only a
//! temp file. These tests exercise the side-effect-free core (`style_preview_epub`)
//! so they never launch an external reader.

use super::*;
use crate::export::ExportStyle;
use crate::mode::{Command, Mode};
use std::io::Read;

/// Whether any text entry in the EPUB zip contains `needle` — enough to prove a
/// resolved style glyph reached the rendered output.
fn epub_contains(bytes: &[u8], needle: &str) -> bool {
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).expect("valid zip");
    for i in 0..zip.len() {
        let mut entry = zip.by_index(i).unwrap();
        let mut s = String::new();
        if entry.read_to_string(&mut s).is_ok() && s.contains(needle) {
            return true;
        }
    }
    false
}

#[test]
fn style_preview_reflects_the_resolved_preset_and_overrides() {
    let (mut app, _dir) = two_chapter_app("preview-style-classic");
    // `classic` plus one explicit override: the override must win in the preview.
    app.project.manifest.style = Some(ExportStyle {
        preset: Some("classic".into()),
        scene_break: Some("~ ~ ~".into()),
        ..Default::default()
    });

    let (bytes, warnings) = app.style_preview_epub().expect("preview builds");
    assert_eq!(&bytes[..2], b"PK", "an EPUB zip");
    assert!(
        warnings.is_empty(),
        "clean style, no warnings: {warnings:?}"
    );
    // The preset's title ornament renders; the overridden scene break wins over the
    // preset's `* * *`.
    assert!(epub_contains(&bytes, "❦"), "preset ornament in the preview");
    assert!(
        epub_contains(&bytes, "~ ~ ~"),
        "override scene break in the preview"
    );
    assert!(
        !epub_contains(&bytes, "* * *"),
        "preset scene break was overridden"
    );
}

#[test]
fn style_preview_surfaces_style_warnings() {
    let (mut app, _dir) = two_chapter_app("preview-style-warn");
    app.project.manifest.style = Some(ExportStyle {
        preset: Some("nonesuch".into()),
        ..Default::default()
    });

    let (_, warnings) = app.style_preview_epub().expect("preview still builds");
    assert!(
        warnings.iter().any(|w| w.contains("unknown preset")),
        "an unknown preset is reported, got: {warnings:?}"
    );
}

#[test]
fn style_preview_does_not_touch_the_project() {
    let (mut app, dir) = two_chapter_app("preview-style-readonly");
    app.project.manifest.style = Some(ExportStyle {
        preset: Some("classic".into()),
        ..Default::default()
    });
    let before = fs::read_to_string(dir.join("one.md")).unwrap();

    app.style_preview_epub().expect("preview builds");

    assert!(
        !dir.join("export").exists(),
        "preview writes no export/ dir in the project"
    );
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        before,
        "the manuscript is untouched"
    );
}

#[test]
fn the_export_menu_offers_the_style_setter_and_preview() {
    let (mut app, _dir) = two_chapter_app("preview-style-menu");
    app.open_export_palette();
    let Mode::Palette(p) = &app.mode else {
        panic!("export palette open");
    };
    // The style setter (#569) leads, then the preview — both before the formats.
    assert!(
        matches!(
            p.items.first().map(|it| it.command),
            Some(Command::Run(crate::mode::Action::SetExportStyle))
        ),
        "the style setter is the first export-menu entry"
    );
    assert!(
        matches!(
            p.items.get(1).map(|it| it.command),
            Some(Command::PreviewStyle)
        ),
        "the preview is the second export-menu entry"
    );
}
