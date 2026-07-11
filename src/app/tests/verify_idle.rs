//! Verify open findings on idle (#1053): the opt-in auto-recheck that re-runs only
//! the findings whose chapters changed since they were generated, scoped to those
//! chapters, and reconciles the result exactly as a manual re-check-all does.
//!
//! Hermetic (#531): no live backend or network anywhere. The plan (which findings a
//! change marks for re-verify, and that the request is scoped to only the changed
//! chapter) is asserted directly; the reconcile is driven through the same
//! `pending_recheck_all` + `apply_recheck_all` path the worker delivers into, with
//! a mock response, so a resolved finding clears and an unchanged one is untouched.

use super::*;
use std::fs;

/// A continuity UI finding on `chapter`, tagged to the continuity pass so the
/// idle verification can re-run it. Mirrors `apply_continuity`'s summary shape so a
/// re-parse of the same content matches it by identity.
fn ui_finding(chapter: &str, detail: &str) -> crate::mode::Finding {
    crate::mode::Finding {
        summary: format!("object: {detail}"),
        kind: crate::mode::FindingKind::Continuity,
        chapters: vec![chapter.into()],
        book: None,
        action: Some(crate::mode::AiAction::Continuity),
        corroborators: Vec::new(),
    }
}

/// A two-chapter project with the opt-in on, two open continuity findings (one per
/// chapter) seeded into triage, and the verify baselines recorded at the chapters'
/// current text — the standing state a revision pass leaves behind.
fn verify_app(name: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    app.ai.verify_on_idle = true;
    let seeded = vec![
        ui_finding("One", "the locket changes color"),
        ui_finding("Two", "sunset happens twice"),
    ];
    // accumulate_triage records the baselines for these findings' chapters (#1053).
    app.accumulate_triage(&seeded);
    (app, dir)
}

#[test]
fn opt_in_off_records_no_baselines_and_verifies_nothing() {
    // Off by default: with the opt-in unset, a pass that accumulates findings
    // records no baselines, and the idle plan targets nothing — no re-check ever.
    let (mut app, _dir) = two_chapter_app("verify-off");
    app.ai.verify_on_idle = false;
    app.accumulate_triage(&[ui_finding("One", "the locket changes color")]);

    // Even after a real edit to the chapter, nothing is marked for re-verify.
    fs::write(_dir.join("one.md"), "first chapter, rewritten\n").unwrap();
    assert!(
        app.verify_on_idle_targets().is_empty(),
        "opt-in off → no finding is ever re-verified"
    );
    // And the idle tick itself spawns nothing (no request in flight).
    app.verify_findings_on_idle();
    assert!(!app.ai_busy(), "opt-in off → the idle tick is a no-op");
}

#[test]
fn editing_a_chapter_marks_only_its_findings_for_reverify() {
    let (app, dir) = verify_app("verify-scope-changed");
    // The author revises chapter Two on disk (it's not the active chapter, so this
    // is a plain content change, not an active-buffer edit). One is untouched.
    fs::write(dir.join("two.md"), "second chapter, heavily revised\n").unwrap();

    let targets = app.verify_on_idle_targets();
    assert_eq!(
        targets,
        vec![ui_finding("Two", "sunset happens twice")],
        "only the changed chapter's finding is marked for re-verify: {targets:?}"
    );
}

#[test]
fn unchanged_chapters_are_never_re_verified() {
    // Nothing changed since the findings were baselined → the plan is empty, so an
    // idle tick does no re-analysis at all (the common case during a reading pause).
    let (app, _dir) = verify_app("verify-none-changed");
    assert!(
        app.verify_on_idle_targets().is_empty(),
        "no chapter changed → nothing to re-verify"
    );
    assert!(
        app.verify_on_idle_requests().is_empty(),
        "and no scoped request is built"
    );
}

#[test]
fn the_re_check_is_scoped_to_only_the_changed_chapter() {
    // No whole-manuscript re-analysis: when Two changes, the built request carries
    // Two's prose and not One's — exactly the scoping a manual re-check does (#830).
    let (app, dir) = verify_app("verify-scope-request");
    fs::write(dir.join("two.md"), "second chapter, revised\n").unwrap();

    let reqs = app.verify_on_idle_requests();
    assert_eq!(
        reqs.len(),
        1,
        "one scoped request for the one changed chapter"
    );
    assert!(
        reqs[0].cached_context.contains("# Two") && reqs[0].cached_context.contains("revised"),
        "the changed chapter is in the request: {}",
        reqs[0].cached_context
    );
    assert!(
        !reqs[0].cached_context.contains("first chapter"),
        "the unchanged chapter is not sent (no whole-manuscript sweep): {}",
        reqs[0].cached_context
    );
}

