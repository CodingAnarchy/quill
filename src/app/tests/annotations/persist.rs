//! When the annotations side file is (and isn't) written: a clean chapter switch
//! leaves `annotations.toml` alone, a moved note still persists on a switch or a
//! save, the continuous view writes its whole window in one go, and switching
//! *through* a chapter (or adding a note right after entering one) within one
//! input burst never erases its notes.

use super::*;
use crate::buffer::Cursor;
use crate::project::{annotation_writes, Project};

/// Mark the side file with a leading comment. Any rewrite drops it, so its
/// survival proves the file wasn't touched.
fn mark_side_file(dir: &Path) -> String {
    let path = dir.join("annotations.toml");
    let marked = format!("# untouched\n{}", fs::read_to_string(&path).unwrap());
    fs::write(&path, &marked).unwrap();
    marked
}

fn side_file(dir: &Path) -> String {
    fs::read_to_string(dir.join("annotations.toml")).unwrap()
}

/// A three-chapter app (`c0..c2.md` = "alpha"/"beta"/"gamma") opened with a note
/// already on disk for each chapter's whole word.
fn three_annotated(name: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    let mut manifest = String::from("title = \"Book\"\n\n");
    let mut side = String::new();
    for (i, body) in ["alpha\n", "beta\n", "gamma\n"].iter().enumerate() {
        write!(
            manifest,
            "[[chapters]]\ntitle = \"C{i}\"\nfile = \"c{i}.md\"\n\n"
        )
        .unwrap();
        fs::write(dir.join(format!("c{i}.md")), body).unwrap();
        write!(
            side,
            "[[annotation]]\nchapter = \"c{i}.md\"\nstart = 0\nend = {}\nnote = \"note {i}\"\n\n",
            body.trim_end().len()
        )
        .unwrap();
    }
    fs::write(Project::manifest_path(&dir), manifest).unwrap();
    fs::write(dir.join("annotations.toml"), side).unwrap();
    let mut app = open_test_app(&dir);
    app.on_input_settled(); // load the active chapter's notes, as the event loop does
    (app, TempProject::new(dir))
}

#[test]
fn a_clean_chapter_switch_does_not_rewrite_the_side_file() {
    let (mut app, dir) = three_annotated("anno-clean-switch");
    let marked = mark_side_file(&dir);
    let before = annotation_writes::get();
    for k in [ctrl('n'), ctrl('n'), ctrl('p'), ctrl('p')] {
        app.on_key(k);
        app.on_input_settled();
    }
    assert_eq!(annotation_writes::get(), before, "no annotation changed");
    assert_eq!(side_file(&dir), marked);
}

#[test]
fn a_moved_note_still_persists_on_a_switch() {
    let (mut app, dir) = three_annotated("anno-moved-switch");
    // Type at the start of "alpha": an insertion at a note's start grows it.
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    type_str(&mut app, "A ");
    app.on_key(ctrl('n')); // the flush saves, re-anchors, and persists
    let reopened = Project::open(&dir).unwrap();
    let live = reopened.annotations_for("c0.md");
    assert_eq!((live[0].start, live[0].end), (0, 7));
    // The other chapters' notes are untouched.
    assert_eq!(reopened.annotations_for("c1.md").len(), 1);
    assert_eq!(reopened.annotations_for("c2.md").len(), 1);
}

#[test]
fn an_edited_note_persists_on_save() {
    let (mut app, dir) = three_annotated("anno-edit-save");
    app.annotations[0].note = "revised note".into();
    app.on_key(ctrl('s'));
    assert!(side_file(&dir).contains("revised note"));
    // A second, clean save has nothing new to write.
    let before = annotation_writes::get();
    app.on_key(ctrl('s'));
    assert_eq!(annotation_writes::get(), before);
}

#[test]
fn a_continuous_save_writes_every_segment_once() {
    let (mut app, dir) = three_annotated("anno-continuous-save");
    app.on_key(ctrl('l'));
    app.on_input_settled();
    assert_eq!(app.annotations.len(), 3, "the window holds all three notes");
    // Insert at the very top: c0's note grows; c1/c2 stay chapter-local.
    app.manuscript.as_mut().unwrap().buffer_mut().cursor = Cursor { line: 0, col: 0 };
    type_str(&mut app, "X");
    app.on_input_settled();
    let before = annotation_writes::get();
    app.on_key(ctrl('s'));
    assert_eq!(
        annotation_writes::get() - before,
        1,
        "one write for the window"
    );

    let reopened = Project::open(&dir).unwrap();
    let span = |f: &str| {
        let live = reopened.annotations_for(f);
        assert_eq!(live.len(), 1, "{f}");
        (live[0].start, live[0].end)
    };
    assert_eq!(span("c0.md"), (0, 6));
    assert_eq!(span("c1.md"), (0, 4));
    assert_eq!(span("c2.md"), (0, 5));
}

#[test]
fn switching_through_a_chapter_in_one_burst_keeps_its_notes() {
    // Two Ctrl-n's in one input burst (key repeat): the middle chapter is entered
    // and left before the settle-time reload ever loads its notes. Leaving it must
    // not persist the previous chapter's (empty, for it) set over its own.
    let (mut app, dir) = three_annotated("anno-switch-through");
    let marked = mark_side_file(&dir);
    app.on_key(ctrl('n'));
    app.on_key(ctrl('n'));
    assert_eq!(side_file(&dir), marked, "no note was rewritten or lost");
    app.on_input_settled();
    assert_eq!(app.annotations.len(), 1, "c2's note loads");

    // Same for a save straight after switching in, before the settle.
    app.on_key(ctrl('p'));
    app.on_key(ctrl('s'));
    let reopened = Project::open(&dir).unwrap();
    for f in ["c0.md", "c1.md", "c2.md"] {
        assert_eq!(reopened.annotations_for(f).len(), 1, "{f} kept its note");
    }
}

#[test]
fn adding_a_note_in_the_same_burst_as_a_switch_keeps_the_chapters_notes() {
    // A switch, then a note created before the settle-time reload: the new note
    // must join the entered chapter's existing notes, not replace them.
    let (mut app, dir) = three_annotated("anno-switch-then-add");
    app.on_key(ctrl('n')); // into c1 ("beta"), its note not yet loaded
    app.create_annotation(0, 2, "fresh".into());
    let reopened = Project::open(&dir).unwrap();
    let notes: Vec<String> = reopened
        .annotations_for("c1.md")
        .into_iter()
        .map(|a| a.note)
        .collect();
    assert_eq!(
        notes.len(),
        2,
        "c1 kept its note and gained the new one: {notes:?}"
    );
    assert!(notes.iter().any(|n| n == "note 1") && notes.iter().any(|n| n == "fresh"));
    assert_eq!(reopened.annotations_for("c0.md").len(), 1, "c0 untouched");
}

#[test]
fn removing_the_last_note_removes_the_side_file() {
    let (mut app, dir) = two_chapter_app("anno-remove-last");
    app.create_annotation(0, 5, "only note".into());
    assert!(dir.join("annotations.toml").exists());
    app.annotations.clear();
    app.on_key(ctrl('s'));
    assert!(!dir.join("annotations.toml").exists());
}
