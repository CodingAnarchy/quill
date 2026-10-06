//! The style-flag sweep wired into the editor (#1001): the idle pass caches the
//! active category's hits, `]r` / `[r` step the cursor onto each (crossing chapters
//! and reporting the manuscript's ends), toggling the sweep off clears the cache,
//! and the prose is never altered.

use super::*;
use crate::analysis::FlagCategory;
use crate::project::Analysis;

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
    app.flags.vim = true;
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
fn switching_chapters_clears_the_old_chapters_flag_cache() {
    // BUG 1 (#1001): the highlight cache holds byte spans into the chapter just
    // left. `activate_chapter` must refresh it so the new chapter never renders a
    // frame of phantom underlines from the old chapter's spans before the next
    // idle tick. Chapter one has two adverbs; chapter two has none — so a correctly
    // refreshed cache is *empty* on chapter two, and every cached span (there are
    // none) would otherwise mark unrelated prose.
    let (mut app, _dir) = adverb_app("flag-switch-clears-cache");
    app.start_style_flag_sweep(FlagCategory::Adverbs);
    assert_eq!(app.style_flags.len(), 2, "chapter one's adverbs are cached");

    // Switch to chapter two directly (as a browser jump / `:goto` would), WITHOUT
    // an intervening idle tick. The cache must already reflect chapter two.
    app.activate_chapter(1);
    assert_eq!(app.project.active_index(), 1);
    assert!(
        app.style_flags.is_empty(),
        "no phantom marks carried over: chapter two has no adverbs, so the \
         refreshed cache is empty — got {:?}",
        app.style_flags
    );

    // Belt-and-braces for a chapter that *does* have hits: every cached span must
    // index live chapter-three prose (not a stale chapter-one offset).
    app.activate_chapter(2);
    let text = app.buffer.text();
    assert!(
        app.style_flags
            .iter()
            .all(|&(s, e)| e <= text.len() && text[s..e].ends_with("ly")),
        "every cached span marks a live chapter-three adverb: {:?} over {text:?}",
        app.style_flags
    );
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

#[test]
fn editing_invalidates_the_flag_cache_until_the_next_idle_tick() {
    // #1035: the flag cache holds byte spans computed on the last idle tick. An
    // edit shifts every offset after the caret, so between the keystroke and the
    // next idle recompute a cached span points at the wrong characters. The edit
    // path drops the cache so no stale span renders during an active typing
    // burst; the idle recompute restores it at the correct new offsets.
    let (mut app, _dir) = adverb_app("flag-edit-invalidates");
    app.mode = Mode::Insert;
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.start_style_flag_sweep(FlagCategory::Adverbs);
    let before = app.style_flags.clone();
    assert_eq!(before.len(), 2, "chapter one's two adverbs are cached");

    // Type one character at the start of the line. Every hit now sits one byte
    // later than the cached span says — so the cache must not survive the edit.
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    type_str(&mut app, "X");
    assert!(
        app.style_flags.is_empty(),
        "the edit drops the stale cache so no mark renders on a shifted span: {:?}",
        app.style_flags
    );

    // The idle recompute brings the marks back, now addressing the post-edit
    // offsets — each still lands on a real "-ly" adverb, shifted by the insert.
    app.on_idle();
    let after = app.style_flags.clone();
    assert_eq!(after.len(), 2, "the idle pass recomputes both hits");
    let text = app.buffer.text();
    assert!(
        after
            .iter()
            .all(|&(s, e)| e <= text.len() && text[s..e].ends_with("ly")),
        "every recomputed span marks a live adverb at its new offset: {after:?} over {text:?}"
    );
    // The recompute shifted the spans forward by the inserted byte — proof the
    // pre-edit cache would have marked the wrong characters had it survived.
    assert_eq!(
        after[0].0,
        before[0].0 + 1,
        "the first hit moved one byte past the insert"
    );
}

#[test]
fn toggling_off_still_clears_after_an_edit_invalidation() {
    // The edit invalidation and the explicit "(off)" clear must compose: an edit
    // empties the cache, and turning the sweep off then leaves it off (no active
    // category, nothing to recompute on the next idle tick).
    let (mut app, _dir) = adverb_app("flag-edit-then-off");
    app.mode = Mode::Insert;
    app.start_style_flag_sweep(FlagCategory::Adverbs);
    assert!(!app.style_flags.is_empty());

    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    type_str(&mut app, "X");
    assert!(app.style_flags.is_empty(), "the edit invalidated the cache");

    app.clear_style_flags();
    assert!(app.style_flag_category.is_none(), "sweep is off");
    // An idle tick after turning the sweep off must not resurrect the marks.
    app.on_idle();
    assert!(
        app.style_flags.is_empty(),
        "the sweep is off, so idle recomputes nothing: {:?}",
        app.style_flags
    );
}

#[test]
fn edit_invalidation_does_no_document_proportional_work() {
    // #1035: the invalidation must stay O(1) on the keystroke path — it truncates
    // two small window-scoped vectors and does no rope or document-sized work.
    // A pre-populated cache and an empty one must invalidate in the same trivial
    // time regardless of buffer size, so this can never regress into a per-
    // keystroke document scan. (The heavy recompute stays on the idle tick, timed
    // by `flag_pass_is_fast_at_window_size` / `proximity_pass_is_fast_at_window_size`.)
    let (mut app, _dir) = adverb_app("flag-invalidate-o1");
    // Fill both caches with a spread of spans, then invalidate directly (the same
    // call the edit path makes) — it clears both without touching the buffer.
    app.style_flag_category = Some(FlagCategory::Adverbs);
    app.style_flags = (0..500).map(|i| (i * 3, i * 3 + 2)).collect();
    app.proximity_repeats_on = true;
    app.proximity_repeats = (0..500).map(|i| (i * 3, i * 3 + 2)).collect();
    let before = app.buffer.text();

    app.invalidate_revision_highlights();
    assert!(app.style_flags.is_empty(), "style cache dropped");
    assert!(app.proximity_repeats.is_empty(), "proximity cache dropped");
    // Invalidation only truncates the caches — it never rewrites the prose.
    assert_eq!(
        app.buffer.text(),
        before,
        "invalidation left the buffer alone"
    );

    // A second call with the caches already empty is a no-op (the early-out keeps
    // it free in the common mid-burst case).
    app.invalidate_revision_highlights();
    assert!(app.style_flags.is_empty() && app.proximity_repeats.is_empty());
}

#[test]
fn editing_invalidates_the_proximity_cache_until_the_next_idle_tick() {
    // #1035, the proximity-repeat (#1002) side of the same fix: a near-echo's
    // dim-mark spans are cached on idle; an edit shifts their offsets, so the
    // edit path drops them and the idle recompute restores them in place.
    let dir = app_scratch_dir("proximity-edit-invalidates");
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    // A content word echoed within the window — the near repeat #1002 marks.
    fs::write(dir.join("one.md"), "She turned, then he turned.\n").unwrap();
    let mut app = open_test_app(&dir);
    let _dir = TempProject::new(dir);
    app.mode = Mode::Insert;

    app.toggle_proximity_repeats();
    assert!(
        !app.proximity_repeats.is_empty(),
        "the close repeat is cached"
    );

    // Insert at the start: every cached span is now one byte stale.
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    type_str(&mut app, "X");
    assert!(
        app.proximity_repeats.is_empty(),
        "the edit drops the stale proximity cache: {:?}",
        app.proximity_repeats
    );

    // Idle recompute restores the marks at the shifted offsets (still on).
    app.on_idle();
    assert!(
        !app.proximity_repeats.is_empty(),
        "the idle pass recomputes the near repeat at its new offset"
    );
    let text = app.buffer.text();
    assert!(
        app.proximity_repeats.iter().all(|&(_, e)| e <= text.len()),
        "every recomputed span indexes live prose: {:?}",
        app.proximity_repeats
    );
}

#[test]
fn a_quiet_pass_with_nothing_changed_keeps_the_cached_marks() {
    // A cursor move changes nothing the style-flag or proximity scan reads, so the
    // quiet pass leaves both caches alone (marker values stand in for them, so a
    // rescan would visibly replace them). An edit still forces a fresh scan.
    let (mut app, _dir) = adverb_app("flag-quiet-skip");
    app.start_style_flag_sweep(FlagCategory::Adverbs);
    app.toggle_proximity_repeats();
    app.on_idle();
    let marker = vec![(0, 1)];
    app.style_flags.clone_from(&marker);
    app.proximity_repeats.clone_from(&marker);

    app.on_key(key(KeyCode::Right));
    app.on_quiet();
    assert_eq!(app.style_flags, marker, "no style rescan for a cursor move");
    assert_eq!(app.proximity_repeats, marker, "no proximity rescan either");

    type_str(&mut app, "X");
    app.on_quiet();
    let text = app.buffer.text();
    assert_eq!(app.style_flags.len(), 2, "rescanned after the edit");
    assert!(
        app.style_flags
            .iter()
            .all(|&(s, e)| text[s..e].ends_with("ly")),
        "{:?} over {text:?}",
        app.style_flags
    );
}

#[test]
fn a_stopword_change_reaches_the_next_quiet_pass() {
    // The word lists are cached against the manifest's `[analysis]` section; a
    // change there (settings edit, config reload) must rebuild them, so a word
    // made a stopword stops being marked as a near repeat — with the text unchanged.
    let dir = app_scratch_dir("proximity-stopword-change");
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "She turned, then he turned.\n").unwrap();
    let mut app = open_test_app(&dir);
    let _dir = TempProject::new(dir);
    app.toggle_proximity_repeats();
    assert!(
        !app.proximity_repeats.is_empty(),
        "the close repeat is marked"
    );

    app.project.manifest.analysis = Some(Analysis {
        stopwords_add: vec!["turned".into()],
        ..Analysis::default()
    });
    app.on_quiet();
    assert!(
        app.proximity_repeats.is_empty(),
        "a stopword is never a near repeat: {:?}",
        app.proximity_repeats
    );
}
