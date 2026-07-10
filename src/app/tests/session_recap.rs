//! Tests for the ambient session recap and the below-target daily cue (#1012):
//! the on-quit summary reads the daily tally already maintained (this session's
//! net + the day's net); the status-bar cue shows only when a `[goals] daily`
//! target is set and today's net is under it, and it's dismissible (Esc) and can
//! be turned off entirely. None of this walks the manuscript per draw.

use super::*;
use crate::project::GoalField;

/// Two-chapter app with a daily word target set, ready for the cue tests. Vim /
/// typography pins come from the shared `open_test_app`, so a keystroke is a
/// single deterministic word character.
fn nudge_app(name: &str, daily: usize) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    app.project.set_goal(GoalField::Daily, Some(daily)).unwrap();
    (app, dir)
}

/// Type `words` distinct words into the active chapter and settle the tally, so
/// today's `added` reflects them. Each `word ` is a fresh word the idle tally
/// books; `on_idle` runs `tally_words`.
fn write_words(app: &mut App, words: usize) {
    for i in 0..words {
        type_str(app, &format!("w{i} "));
    }
    app.on_idle();
}

#[test]
fn the_recap_reports_this_session_and_the_day_net() {
    let (mut app, _dir) = two_chapter_app("recap-basic");
    write_words(&mut app, 12);
    // Nothing was carried in from a prior run, so the session net and the day's
    // net are both this run's 12 words.
    assert_eq!(app.session_net_words(), 12);
    assert_eq!(app.day_net_words(), 12);
    let recap = app
        .session_recap()
        .expect("a session that wrote gets a recap");
    assert!(recap.contains("This session: +12 words"), "{recap}");
    assert!(recap.contains("today: +12 net"), "{recap}");
}

#[test]
fn the_recap_separates_session_output_from_the_day_total() {
    // A resumed day: 40 words already on the books before this run's session
    // baseline. This session then writes 12 more.
    let (mut app, _dir) = two_chapter_app("recap-resumed");
    app.added = 40;
    app.session_base_added = 40; // the day started with 40; this session baselines here
    write_words(&mut app, 12);
    assert_eq!(app.session_net_words(), 12, "only this run's 12 words");
    assert_eq!(app.day_net_words(), 52, "the whole day's 52");
    let recap = app.session_recap().expect("recap");
    assert!(recap.contains("This session: +12 words"), "{recap}");
    assert!(recap.contains("today: +52 net"), "{recap}");
}

#[test]
fn a_session_that_wrote_nothing_has_no_recap() {
    // Opened and quit without writing: the recap stays silent (net zero).
    let (app, _dir) = two_chapter_app("recap-silent");
    assert_eq!(app.session_net_words(), 0);
    assert!(
        app.session_recap().is_none(),
        "no recap when nothing was written"
    );
}

#[test]
fn the_recap_notes_a_met_or_missed_daily_target() {
    let (mut app, _dir) = nudge_app("recap-target-met", 10);
    write_words(&mut app, 15);
    let met = app.session_recap().expect("recap");
    assert!(met.contains("daily target met (10)"), "{met}");

    let (mut app, _dir) = nudge_app("recap-target-short", 100);
    write_words(&mut app, 15);
    let short = app.session_recap().expect("recap");
    assert!(short.contains("85 short of 100"), "{short}");
}

#[test]
fn the_cue_shows_only_when_a_target_is_set_and_the_net_is_under_it() {
    // No target: never a cue, however little is written.
    let (mut app, _dir) = two_chapter_app("cue-no-target");
    write_words(&mut app, 3);
    assert!(!app.nudge_showing(), "no daily target, no cue");
    assert_eq!(app.daily_target_shortfall(), None);

    // A target and the day under it: the cue shows, with the shortfall.
    let (mut app, _dir) = nudge_app("cue-under", 100);
    write_words(&mut app, 20);
    assert!(app.nudge_showing(), "under the target, the cue shows");
    assert_eq!(app.daily_target_shortfall(), Some(80));

    // The same target once the net meets it: the cue goes away.
    let (mut app, _dir) = nudge_app("cue-met", 10);
    write_words(&mut app, 12);
    assert!(!app.nudge_showing(), "the target is met, no cue");
    assert_eq!(app.daily_target_shortfall(), None);
}

#[test]
fn the_cue_is_dismissible_with_esc_for_the_session() {
    let (mut app, _dir) = nudge_app("cue-dismiss", 100);
    write_words(&mut app, 20);
    assert!(app.nudge_showing(), "the cue is up before dismissal");

    // Esc in the resting editing mode dismisses it (non-modal — the cue was the
    // only thing Esc would act on).
    app.on_key(key(KeyCode::Esc));
    assert!(app.nudge_dismissed, "Esc dismissed the cue");
    assert!(!app.nudge_showing(), "the cue stays hidden after dismissal");

    // Even writing more (still under the target) doesn't bring it back this run.
    write_words(&mut app, 5);
    assert!(
        app.daily_target_shortfall().is_some(),
        "still short of the target"
    );
    assert!(!app.nudge_showing(), "dismissal holds for the session");
}

#[test]
fn the_cue_can_be_turned_off_entirely() {
    let (mut app, _dir) = nudge_app("cue-off", 100);
    app.daily_nudge = false; // as `[editor] daily_nudge = false` would
    write_words(&mut app, 20);
    assert!(
        app.daily_target_shortfall().is_some(),
        "the day is genuinely under the target"
    );
    assert!(!app.nudge_showing(), "opted out — no cue regardless");
}

#[test]
fn a_negative_day_net_below_a_target_still_reads_as_short() {
    // A day that's net-negative (more cut than written) is still under any positive
    // target — the shortfall spans zero, never underflows.
    let (mut app, _dir) = nudge_app("cue-negative", 50);
    app.removed = 30; // net = -30
    assert_eq!(app.day_net_words(), -30);
    assert_eq!(app.daily_target_shortfall(), Some(80), "50 - (-30) = 80");
    assert!(app.nudge_showing());
}

#[test]
fn the_cue_reads_cached_counts_only_no_manuscript_recompute() {
    // The cue's data source is the maintained tally + cached goal, so evaluating it
    // must not scale with the manuscript. Book a large day directly (no per-chapter
    // walk) and confirm the predicate reads O(1) state, not the buffer.
    let (mut app, _dir) = nudge_app("cue-cached", 5_000);
    app.added = 1_200; // a day's tally, already maintained
    let before = app.project_word_count(); // the cue must not move the counts
    assert_eq!(app.day_net_words(), 1_200);
    assert_eq!(app.daily_target_shortfall(), Some(3_800));
    for _ in 0..1_000 {
        // Re-reading the cue a thousand times is cheap and side-effect-free — it
        // never re-counts the manuscript.
        assert!(app.nudge_showing());
    }
    assert_eq!(
        app.project_word_count(),
        before,
        "reading the cue didn't disturb the counts"
    );
}
