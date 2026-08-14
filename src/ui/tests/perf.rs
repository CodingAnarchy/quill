//! Render performance guard (#792 split). See `docs/performance.md`.

use super::*;
use crate::buffer::{Buffer, Cursor};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{backend::TestBackend, Terminal};
use std::fmt::Write;
use std::time::Instant;

use crate::testutil::perf_budget_ms;

/// Performance guard (run with `cargo test --release -- --ignored
/// --nocapture`). A full frame must render well within one 16ms frame, even
/// for a large chapter shown alongside a many-chapter outline pane — the
/// realistic worst case for per-frame work (the status bar's live word count
/// over the whole buffer, plus a nav row per chapter). If a future change
/// makes rendering re-read files or do per-chapter work proportional to the
/// novel, this catches it.
///
/// Focus mode and typewriter scrolling (#301) are both on here, over a corpus
/// with no blank lines — the pathological case where the focused "paragraph"
/// spans the whole buffer, so `focus_paragraph`'s line walk does its most
/// work — plus a spread of inline annotations (#302) so their per-row/char
/// scan is timed. Confirms none of these adds cost that breaks the frame budget.
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
        text.push_str("The night was dark and the road was very long indeed.\n");
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
    app.focus_mode = true;
    app.typewriter = true;
    // Live word-goal indicators (#1011) on: a stored chapter goal and a session
    // target, both drawn into the status bar every frame. Each reads a cached
    // count (the manifest goal, the maintained running total) — O(1), no file
    // read — so having them active must not push the frame past budget.
    let active = app.project.active_index();
    app.project.manifest.chapters[active].goal = Some(3000);
    app.begin_session_target("50000");
    // The first-line paragraph indent (#722) classifies each visible logical line
    // (a bounded highlight lookup per row); on here so the render guard covers it.
    app.paragraph_indent = true;
    // Fold every section (#975), so the render's per-row fold lookup and the
    // scroll math's zero-row hidden lines are exercised under the frame budget.
    let headings = app.edit_buffer().syntax().headings().to_vec();
    app.folds.fold_all(&headings);
    // A spread of inline annotations (#302) so the per-row/char annotation
    // scan is exercised under the frame budget too.
    app.annotations = (0..50)
        .map(|i| crate::annotation::Live {
            chapter: "7B".into(),
            start: i * 53,
            end: i * 53 + 5,
            note: "note".into(),
            replies: Vec::new(),
            resolved: false,
            reviewer: None,
        })
        .collect();
    // An active style-flag sweep (#1001) with a spread of cached hits, so the body
    // render's per-row/char flag scan is timed too. The cache is read, never
    // recomputed per draw — if a change moved the flag computation onto the render
    // path, this guard (over a window-sized buffer) would blow the frame budget.
    app.style_flag_category = Some(crate::analysis::FlagCategory::Adverbs);
    app.style_flags = (0..200).map(|i| (i * 37, i * 37 + 6)).collect();

    let mut terminal = Terminal::new(TestBackend::new(100, 60)).unwrap();
    terminal.draw(|f| draw(f, &app)).unwrap(); // warm
    let t = Instant::now();
    for _ in 0..30 {
        terminal.draw(|f| draw(f, &app)).unwrap();
    }
    let dt = t.elapsed() / 30;

    println!("render {words}w + 150-chapter outline = {dt:?}");
    assert!(dt.as_millis() < perf_budget_ms(16), "render took {dt:?}");

    // And again with a modal overlay open (#349): the same editor plus the
    // O(screen) backdrop dim and the shadowed box. Confirms the depth pass
    // doesn't push a full frame past budget.
    app.on_key(KeyEvent::new(KeyCode::Char('k'), KeyModifiers::CONTROL)); // open the run-action palette
    assert!(matches!(app.mode, Mode::Palette(_)));
    terminal.draw(|f| draw(f, &app)).unwrap(); // warm
    let t = Instant::now();
    for _ in 0..30 {
        terminal.draw(|f| draw(f, &app)).unwrap();
    }
    let modal_dt = t.elapsed() / 30;
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
    terminal.draw(|f| draw(f, &app)).unwrap(); // warm
    assert!(
        app.scroll_offset.get() > 0,
        "the offset stayed clamped to content"
    );
    let t = Instant::now();
    for _ in 0..30 {
        terminal.draw(|f| draw(f, &app)).unwrap();
    }
    let scrolled_dt = t.elapsed() / 30;
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
    terminal.draw(|f| draw(f, &app)).unwrap(); // warm (builds the wrap cache)
    let t = Instant::now();
    for _ in 0..30 {
        terminal.draw(|f| draw(f, &app)).unwrap();
    }
    let assistant_dt = t.elapsed() / 30;
    println!("render with the assistant pane open = {assistant_dt:?}");
    assert!(
        assistant_dt.as_millis() < perf_budget_ms(16),
        "assistant-pane render took {assistant_dt:?}"
    );
}
