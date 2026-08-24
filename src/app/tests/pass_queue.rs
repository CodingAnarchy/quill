//! The durable multi-pass queue (#923): queue order, the single aggregate cost
//! confirm, per-pass progress, clean cancel, and — the crux — resume after an
//! interrupt/crash rather than losing the passes already done. Hermetic (#531):
//! no network, no model download; the queue state machine is driven with injected
//! work and the durable `bible/.queue.toml` is inspected directly.

use super::*;
use crate::ai;
use crate::ai::incremental::PassQueue;
use crate::mode::AiAction;

/// Enable AI on the always-ready local backend so the queue command clears the
/// readiness gate without a key or network — hermetic (#531).
fn enable_local_ai(app: &mut App) {
    app.ai.enabled = true;
    app.ai.backend = crate::ai::BackendKind::Local;
}

// --- Queue order (#923) -----------------------------------------------------

#[test]
fn the_queue_plans_the_five_trackers_over_every_book() {
    let (app, _dir) = series_app("queue-plan");
    // Five trackers × two books = ten passes, each tracker walking the books in
    // reading order so book two builds on book one — the same suite as the
    // all-bibles refresh, but as a durable queue.
    assert_eq!(
        app.pass_queue_plan_for_test(),
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
fn the_queue_works_for_a_standalone_novel() {
    let (app, _dir) = two_chapter_app("queue-standalone");
    assert_eq!(
        app.pass_queue_plan_for_test(),
        vec![
            (AiAction::Characters, 0),
            (AiAction::Continuity, 0),
            (AiAction::Setting, 0),
            (AiAction::Plot, 0),
            (AiAction::Theme, 0),
        ]
    );
}

// --- One aggregate cost confirm (#923) --------------------------------------

#[test]
fn the_queue_estimate_sums_the_passes_into_one_figure() {
    let (app, _dir) = two_chapter_app("queue-sum");
    // No bible yet → each of the five passes rebuilds the whole book, so the queue's
    // combined estimate is five whole-book passes summed — one figure to confirm,
    // not five (or 5×books) separate ones.
    let combined = app.pass_queue_estimate_for_test(false);
    let whole = app.manuscript_text().len();
    let expected =
        crate::ai::cost::estimate(whole * 5, crate::ai::MAX_OUTPUT_TOKENS, &app.ai.model, true);
    assert_eq!(
        combined.tokens, expected.tokens,
        "combined input = five whole-book passes summed"
    );
}

#[test]
fn one_confirm_gates_the_whole_queue() {
    let (mut app, _dir) = series_app("queue-confirm");
    enable_local_ai(&mut app);
    app.ai.cost_threshold = 0.0; // any non-empty manuscript is over the threshold

    app.start_queue_passes(false);
    // Exactly one confirm opens — not ten.
    match &app.mode {
        Mode::Prompt(s) => {
            assert!(
                matches!(s.pending, Pending::ConfirmQueuePasses { .. }),
                "the single aggregate confirm opens"
            );
            assert!(s.label.contains("Queue"), "{}", s.label);
        }
        _ => panic!("expected the aggregate cost confirm"),
    }
    // Nothing runs until the author confirms.
    assert!(!app.has_pass_queue_run(), "no run before confirm");
    assert!(!app.ai_task.is_busy(), "nothing sent before confirm");
}

// --- Per-pass progress (#923) -----------------------------------------------

#[test]
fn the_status_shows_per_pass_chapter_progress() {
    let (mut app, _dir) = series_app("queue-progress");
    // A spawned-but-unpolled task keeps a pass "in flight" so the run parks and the
    // progress line renders — no backend needed.
    app.ai_task.spawn(|| {
        ai::Outcome::Error(ai::AiError::Network {
            status: None,
            message: "held".into(),
        })
    });
    app.seed_pass_queue_run(vec![(AiAction::Continuity, 0), (AiAction::Setting, 1)]);
    // Stand in for the fold worker: publish "chapter 5/12" into the shared cell.
    app.set_pass_progress_for_test(Some((5, 12)));
    app.show_pass_queue_progress_for_test(AiAction::Continuity, 0);
    let msg = app.message.clone().unwrap_or_default();
    assert!(
        msg.contains("timeline") && msg.contains("chapter 5/12") && msg.contains("1/2"),
        "per-pass chapter progress and queue position: {msg}"
    );
}

// --- Clean cancel (#923) ----------------------------------------------------

#[test]
fn cancelling_stops_the_queue_after_the_current_pass() {
    let (mut app, _dir) = series_app("queue-cancel");
    app.seed_pass_queue_run(vec![(AiAction::Characters, 0), (AiAction::Characters, 1)]);
    app.cancel_run();
    assert!(
        app.has_pass_queue_run(),
        "still running until the in-flight pass finishes"
    );
    app.advance_run(None); // the in-flight pass completes; the cancel takes effect
    assert!(!app.has_pass_queue_run());
    assert!(
        app.message.as_deref().unwrap_or("").contains("cancelled"),
        "{:?}",
        app.message
    );
}

#[test]
fn a_pass_error_stops_the_queue() {
    let (mut app, _dir) = series_app("queue-error");
    app.seed_pass_queue_run(vec![(AiAction::Characters, 0), (AiAction::Characters, 1)]);
    app.advance_run(Some("boom".into()));
    assert!(!app.has_pass_queue_run(), "the run stopped");
    assert!(
        app.message.as_deref().unwrap_or("").contains("boom"),
        "{:?}",
        app.message
    );
}

#[test]
fn a_stuck_queue_pass_times_out() {
    let (mut app, _dir) = series_app("queue-timeout");
    app.ai_task.spawn(|| {
        ai::Outcome::Error(ai::AiError::Network {
            status: None,
            message: "stuck".into(),
        })
    });
    app.seed_pass_queue_run(vec![(AiAction::Characters, 0)]);
    app.expire_pass_queue_step();
    app.tick_run_timeout();
    assert!(!app.has_pass_queue_run(), "the stuck run was stopped");
    assert!(!app.ai_task.is_busy(), "the stuck request was dropped");
    assert!(
        app.message.as_deref().unwrap_or("").contains("timed out"),
        "{:?}",
        app.message
    );
}

// --- Resume after an interrupt (#923) ---------------------------------------

#[test]
fn completing_a_pass_persists_the_cursor_so_a_crash_resumes() {
    let (mut app, dir) = series_app("queue-persist");
    app.seed_pass_queue_run(vec![
        (AiAction::Characters, 0),
        (AiAction::Continuity, 0),
        (AiAction::Setting, 0),
    ]);
    // The whole run persists at `done = 0` up front (crash-safe before the first
    // pass even lands).
    assert_eq!(
        app.pass_queue_done(),
        Some(0),
        "queue persisted before any pass"
    );

    // Inject work so the second pass parks (rather than draining synchronously on
    // this hermetic build): the first pass "completes", the cursor advances and is
    // persisted, then the run suspends on the held second pass.
    app.ai_task.spawn(|| {
        crate::ai::Outcome::Error(crate::ai::AiError::Network {
            status: None,
            message: "held".into(),
        })
    });
    app.advance_run(None); // pass 0 landed cleanly → advance to pass 1

    // The durable cursor recorded the completed pass, and the queue file still
    // exists (two passes remain) — a crash here loses nothing already done.
    assert_eq!(
        app.pass_queue_done(),
        Some(1),
        "the completed pass is recorded on disk"
    );
    let on_disk = PassQueue::load(&dir).expect("a resumable queue file");
    assert_eq!(
        on_disk.remaining().len(),
        2,
        "the two unfinished passes survive to resume"
    );
    assert_eq!(on_disk.remaining()[0].slug, AiAction::Continuity.slug());
}

#[test]
fn resuming_picks_up_from_the_cursor_not_the_top() {
    let (mut app, dir) = series_app("queue-resume");
    // A crashed run left a queue at the second pass (one already done).
    let mut queue = PassQueue::new(
        app.pass_queue_plan_for_test()
            .into_iter()
            .map(|(a, book)| crate::ai::incremental::QueuedPass {
                slug: a.slug().to_string(),
                book,
            })
            .collect(),
        false,
    );
    queue.complete_current(&dir).unwrap(); // pass 0 done before the "crash"
    queue.save(&dir).unwrap();
    enable_local_ai(&mut app);

    // Inject a held (spawned-but-unpolled) task so the resumed run parks on the pass
    // at the cursor rather than draining — deterministic in both build configs. (On
    // the default build a Local pass would skip for want of a compiled backend and
    // the run would drain; under `--features local` it would spawn real inference and
    // park. Holding a task here makes the run park identically either way, so the
    // assertion tests the resume invariant, not the backend that happens to be built.)
    app.ai_task.spawn(|| {
        crate::ai::Outcome::Error(crate::ai::AiError::Network {
            status: None,
            message: "held".into(),
        })
    });

    // A fresh launch offers to resume, and confirming rehydrates from the cursor.
    app.offer_pass_queue_resume();
    match &app.mode {
        Mode::Prompt(s) => assert!(
            matches!(s.pending, Pending::ResumePassQueue),
            "the resume offer opens"
        ),
        _ => panic!("expected the resume offer"),
    }
    app.on_key(key(KeyCode::Char('y')));
    // The run rehydrated from the cursor: it picks up on the SECOND pass in the plan
    // (characters · book 1, since a series runs each tracker across both books), not
    // the top — the completed first pass (characters · book 0) was NOT re-run.
    assert!(app.has_pass_queue_run(), "the resumed run is active");
    assert_eq!(
        app.pass_queue_current_for_test(),
        Some((AiAction::Characters, 1)),
        "resume picks up from the cursor (the second pass), not the top"
    );
    // The durable cursor still records the one completed pass — resume never rewound
    // it, so a second crash here would still lose nothing already done.
    assert_eq!(app.pass_queue_done(), Some(1), "the cursor wasn't rewound");
}

#[test]
fn discarding_the_resume_offer_clears_the_queue() {
    let (mut app, dir) = series_app("queue-discard");
    PassQueue::new(
        vec![crate::ai::incremental::QueuedPass {
            slug: AiAction::Characters.slug().to_string(),
            book: 0,
        }],
        false,
    )
    .save(&dir)
    .unwrap();
    app.offer_pass_queue_resume();
    app.on_key(key(KeyCode::Esc)); // discard
    assert!(
        PassQueue::load(&dir).is_none(),
        "discarding removes the persisted queue"
    );
    assert!(!app.has_pass_queue_run());
}
