//! The opt-in background batch bible update (#950): the batch-eligibility plan,
//! the half-price cost estimate, that a submit persists the batch id, and that a
//! reconcile applies a finished batch / surfaces a failed one / leaves a still-
//! processing one / refuses a stale one. Hermetic (#531): no network and no real
//! batch submit — the plan/estimate are pure, and the reconcile handler is driven
//! with injected worker results through a test seam.

use super::*;
use crate::ai::batch::{BatchUpdate, PendingBatch};
use crate::ai::incremental::{Digests, State};
use crate::app::batch_update::BatchTaskResult;
use std::fs;

/// Opt into the background update on a keyed Claude backend so the gate + plan run
/// without touching the network. A dummy key is enough — the tests never submit.
fn enable_batch_update(app: &mut App) {
    app.ai.enabled = true;
    app.ai.backend = crate::ai::BackendKind::Claude;
    app.ai.api_key = Some("sk-test".into());
    app.ai.background_batch_update = true;
    app.ai.cost_threshold = 1000.0; // don't trip the confirm in the plan tests
}

/// A JSON continuity response body — what a finished batch's rebuild carries, so
/// `apply_continuity`'s parse writes `bible/timeline.md`.
fn continuity_json() -> String {
    serde_json::to_string(&cont_with(vec![])).unwrap()
}

// --- The plan + eligibility (#950) ------------------------------------------

#[test]
fn the_plan_is_the_five_trackers_over_every_book() {
    let (mut app, _dir) = series_app("batch-plan");
    enable_batch_update(&mut app);
    // Five trackers × two books, in tracker-then-book order.
    assert_eq!(
        app.batch_update_plan_for_test(),
        vec![
            ("ai:characters".into(), 0),
            ("ai:characters".into(), 1),
            ("ai:continuity".into(), 0),
            ("ai:continuity".into(), 1),
            ("ai:setting".into(), 0),
            ("ai:setting".into(), 1),
            ("ai:plot".into(), 0),
            ("ai:plot".into(), 1),
            ("ai:theme".into(), 0),
            ("ai:theme".into(), 1),
        ]
    );
}

#[test]
fn the_plan_skips_passes_already_in_flight() {
    let (mut app, dir) = two_chapter_app("batch-skip");
    enable_batch_update(&mut app);
    // A continuity batch is already pending for book 0 — the next queueing must not
    // re-submit it (double-billing), so it drops from the plan.
    let mut rec = BatchUpdate::default();
    rec.push(
        &dir,
        PendingBatch {
            batch_id: "id1".into(),
            slug: "ai:continuity".into(),
            book: 0,
            digests: Digests::new(),
            submitted: "2026-06-17".into(),
        },
    )
    .unwrap();
    let plan = app.batch_update_plan_for_test();
    assert!(
        !plan.contains(&("ai:continuity".to_string(), 0)),
        "the in-flight pass is skipped: {plan:?}"
    );
    assert!(plan.contains(&("ai:characters".to_string(), 0)), "{plan:?}");
}

#[test]
fn the_estimate_applies_the_batch_half_price_discount() {
    let (mut app, _dir) = two_chapter_app("batch-est");
    enable_batch_update(&mut app);
    let batched = app.batch_update_estimate_for_test();
    // The same plan priced without the discount is exactly twice — the batch path
    // bills input + output at half (#691), so the confirm quotes the true charge.
    let jobs_chars: usize = {
        // Re-derive the summed input the estimate uses (the whole book per pass).
        // Five trackers over one book, each sending the whole manuscript.
        5 * app.manuscript_text().len()
    };
    let standard = crate::ai::cost::estimate(
        jobs_chars,
        crate::ai::MAX_OUTPUT_TOKENS,
        &app.ai.model,
        false,
    );
    // Batched dollars are (about) half the standard figure; the token count matches.
    assert!(
        batched.dollars > 0.0,
        "a non-empty manuscript has a non-zero estimate"
    );
    assert!(
        (batched.dollars - standard.dollars / 2.0).abs() < standard.dollars * 0.02,
        "batched {} ~= half standard {}",
        batched.dollars,
        standard.dollars
    );
}

// --- Submit persists the batch id (#950) ------------------------------------

