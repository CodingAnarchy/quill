//! The style-flag sweep wired into the editor (#1001): the idle pass caches the
//! active category's hits, `]r` / `[r` step the cursor onto each (crossing chapters
//! and reporting the manuscript's ends), toggling the sweep off clears the cache,
//! and the prose is never altered.

use super::*;
use crate::analysis::FlagCategory;

/// A three-chapter project whose bodies carry a known number of `-ly` adverbs, so
/// the sweep's hits sit at predictable places. Chapter one has two, chapter two
/// none, chapter three one — exercising the skip-empty-chapter cross step.
fn adverb_app(name: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n\n\
         [[chapters]]\ntitle = \"Three\"\nfile = \"three.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "She slowly walked and quietly left.\n").unwrap();
    fs::write(
        dir.join("two.md"),
        "The plain grey stone door stood shut.\n",
    )
    .unwrap();
    fs::write(dir.join("three.md"), "He softly closed it.\n").unwrap();
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

#[test]
fn turning_the_sweep_on_caches_hits_and_jumps_to_the_first() {
    let (mut app, _dir) = adverb_app("flag-on");
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.start_style_flag_sweep(FlagCategory::Adverbs);

    // Two adverbs in chapter one, cached; the cursor lands on the first ("slowly").
    assert_eq!(app.style_flags.len(), 2, "{:?}", app.style_flags);
    let (s, e) = app.style_flags[0];
    assert_eq!(&app.buffer.text()[s..e], "slowly");
    assert_eq!(app.buffer.cursor_byte(), s, "cursor on the first hit");
}

#[test]
fn stepping_forward_and_back_lands_on_each_hit() {
    let (mut app, _dir) = adverb_app("flag-step");
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.style_flag_category = Some(FlagCategory::Adverbs);
    app.refresh_style_flags();
    let text = app.buffer.text();
    let hits: Vec<(usize, usize)> = app.style_flags.clone();
    assert_eq!(hits.len(), 2);

    // Cursor before the first hit → step lands on "slowly", then "quietly".
    app.navigate_style_flag(1);
    assert_eq!(app.buffer.cursor_byte(), hits[0].0);
    assert_eq!(&text[hits[0].0..hits[0].1], "slowly");
    app.navigate_style_flag(1);
    assert_eq!(app.buffer.cursor_byte(), hits[1].0);
    assert_eq!(&text[hits[1].0..hits[1].1], "quietly");

    // Stepping back returns to the first.
    app.navigate_style_flag(-1);
    assert_eq!(app.buffer.cursor_byte(), hits[0].0);
}

#[test]
fn stepping_forward_crosses_chapters_and_skips_empty_ones() {
    let (mut app, _dir) = adverb_app("flag-cross");
    app.style_flag_category = Some(FlagCategory::Adverbs);
    app.refresh_style_flags();
    // Land on chapter one's last adverb, then step forward: chapter two has none,
    // so the sweep skips it and opens chapter three on "softly".
    let last = *app.style_flags.last().unwrap();
    app.buffer.cursor = app.buffer.cursor_at_byte(last.0);
    assert_eq!(app.project.active_index(), 0);

    app.navigate_style_flag(1);
    assert_eq!(
        app.project.active_index(),
        2,
        "skipped the empty chapter two"
    );
    let (s, e) = app.style_flags[0];
    assert_eq!(&app.buffer.text()[s..e], "softly");
    assert_eq!(app.buffer.cursor_byte(), s);
}

#[test]
fn stepping_reports_the_ends_of_the_manuscript() {
    let (mut app, _dir) = adverb_app("flag-ends");
    app.style_flag_category = Some(FlagCategory::Adverbs);
    app.refresh_style_flags();

    // From the first hit in the first chapter, stepping back past it reports the
    // manuscript start rather than moving silently.
    app.buffer.cursor = app.buffer.cursor_at_byte(app.style_flags[0].0);
    app.navigate_style_flag(-1);
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("start of the manuscript"),
        "{:?}",
        app.message
    );

    // Walk to the last chapter's last hit, then step forward past it → end reported.
    app.activate_chapter(2);
    app.refresh_style_flags();
    let last = *app.style_flags.last().unwrap();
    app.buffer.cursor = app.buffer.cursor_at_byte(last.0);
    app.navigate_style_flag(1);
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("end of the manuscript"),
        "{:?}",
        app.message
    );
}

