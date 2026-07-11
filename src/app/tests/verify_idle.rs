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
fn verifying_re_baselines_so_it_does_not_re_fire_on_the_same_edit() {
    // After the idle tick plans a verification for a changed chapter, that chapter is
    // re-baselined at its current digest, so a second idle tick before the result
    // lands doesn't queue the same re-check again (no busy-loop on one edit, #1053).
    // AI is left disabled so the worker spawn is gated (no live thread / backend);
    // the re-baseline runs first, ahead of the gated spawn, which is what we assert.
    let (mut app, dir) = verify_app("verify-rebaseline");
    fs::write(dir.join("two.md"), "second chapter, changed once\n").unwrap();
    assert_eq!(
        app.verify_on_idle_targets(),
        vec![ui_finding("Two", "sunset happens twice")],
        "the changed chapter is targeted before the idle tick runs"
    );

    app.verify_findings_on_idle(); // re-baselines Two, then the spawn no-ops (AI off)
    assert!(
        !app.ai_busy(),
        "the spawn is gated with AI disabled — this stays hermetic"
    );
    assert!(
        app.verify_on_idle_targets().is_empty(),
        "the changed chapter was re-baselined, so it isn't re-verified again on the next tick"
    );
}
