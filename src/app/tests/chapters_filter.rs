//! Type-to-filter / quick-jump in the chapter browser (#410).

use super::*;

/// A project whose chapters have the given `(title, front_matter)` pairs, opened
/// with the chapter browser focused (Ctrl-G). Front matter is written verbatim
/// before the body, so a test can give a chapter a `pov:` or `status:`.
fn browser_app(name: &str, chapters: &[(&str, &str)]) -> (App, TempProject) {
    let dir = crate::testutil::scratch_dir("filter", name);
    let mut manifest = String::from("title = \"Book\"\n\n");
    for (i, (title, _)) in chapters.iter().enumerate() {
        manifest.push_str(&format!(
            "[[chapters]]\ntitle = \"{title}\"\nfile = \"ch{i}.md\"\n\n"
        ));
    }
    for (i, (_, fm)) in chapters.iter().enumerate() {
        fs::write(dir.join(format!("ch{i}.md")), format!("{fm}body\n")).unwrap();
    }
    fs::write(Project::manifest_path(&dir), manifest).unwrap();
    let mut app = open_test_app(&dir);
    app.on_key(ctrl('g')); // focus the chapter browser
    (app, TempProject::new(dir))
}

fn nav_sel(app: &App) -> usize {
    match &app.mode {
        Mode::Nav { selected, .. } => *selected,
        _ => panic!("expected Nav mode"),
    }
}

fn nav_filter(app: &App) -> String {
    match &app.mode {
        Mode::Nav { filter, .. } => filter.clone(),
        _ => panic!("expected Nav mode"),
    }
}

fn nav_typing(app: &App) -> bool {
    match &app.mode {
        Mode::Nav { typing, .. } => *typing,
        _ => panic!("expected Nav mode"),
    }
}

fn type_into(app: &mut App, s: &str) {
    for c in s.chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
}

fn match_count(app: &App) -> usize {
    app.nav_match_count()
}

#[test]
fn slash_filters_by_title_and_enter_quick_jumps() {
    let (mut app, _dir) = browser_app(
        "jump",
        &[
            ("Dawn", ""),
            ("The Meeting", ""),
            ("Departure", ""),
            ("Nightfall", ""),
        ],
    );
    app.on_key(key(KeyCode::Char('/')));
    assert!(nav_typing(&app), "slash enters the filter-input sub-mode");
    type_into(&mut app, "meet");
    assert_eq!(nav_filter(&app), "meet");
    assert_eq!(nav_sel(&app), 1, "selection snaps to the matching chapter");
    app.on_key(key(KeyCode::Enter));
    assert!(
        matches!(app.mode, Mode::Insert),
        "Enter opens the match and leaves the browser"
    );
    assert_eq!(app.project.active_index(), 1);
}

#[test]
fn filtering_is_case_insensitive_substring() {
    let (mut app, _dir) = browser_app("ci", &[("Dawn", ""), ("Nightfall", "")]);
    app.on_key(key(KeyCode::Char('/')));
    type_into(&mut app, "NIGHT");
    assert_eq!(nav_sel(&app), 1);
    assert_eq!(match_count(&app), 1);
}

#[test]
fn arrows_move_only_within_the_matches() {
    let (mut app, _dir) = browser_app(
        "within",
        &[
            ("Alpha", ""),
            ("Beta meet", ""),
            ("Gamma", ""),
            ("Delta meet", ""),
        ],
    );
    app.on_key(key(KeyCode::Char('/')));
    type_into(&mut app, "meet");
    assert_eq!(nav_sel(&app), 1, "first match");
    app.on_key(key(KeyCode::Down));
    assert_eq!(nav_sel(&app), 3, "skips the non-matching Gamma");
    app.on_key(key(KeyCode::Down));
    assert_eq!(nav_sel(&app), 3, "no further match — stays put");
    app.on_key(key(KeyCode::Up));
    assert_eq!(nav_sel(&app), 1);
}

#[test]
fn esc_keeps_the_filter_then_clears_it_then_exits() {
    let (mut app, _dir) = browser_app(
        "esc",
        &[("Alpha", ""), ("Beta meet", ""), ("Gamma meet", "")],
    );
    app.on_key(key(KeyCode::Char('/')));
    type_into(&mut app, "meet");
    assert!(nav_typing(&app));

    // First Esc leaves the input but keeps the filter, so commands now act on
    // the narrowed list.
    app.on_key(key(KeyCode::Esc));
    assert!(!nav_typing(&app));
    assert_eq!(nav_filter(&app), "meet");
    app.on_key(key(KeyCode::Char('j'))); // a command-mode move, within matches
    assert_eq!(nav_sel(&app), 2);

    // Second Esc clears the filter, restoring the full list.
    app.on_key(key(KeyCode::Esc));
    assert_eq!(nav_filter(&app), "");
    assert!(matches!(app.mode, Mode::Nav { .. }));

    // Third Esc leaves the browser.
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.mode, Mode::Insert));
}

