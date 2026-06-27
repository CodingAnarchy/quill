//! Tests for split panes: window splits, docked surfaces, reference files,
//! resize, and narrow-terminal collapse.

use super::*;
use crate::ui::render_to_string as screen;
use std::fs;

// --- Window splits (#163) --------------------------------------------------

/// Send a Vim window command: the `Ctrl-w` prefix then `c`.
fn window_cmd(app: &mut App, c: char) {
    app.on_key(ctrl('w'));
    app.on_key(key(KeyCode::Char(c)));
}

#[test]
fn ctrl_w_v_splits_and_ctrl_w_c_closes() {
    let (mut app, _dir) = two_chapter_app("split-open-close");
    assert!(app.split_view().is_none());
    window_cmd(&mut app, 'v'); // vertical split
    assert!(app.split_view().is_some(), "Ctrl-w v splits");
    window_cmd(&mut app, 'c'); // close
    assert!(app.split_view().is_none(), "Ctrl-w c closes the split");
}

#[test]
fn split_panes_edit_different_chapters_independently() {
    let (mut app, dir) = two_chapter_app("split-edit"); // active = "One"
    window_cmd(&mut app, 'v');
    // Focus the other pane and move it to chapter Two; edit + save there.
    window_cmd(&mut app, 'w');
    app.on_key(ctrl('n')); // other pane → "Two"
    type_str(&mut app, "TWO-EDIT");
    app.on_key(ctrl('s'));
    // Back to the first pane (still "One"); edit + save there.
    window_cmd(&mut app, 'w');
    type_str(&mut app, "ONE-EDIT");
    app.on_key(ctrl('s'));

    let one = fs::read_to_string(dir.join("one.md")).unwrap();
    let two = fs::read_to_string(dir.join("two.md")).unwrap();
    assert!(
        one.contains("ONE-EDIT") && two.contains("TWO-EDIT"),
        "one:{one:?} two:{two:?}"
    );
    assert!(
        !one.contains("TWO-EDIT"),
        "panes didn't cross-corrupt: {one:?}"
    );
}

#[test]
fn editing_the_same_chapter_in_both_panes_stays_consistent() {
    let (mut app, dir) = two_chapter_app("split-same"); // both panes start on "One"
    window_cmd(&mut app, 'v');
    type_str(&mut app, "AAA"); // edit the focused pane
    window_cmd(&mut app, 'w'); // switch: flush, then reload the sibling on the same chapter
    assert!(
        app.buffer.text().contains("AAA"),
        "the other pane reflects the saved edit: {:?}",
        app.buffer.text()
    );
    type_str(&mut app, "BBB");
    window_cmd(&mut app, 'w'); // switch back
    let text = app.buffer.text();
    assert!(
        text.contains("AAA") && text.contains("BBB"),
        "edits accumulate, no clobber: {text:?}"
    );
    let one = fs::read_to_string(dir.join("one.md")).unwrap();
    assert!(one.contains("AAA") && one.contains("BBB"), "{one}");
}

#[test]
fn split_is_guarded_in_the_continuous_view() {
    let (mut app, _dir) = two_chapter_app("split-in-cont");
    app.on_key(ctrl('l'));
    assert!(app.manuscript.is_some());
    window_cmd(&mut app, 'v');
    assert!(
        app.split_view().is_none(),
        "can't split the continuous view"
    );
}

#[test]
fn entering_continuous_closes_a_split() {
    let (mut app, _dir) = two_chapter_app("cont-closes-split");
    window_cmd(&mut app, 'v');
    assert!(app.split_view().is_some());
    app.on_key(ctrl('l')); // continuous view takes the editor region
    assert!(app.split_view().is_none());
    assert!(app.manuscript.is_some());
}

#[test]
fn ctrl_w_docks_an_open_report_beside_the_editor() {
    let (mut app, _dir) = two_chapter_app("dock-report");
    app.on_key(ctrl('r')); // open the progress report full-screen
    assert!(app.overlay.is_some());
    window_cmd(&mut app, 'v'); // dock it into a split (#164)
    assert!(
        app.overlay.is_none(),
        "the full-screen overlay moved into the pane"
    );
    assert!(
        matches!(
            app.split_view(),
            Some(SplitView {
                content: SplitContent::Surface(_),
                ..
            })
        ),
        "now a surface split"
    );
    // The editor stays editable in the other pane.
    type_str(&mut app, "EDIT");
    assert!(app.buffer.text().contains("EDIT"));
    // Closing the pane returns to the single editor.
    window_cmd(&mut app, 'c');
    assert!(app.split_view().is_none());
}