#[test]
fn a_submitted_batch_id_is_persisted_and_reconcilable() {
    let (mut app, dir) = two_chapter_app("batch-submit");
    enable_batch_update(&mut app);
    // The worker submitted one batch (a fake backend produced the id); the handler
    // must persist it so a later launch can reconcile it.
    app.apply_batch_results_for_test(vec![BatchTaskResult::Submitted(Ok(PendingBatch {
        batch_id: "batch-xyz".into(),
        slug: "ai:continuity".into(),
        book: 0,
        digests: app.book_digests(0),
        submitted: "2026-06-17".into(),
    }))]);
    let record = BatchUpdate::load(&dir);
    assert_eq!(record.batches.len(), 1);
    assert_eq!(record.batches[0].batch_id, "batch-xyz");
    assert_eq!(record.batches[0].slug, "ai:continuity");
}

// --- Reconcile outcomes (#950) ----------------------------------------------

#[test]
fn a_ready_batch_is_applied_and_dropped_from_the_record() {
    let (mut app, dir) = two_chapter_app("batch-ready");
    enable_batch_update(&mut app);
    let digests = app.book_digests(0);
    // Pre-seed the record so we can prove the reconciled batch is removed.
    let mut rec = BatchUpdate::default();
    rec.push(
        &dir,
        PendingBatch {
            batch_id: "batch-ai:continuity-0".into(),
            slug: "ai:continuity".into(),
            book: 0,
            digests: digests.clone(),
            submitted: "2026-06-17".into(),
        },
    )
    .unwrap();

    app.apply_batch_results_for_test(vec![BatchTaskResult::ready(
        "ai:continuity",
        0,
        digests,
        &continuity_json(),
    )]);

    // The rebuild was applied to the bible…
    assert!(
        dir.join("bible/timeline.md").exists(),
        "the finished batch wrote the timeline"
    );
    // …its incremental state was recorded (so the next pass is scoped)…
    assert!(
        State::load(&dir).built_from("timeline").is_some(),
        "the applied pass recorded its state"
    );
    // …and it's gone from the in-flight record (nothing left → file cleared).
    assert!(
        BatchUpdate::load(&dir).is_empty(),
        "the reconciled batch is dropped"
    );
}

#[test]
fn a_stale_batch_is_refused_not_applied() {
    let (mut app, dir) = two_chapter_app("batch-stale");
    enable_batch_update(&mut app);
    // The batch was built against a manuscript state that no longer matches (a
    // chapter changed while it was in flight) — applying it would clobber a bible
    // the author has moved past. It must be refused, reported, and dropped.
    let stale_digests: Digests = [("one.md".to_string(), "OLD-DIGEST".to_string())]
        .into_iter()
        .collect();
    let mut rec = BatchUpdate::default();
    rec.push(
        &dir,
        PendingBatch {
            batch_id: "batch-ai:continuity-0".into(),
            slug: "ai:continuity".into(),
            book: 0,
            digests: stale_digests.clone(),
            submitted: "2026-06-17".into(),
        },
    )
    .unwrap();

    app.apply_batch_results_for_test(vec![BatchTaskResult::ready(
        "ai:continuity",
        0,
        stale_digests,
        &continuity_json(),
    )]);

    assert!(
        !dir.join("bible/timeline.md").exists(),
        "a stale batch must not overwrite the bible"
    );
    let msg = app.message.clone().unwrap_or_default();
    assert!(
        msg.contains("stale"),
        "the stale skip is surfaced, never silent: {msg:?}"
    );
    assert!(
        BatchUpdate::load(&dir).is_empty(),
        "a refused stale batch is still dropped from the record"
    );
}

#[test]
fn a_failed_batch_is_surfaced_not_swallowed() {
    let (mut app, dir) = two_chapter_app("batch-failed");
    enable_batch_update(&mut app);
    let mut rec = BatchUpdate::default();
    rec.push(
        &dir,
        PendingBatch {
            batch_id: "batch-ai:continuity-0".into(),
            slug: "ai:continuity".into(),
            book: 0,
            digests: Digests::new(),
            submitted: "2026-06-17".into(),
        },
    )
    .unwrap();

    app.apply_batch_results_for_test(vec![BatchTaskResult::failed("ai:continuity", 0, "expired")]);

    let msg = app.message.clone().unwrap_or_default();
    assert!(
        msg.contains("failed") && msg.contains("expired"),
        "a failed batch is surfaced with its reason: {msg:?}"
    );
    assert!(
        !dir.join("bible/timeline.md").exists(),
        "a failed batch writes no bible"
    );
    assert!(
        BatchUpdate::load(&dir).is_empty(),
        "a failed batch is dropped (re-queue to retry)"
    );
}

