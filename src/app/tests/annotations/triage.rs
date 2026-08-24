//! Multi-reviewer triage of the review list (#933): sorting by position /
//! status / reviewer, filtering by reviewer and composing that with the status
//! filter, jumping to the next open thread, the reviewer shown on a row, and the
//! reviewer identity's round trip (a pre-reviewer annotation loading as the
//! author's).

use super::*;
use crate::annotation::Live;
use crate::mode::{AnnoShow, AnnoSort, Mode};
use crate::project::Project;

/// The reviewer of the annotation the review list currently has selected — the
/// label the list shows and filters by. Panics off the list.
fn selected_reviewer(app: &App) -> String {
    let sel = match &app.mode {
        Mode::Annotations { selected, .. } => *selected,
        _ => panic!("expected the annotation list"),
    };
    app.annotations[sel].reviewer_label().to_string()
}

/// The active sort order of the open review list. Panics off the list.
fn anno_sort(app: &App) -> AnnoSort {
    match &app.mode {
        Mode::Annotations { sort, .. } => *sort,
        _ => panic!("expected the annotation list"),
    }
}

/// Seed the active chapter with four annotations from a mix of reviewers and
/// statuses (in document order), each anchored to a distinct byte span, then open
/// the review list. Returns the app + its temp project.
///
/// Order on disk / in `app.annotations` (position order):
///   0: "you" note, open       — anchored at 0..5
///   1: "Copy Ed" note, open   — anchored at 6..13
///   2: "Beta" note, resolved  — anchored at 6..13
///   3: "you" note, resolved   — anchored at 6..13
fn triage_app(name: &str) -> (App, TempProject) {
    let (mut app, tmp) = two_chapter_app(name);
    let make = |start, end, note: &str, reviewer: Option<&str>, resolved| Live {
        chapter: "one.md".into(),
        start,
        end,
        note: note.into(),
        replies: Vec::new(),
        resolved,
        reviewer: reviewer.map(str::to_string),
    };
    app.annotations = vec![
        make(0, 5, "yours open", None, false),
        make(6, 13, "copyed open", Some("Copy Ed"), false),
        make(6, 13, "beta resolved", Some("Beta"), true),
        make(6, 13, "yours resolved", None, true),
    ];
    app.open_annotation_review();
    (app, tmp)
}

#[test]
fn sort_by_status_groups_open_threads_first() {
    let (mut app, _tmp) = triage_app("anno-sort-status");
    // Default is position order: indices 0,1,2,3 as seeded.
    assert_eq!(app.annotation_visible_order(), vec![0, 1, 2, 3]);
    assert_eq!(anno_sort(&app), AnnoSort::Position);

    // `s` cycles to status sort: the open notes (0, 1) lead, then the resolved
    // ones (2, 3), position order kept within each group.
    app.on_key(key(KeyCode::Char('s')));
    assert_eq!(anno_sort(&app), AnnoSort::Status);
    assert_eq!(app.annotation_visible_order(), vec![0, 1, 2, 3]);
    // Prove it actually groups by reversing the seed's status: flip 0 resolved, 1
    // open stays — now the sort must move the still-open one ahead of it.
    app.annotations[0].resolved = true;
    assert_eq!(
        app.annotation_visible_order(),
        vec![1, 0, 2, 3],
        "the one open note leads; the rest (resolved) follow in position order"
    );
}

#[test]
fn sort_by_reviewer_groups_alphabetically() {
    let (mut app, _tmp) = triage_app("anno-sort-reviewer");
    // `s` twice: position → status → reviewer.
    app.on_key(key(KeyCode::Char('s')));
    app.on_key(key(KeyCode::Char('s')));
    assert_eq!(anno_sort(&app), AnnoSort::Reviewer);
    // Grouped by reviewer label alphabetically: "Beta" (2), "Copy Ed" (1), then
    // "you" (0, 3) — position order within the "you" bucket.
    assert_eq!(app.annotation_visible_order(), vec![2, 1, 0, 3]);

    // A third `s` returns to position order.
    app.on_key(key(KeyCode::Char('s')));
    assert_eq!(anno_sort(&app), AnnoSort::Position);
    assert_eq!(app.annotation_visible_order(), vec![0, 1, 2, 3]);
}

