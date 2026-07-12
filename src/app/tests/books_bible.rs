//! Series-scoped story bible: book-scoped passes (#339), the series-bible refresh
//! driver (#340), the one-confirm "refresh all bibles" batch (#655), and
//! cross-book findings attribution and jumping (#341).

use super::*;
use crate::project::Project;

// --- Series bible: book-scoped passes (#339) --------------------------------

#[test]
fn book_digests_are_scoped_to_one_book() {
    let (app, _dir) = series_app("book-digests");
    let b0: Vec<String> = app.book_digests(0).into_keys().collect();
    let b1: Vec<String> = app.book_digests(1).into_keys().collect();
    assert_eq!(b0, ["b1/one.md", "b1/two.md"], "book one's files only");
    assert_eq!(b1, ["b2/one.md"], "book two's files only");
}

#[test]
fn folding_a_book_into_the_bible_keeps_the_other_books_state() {
    use crate::ai::incremental::State;
    let (mut app, dir) = series_app("fold-keeps");
    // Book one's character pass already recorded what it was built from.
    State::record(&dir, "characters", "2026-06-24", app.book_digests(0)).unwrap();
    // Folding book two in is a series pass: it merges, not replaces (preserving
    // book one's files).
    app.pending_pass = Some((
        "characters".into(),
        app.book_digests(1),
        Some(app.other_books_files(1)),
    ));
    app.record_pass_state();

    let recorded = State::load(&dir).built_from("characters").unwrap().clone();
    assert!(
        recorded.contains_key("b1/one.md") && recorded.contains_key("b2/one.md"),
        "book one wasn't forgotten when book two folded in: {recorded:?}"
    );
}