#[test]
fn focusing_a_docked_surface_scrolls_it() {
    let (mut app, _dir) = two_chapter_app("dock-scroll");
    app.on_key(ctrl('t')); // analysis report — tall enough to scroll
    window_cmd(&mut app, 'v'); // dock
    app.surface_height.set(5); // the renderer sets this; supply it for the test
    window_cmd(&mut app, 'w'); // focus the surface for scrolling
    assert!(app.surface_pane_focused());
    app.on_key(key(KeyCode::Char('j'))); // scroll down one row
    let scrolled = matches!(app.split_view(), Some(SplitView { content: SplitContent::Surface(overlay), .. }) if overlay.scroll == 1);
    assert!(scrolled, "j scrolls the docked surface");
    app.on_key(key(KeyCode::Esc)); // Esc returns focus to the editor
    assert!(!app.surface_pane_focused());
}

#[test]
fn docking_is_guarded_in_the_continuous_view() {
    let (mut app, _dir) = two_chapter_app("dock-cont");
    app.on_key(ctrl('l')); // continuous view
    app.on_key(ctrl('r')); // report opens full-screen
    window_cmd(&mut app, 'v'); // try to dock
    assert!(
        app.split_view().is_none(),
        "no docking in the continuous view"
    );
    assert!(app.overlay.is_some(), "the overlay stays full-screen");
}

// --- Reference files in a pane (#165) --------------------------------------

/// The title a reference pane shows for the non-focused buffer.
fn other_pane_title(app: &App) -> String {
    match app.split_view() {
        Some(SplitView {
            content: SplitContent::Editor(other),
            ..
        }) => app.pane_title_for(other),
        _ => panic!("expected two editor panes"),
    }
}

/// Add a couple of bible files to a project on disk (created after open, which is
/// fine — `reference_files` reads the filesystem live).
fn write_bible(dir: &std::path::Path) {
    let chars = dir.join("bible").join("characters");
    fs::create_dir_all(&chars).unwrap();
    fs::write(
        dir.join("bible").join("setting.md"),
        "# Setting\n\nthe city\n",
    )
    .unwrap();
    fs::write(chars.join("mara.md"), "# Mara\n\na sailor\n").unwrap();
}

#[test]
fn reference_files_list_the_bible_and_skip_chapters() {
    let (app, dir) = two_chapter_app("ref-list");
    write_bible(&dir);
    let files = app.project.reference_files();
    let names: Vec<String> = files
        .iter()
        .map(|p| {
            p.strip_prefix(&dir)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/")
        })
        .collect();
    assert!(
        names.contains(&"bible/characters/mara.md".to_string()),
        "{names:?}"
    );
    assert!(names.contains(&"bible/setting.md".to_string()), "{names:?}");
    assert!(
        !names
            .iter()
            .any(|n| n.ends_with("one.md") || n.ends_with("two.md")),
        "chapters excluded: {names:?}"
    );
}

#[test]
fn f_in_the_browser_opens_the_reference_file_list() {
    let (mut app, dir) = two_chapter_app("ref-browse");
    write_bible(&dir);
    app.on_key(ctrl('g')); // chapter browser
    app.on_key(key(KeyCode::Char('f'))); // switch to reference files
    let count = match &app.mode {
        Mode::RefFiles(state) => state.files.len(),
        _ => panic!("expected RefFiles mode"),
    };
    assert_eq!(count, 2, "both bible files listed");
}

#[test]
fn reference_browser_reports_when_there_are_no_files() {
    let (mut app, _dir) = two_chapter_app("ref-empty");
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('f')));
    assert!(
        matches!(app.mode, Mode::Nav { .. }),
        "stays in the chapter browser"
    );
    assert!(app
        .message
        .as_deref()
        .unwrap_or("")
        .contains("No reference files"));
}

#[test]
fn opening_a_reference_file_puts_it_in_a_pane() {
    let (mut app, dir) = two_chapter_app("ref-open");
    write_bible(&dir);
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('f')));
    app.on_key(key(KeyCode::Enter)); // open the first file (bible/characters/mara.md, sorted)
    assert!(app.split_view().is_some(), "a reference pane opened");
    assert_eq!(
        other_pane_title(&app),
        "mara",
        "the pane is titled by the file"
    );
    assert!(
        matches!(app.mode, Mode::Insert),
        "focus returns to the editor"
    );
}