#[test]
fn a_resolved_finding_is_cleared_and_a_still_flagged_one_kept() {
    // The full idle cycle, reconciled through the same apply path the worker uses:
    // Two was edited and the fix resolved it (the fresh run no longer produces it),
    // while One (unchanged, not even re-run) stays. Drive apply_recheck_all with a
    // mock combined response for the verifying set.
    let (mut app, dir) = verify_app("verify-reconcile");
    fs::write(dir.join("two.md"), "second chapter, fixed\n").unwrap();
    app.open_triage();

    let verifying = app.verify_on_idle_targets();
    assert_eq!(verifying, vec![ui_finding("Two", "sunset happens twice")]);

    // The scoped re-run of Two produces no findings (the fix held). Reconcile it the
    // same way a landed RecheckAll outcome would.
    app.pending_recheck_all = Some(verifying);
    app.apply_recheck_all(Ok(Vec::new()));

    assert!(
        !app.triage
            .contains(&ui_finding("Two", "sunset happens twice")),
        "the resolved finding is cleared from triage"
    );
    assert!(
        app.triage
            .contains(&ui_finding("One", "the locket changes color")),
        "the unchanged chapter's finding is untouched"
    );
}

#[test]
fn a_still_flagged_finding_survives_the_reconcile() {
    let (mut app, dir) = verify_app("verify-still-flagged");
    fs::write(dir.join("two.md"), "second chapter, still broken\n").unwrap();
    app.open_triage();

    let verifying = app.verify_on_idle_targets();
    // The scoped re-run re-produces the same finding: the edit didn't resolve it.
    let fresh = vec![ui_finding("Two", "sunset happens twice")];
    app.pending_recheck_all = Some(verifying);
    app.apply_recheck_all(Ok(fresh));

    assert!(
        app.triage
            .contains(&ui_finding("Two", "sunset happens twice")),
        "a still-flagged finding is kept (reconcile, not blind clear)"
    );
}

#[test]
fn a_finding_being_actively_edited_is_not_clobbered() {
    // The author is mid-edit on the active chapter (One), unsaved. Its finding's
    // digest is in flux, so verify-on-idle leaves it alone — reconcile, don't
    // overwrite a finding they're still working (#1053).
    let (mut app, _dir) = verify_app("verify-active-edit");
    // Type into the active chapter (One): its buffer is now dirty and its text (and
    // digest) has changed — but we must NOT re-verify it while the edit is in flight.
    type_str(&mut app, "revising ");
    assert!(app.edit_dirty(), "the active chapter is mid-edit");

    let targets = app.verify_on_idle_targets();
    assert!(
        !targets.contains(&ui_finding("One", "the locket changes color")),
        "the finding on the chapter being actively edited is left alone: {targets:?}"
    );
}

#[test]
fn a_gated_out_spawn_does_not_advance_the_baseline_so_the_recheck_survives() {
    // BUG 1 (#1053): the idle tick must NOT re-baseline a changed chapter when the
    // spawn gates out (backend not ready — here AI is disabled, so `spawn_outcome`
    // returns early without spawning). If it did, the baseline would advance to the
    // current digest, the next tick would see no change, and the stale finding would
    // never be re-verified until the author edited that chapter *again* — an edit
    // that resolved a finding would silently never clear it.
    //
    // Fail-before / pass-after: before the fix the baseline advanced ahead of the
    // gated spawn and this target went empty; after the fix the baseline is left
    // alone, so the finding is still queued for re-check the moment a backend is
    // ready.
    let (mut app, dir) = verify_app("verify-gated-keeps-target");
    fs::write(dir.join("two.md"), "second chapter, changed once\n").unwrap();
    assert_eq!(
        app.verify_on_idle_targets(),
        vec![ui_finding("Two", "sunset happens twice")],
        "the changed chapter is targeted before the idle tick runs"
    );

    // AI is off (the two-chapter fixture configures no backend), so the spawn is
    // gated: `spawn_outcome` returns early and nothing goes in flight.
    app.verify_findings_on_idle();
    assert!(
        !app.ai_busy(),
        "the spawn is gated with the backend not ready — this stays hermetic"
    );
    // The baseline was NOT advanced, so the finding is STILL marked for re-verify:
    // once a backend is ready the re-check runs. (Pre-fix: this was empty — the
    // re-check was permanently dropped.)
    assert_eq!(
        app.verify_on_idle_targets(),
        vec![ui_finding("Two", "sunset happens twice")],
        "a gated-out pass leaves the baseline untouched → the re-check is not lost"
    );
}

#[test]
fn a_successful_run_re_baselines_the_verified_chapters_exactly_once() {
    // The success-path contract of the BUG 1 fix (#1053): when the pass *does* run,
    // the verified chapters are re-baselined to their current digest exactly once, so
    // a second idle tick before the result lands doesn't queue the same re-check
    // again (no busy-loop on one edit; the reconcile that lands later is the source of
    // truth). We can't spawn a live worker hermetically (#531), so we drive the
    // re-baseline the fix performs on a successful spawn directly — exactly as the
    // reconcile tests drive `apply_recheck_all` directly rather than through a worker.
    let (mut app, dir) = verify_app("verify-success-rebaseline");
    fs::write(dir.join("two.md"), "second chapter, changed once\n").unwrap();
    let verifying = app.verify_on_idle_targets();
    assert_eq!(
        verifying,
        vec![ui_finding("Two", "sunset happens twice")],
        "the changed chapter is targeted"
    );

    // The successful-spawn branch re-baselines each verifying finding's chapters.
    for finding in &verifying {
        app.baseline_finding_chapters_for_test(finding);
    }
    assert!(
        app.verify_on_idle_targets().is_empty(),
        "after a successful run re-baselines Two, a second tick doesn't re-fire on the same edit"
    );

    // Re-baselining is idempotent for an unchanged chapter — a third pass over the
    // same content re-checks nothing, so there's no double re-check.
    for finding in &verifying {
        app.baseline_finding_chapters_for_test(finding);
    }
    assert!(
        app.verify_on_idle_targets().is_empty(),
        "re-baselining an unchanged chapter is a no-op (no double re-check)"
    );

    // A fresh edit after the re-baseline is still detected — the baseline tracks the
    // live content, it isn't frozen.
    fs::write(dir.join("two.md"), "second chapter, changed AGAIN\n").unwrap();
    assert_eq!(
        app.verify_on_idle_targets(),
        vec![ui_finding("Two", "sunset happens twice")],
        "a later edit past the new baseline is re-detected"
    );
}

