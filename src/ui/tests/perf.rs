//! Render performance guard (#792 split). See `docs/performance.md`.

use super::*;
use crate::buffer::{Buffer, Cursor};
use crate::ui::nav_pane::NavPane;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use std::fmt::Write;
use std::time::{Duration, Instant};

use crate::testutil::{perf_budget_ms, perf_budget_us};

/// The budget for the prose and search passes, in µs (CI-widened): a quarter of
/// a frame. They measure ~0.55ms on an M-series laptop; with the per-character
/// whole-list scans this guard was rebuilt to catch, the prose pass took ~45ms.
const PROSE_BUDGET_US: u128 = 4000;

/// Time 30 draws of `app` after one warm-up draw, returning the mean.
fn mean_frame(terminal: &mut Terminal<TestBackend>, app: &App) -> Duration {
    terminal.draw(|f| draw(f, app)).unwrap(); // warm
    let t = Instant::now();
    for _ in 0..30 {
        terminal.draw(|f| draw(f, app)).unwrap();
    }
    t.elapsed() / 30
}

/// Performance guard (run with `cargo test --release -- --ignored
/// --nocapture`). A full frame must render well within one 16ms frame, even
/// for a large chapter shown alongside a many-chapter outline pane — the
/// realistic worst case for per-frame work (the status bar's live word count
/// over the whole buffer, plus a nav row per chapter). If a future change
/// makes rendering re-read files or do per-chapter work proportional to the
/// novel, this catches it.
///
/// The main pass renders a screenful of *unfolded, wrapped prose* from the
/// middle of the chapter, with `*emphasis*` and `**strong**` on every line,
/// a few hundred spell flags, proximity repeats, style-flag hits, and inline
/// annotations (#302) all live — every per-row / per-character styling layer at
/// once, against whole-buffer lists thousands of entries long. A second pass adds
/// an active search with thousands of matches. Both hold a tight budget (well
/// under a frame): the body renderer narrows each list to the row by binary
/// search, and a regression to scanning whole lists per character (what this
/// guard used to miss, because it only ever timed folded headings) costs several
/// times that budget here.
///
/// Focus mode and typewriter scrolling (#301) are both on, over a corpus with no
/// blank lines — the pathological case where the focused "paragraph" spans the
/// whole buffer, so `focus_paragraph`'s line walk does its most work. Folding
/// every section (#975) is timed as its own pass, then a modal, a wheel-scroll
/// offset, and the assistant pane.
#[test]
#[ignore = "release-only perf guard; run via cargo perf"]
fn render_stays_within_a_frame() {
    let titles: Vec<String> = (0..150).map(|i| format!("Chapter {i}")).collect();
    let title_refs: Vec<&str> = titles.iter().map(String::as_str).collect();
    let project = Project::scratch_with(&title_refs);

    let mut text = String::new();
    for i in 0..7500 {
        // A `##` scene heading every 50 lines, so the fold path (#975) has real
        // sections to collapse and the per-row fold lookup is exercised below.
        if i % 50 == 0 {
            let _ = writeln!(text, "## Scene {}", i / 50);
        }
        // A `---` scene break every 100 lines (#908), set off by blank lines so the
        // grammar reads it as a thematic break, not a setext underline. This gives
        // the scene-break divider path real breaks to render — the corpus otherwise
        // had none, so the per-frame break scan + line-lookup allocation went
        // untimed. The cached scene-break lines and the `merge_inserts` fast path
        // must keep the whole thing within the frame budget.
        if i % 100 == 0 && i > 0 {
            text.push_str("\n---\n\n");
        }
        // Emphasis and strong on every line, so every visible row takes the
        // per-character styling path (concealed markers, italic/bold runs).
        text.push_str("The night was *dark* and the road was **very** long indeed.\n");
    }
    let words = text.split_whitespace().count();
    let mut buffer = Buffer::from_text(&text);
    // A chapter epigraph (#518) so the header-block build + the `merge_inserts`
    // combine with the scene-break rows are both exercised under the frame budget;
    // the corpus otherwise had no epigraph, so `merge_inserts` never actually
    // merged and the header deep-clone it used to do went untimed.
    buffer
        .front_matter_mut()
        .set_epigraph_text("Beware the fury of a patient man.");
    buffer.front_matter_mut().set_epigraph_by("Dryden");
    buffer.refresh_syntax();
    let mut app = App::new(buffer, project);
    app.outline_visible = true;
    app.flags.focus_mode = true;
    app.flags.typewriter = true;
    // Live word-goal indicators (#1011) on: a stored chapter goal and a session
    // target, both drawn into the status bar every frame. Each reads a cached
    // count (the manifest goal, the maintained running total) — O(1), no file
    // read — so having them active must not push the frame past budget.
    let active = app.project.active_index();
    app.project.manifest.chapters[active].goal = Some(3000);
    app.begin_session_target("50000");
    // The first-line paragraph indent (#722) classifies each visible logical line
    // (a bounded highlight lookup per row); on here so the render guard covers it.
    app.flags.paragraph_indent = true;
    // A spread of inline annotations (#302) across the whole chapter, plus one
    // long note spanning the middle (so rows far past its start are still
    // tinted), so the per-row annotation lookup is exercised under the budget.
    let len = text.len();
    app.annotations = (0..200)
        .map(|i| (i * len / 200, i * len / 200 + 5))
        .chain([(len / 3, 2 * len / 3)])
        .map(|(start, end)| crate::annotation::Live {
            chapter: "7B".into(),
            start,
            end,
            note: "note".into(),
            replies: Vec::new(),
            resolved: false,
            reviewer: None,
        })
        .collect();
    // Idle-pass results as they'd stand on a long chapter: a few hundred spell
    // flags and proximity repeats spread over the buffer (whole-buffer lists the
    // renderer must narrow per row, never scan per character).
    app.spell_flags = (0..300)
        .map(|i| (i * len / 300, i * len / 300 + 3))
        .collect();
    app.proximity_repeats = (0..600)
        .map(|i| (i * len / 600 + 5, i * len / 600 + 9))
        .collect();
    // An active style-flag sweep (#1001) with a spread of cached hits, so the body
    // render's per-row/char flag scan is timed too. The cache is read, never
    // recomputed per draw — if a change moved the flag computation onto the render
    // path, this guard (over a window-sized buffer) would blow the frame budget.
    app.style_flag_category = Some(crate::analysis::FlagCategory::Adverbs);
    app.style_flags = (0..200)
        .map(|i| (i * len / 200 + 30, i * len / 200 + 36))
        .collect();
    // The cursor mid-chapter, on a prose line (not a heading), so the screen is
    // wrapped prose deep into every list.
    let mid = (app.buffer.line_count() / 2..app.buffer.line_count())
        .find(|&l| app.buffer.line(l).starts_with("The night"))
        .unwrap();
    app.buffer.cursor = Cursor { line: mid, col: 4 };

    let mut terminal = Terminal::new(TestBackend::new(100, 60)).unwrap();
    let dt = mean_frame(&mut terminal, &app);
    println!("render {words}w prose + 150-chapter outline = {dt:?}");
    assert!(
        dt.as_micros() < perf_budget_us(PROSE_BUDGET_US),
        "prose render took {dt:?}"
    );

    // An active find (#144) with thousands of matches: each visible row narrows
    // the match list too, and the current match draws bright.
    app.on_key(KeyEvent::new(KeyCode::Char('f'), KeyModifiers::CONTROL));
    for c in "road".chars() {
        app.on_key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE));
    }
    let matches = match &app.mode {
        Mode::Search(s) => s.matches.len(),
        _ => panic!("find is open"),
    };
    assert!(matches > 3000, "thousands of matches ({matches})");
    let search_dt = mean_frame(&mut terminal, &app);
    println!("render with an active search ({matches} matches) = {search_dt:?}");
    assert!(
        search_dt.as_micros() < perf_budget_us(PROSE_BUDGET_US),
        "search render took {search_dt:?}"
    );
    app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)); // close the find
    assert!(!matches!(app.mode, Mode::Search(_)));

    // Fold every section (#975), so the render's per-row fold lookup and the
    // scroll math's zero-row hidden lines are exercised under the frame budget.
    let headings = app.edit_buffer().syntax().headings().to_vec();
    app.folds.fold_all(&headings);
    app.buffer.cursor = Cursor { line: 0, col: 0 }; // on the first heading
    let folded_dt = mean_frame(&mut terminal, &app);
    println!("render with every section folded = {folded_dt:?}");
    assert!(
        folded_dt.as_millis() < perf_budget_ms(16),
        "folded render took {folded_dt:?}"
    );

    // And again with a modal overlay open (#349): the same editor plus the
    // O(screen) backdrop dim and the shadowed box. Confirms the depth pass
    // doesn't push a full frame past budget.
    app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)); // open the run-action palette
    assert!(matches!(app.mode, Mode::Palette(_)));
    let modal_dt = mean_frame(&mut terminal, &app);
    println!("render with a modal overlay = {modal_dt:?}");
    assert!(
        modal_dt.as_millis() < perf_budget_ms(16),
        "modal render took {modal_dt:?}"
    );

    // And with a read-ahead wheel-scroll offset active (#764): the cursor sits at
    // the top of a long chapter while the view is scrolled a screenful down. This
    // exercises the scroll-clamp walk (#778), which naively counted every visual
    // row from the cursor-driven top to the end of the buffer once per frame —
    // O(document) work while the offset is non-zero, which the offset-0 passes
    // above never hit. The walk is now bounded to ~a screenful past the offset;
    // confirm the full chapter still renders within budget while scrolled.
    app.on_key(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE)); // close the palette
    assert!(!matches!(app.mode, Mode::Palette(_)));
    app.buffer.cursor = Cursor { line: 0, col: 0 }; // pin the cursor at the top
    app.scroll_offset.set(40); // wheel-scrolled a screenful down into the chapter
    let scrolled_dt = mean_frame(&mut terminal, &app);
    assert!(
        app.scroll_offset.get() > 0,
        "the offset stayed clamped to content"
    );
    println!("render with a wheel-scroll offset = {scrolled_dt:?}");
    assert!(
        scrolled_dt.as_millis() < perf_budget_ms(16),
        "scrolled render took {scrolled_dt:?}"
    );

    // And with the assistant pane open beside the editor (#307), holding a long
    // conversation. The pane used to re-wrap every transcript entry (a `format!` +
    // `row_starts` + a per-row `to_string`) for the whole conversation on every
    // frame, then discard the off-screen rows — untimed by the passes above, which
    // never open the pane. The wrapped rows are now cached (rebuilt only on an
    // append/resize/theme change), so an open pane over a long transcript must stay
    // well within the frame budget too.
    app.scroll_offset.set(0);
    app.test_open_assistant_with_entries();
    for i in 0..200 {
        app.test_assistant_append(&format!(
            "Turn {i}: a reply long enough to wrap across the assistant pane's interior \
             width onto several rows, so the per-entry wrap work is real."
        ));
    }
    let assistant_dt = mean_frame(&mut terminal, &app); // warm-up builds the wrap cache
    println!("render with the assistant pane open = {assistant_dt:?}");
    assert!(
        assistant_dt.as_millis() < perf_budget_ms(16),
        "assistant-pane render took {assistant_dt:?}"
    );
}