#[test]
fn clearing_the_sweep_empties_the_cache() {
    let (mut app, _dir) = adverb_app("flag-clear");
    app.start_style_flag_sweep(FlagCategory::Adverbs);
    assert!(!app.style_flags.is_empty());
    app.clear_style_flags();
    assert!(app.style_flag_category.is_none());
    assert!(
        app.style_flags.is_empty(),
        "cache cleared → underlines gone"
    );

    // With no sweep active, a step just prompts to pick one; nothing moves.
    let before = app.buffer.cursor_byte();
    app.navigate_style_flag(1);
    assert_eq!(app.buffer.cursor_byte(), before);
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("pick a category"),
        "{:?}",
        app.message
    );
}

#[test]
fn the_sweep_never_edits_the_prose() {
    // The load-bearing boundary: navigating and marking must not touch a byte.
    let (mut app, _dir) = adverb_app("flag-readonly");
    let before: Vec<String> = ["one.md", "two.md", "three.md"]
        .iter()
        .map(|f| fs::read_to_string(app.project.root.join(f)).unwrap())
        .collect();
    let live_before = app.buffer.text();

    app.start_style_flag_sweep(FlagCategory::Adverbs);
    app.navigate_style_flag(1);
    app.navigate_style_flag(1); // crosses into chapter three
    app.navigate_style_flag(-1);
    app.clear_style_flags();

    // The live buffer's text is byte-identical (the cursor moved, the prose didn't),
    // and no chapter file was rewritten.
    app.activate_chapter(0);
    assert_eq!(
        app.buffer.text(),
        live_before,
        "chapter one prose untouched"
    );
    for (f, was) in ["one.md", "two.md", "three.md"].iter().zip(&before) {
        let now = fs::read_to_string(app.project.root.join(f)).unwrap();
        assert_eq!(&now, was, "{f} unchanged on disk");
    }
}

#[test]
fn the_vim_bracket_r_bindings_step_the_sweep() {
    let (mut app, _dir) = adverb_app("flag-vim");
    app.vim = true;
    app.mode = Mode::Normal;
    app.style_flag_category = Some(FlagCategory::Adverbs);
    app.refresh_style_flags();
    let hits = app.style_flags.clone();
    app.buffer.cursor = app.buffer.cursor_at_byte(0);

    // `]r` steps forward onto the first, then the second adverb.
    app.on_key(key(KeyCode::Char(']')));
    app.on_key(key(KeyCode::Char('r')));
    assert_eq!(app.buffer.cursor_byte(), hits[0].0);
    app.on_key(key(KeyCode::Char(']')));
    app.on_key(key(KeyCode::Char('r')));
    assert_eq!(app.buffer.cursor_byte(), hits[1].0);

    // `[r` steps back onto the first.
    app.on_key(key(KeyCode::Char('[')));
    app.on_key(key(KeyCode::Char('r')));
    assert_eq!(app.buffer.cursor_byte(), hits[0].0);
}

#[test]
fn the_picker_lists_categories_and_selecting_one_starts_the_sweep() {
    let (mut app, _dir) = adverb_app("flag-picker");
    app.open_style_flag_picker();
    // The picker is a palette with the "(off)" row plus every category.
    let labels: Vec<String> = match &app.mode {
        Mode::Palette(p) => p.items.iter().map(|i| i.label.clone()).collect(),
        _ => panic!("the picker should open a palette"),
    };
    assert!(labels[0].contains("off"));
    assert!(labels.iter().any(|l| l == FlagCategory::Adverbs.label()));

    // Select "Adverbs" and confirm — the sweep turns on and caches its hits.
    let idx = match &app.mode {
        Mode::Palette(p) => p
            .items
            .iter()
            .position(|i| i.label == FlagCategory::Adverbs.label())
            .unwrap(),
        _ => unreachable!(),
    };
    if let Mode::Palette(p) = &mut app.mode {
        p.selected = idx; // flat palette: item index == visible row
    }
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.style_flag_category, Some(FlagCategory::Adverbs));
    assert!(!app.style_flags.is_empty());
}