/// A series where books 0 and 1 each hold a chapter that resolves to the SAME
/// root-level file string `shared.md` (subdirs are optional, so a series author can
/// name files identically across books). A third book is the active one, so books 0
/// and 1 both read `shared.md` from disk — the exact surface where a file-only
/// baseline key (BUG 2) lets one book's baseline clobber the other's. The opt-in is
/// on. Book 0's finding is on `shared.md` in book 0; book 1's is on `shared.md` in
/// book 1; the active book has its own chapter, untouched by the test.
fn shared_filename_series(name: &str) -> (App, TempProject) {
    let dir = crate::testutil::scratch_dir("books", name);
    fs::write(dir.join("shared.md"), "shared chapter, v1\n").unwrap();
    fs::write(dir.join("active.md"), "the active book's chapter\n").unwrap();
    fs::write(
        Project::manifest_path(&dir),
        "title = \"The Saga\"\n\n\
         [[books]]\ntitle = \"Active Book\"\n\n\
         [[books.chapters]]\ntitle = \"Live\"\nfile = \"active.md\"\n\n\
         [[books]]\ntitle = \"Book One\"\n\n\
         [[books.chapters]]\ntitle = \"Alpha\"\nfile = \"shared.md\"\n\n\
         [[books]]\ntitle = \"Book Two\"\n\n\
         [[books.chapters]]\ntitle = \"Beta\"\nfile = \"shared.md\"\n",
    )
    .unwrap();

    let mut app = open_test_app(&dir);
    app.ai.verify_on_idle = true;
    (app, TempProject::new(dir))
}

/// A finding on `chapter` attributed to `book` — so its baseline is keyed under the
/// right book (#1053).
fn book_finding(book: usize, chapter: &str, detail: &str) -> crate::mode::Finding {
    crate::mode::Finding {
        book: Some(book),
        ..ui_finding(chapter, detail)
    }
}

#[test]
fn a_baseline_is_keyed_by_book_so_a_series_shared_filename_does_not_collide() {
    // BUG 2 (#1053): `verify_baselines` is keyed by (book, file), not the file alone,
    // so two books that hold a same-named root chapter file don't clobber each other's
    // baselines. Books 1 and 2 (0-indexed) both point at `shared.md`. Book 1's finding
    // is baselined while `shared.md` reads "v1"; then `shared.md` becomes "v2" and
    // book 2's finding is baselined. Pre-fix (file-only key), book 2's "v2" baseline
    // OVERWRITES book 1's "v1" under the same `shared.md` key — so book 1's finding,
    // which should now read as changed (v1 → v2), is silently seen as unchanged and
    // never re-checked. Post-fix each book keeps its own baseline.
    let (mut app, dir) = shared_filename_series("verify-book-key-collision");
    assert_eq!(app.project.active_book(), 0, "the active book is book 0");

    let f1 = book_finding(1, "Alpha", "the sword is described twice");
    let f2 = book_finding(2, "Beta", "the map is upside down");

    // t0: `shared.md` is "v1". Baseline book 1's finding here → (1, shared.md) = v1.
    app.accumulate_triage(std::slice::from_ref(&f1));

    // t1: the shared file is revised on disk, then book 2's finding is baselined at
    // the NEW content → (2, shared.md) = v2. Pre-fix this insert clobbers book 1's.
    fs::write(dir.join("shared.md"), "shared chapter, v2\n").unwrap();
    app.accumulate_triage(std::slice::from_ref(&f2));

    let targets = app.verify_on_idle_targets();
    // Book 1's finding was baselined at v1; the file is now v2 → it MUST be re-checked.
    // Pre-fix it was dropped (its baseline was clobbered to v2 by book 2). This is the
    // fail-before / pass-after assertion for the key collision.
    assert!(
        targets.contains(&f1),
        "book 1's finding IS re-checked — its baseline (v1) differs from the file (v2), \
         and book 2 did not clobber it: {targets:?}"
    );
    // Book 2's finding was baselined at v2 and the file is still v2 → not stale.
    assert!(
        !targets.contains(&f2),
        "book 2's finding is unchanged against its own baseline: {targets:?}"
    );
}