/// An app over `chapters` chapters with cached counts, the outline pane visible,
/// and the active chapter near the end of the list (so the pane must scroll to
/// it) with a heading outline nested under it.
fn outline_app(chapters: usize) -> App {
    let titles: Vec<String> = (0..chapters).map(|i| format!("Chapter {i}")).collect();
    let title_refs: Vec<&str> = titles.iter().map(String::as_str).collect();
    let mut project = Project::scratch_with(&title_refs);
    for i in 0..chapters {
        project.set_chapter_words(i, 1000 + i);
    }
    let mut text = String::new();
    for i in 0..40 {
        let _ = writeln!(text, "## Scene {i}\n\nThe night was *dark* and long.\n");
    }
    let mut buffer = Buffer::from_text(&text);
    buffer.refresh_syntax();
    let mut app = App::new(buffer, project);
    app.outline_visible = true;
    app.project.set_active(chapters - chapters / 10);
    app
}

/// The fastest of five 30-draw means of the outline pane alone (min-of-N keeps
/// scheduler noise out of the ratio below).
fn pane_time(app: &App) -> Duration {
    let mut terminal = Terminal::new(TestBackend::new(28, 60)).unwrap();
    let active = app.project.active_index();
    let draw_pane = |t: &mut Terminal<TestBackend>| {
        t.draw(|f| NavPane { app, active }.render(f, f.area()))
            .unwrap();
    };
    draw_pane(&mut terminal); // warm
    (0..5)
        .map(|_| {
            let t = Instant::now();
            for _ in 0..30 {
                draw_pane(&mut terminal);
            }
            t.elapsed() / 30
        })
        .min()
        .unwrap()
}