#[test]
fn filter_by_reviewer_narrows_the_list() {
    let (mut app, _tmp) = triage_app("anno-filter-reviewer");
    // `R` cycles the per-reviewer filter through the present reviewers in the same
    // order they cycle: "you" (author bucket) first, then named alphabetically.
    app.on_key(key(KeyCode::Char('R')));
    assert_eq!(selected_reviewer(&app), "you");
    assert_eq!(
        app.annotation_visible_order(),
        vec![0, 3],
        "only the author's own notes remain"
    );

    // Next: "Beta".
    app.on_key(key(KeyCode::Char('R')));
    assert_eq!(app.annotation_visible_order(), vec![2]);
    assert_eq!(selected_reviewer(&app), "Beta");

    // Next: "Copy Ed".
    app.on_key(key(KeyCode::Char('R')));
    assert_eq!(app.annotation_visible_order(), vec![1]);
    assert_eq!(selected_reviewer(&app), "Copy Ed");

    // Once more wraps back to all reviewers.
    app.on_key(key(KeyCode::Char('R')));
    assert_eq!(app.annotation_visible_order(), vec![0, 1, 2, 3]);
}

#[test]
fn reviewer_and_status_filters_compose() {
    let (mut app, _tmp) = triage_app("anno-filter-compose");
    // Filter to "you" (notes 0 open, 3 resolved).
    app.on_key(key(KeyCode::Char('R')));
    assert_eq!(app.annotation_visible_order(), vec![0, 3]);
    // Then the open-only status filter (`f`: all → open) drops the resolved one.
    app.on_key(key(KeyCode::Char('f')));
    assert_eq!(
        app.annotation_visible_order(),
        vec![0],
        "the two filters intersect: your open notes only"
    );
}

#[test]
fn esc_clears_the_reviewer_filter_before_closing() {
    let (mut app, _tmp) = triage_app("anno-reviewer-esc");
    app.on_key(key(KeyCode::Char('R'))); // filter to "you"
    assert_eq!(app.annotation_visible_order(), vec![0, 3]);

    app.on_key(key(KeyCode::Esc)); // clears the reviewer filter, list restored
    assert!(matches!(app.mode, Mode::Annotations { reviewer: None, .. }));
    assert_eq!(app.annotation_visible_order(), vec![0, 1, 2, 3]);

    app.on_key(key(KeyCode::Esc)); // now closes
    assert!(matches!(app.mode, Mode::Insert));
}

#[test]
fn jump_to_next_open_moves_between_open_threads() {
    let (mut app, _tmp) = triage_app("anno-jump-open");
    // Selection starts at 0 (an open note). `]` jumps to the next open thread — the
    // only other open one is index 1. The resolved 2 and 3 are skipped.
    app.on_key(key(KeyCode::Char(']')));
    assert_eq!(
        match app.mode {
            Mode::Annotations { selected, .. } => selected,
            _ => panic!(),
        },
        1,
        "] skips the resolved threads to the next open one"
    );

    // Another `]` wraps back to the first open note (0).
    app.on_key(key(KeyCode::Char(']')));
    assert!(matches!(app.mode, Mode::Annotations { selected: 0, .. }));

    // `[` steps back to the previous open thread (wraps to 1).
    app.on_key(key(KeyCode::Char('[')));
    assert!(matches!(app.mode, Mode::Annotations { selected: 1, .. }));
}

#[test]
fn jump_from_a_resolved_note_lands_on_the_nearest_open_thread() {
    let (mut app, _tmp) = triage_app("anno-jump-from-resolved");
    // Notes 2 and 3 are resolved (they anchor at 6..13, after the open notes 0/1).
    // Select the resolved note 2, then `]` moves forward past the end of the visible
    // list and wraps to the first open note (0); `[` from there goes to the last
    // open one before it.
    app.set_annotations_selected(2);
    app.on_key(key(KeyCode::Char(']')));
    assert!(
        matches!(app.mode, Mode::Annotations { selected: 0, .. }),
        "forward from a resolved note wraps to the first open thread"
    );

    // From resolved note 3 (past both open notes), `[` steps back to the last open
    // note (1).
    app.set_annotations_selected(3);
    app.on_key(key(KeyCode::Char('[')));
    assert!(
        matches!(app.mode, Mode::Annotations { selected: 1, .. }),
        "backward from a resolved note lands on the previous open thread"
    );
}

