//! The history view's type-to-filter (#822): `/` narrows commits by summary and
//! the diff/restore actions target the filtered selection. Pure move from `vcs.rs`.
use super::*;

// --- History view: type-to-filter (#822) -----------------------------------

/// The history view's `(selected, filter, typing)` state.
fn history_state(app: &App) -> (usize, String, bool) {
    match &app.mode {
        Mode::History(h) => (h.selected, h.filter.clone(), h.typing),
        _ => panic!("expected the history view to be open"),
    }
}

/// A repo with three commits (summaries alpha/beta/gamma), history open. Each
/// commit captures a distinct chapter body so a restore is observable. Git may
/// order same-second commits either way, so tests resolve rows by summary rather
/// than assuming a fixed position.
fn history_over_three(name: &str) -> (App, TempProject) {
    let (mut app, dir) = two_chapter_app(name);
    git_menu(&mut app, 0); // Initialize
                           // The base commit captures the seeded "first chapter\n".
    commit_via_menu(&mut app, "alpha groundwork");
    type_str(&mut app, " edit b");
    app.on_key(ctrl('s'));
    commit_via_menu(&mut app, "beta revision");
    type_str(&mut app, " edit c");
    app.on_key(ctrl('s'));
    commit_via_menu(&mut app, "gamma polish");
    git_menu_label(&mut app, "History");
    (app, dir)
}

/// The history row whose summary contains `needle`.
fn history_row_of(app: &App, needle: &str) -> usize {
    history_summaries(app)
        .iter()
        .position(|s| s.contains(needle))
        .expect("a commit with that summary")
}

#[test]
fn slash_filters_history_by_summary_and_moves_within_matches() {
    let (mut app, _dir) = history_over_three("vc-history-filter");
    let beta_row = history_row_of(&app, "beta");

    app.on_key(key(KeyCode::Char('/')));
    assert!(history_state(&app).2, "typing after /");
    // "beta" matches exactly one commit; the selection snaps onto its row.
    type_str(&mut app, "beta");
    assert_eq!(history_state(&app).0, beta_row);

    // A broader query still matching only "beta revision" (by its word).
    for _ in 0..4 {
        app.on_key(key(KeyCode::Backspace));
    }
    type_str(&mut app, "revision");
    assert_eq!(history_state(&app).0, beta_row);
    // No further match in either direction — Down/Up stay put on the one match.
    app.on_key(key(KeyCode::Down));
    assert_eq!(history_state(&app).0, beta_row);
    app.on_key(key(KeyCode::Up));
    assert_eq!(history_state(&app).0, beta_row);
}

#[test]
fn history_esc_clears_the_filter_then_closes() {
    let (mut app, _dir) = history_over_three("vc-history-filter-esc");

    app.on_key(key(KeyCode::Char('/')));
    type_str(&mut app, "gamma");
    app.on_key(key(KeyCode::Esc)); // leave input, keep the filter
    assert_eq!(history_state(&app).1, "gamma");
    assert!(!history_state(&app).2);

    app.on_key(key(KeyCode::Esc)); // clear filter; list restored
    assert_eq!(history_state(&app).1, "");
    assert!(matches!(app.mode, Mode::History(_)));

    app.on_key(key(KeyCode::Esc)); // close
    assert!(!matches!(app.mode, Mode::History(_)));
}

#[test]
fn history_restore_acts_on_the_filtered_match() {
    let (mut app, dir) = history_over_three("vc-history-filter-restore");
    let alpha_row = history_row_of(&app, "alpha");
    // Filter to the base commit and restore the chapter to it; the buffer must
    // load that commit's version (review-and-save), proving the action targets
    // the filtered selection, not the top row.
    app.on_key(key(KeyCode::Char('/')));
    type_str(&mut app, "alpha"); // the base commit: "first chapter\n"
    assert_eq!(history_state(&app).0, alpha_row);
    app.on_key(key(KeyCode::Enter)); // leave input, keep filter
    app.on_key(key(KeyCode::Char('r'))); // restore this chapter to the match

    assert!(!matches!(app.mode, Mode::History(_)), "the view closed");
    // The base commit's version loads into the buffer, without the later edits —
    // proving the restore targeted the filtered selection, not the top row.
    assert_eq!(app.buffer.text(), "first chapter\n");
    let _ = &dir;
}