/// Book two starts with two chapters; the author deletes one. The next series
/// pass on book two must (a) see the removal and rebuild rather than report the
/// bible current, and (b) prune the deleted chapter from the shared state instead
/// of leaving it stale until a full rebuild (#645) — while book one is untouched.
#[test]
fn deleting_a_series_book_chapter_prunes_its_stale_bible_state() {
    use crate::ai::incremental::{Scope, State};
    let dir = crate::testutil::scratch_dir("books", "prune-deleted");
    for sub in ["b1", "b2"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(dir.join("b1/one.md"), "alpha one\n").unwrap();
    fs::write(dir.join("b2/one.md"), "beta one\n").unwrap();
    fs::write(dir.join("b2/two.md"), "beta two\n").unwrap();
    let two_chapter_book_two = "title = \"The Saga\"\n\n\
         [[books]]\ntitle = \"Book One\"\n\n\
         [[books.chapters]]\ntitle = \"Dawn\"\nfile = \"b1/one.md\"\n\n\
         [[books]]\ntitle = \"Book Two\"\n\n\
         [[books.chapters]]\ntitle = \"Dusk\"\nfile = \"b2/one.md\"\n\n\
         [[books.chapters]]\ntitle = \"Night\"\nfile = \"b2/two.md\"\n";
    fs::write(Project::manifest_path(&dir), two_chapter_book_two).unwrap();
    // A non-empty bible so `plan_bible_pass` scopes off the delta rather than
    // short-circuiting to Full on an empty bible.
    fs::create_dir_all(dir.join("bible/characters")).unwrap();
    fs::write(dir.join("bible/characters/mara.md"), "# Mara\n\nA hero.\n").unwrap();
    let mut app = open_test_app(&dir);

    // Book one recorded; book two (two chapters) folded in beside it (#339).
    State::record(&dir, "characters", "2026-06-24", app.book_digests(0)).unwrap();
    app.pending_pass = Some((
        "characters".into(),
        app.book_digests(1),
        Some(app.other_books_files(1)),
    ));
    app.record_pass_state();
    assert!(
        State::load(&dir)
            .built_from("characters")
            .unwrap()
            .contains_key("b2/two.md"),
        "book two's second chapter was recorded"
    );

    // The author deletes book two's second chapter from disk and the manifest,
    // then reopens on book two.
    fs::remove_file(dir.join("b2/two.md")).unwrap();
    fs::write(
        Project::manifest_path(&dir),
        "title = \"The Saga\"\n\n\
         [[books]]\ntitle = \"Book One\"\n\n\
         [[books.chapters]]\ntitle = \"Dawn\"\nfile = \"b1/one.md\"\n\n\
         [[books]]\ntitle = \"Book Two\"\n\n\
         [[books.chapters]]\ntitle = \"Dusk\"\nfile = \"b2/one.md\"\n",
    )
    .unwrap();
    let mut app = open_test_app(&dir);
    app.switch_book(1);

    // A character pass on book two now sees the removal, so it rebuilds rather
    // than reporting the bible current…
    let t = super::worker::bible_tracker(crate::mode::AiAction::Characters).unwrap();
    let (scope, _prior, _current) = app.plan_bible_pass_for_test(&t, 1);
    assert_eq!(
        scope,
        Scope::Full,
        "a removal forces a rebuild for the book"
    );

    // …and folding the pass in prunes the stale entry, while book one survives.
    app.pending_pass = Some((
        "characters".into(),
        app.book_digests(1),
        Some(app.other_books_files(1)),
    ));
    app.record_pass_state();
    let recorded = State::load(&dir).built_from("characters").unwrap().clone();
    assert!(
        !recorded.contains_key("b2/two.md"),
        "book two's deleted chapter no longer lingers in the state: {recorded:?}"
    );
    assert!(
        recorded.contains_key("b2/one.md"),
        "book two's surviving chapter is kept: {recorded:?}"
    );
    assert!(
        recorded.contains_key("b1/one.md"),
        "book one's entry survives: {recorded:?}"
    );
}

// --- Series bible refresh driver (#340) -------------------------------------

#[test]
fn the_series_bible_refresh_is_gated_off_for_one_book() {
    use crate::mode::Action;
    let (app, _dir) = two_chapter_app("bible-gate");
    assert!(
        app.action_status(Action::RefreshSeriesBible).is_some(),
        "single-book: unavailable"
    );
}

#[test]
fn starting_the_refresh_with_one_book_is_a_no_op() {
    let (mut app, _dir) = two_chapter_app("bible-noop");
    app.start_refresh_series_bible();
    assert!(!app.has_series_bible_run());
    assert!(app.message.as_deref().unwrap_or("").contains("one book"));
}

#[test]
fn a_pass_error_stops_the_refresh() {
    use crate::mode::AiAction;
    let (mut app, _dir) = series_app("bible-error");
    app.seed_series_bible_run(vec![(AiAction::Characters, 0), (AiAction::Characters, 1)]);
    app.advance_run(Some("boom".into()));
    assert!(!app.has_series_bible_run(), "the run stopped");
    assert!(
        app.message.as_deref().unwrap_or("").contains("boom"),
        "{:?}",
        app.message
    );
}

#[test]
fn cancelling_stops_the_refresh_after_the_current_pass() {
    use crate::mode::AiAction;
    let (mut app, _dir) = series_app("bible-cancel");
    app.seed_series_bible_run(vec![(AiAction::Characters, 0), (AiAction::Characters, 1)]);
    app.cancel_run();
    assert!(
        app.has_series_bible_run(),
        "still running until the pass finishes"
    );
    // The in-flight pass completes; the cancel takes effect, later passes skipped.
    app.advance_run(None);
    assert!(!app.has_series_bible_run());
    assert!(
        app.message.as_deref().unwrap_or("").contains("cancelled"),
        "{:?}",
        app.message
    );
}

#[test]
fn a_stuck_series_bible_pass_times_out() {
    use crate::ai;
    use crate::mode::AiAction;
    let (mut app, _dir) = series_app("bible-timeout");
    // A spawned-but-unpolled task keeps `ai_task` busy (a request in flight); the
    // run is backdated past the per-step timeout, so the shared guard (#344) drops it.
    app.ai_task.spawn(|| {
        ai::Outcome::Error(ai::AiError::Network {
            status: None,
            message: "stuck".into(),
        })
    });
    app.seed_series_bible_run(vec![(AiAction::Characters, 0)]);
    app.expire_series_bible_step();
    app.tick_run_timeout();
    assert!(!app.has_series_bible_run(), "the stuck run was stopped");
    assert!(!app.ai_task.is_busy(), "the stuck request was dropped");
    assert!(
        app.message.as_deref().unwrap_or("").contains("timed out"),
        "{:?}",
        app.message
    );
}

// --- Refresh all bibles: one combined confirm (#655) ------------------------

/// Enable AI on the always-ready local backend so the bible-refresh command gets
/// past the readiness gate without a key or network — hermetic (#531).
fn enable_local_ai(app: &mut App) {
    app.ai.enabled = true;
    app.ai.backend = crate::ai::BackendKind::Local;
}

#[test]
fn refresh_all_bibles_plans_the_five_trackers_over_every_book() {
    use crate::mode::AiAction;
    let (app, _dir) = series_app("bible-all-plan");
    let queue = app.bible_refresh_queue_for_test();
    // Five trackers × two books = ten passes, each tracker walking the books in
    // reading order so book two builds on book one.
    assert_eq!(
        queue,
        vec![
            (AiAction::Characters, 0),
            (AiAction::Characters, 1),
            (AiAction::Continuity, 0),
            (AiAction::Continuity, 1),
            (AiAction::Setting, 0),
            (AiAction::Setting, 1),
            (AiAction::Plot, 0),
            (AiAction::Plot, 1),
            (AiAction::Theme, 0),
            (AiAction::Theme, 1),
        ]
    );
}

#[test]
fn refresh_all_bibles_works_for_a_standalone_novel() {
    use crate::mode::AiAction;
    let (app, _dir) = two_chapter_app("bible-all-standalone");
    // A standalone novel refreshes its one book's five trackers under the one
    // command — unlike the series refresh, it isn't gated off for a single book.
    assert_eq!(
        app.bible_refresh_queue_for_test(),
        vec![
            (AiAction::Characters, 0),
            (AiAction::Continuity, 0),
            (AiAction::Setting, 0),
            (AiAction::Plot, 0),
            (AiAction::Theme, 0),
        ]
    );
}

#[test]
fn refresh_all_bibles_estimate_sums_the_five_passes() {
    let (app, _dir) = two_chapter_app("bible-all-sum");
    // With no bible yet, each of the five passes rebuilds from the whole book, so
    // the combined estimate covers five whole-book passes' worth of input — the sum
    // of the parts, one figure to confirm rather than five separate ones.
    let combined = app.bibles_refresh_estimate_for_test(false);
    let whole = app.manuscript_text().len();
    // No bible yet + the Claude backend means each pass is a from-scratch full
    // rebuild, which runs through the Batch API at half rate (#691), so the
    // combined figure is priced batched too.
    let expected =
        crate::ai::cost::estimate(whole * 5, crate::ai::MAX_OUTPUT_TOKENS, &app.ai.model, true);
    assert_eq!(
        combined.tokens, expected.tokens,
        "combined input = five whole-book passes summed"
    );
    // And it's five times a single whole-book pass, up to token truncation.
    let one_pass =
        crate::ai::cost::estimate(whole, crate::ai::MAX_OUTPUT_TOKENS, &app.ai.model, true);
    assert!(
        combined.tokens >= one_pass.tokens * 5,
        "at least five passes' input: {} vs {}",
        combined.tokens,
        one_pass.tokens * 5,
    );
    assert!(
        combined.dollars > one_pass.dollars,
        "the batch costs more than one pass"
    );
}

#[test]
fn one_confirm_gates_the_whole_batch_then_seeds_every_pass() {
    let (mut app, _dir) = series_app("bible-all-confirm");
    enable_local_ai(&mut app);
    app.ai.cost_threshold = 0.0; // any non-empty manuscript is over

    app.start_refresh_bibles(false);
    // Exactly one confirm opens — not four, not 4×books.
    match &app.mode {
        Mode::Prompt(s) => {
            assert!(
                matches!(s.pending, Pending::ConfirmRefreshBibles { .. }),
                "the single combined confirm opens"
            );
            assert!(s.label.contains("Refresh all bibles"), "{}", s.label);
        }
        _ => panic!("expected the combined cost confirm"),
    }
    // Nothing has been seeded or spawned until the author confirms.
    assert!(!app.has_series_bible_run(), "no run before confirm");
    assert!(!app.ai_task.is_busy(), "nothing sent before confirm");

    // Confirming runs the batch as one sequenced run over all ten passes. On this
    // hermetic build the local backend isn't compiled, so every pass is skipped and
    // the run drains synchronously — but it ran as a *single* batch (one summary),
    // never ten separate confirms.
    app.on_key(key(KeyCode::Char('y')));
    assert!(
        !matches!(app.mode, Mode::Prompt(_)),
        "confirming closes the single prompt — no further confirms"
    );
    // The batch that ran was the full ten-pass plan.
    assert_eq!(app.bible_refresh_queue_for_test().len(), 10);
}

#[test]
fn a_cheap_batch_skips_the_confirm() {
    let (mut app, _dir) = series_app("bible-all-cheap");
    enable_local_ai(&mut app);
    app.ai.cost_threshold = 1_000.0; // nothing this small could exceed it
    app.start_refresh_bibles(false);
    assert!(
        !matches!(app.mode, Mode::Prompt(_)),
        "no confirm for a batch under the threshold"
    );
    // It went straight to seeding/driving the batch rather than prompting.
}

#[test]
fn declining_the_batch_confirm_cancels_it() {
    let (mut app, _dir) = series_app("bible-all-decline");
    enable_local_ai(&mut app);
    app.ai.cost_threshold = 0.0;
    app.start_refresh_bibles(false);
    assert!(matches!(app.mode, Mode::Prompt(_)));
    app.on_key(key(KeyCode::Char('n')));
    assert!(!app.has_series_bible_run(), "nothing runs after declining");
    assert!(!app.ai_task.is_busy(), "nothing was sent");
    assert!(
        app.message.as_deref().unwrap_or("").contains("cancelled"),
        "{:?}",
        app.message
    );
}

#[test]
fn refresh_all_bibles_needs_a_ready_backend() {
    let (mut app, _dir) = series_app("bible-all-gate");
    app.ai.enabled = false; // AI off
    app.start_refresh_bibles(false);
    assert!(!app.has_series_bible_run(), "gated off when AI isn't ready");
    assert!(matches!(app.mode, Mode::Insert), "no confirm prompt opens");
}

// --- Cross-book findings (#341) ---------------------------------------------

#[test]
fn series_findings_are_attributed_to_their_book_and_accumulate_silently() {
    use crate::ai::continuity::{Continuity, Finding};
    use crate::mode::AiAction;
    let (mut app, _dir) = series_app("bible-cross");
    let mk = |chapter: &str| Continuity {
        events: vec![],
        findings: vec![Finding {
            kind: continuity::FindingKind::Object,
            detail: "the locket changes color".into(),
            chapters: vec![chapter.into()],
        }],
    };
    // Book one's continuity pass is in flight: its finding is tagged to book 0,
    // accumulated into triage, and must NOT pop a findings overlay mid-refresh.
    app.seed_series_bible_run(vec![(AiAction::Continuity, 0)]);
    app.apply_continuity(Ok(mk("Dawn")));
    assert!(
        !matches!(app.mode, Mode::Findings(_)),
        "no overlay pops mid-refresh"
    );

    // Book two's pass next: an identical-looking finding is a *distinct* one,
    // because it belongs to a different book (titles can repeat across books).
    app.seed_series_bible_run(vec![(AiAction::Continuity, 1)]);
    app.apply_continuity(Ok(mk("Dusk")));
    assert_eq!(app.triage.len(), 2, "both books' findings accumulated");
    assert_eq!(app.triage[0].book, Some(0));
    assert_eq!(app.triage[1].book, Some(1));

    // Re-running book two's pass doesn't double-count its own finding.
    app.apply_continuity(Ok(mk("Dusk")));
    assert_eq!(app.triage.len(), 2, "deduped within a book");
}

#[test]
fn jumping_to_a_cross_book_finding_switches_books() {
    use crate::mode::Finding;
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    let (mut app, _dir) = series_app("bible-jump");
    assert_eq!(app.project.active_book(), 0, "opens on book one");
    // A finding in book two, surfaced in the (session-wide) triage list.
    app.triage.push(Finding {
        summary: "object: the locket".into(),
        kind: crate::mode::FindingKind::Continuity,
        chapters: vec!["Dusk".into()],
        book: Some(1),
        action: None,
        corroborators: Vec::new(),
    });
    app.open_triage();
    assert!(matches!(app.mode, Mode::Findings(_)));
    app.on_key_findings(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    assert_eq!(
        app.project.active_book(),
        1,
        "Enter switched to the finding's book"
    );
    assert!(
        app.buffer.text().contains("beta one"),
        "opened book two's chapter"
    );
}