#[test]
fn o_in_the_browser_opens_the_selected_chapter_in_a_pane() {
    let (mut app, _dir) = two_chapter_app("ref-chapter-pane"); // active = "One"
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Down)); // select "Two"
    app.on_key(key(KeyCode::Char('o')));
    assert_eq!(
        other_pane_title(&app),
        "Two",
        "the other chapter opens in the pane"
    );
}

#[test]
fn a_reference_pane_saves_independently() {
    let (mut app, dir) = two_chapter_app("ref-save");
    write_bible(&dir);
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('f')));
    app.on_key(key(KeyCode::Enter)); // mara.md into the reference pane
    window_cmd(&mut app, 'w'); // focus it (swap into the editor)
    type_str(&mut app, "NOTE");
    app.on_key(ctrl('s'));
    let mara = fs::read_to_string(dir.join("bible").join("characters").join("mara.md")).unwrap();
    let one = fs::read_to_string(dir.join("one.md")).unwrap();
    assert!(
        mara.contains("NOTE"),
        "the bible file saved its edit: {mara:?}"
    );
    assert!(!one.contains("NOTE"), "the chapter is untouched: {one:?}");
}

// --- Resize & narrow-terminal collapse (#166) ------------------------------

fn split_ratio(app: &App) -> u16 {
    app.split.as_ref().expect("split").first_ratio
}

#[test]
fn resize_grows_and_shrinks_the_focused_pane_within_bounds() {
    let (mut app, _dir) = two_chapter_app("resize");
    window_cmd(&mut app, 'v'); // side-by-side, even split (focused pane is first)
    let even = split_ratio(&app);
    window_cmd(&mut app, '>'); // grow the focused pane
    assert!(split_ratio(&app) > even, "Ctrl-w > grows the focused pane");
    window_cmd(&mut app, '<');
    window_cmd(&mut app, '<');
    assert!(split_ratio(&app) < even, "Ctrl-w < shrinks it past even");
    // Shrinking can't starve the other pane below the minimum.
    for _ in 0..20 {
        window_cmd(&mut app, '<');
    }
    assert_eq!(split_ratio(&app), 20, "clamped at the minimum ratio");
    // Ctrl-w = resets to an even balance (#173).
    window_cmd(&mut app, '=');
    assert_eq!(split_ratio(&app), even, "Ctrl-w = equalizes the split");
}

#[test]
fn a_narrow_terminal_draws_only_the_focused_pane() {
    let (mut app, _dir) = two_chapter_app("narrow"); // one.md="first chapter"
    window_cmd(&mut app, 'v'); // split (both panes on chapter One)
    window_cmd(&mut app, 'w'); // focus the other pane
    app.on_key(ctrl('n')); // move it to chapter Two ("second chapter")
    window_cmd(&mut app, 'w'); // focus back: first pane = "first chapter"

    let wide = screen(&app, 100, 20);
    assert!(
        wide.contains("first chapter") && wide.contains("second chapter"),
        "both panes draw when wide:\n{wide}"
    );

    let narrow = screen(&app, 50, 20); // below 2 × MIN_PANE_WIDTH
    assert!(
        narrow.contains("first chapter"),
        "the focused pane still draws:\n{narrow}"
    );
    assert!(
        !narrow.contains("second chapter"),
        "the other pane collapses away:\n{narrow}"
    );

    // The split survives the squeeze: widening restores it.
    assert!(
        app.split_view().is_some(),
        "the split state is kept, just not drawn"
    );
}

#[test]
fn editing_a_reference_pane_does_not_corrupt_the_chapter_word_cache() {
    let (mut app, dir) = two_chapter_app("ref-cache"); // active = "One"
    write_bible(&dir);
    let one_words = app.project.chapter_words(0);
    app.on_key(ctrl('g'));
    app.on_key(key(KeyCode::Char('f')));
    app.on_key(key(KeyCode::Enter)); // mara.md into the pane
    window_cmd(&mut app, 'w'); // focus the bible file (a non-chapter buffer)
    type_str(&mut app, "a longer note with several extra words here");
    app.on_key(ctrl('s'));
    // The bible file's word count must not have been written into chapter One's
    // cached entry (which feeds the outline pane and the progress report).
    assert_eq!(
        app.project.chapter_words(0),
        one_words,
        "chapter One's cached count is untouched"
    );
}