/// Performance guard (`cargo perf`). The outline pane walks a cheap row model to
/// place its scroll and styles only the rows on screen, so its cost barely moves
/// between a novel (150 chapters) and a series (1500) — and a full frame beside a
/// series-sized outline stays well within budget. It used to format a styled
/// `Line` for every chapter on every frame (and never scrolled), so the pane cost
/// grew linearly with the project.
#[test]
#[ignore = "release-only perf guard; run via cargo perf"]
fn outline_pane_is_flat_in_chapter_count() {
    let (small_app, large_app) = (outline_app(150), outline_app(1500));
    let small = pane_time(&small_app);
    let large = pane_time(&large_app);
    println!("outline pane: 150 chapters = {small:?}, 1500 chapters = {large:?}");
    // Scale-independence: 10× the chapters may cost a little (one cheap pass over
    // cached metadata for band grouping and the section total, two row walks),
    // never what a styled row per chapter costs (~66µs → ~190µs here, ~2.9×; now
    // ~1.2×).
    assert!(
        large.as_micros() < small.as_micros() * 3 / 2 + 20,
        "the outline pane grew with the chapter count: {small:?} → {large:?}"
    );

    let mut terminal = Terminal::new(TestBackend::new(100, 60)).unwrap();
    let frame = mean_frame(&mut terminal, &large_app);
    println!("full frame beside a 1500-chapter outline = {frame:?}");
    assert!(
        frame.as_micros() < perf_budget_us(PROSE_BUDGET_US),
        "series-sized outline frame took {frame:?}"
    );
}