#[test]
fn backspacing_to_empty_restores_the_full_list() {
    let (mut app, _dir) = browser_app("bs", &[("Alpha", ""), ("Beta", ""), ("Gamma", "")]);
    app.on_key(key(KeyCode::Char('/')));
    type_into(&mut app, "zzz");
    assert_eq!(match_count(&app), 0, "no chapter matches zzz");
    for _ in 0..3 {
        app.on_key(key(KeyCode::Backspace));
    }
    assert_eq!(nav_filter(&app), "");
    assert_eq!(match_count(&app), 3, "the full list is back");
}

#[test]
fn bare_letters_run_commands_outside_input_and_are_query_text_inside() {
    let (mut app, _dir) = browser_app("collide", &[("Alpha", ""), ("Beta", ""), ("Gamma", "")]);
    // In command mode, `j` moves the selection — it does not start a filter.
    app.on_key(key(KeyCode::Char('j')));
    assert_eq!(nav_sel(&app), 1);
    assert_eq!(nav_filter(&app), "");
    // Inside the filter-input sub-mode, the same key is query text.
    app.on_key(key(KeyCode::Char('/')));
    app.on_key(key(KeyCode::Char('j')));
    assert_eq!(nav_filter(&app), "j");
}

#[test]
fn filter_matches_a_pov_character() {
    let (mut app, _dir) = browser_app(
        "pov",
        &[
            ("Alpha", "---\npov: Sarah\n---\n"),
            ("Beta", ""),
            ("Gamma", ""),
        ],
    );
    app.on_key(key(KeyCode::Char('/')));
    type_into(&mut app, "sarah");
    assert_eq!(match_count(&app), 1);
    assert_eq!(nav_sel(&app), 0);
}

#[test]
fn filter_matches_a_status() {
    let (mut app, _dir) = browser_app(
        "status",
        &[
            ("Alpha", ""),
            ("Beta", "---\nstatus: draft\n---\n"),
            ("Gamma", ""),
        ],
    );
    app.on_key(key(KeyCode::Char('/')));
    type_into(&mut app, "draft");
    assert_eq!(match_count(&app), 1);
    assert_eq!(nav_sel(&app), 1);
}

// --- Go to chapter by number / title (#588) --------------------------------

#[test]
fn go_to_chapter_jumps_by_number() {
    let (mut app, _dir) = browser_app("goto-num", &[("Alpha", ""), ("Beta", ""), ("Gamma", "")]);
    app.on_key(key(KeyCode::Char('#')));
    type_into(&mut app, "3"); // 1-based
    app.on_key(key(KeyCode::Enter));

    assert_eq!(app.project.active_index(), 2, "jumped to the third chapter");
    assert!(
        matches!(app.mode, Mode::Insert),
        "go-to opens the chapter (leaves the browser)"
    );
}

#[test]
fn go_to_chapter_jumps_by_title_substring() {
    let (mut app, _dir) = browser_app("goto-title", &[("Alpha", ""), ("Beta", ""), ("Gamma", "")]);
    app.on_key(key(KeyCode::Char('#')));
    type_into(&mut app, "gam"); // case-insensitive substring of "Gamma"
    app.on_key(key(KeyCode::Enter));

    assert_eq!(app.project.active_index(), 2);
    assert!(matches!(app.mode, Mode::Insert));
}

#[test]
fn go_to_chapter_rejects_an_out_of_range_number() {
    let (mut app, _dir) = browser_app("goto-oob", &[("Alpha", ""), ("Beta", "")]);
    app.on_key(key(KeyCode::Char('#')));
    type_into(&mut app, "9");
    app.on_key(key(KeyCode::Enter));

    assert_eq!(app.project.active_index(), 0, "no jump for a bad number");
    assert!(
        matches!(app.mode, Mode::Nav { .. }),
        "an unresolved go-to stays in the browser"
    );
}

#[test]
fn go_to_chapter_with_no_title_match_stays_in_the_browser() {
    let (mut app, _dir) = browser_app("goto-nomatch", &[("Alpha", ""), ("Beta", "")]);
    app.on_key(key(KeyCode::Char('#')));
    type_into(&mut app, "zzz");
    app.on_key(key(KeyCode::Enter));

    assert_eq!(app.project.active_index(), 0);
    assert!(matches!(app.mode, Mode::Nav { .. }));
}
