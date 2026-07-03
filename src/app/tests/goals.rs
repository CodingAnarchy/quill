//! Tests for setting project goals from within the app (#550): each goal
//! persists to the `[goals]` table of `book.toml` and round-trips, clearing the
//! daily goal removes its key (not a zero), invalid input is rejected, and the
//! progress report reflects a changed goal immediately.

use super::*;
use crate::mode::{Action, Pending, PromptState};
use crate::project::GoalField;

/// Drive one goal prompt: clear the prefilled value, type `text`, and press
/// Enter (a blank `text` leaves the input empty — clearing an optional goal or
/// triggering the rejection of a required one). Mirrors a user clearing the box
/// and typing a value over the prefill.
fn enter(app: &mut App, text: &str) {
    // Clear whatever was prefilled into the prompt's input.
    while matches!(&app.mode, Mode::Prompt(p) if !p.input.is_empty()) {
        app.on_key(key(KeyCode::Backspace));
    }
    type_str(app, text);
    app.on_key(key(KeyCode::Enter));
}

/// The report's plain text, for content assertions.
fn report_text(app: &App) -> String {
    app.report_lines()
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// A one-chapter project on disk whose manifest the test inspects after editing.
fn goals_app(name: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "first chapter\n").unwrap();
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

#[test]
fn the_set_goals_action_is_registered_and_named() {
    assert!(
        Action::ALL.iter().any(|a| a.name() == "goals:set"),
        "goals:set is in the registry"
    );
    assert!(matches!(
        Action::from_name("goals:set"),
        Some(Action::SetGoals)
    ));
}

#[test]
fn walking_the_flow_persists_every_goal_to_book_toml() {
    let (mut app, dir) = goals_app("goals-walk");
    app.run_action(Action::SetGoals, false);
    enter(&mut app, "80000"); // words
    enter(&mut app, "30"); // chapters
    enter(&mut app, "1000"); // daily
    enter(&mut app, "2026-12-31"); // deadline (#814)
    enter(&mut app, "250"); // words per page (default, kept)

    // The in-memory manifest carries every value.
    let goals = app.project.goals().expect("goals set");
    assert_eq!(goals.words, 80_000);
    assert_eq!(goals.chapters, 30);
    assert_eq!(goals.daily, Some(1_000));
    assert_eq!(goals.deadline.as_deref(), Some("2026-12-31"));
    assert_eq!(goals.words_per_page, 250);

    // And the manifest on disk has the `[goals]` table, re-opening identically.
    let reopened = Project::open(&*dir).unwrap();
    let g = reopened.goals().expect("goals persisted");
    assert_eq!(g.words, 80_000);
    assert_eq!(g.chapters, 30);
    assert_eq!(g.daily, Some(1_000));
    assert_eq!(g.deadline.as_deref(), Some("2026-12-31"));
}

#[test]
fn the_deadline_step_accepts_a_date_rejects_garbage_and_clears_on_blank() {
    let (mut app, dir) = goals_app("goals-deadline");
    app.run_action(Action::SetGoals, false);
    enter(&mut app, "80000"); // words
    enter(&mut app, "30"); // chapters
    enter(&mut app, ""); // daily: none
    enter(&mut app, "next spring"); // deadline: rejected as a non-date, re-prompts
    assert!(matches!(
        app.mode,
        Mode::Prompt(PromptState {
            pending: Pending::SetGoal(GoalField::Deadline),
            ..
        })
    ));
    assert!(
        app.message.as_deref().unwrap_or("").contains("YYYY-MM-DD"),
        "rejection message: {:?}",
        app.message
    );
    // A valid ISO date is accepted and persists as a quoted string.
    enter(&mut app, "2026-05-01");
    enter(&mut app, "250"); // words per page
    assert_eq!(
        app.project.goals().unwrap().deadline.as_deref(),
        Some("2026-05-01")
    );
    let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
    assert!(
        toml.contains("deadline = \"2026-05-01\""),
        "quoted ISO string on disk: {toml}"
    );

    // Re-running and blanking the deadline clears it (key removed, not empty).
    app.run_action(Action::SetGoals, false);
    enter(&mut app, "80000");
    enter(&mut app, "30");
    enter(&mut app, "");
    enter(&mut app, ""); // deadline — blank clears it
    enter(&mut app, "250");
    assert_eq!(app.project.goals().unwrap().deadline, None);
    let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
    assert!(!toml.contains("deadline"), "deadline key removed: {toml}");
}

#[test]
fn an_app_set_goal_reads_back_identical_to_a_hand_typed_one() {
    // Set via the app...
    let (mut app, dir) = goals_app("goals-roundtrip");
    app.run_action(Action::SetGoals, false);
    enter(&mut app, "90000");
    enter(&mut app, "40");
    enter(&mut app, ""); // no daily target
    enter(&mut app, ""); // no deadline
    enter(&mut app, "250");
    let from_app = Project::open(&*dir).unwrap().goals().cloned().unwrap();

    // ...and the same goals hand-typed into a fresh manifest.
    let hand = app_scratch_dir("goals-hand");
    fs::write(
        Project::manifest_path(&hand),
        "title = \"Book\"\n\n[goals]\nwords = 90000\nchapters = 40\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    fs::write(hand.join("one.md"), "x\n").unwrap();
    let from_hand = Project::open(&hand).unwrap().goals().cloned().unwrap();
    let _hand = TempProject::new(hand);

    assert_eq!(from_app, from_hand, "app-set goal == hand-typed goal");
}

#[test]
fn clearing_the_daily_goal_removes_the_key_not_a_zero() {
    let (mut app, dir) = goals_app("goals-clear-daily");
    // Set a daily target first.
    app.project
        .set_goal(GoalField::Words, Some(50_000))
        .unwrap();
    app.project.set_goal(GoalField::Chapters, Some(20)).unwrap();
    app.project.set_goal(GoalField::Daily, Some(800)).unwrap();
    assert!(
        fs::read_to_string(Project::manifest_path(&dir))
            .unwrap()
            .contains("daily = 800"),
        "daily written"
    );

    // Now clear it via the app: blank at the daily prompt.
    app.run_action(Action::SetGoals, false);
    enter(&mut app, "50000"); // words (re-confirmed)
    enter(&mut app, "20"); // chapters
    enter(&mut app, ""); // daily — blank clears it
    enter(&mut app, ""); // no deadline
    enter(&mut app, "250"); // words per page

    assert_eq!(
        app.project.goals().unwrap().daily,
        None,
        "cleared in memory"
    );
    let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
    assert!(
        !toml.contains("daily"),
        "daily key removed, not zeroed: {toml}"
    );
    // Re-opening confirms it's gone (not a stray `daily = 0`).
    assert_eq!(Project::open(&*dir).unwrap().goals().unwrap().daily, None);
}

#[test]
fn setting_goals_leaves_the_rest_of_the_manifest_intact() {
    let dir = app_scratch_dir("goals-preserve");
    let manifest = "# my book\ntitle = \"Book\"\nauthor = \"Ada\"\n\n\
         [analysis]\nstopwords_add = [\"foo\"]\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n";
    fs::write(Project::manifest_path(&dir), manifest).unwrap();
    fs::write(dir.join("one.md"), "x\n").unwrap();
    let mut app = open_test_app(&dir);
    let _dir = TempProject::new(dir.clone());

    app.project
        .set_goal(GoalField::Words, Some(70_000))
        .unwrap();
    let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
    // The comment, other scalars, and the other section all survive verbatim.
    assert!(toml.contains("# my book"), "comment kept: {toml}");
    assert!(toml.contains("author = \"Ada\""), "author kept: {toml}");
    assert!(
        toml.contains("[analysis]") && toml.contains("stopwords_add = [\"foo\"]"),
        "other section kept: {toml}"
    );
    assert!(
        toml.contains("[goals]") && toml.contains("words = 70000"),
        "{toml}"
    );
    // And it still parses (round-trips).
    assert_eq!(Project::open(&dir).unwrap().goals().unwrap().words, 70_000);
}

#[test]
fn a_non_positive_or_non_numeric_value_is_rejected_and_not_saved() {
    let (mut app, dir) = goals_app("goals-reject");
    app.run_action(Action::SetGoals, false);

    // Zero is rejected: a message is shown and the prompt re-opens on the same field.
    enter(&mut app, "0");
    assert!(matches!(
        app.mode,
        Mode::Prompt(PromptState {
            pending: Pending::SetGoal(GoalField::Words),
            ..
        })
    ));
    assert!(
        app.message.as_deref().unwrap_or("").contains("positive"),
        "rejection message: {:?}",
        app.message
    );

    // Non-numeric is rejected too, still on the words field.
    enter(&mut app, "lots");
    assert!(matches!(
        app.mode,
        Mode::Prompt(PromptState {
            pending: Pending::SetGoal(GoalField::Words),
            ..
        })
    ));

    // Nothing was written, in memory or on disk.
    assert!(app.project.goals().is_none(), "no goals saved on rejection");
    let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
    assert!(!toml.contains("[goals]"), "nothing spliced in: {toml}");
}

#[test]
fn the_progress_report_reflects_a_changed_goal_immediately() {
    let (mut app, _dir) = goals_app("goals-report");
    app.project
        .set_goal(GoalField::Words, Some(10_000))
        .unwrap();
    app.project.set_goal(GoalField::Chapters, Some(5)).unwrap();
    assert!(
        report_text(&app).contains("/ 10,000  words"),
        "initial goal shown: {}",
        report_text(&app)
    );

    // Change the target words; the report (rebuilt on open) reflects it with no restart.
    app.project
        .set_goal(GoalField::Words, Some(20_000))
        .unwrap();
    let report = report_text(&app);
    assert!(
        report.contains("/ 20,000  words"),
        "updated goal shown: {report}"
    );
    assert!(
        !report.contains("/ 10,000  words"),
        "old goal gone: {report}"
    );
}

#[test]
fn the_report_points_at_the_in_app_setter_when_no_goals_are_set() {
    let (app, _dir) = goals_app("goals-hint");
    let report = report_text(&app);
    assert!(report.contains("No writing goals set."), "{report}");
    assert!(report.contains("goals:set"), "discoverable hint: {report}");
}