#[test]
fn jump_to_next_open_reports_when_nothing_is_open() {
    let (mut app, _tmp) = triage_app("anno-jump-none");
    // Resolve every note, then a jump has nothing to move to.
    for a in &mut app.annotations {
        a.resolved = true;
    }
    app.on_key(key(KeyCode::Char(']')));
    assert_eq!(app.message.as_deref(), Some("No open threads."));
}

#[test]
fn imported_reviewer_identity_is_shown_in_the_row() {
    // A note imported from a named reviewer (#886/#933) shows that reviewer's name
    // in the review-list row; an author-authored note shows none.
    let (app, _tmp) = triage_app("anno-reviewer-row");
    let screen = crate::ui::render_to_string(&app, 90, 20);
    assert!(
        screen.contains("Copy Ed:") && screen.contains("Beta:"),
        "each imported note is attributed to its reviewer:\n{screen}"
    );
}

#[test]
fn a_pre_reviewer_annotation_loads_as_author_authored() {
    // Backward-compat (#933): an annotations.toml written before the reviewer field
    // existed (no `reviewer` key) loads cleanly, as an author-authored note.
    let dir = crate::testutil::scratch_dir("anno-compat", "load");
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "first chapter\n").unwrap();
    // A pre-reviewer side file: only the historic fields, no `reviewer`.
    fs::write(
        dir.join("annotations.toml"),
        "[[annotation]]\nchapter = \"one.md\"\nstart = 0\nend = 5\nnote = \"legacy note\"\n",
    )
    .unwrap();

    let project = Project::open(&dir).unwrap();
    let live = project.annotations_for("one.md");
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].note, "legacy note");
    assert_eq!(live[0].reviewer, None, "an old note is author-authored");
    assert_eq!(live[0].reviewer_label(), "you");
}

#[test]
fn a_reviewer_round_trips_through_the_side_file() {
    let (mut app, dir) = triage_app("anno-reviewer-roundtrip");
    app.persist_annotations();
    let side = fs::read_to_string(dir.join("annotations.toml")).unwrap();
    assert!(
        side.contains("reviewer = \"Copy Ed\"") && side.contains("reviewer = \"Beta\""),
        "named reviewers are written to disk:\n{side}"
    );

    // Reopening the project preserves each note's reviewer (and the author-authored
    // ones stay unattributed, no `reviewer` key spuriously added).
    let reopened = Project::open(&dir).unwrap();
    let live = reopened.annotations_for("one.md");
    let reviewers: Vec<Option<String>> = live.iter().map(|a| a.reviewer.clone()).collect();
    assert!(reviewers.contains(&Some("Copy Ed".to_string())));
    assert!(reviewers.contains(&Some("Beta".to_string())));
    assert_eq!(reviewers.iter().filter(|r| r.is_none()).count(), 2);
}

#[test]
fn existing_filter_and_resolve_behavior_is_unchanged_at_defaults() {
    // With sort at position and no reviewer filter (#933 defaults), the list behaves
    // exactly as before: `/` text filter and `f` status toggle work as they did.
    let (mut app, _tmp) = two_chapter_app("anno-defaults");
    app.create_annotation(0, 5, "opening beat".into());
    app.create_annotation(6, 13, "closing beat".into());
    app.open_annotation_review();
    assert_eq!(anno_sort(&app), AnnoSort::Position);
    assert!(matches!(app.mode, Mode::Annotations { reviewer: None, .. }));

    // `/` text filter still narrows to the matching note.
    app.on_key(key(KeyCode::Char('/')));
    type_str(&mut app, "closing");
    assert_eq!(app.annotation_visible_order(), vec![1]);
    app.on_key(key(KeyCode::Esc)); // leave input
    app.on_key(key(KeyCode::Esc)); // clear filter
    assert_eq!(app.annotation_visible_order(), vec![0, 1]);

    // `f` status toggle (all → open) still works; both are open so both remain.
    app.on_key(key(KeyCode::Char('f')));
    assert_eq!(anno_show(&app), AnnoShow::Open);
    assert_eq!(app.annotation_visible_order(), vec![0, 1]);
}
