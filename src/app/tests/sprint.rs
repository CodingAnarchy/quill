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
    assert!(app.message.as_deref().unwrap_or("").contains("Sprint started"));
}

#[test]
fn a_non_number_length_is_rejected() {
    let (mut app, _dir) = two_chapter_app("sprint-bad-len");
    app.begin_sprint("soon");
    assert!(app.sprint.is_none(), "no sprint starts for an unparseable length");
    assert!(app.message.as_deref().unwrap_or("").contains("whole minutes"));
}

#[test]
fn the_word_count_tracks_writing_since_the_start() {
    let (mut app, _dir) = two_chapter_app("sprint-words");
    app.begin_sprint("25");
    let before = app.sprint.as_ref().unwrap().words(app.project_word_count());
    type_str(&mut app, "three more words");
    let after = app.sprint.as_ref().unwrap().words(app.project_word_count());
    assert!(after > before, "words written rises as the author types: {before} → {after}");
}

#[test]
fn stopping_reports_the_result_and_clears_the_sprint() {
    let (mut app, _dir) = two_chapter_app("sprint-stop");
    app.begin_sprint("25");
    app.stop_sprint();
    assert!(app.sprint.is_none(), "the sprint is cleared");
    assert!(app.message.as_deref().unwrap_or("").contains("Sprint stopped"));
}

#[test]
fn stopping_with_no_sprint_says_so() {
    let (mut app, _dir) = two_chapter_app("sprint-stop-none");
    app.stop_sprint();
    assert!(app.message.as_deref().unwrap_or("").contains("No sprint is running"));
}

#[test]
fn availability_reflects_whether_a_sprint_is_running() {
    let (mut app, _dir) = two_chapter_app("sprint-status");
    assert!(app.action_status(Action::StartSprint).is_none(), "can start when idle");
    assert!(app.action_status(Action::StopSprint).is_some(), "nothing to stop when idle");
    app.begin_sprint("25");
    assert!(app.action_status(Action::StartSprint).is_some(), "can't start a second sprint");
    assert!(app.action_status(Action::StopSprint).is_none(), "can stop the running one");
}

#[test]
fn the_event_loop_tick_finishes_an_elapsed_sprint() {
    let (mut app, _dir) = two_chapter_app("sprint-finish");
    app.sprint = Some(crate::app::sprint::test_finished_sprint(app.project_word_count()));
    app.tick_sprint();
    assert!(app.sprint.is_none(), "an elapsed sprint ends on the next tick");
    assert!(app.message.as_deref().unwrap_or("").contains("Sprint over"));
}

#[test]
fn a_running_sprint_does_not_finish_early() {
    let (mut app, _dir) = two_chapter_app("sprint-running");
    app.begin_sprint("25");
    app.tick_sprint();
    assert!(app.sprint.is_some(), "a fresh sprint keeps running across ticks");
}
