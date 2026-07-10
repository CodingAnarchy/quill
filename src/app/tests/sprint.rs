//! Tests for writing sprints (#299): starting / stopping a timed session, the
//! live word delta, availability, and the event-loop auto-finish. The time-bound
//! cases use a back-dated sprint so they don't depend on the wall clock.

use super::*;
use crate::mode::Action;

#[test]
fn the_start_action_opens_the_duration_prompt() {
    let (mut app, _dir) = two_chapter_app("sprint-start-action");
    app.run_action(Action::StartSprint, false);
    assert!(matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::SprintDuration)));
}

#[test]
fn choosing_a_length_starts_a_running_sprint() {
    let (mut app, _dir) = two_chapter_app("sprint-begin");
    app.begin_sprint("25");
    assert!(app.sprint.is_some(), "a sprint is now running");
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("Sprint started"));
}

#[test]
fn a_non_number_length_is_rejected() {
    let (mut app, _dir) = two_chapter_app("sprint-bad-len");
    app.begin_sprint("soon");
    assert!(
        app.sprint.is_none(),
        "no sprint starts for an unparseable length"
    );
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("whole minutes"));
}

#[test]
fn the_word_count_tracks_writing_since_the_start() {
    let (mut app, _dir) = two_chapter_app("sprint-words");
    app.begin_sprint("25");
    let before = app.sprint.as_ref().unwrap().words(app.project_word_count());
    type_str(&mut app, "three more words");
    let after = app.sprint.as_ref().unwrap().words(app.project_word_count());
    assert!(
        after > before,
        "words written rises as the author types: {before} → {after}"
    );
}

#[test]
fn stopping_reports_the_result_and_clears_the_sprint() {
    let (mut app, _dir) = two_chapter_app("sprint-stop");
    app.begin_sprint("25");
    app.stop_sprint();
    assert!(app.sprint.is_none(), "the sprint is cleared");
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("Sprint stopped"));
}

#[test]
fn stopping_with_no_sprint_says_so() {
    let (mut app, _dir) = two_chapter_app("sprint-stop-none");
    app.stop_sprint();
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("No sprint is running"));
}

#[test]
fn availability_reflects_whether_a_sprint_is_running() {
    let (mut app, _dir) = two_chapter_app("sprint-status");
    assert!(
        app.action_status(Action::StartSprint).is_none(),
        "can start when idle"
    );
    assert!(
        app.action_status(Action::StopSprint).is_some(),
        "nothing to stop when idle"
    );
    app.begin_sprint("25");
    assert!(
        app.action_status(Action::StartSprint).is_some(),
        "can't start a second sprint"
    );
    assert!(
        app.action_status(Action::StopSprint).is_none(),
        "can stop the running one"
    );
}

#[test]
fn the_event_loop_tick_finishes_an_elapsed_sprint() {
    let (mut app, _dir) = two_chapter_app("sprint-finish");
    app.sprint = Some(crate::app::sprint::test_finished_sprint(
        app.project_word_count(),
    ));
    app.tick_sprint();
    assert!(
        app.sprint.is_none(),
        "an elapsed sprint ends on the next tick"
    );
    assert!(app.message.as_deref().unwrap_or("").contains("Sprint over"));
}

#[test]
fn a_running_sprint_does_not_finish_early() {
    let (mut app, _dir) = two_chapter_app("sprint-running");
    app.begin_sprint("25");
    app.tick_sprint();
    assert!(
        app.sprint.is_some(),
        "a fresh sprint keeps running across ticks"
    );
}

// --- Per-session word target (#1011) ---

#[test]
fn the_target_action_opens_the_word_prompt() {
    let (mut app, _dir) = two_chapter_app("target-action");
    app.run_action(Action::SetSessionTarget, false);
    assert!(matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::SessionTarget)));
}

#[test]
fn choosing_a_count_sets_a_running_target() {
    let (mut app, _dir) = two_chapter_app("target-begin");
    app.begin_session_target("500");
    assert!(app.session_target.is_some(), "a target is now set");
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("Session target set"));
}

#[test]
fn a_non_number_target_is_rejected() {
    let (mut app, _dir) = two_chapter_app("target-bad");
    app.begin_session_target("lots");
    assert!(app.session_target.is_none(), "no target for bad input");
    assert!(app.message.as_deref().unwrap_or("").contains("whole words"));
}

#[test]
fn a_blank_target_clears_any_running_one() {
    let (mut app, _dir) = two_chapter_app("target-clear");
    app.begin_session_target("500");
    assert!(app.session_target.is_some());
    app.begin_session_target("   ");
    assert!(app.session_target.is_none(), "blank clears the target");
    assert!(app.message.as_deref().unwrap_or("").contains("cleared"));
}

#[test]
fn the_indicator_fills_from_cached_counts_as_words_are_written() {
    let (mut app, _dir) = two_chapter_app("target-fills");
    app.begin_session_target("500");
    let before = app
        .session_target
        .as_ref()
        .unwrap()
        .written(app.project_word_count());
    assert_eq!(before, 0, "the indicator starts empty");
    type_str(&mut app, "three more words here now");
    let after = app
        .session_target
        .as_ref()
        .unwrap()
        .written(app.project_word_count());
    assert!(after > before, "the indicator fills as words are added");
}

#[test]
fn removing_words_does_not_fill_the_indicator_backwards() {
    let (mut app, _dir) = two_chapter_app("target-cut");
    type_str(&mut app, "some words to cut back down");
    app.begin_session_target("500"); // baseline after the writing
    let baseline = app.project_word_count();
    // Cut back below the baseline: progress floors at zero, not negative.
    for _ in 0..30 {
        app.on_key(key(KeyCode::Backspace));
    }
    assert!(app.project_word_count() < baseline, "words were removed");
    assert_eq!(
        app.session_target
            .as_ref()
            .unwrap()
            .written(app.project_word_count()),
        0,
        "a cut below the baseline shows no progress"
    );
}

#[test]
fn reaching_the_target_is_signaled_once_and_non_disruptively() {
    let (mut app, _dir) = two_chapter_app("target-reached");
    app.begin_session_target("3"); // a tiny target, easy to cross
    app.message = None;
    type_str(&mut app, "one two three four");
    app.tick_session_target();
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Session target reached"),
        "reaching the target posts a status message"
    );
    // Non-disruptive: the target stays set (the filled indicator remains) and a
    // second tick doesn't re-announce it.
    assert!(app.session_target.is_some(), "the target is not cleared");
    app.message = None;
    app.tick_session_target();
    assert!(
        app.message.is_none(),
        "the reached target is announced only once"
    );
}

#[test]
fn live_chapter_progress_reflects_the_cached_goal_and_live_count() {
    let (mut app, _dir) = two_chapter_app("chapter-goal-live");
    // The active chapter carries a stored front-matter goal (cached on the
    // manifest at open); progress reads that plus the live buffer count — no file
    // read on the render path.
    let active = app.project.active_index();
    app.project.manifest.chapters[active].goal = Some(6);
    let goal = app.project.chapter_goal(active).unwrap();
    let before = app.chapter_word_count(active);
    assert!(before < goal, "starts short of the goal");
    type_str(&mut app, "several more words toward the goal now");
    let after = app.chapter_word_count(active);
    assert!(
        after > before,
        "the live chapter count rises as the author types: {before} → {after}"
    );
    assert!(after >= goal, "and crosses the stored goal");
}
