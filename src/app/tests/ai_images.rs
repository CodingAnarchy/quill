//! AI image generation: character portraits and scene illustration.

use super::*;
use std::fs;

// --- #238: character portraits ----------------------------------------------

fn image_on(app: &mut App) {
    app.image = ai::image::ImageSettings { enabled: true, api_key: Some("k".into()), ..Default::default() };
}

fn write_character(dir: &std::path::Path, slug: &str, name: &str) {
    let chars = dir.join("bible").join("characters");
    fs::create_dir_all(&chars).unwrap();
    fs::write(chars.join(format!("{slug}.md")), format!("---\nname: {name}\n---\n\n## Physical\n- green eyes\n")).unwrap();
}

#[test]
fn portrait_pass_needs_image_generation_enabled() {
    let (mut app, dir) = two_chapter_app("portrait-off");
    app.image = ai::image::ImageSettings::default(); // off
    write_character(&dir, "mara", "Mara");
    app.start_portrait();
    assert!(!matches!(app.mode, Mode::Prompt(_)), "no confirm when image gen is off");
    assert!(
        app.message.as_deref().unwrap_or("").contains("Image generation is off"),
        "{:?}",
        app.message
    );
}

#[test]
fn portrait_pass_needs_the_character_bible() {
    let (mut app, _dir) = two_chapter_app("portrait-no-bible");
    image_on(&mut app);
    app.start_portrait(); // no bible/characters yet
    assert!(!matches!(app.mode, Mode::Prompt(_)));
    assert!(app.message.as_deref().unwrap_or("").contains("character pass first"), "{:?}", app.message);
}

#[test]
fn portrait_pass_opens_a_character_picker() {
    let (mut app, dir) = two_chapter_app("portrait-picker");
    image_on(&mut app);
    write_character(&dir, "mara", "Mara");
    write_character(&dir, "tomas", "Tomas");
    app.start_portrait();
    match &app.mode {
        // "All characters (2)" plus one item per character (#244).
        Mode::Palette(p) => {
            assert_eq!(p.title, "Character portrait");
            assert_eq!(p.items.len(), 3);
            assert!(p.items[0].label.contains("All characters (2)"), "{}", p.items[0].label);
        }
        _ => panic!("expected the character picker"),
    }
}

#[test]
fn picking_all_confirms_the_whole_cast() {
    let (mut app, dir) = two_chapter_app("portrait-all");
    image_on(&mut app);
    write_character(&dir, "mara", "Mara");
    write_character(&dir, "tomas", "Tomas");
    app.confirm_portraits(crate::mode::PortraitChoice::All);
    match &app.mode {
        Mode::Prompt(p) => assert!(p.label.contains("2 character portraits"), "{}", p.label),
        _ => panic!("expected the cost confirm"),
    }
}

#[test]
fn picking_one_confirms_and_generates_only_that_character() {
    let (mut app, dir) = two_chapter_app("portrait-one");
    image_on(&mut app);
    write_character(&dir, "mara", "Mara");
    write_character(&dir, "tomas", "Tomas");
    // Sorted by file: mara.md (0), tomas.md (1) → pick the second.
    app.confirm_portraits(crate::mode::PortraitChoice::One(1));
    match &app.mode {
        Mode::Prompt(p) => assert!(p.label.contains("1 character portrait"), "{}", p.label),
        _ => panic!("expected the cost confirm"),
    }
    // Only the one picked target is staged, so generation won't touch the others.
    let pending = app.pending_portraits.as_ref().expect("a selection is staged");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].0, "tomas", "the picked slug");
}

#[test]
fn cancelling_the_portrait_confirm_discards_the_selection() {
    let (mut app, dir) = two_chapter_app("portrait-cancel");
    image_on(&mut app);
    write_character(&dir, "mara", "Mara");
    app.confirm_portraits(crate::mode::PortraitChoice::All);
    assert!(app.pending_portraits.is_some());
    app.on_key(key(KeyCode::Esc)); // decline the confirm
    assert!(app.pending_portraits.is_none(), "the staged selection is dropped");
}

#[test]
fn image_results_summarize_saved_and_failed() {
    let (mut app, _dir) = two_chapter_app("image-apply");
    app.apply_images(Ok(ai::image::ImageReport {
        saved: vec!["marketing/images/characters/mara.jpg".into()],
        failed: vec!["Tomas: network error".into()],
    }));
    assert!(app.overlay.is_some(), "summary overlay opens");
}

#[test]
#[cfg(not(feature = "local"))]
fn image_pass_reports_when_the_local_backend_isnt_compiled() {
    let (mut app, dir) = two_chapter_app("image-local-uncompiled");
    app.image = ai::image::ImageSettings {
        enabled: true,
        backend: ai::image::ImageBackendKind::Local,
        ..Default::default()
    };
    write_character(&dir, "mara", "Mara");
    app.start_portrait();
    assert!(!matches!(app.mode, Mode::Prompt(_)), "no confirm; local model isn't built in");
    assert!(
        app.message.as_deref().unwrap_or("").contains("--features local"),
        "explains how to enable: {:?}",
        app.message
    );
}

#[test]
fn scene_pass_needs_image_generation_enabled() {
    let (mut app, _dir) = two_chapter_app("scene-off");
    app.image = ai::image::ImageSettings::default(); // off
    app.start_scene();
    assert!(!matches!(app.mode, Mode::Prompt(_)), "no confirm when image gen is off");
    assert!(app.message.as_deref().unwrap_or("").contains("Image generation is off"), "{:?}", app.message);
}

#[test]
fn scene_pass_confirms_for_the_active_chapter() {
    let (mut app, _dir) = two_chapter_app("scene-confirm");
    image_on(&mut app);
    // The active chapter ("One") has prose in two_chapter_app, so there's a scene.
    app.start_scene();
    match &app.mode {
        Mode::Prompt(p) => {
            assert!(p.label.contains("One"), "names the active chapter: {}", p.label);
            assert!(p.label.contains("fal.ai") && p.label.contains("billed"), "{}", p.label);
        }
        _ => panic!("expected the scene cost confirm"),
    }
}

#[test]
fn scene_pass_needs_chapter_text() {
    let (mut app, _dir) = two_chapter_app("scene-empty");
    image_on(&mut app);
    // Empty the active chapter's buffer so there's nothing to illustrate.
    app.buffer = crate::buffer::Buffer::from_text("");
    app.start_scene();
    assert!(!matches!(app.mode, Mode::Prompt(_)));
    assert!(app.message.as_deref().unwrap_or("").contains("no text to illustrate"), "{:?}", app.message);
}