#[test]
fn a_still_processing_batch_is_left_queued() {
    let (mut app, dir) = two_chapter_app("batch-processing");
    enable_batch_update(&mut app);
    let mut rec = BatchUpdate::default();
    rec.push(
        &dir,
        PendingBatch {
            batch_id: "batch-ai:continuity-0".into(),
            slug: "ai:continuity".into(),
            book: 0,
            digests: Digests::new(),
            submitted: "2026-06-17".into(),
        },
    )
    .unwrap();

    app.apply_batch_results_for_test(vec![BatchTaskResult::processing("ai:continuity", 0)]);

    // Never a false "timed out" (the #955 bug): a processing batch stays queued and
    // nothing is written or reported.
    assert!(!dir.join("bible/timeline.md").exists());
    assert!(
        BatchUpdate::load(&dir).batches.len() == 1,
        "a still-processing batch is left in the record to check again"
    );
}

// --- The gate (#950) --------------------------------------------------------

#[test]
fn the_action_is_gated_on_the_opt_in_and_a_cloud_backend() {
    let (mut app, _dir) = two_chapter_app("batch-gate");
    // Off by default → refused.
    app.ai.enabled = true;
    app.ai.backend = crate::ai::BackendKind::Claude;
    app.ai.api_key = Some("sk-test".into());
    assert!(
        app.batch_update_status().is_err(),
        "off by default without the opt-in"
    );
    // Opt in, but pin local → refused (batch is a cloud-only path).
    app.ai.background_batch_update = true;
    app.ai.backend = crate::ai::BackendKind::Local;
    assert!(
        app.batch_update_status().is_err(),
        "the batch API needs the cloud backend"
    );
    // Opt in on a keyed Claude backend → ready.
    app.ai.backend = crate::ai::BackendKind::Claude;
    assert!(app.batch_update_status().is_ok(), "opt-in + cloud + key");
}

// --- Reconciled Characters batch is scoped to the batch's book (#950/#871) --

/// A characters-pass JSON body naming one character, so a finished Characters batch's
/// rebuild parses and `apply_characters` writes `bible/characters/<slug>.md`.
fn characters_json(name: &str) -> String {
    serde_json::json!({ "characters": [ { "name": name } ] }).to_string()
}

#[test]
fn a_reconciled_characters_batch_enriches_mentions_from_its_own_book_not_the_active_one() {
    // A background Characters batch for book 1 (non-active) is reconciled while book 0
    // is active. The local mention enrichment must count book 1's prose — the batch's
    // book — not the active book's, or the profile records the wrong "screen time".
    let (mut app, dir) = series_app("batch-book-scope");
    enable_batch_update(&mut app);
    assert_eq!(app.project.active_book(), 0, "book 0 is active");

    // Seed a character name that appears only in book 1 (b2/one.md = "Dusk"), never in
    // book 0's chapters — so a book-0-scoped count is 0 and a book-1-scoped count is 3.
    fs::write(
        dir.join("b2/one.md"),
        "Mordecai woke. Mordecai walked. Mordecai slept.\n",
    )
    .unwrap();

    // The batch was built against book 1 as it is now (post-seed), so it isn't stale.
    let digests = app.book_digests(1);
    app.apply_batch_results_for_test(vec![BatchTaskResult::ready(
        "ai:characters",
        1,
        digests,
        &characters_json("Mordecai"),
    )]);

    let profile = fs::read_to_string(dir.join("bible/characters/mordecai.md"))
        .expect("the book-1 characters batch wrote the profile");
    // Mentions are counted from BOOK 1's prose (3), not book 0's (0). Fail-before: the
    // old active-book-scoped enrichment counted book 0 and wrote `mentions: 0`.
    assert!(
        profile.contains("mentions: 3"),
        "mentions come from book 1's chapters, not the active book: {profile}"
    );
    // And the "last seen" chapter is book 1's, never a book-0 title.
    assert!(
        profile.contains("last_chapter: Dusk"),
        "last_chapter is book 1's chapter, not the active book's: {profile}"
    );
    assert!(
        !profile.contains("last_chapter: Dawn") && !profile.contains("last_chapter: Noon"),
        "no book-0 chapter title leaks into a book-1 profile: {profile}"
    );

    // The interactive (active-book) path is unchanged: enrich_mentions outside a run
    // still scopes to the active book.
    let mut c = crate::ai::character::Character {
        name: "alpha".into(),
        ..Default::default()
    };
    app.enrich_mentions(&mut c);
    assert_eq!(
        c.mentions, 2,
        "active-book enrichment still counts book 0's 'alpha one' / 'alpha two'"
    );
}
