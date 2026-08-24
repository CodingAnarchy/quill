//! Annotation threads: reply and resolve (#889). Replies append to a thread and
//! persist, an empty or cancelled reply changes nothing, resolve/reopen toggles
//! and persists, the show filter cycles open/resolved/all, a pre-thread side file
//! still loads (open, no replies), and a full thread survives a round trip.

use super::*;
use crate::mode::{AnnoShow, Mode};
use crate::project::Project;

#[test]
fn a_reply_is_added_to_the_thread_and_persists() {
    let (mut app, dir) = two_chapter_app("anno-reply");
    app.create_annotation(0, 5, "opening note".into());
    app.open_annotation_review();

    // `a` opens the reply prompt; typing + Enter appends to the thread.
    app.on_key(key(KeyCode::Char('a')));
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "the reply prompt opened"
    );
    type_str(&mut app, "addressed in the next draft");
    app.on_key(key(KeyCode::Enter));

    // Back on the review list, on the same annotation, with the reply recorded.
    assert!(matches!(app.mode, Mode::Annotations { selected: 0, .. }));
    assert_eq!(app.annotations[0].replies.len(), 1);
    assert_eq!(
        app.annotations[0].replies[0].text,
        "addressed in the next draft"
    );

    // It round-trips through the side file.
    app.on_key(ctrl('s'));
    let reopened = Project::open(&dir).unwrap();
    let live = reopened.annotations_for("one.md");
    assert_eq!(live[0].replies.len(), 1);
    assert_eq!(live[0].replies[0].text, "addressed in the next draft");
}

#[test]
fn an_empty_reply_adds_nothing_and_returns_to_the_list() {
    let (mut app, _dir) = two_chapter_app("anno-reply-empty");
    app.create_annotation(0, 5, "note".into());
    app.open_annotation_review();

    app.on_key(key(KeyCode::Char('a')));
    app.on_key(key(KeyCode::Enter)); // confirm with no text
    assert!(
        app.annotations[0].replies.is_empty(),
        "a blank reply is discarded"
    );
    assert!(
        matches!(app.mode, Mode::Annotations { .. }),
        "back on the list"
    );
}

#[test]
fn cancelling_a_reply_returns_to_the_review_list() {
    let (mut app, _dir) = two_chapter_app("anno-reply-cancel");
    app.create_annotation(0, 5, "note".into());
    app.open_annotation_review();

    app.on_key(key(KeyCode::Char('a')));
    app.on_key(key(KeyCode::Esc)); // cancel — stay with the thread, not the editor
    assert!(matches!(app.mode, Mode::Annotations { selected: 0, .. }));
    assert!(app.annotations[0].replies.is_empty());
}

#[test]
fn resolve_and_reopen_toggle_the_state_and_persist() {
    let (mut app, dir) = two_chapter_app("anno-resolve");
    app.create_annotation(0, 5, "check this".into());
    app.open_annotation_review();
    assert!(!app.annotations[0].resolved, "starts open");

    // `r` resolves; it round-trips.
    app.on_key(key(KeyCode::Char('r')));
    assert!(app.annotations[0].resolved, "resolved");
    app.on_key(ctrl('s'));
    let reopened = Project::open(&dir).unwrap();
    assert!(reopened.annotations_for("one.md")[0].resolved);

    // `r` again reopens it.
    app.on_key(key(KeyCode::Char('r')));
    assert!(!app.annotations[0].resolved, "reopened");
}

#[test]
fn the_show_filter_cycles_and_hides_the_other_state() {
    let (mut app, _dir) = two_chapter_app("anno-show-filter");
    app.create_annotation(0, 5, "first".into());
    app.create_annotation(6, 13, "second".into());
    app.open_annotation_review();

    // Resolve the first note, then cycle the filter to "open": only the second
    // note passes, so the selection snaps off the now-hidden first.
    app.on_key(key(KeyCode::Char('r'))); // resolve #0
    assert_eq!(anno_show(&app), AnnoShow::All);
    app.on_key(key(KeyCode::Char('f'))); // All -> Open
    assert_eq!(anno_show(&app), AnnoShow::Open);
    assert!(
        !app.annotation_matches(0, ""),
        "resolved note hidden by the open filter"
    );
    assert!(app.annotation_matches(1, ""), "open note still shows");

    // Cycle to "resolved": now only the first passes.
    app.on_key(key(KeyCode::Char('f'))); // Open -> Resolved
    assert_eq!(anno_show(&app), AnnoShow::Resolved);
    assert!(app.annotation_matches(0, ""));
    assert!(!app.annotation_matches(1, ""));

    // And back to "all".
    app.on_key(key(KeyCode::Char('f'))); // Resolved -> All
    assert_eq!(anno_show(&app), AnnoShow::All);
    assert!(app.annotation_matches(0, "") && app.annotation_matches(1, ""));
}

#[test]
fn an_old_format_annotation_loads_open_with_no_replies() {
    // A side file written before threads existed (#889): just chapter/start/end/note,
    // no `replies` or `resolved` keys. It must load cleanly, open, with no replies —
    // the backward-compat guarantee.
    let (_app, dir) = two_chapter_app("anno-backcompat");
    let old_format = "[[annotation]]\n\
         chapter = \"one.md\"\n\
         start = 0\n\
         end = 5\n\
         note = \"a legacy note\"\n";
    fs::write(dir.join("annotations.toml"), old_format).unwrap();

    let reopened = Project::open(&dir).unwrap();
    let live = reopened.annotations_for("one.md");
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].note, "a legacy note");
    assert!(live[0].replies.is_empty(), "no replies on a legacy note");
    assert!(!live[0].resolved, "a legacy note loads open");
}

#[test]
fn a_thread_survives_a_full_persistence_round_trip() {
    let (mut app, dir) = two_chapter_app("anno-thread-roundtrip");
    app.create_annotation(0, 5, "opening".into());
    app.open_annotation_review();
    app.on_key(key(KeyCode::Char('a')));
    type_str(&mut app, "first reply");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('a')));
    type_str(&mut app, "second reply");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('r'))); // resolve
    app.on_key(ctrl('s'));

    let reopened = Project::open(&dir).unwrap();
    let live = reopened.annotations_for("one.md");
    assert_eq!(live.len(), 1);
    let a = &live[0];
    assert_eq!(a.note, "opening");
    assert!(a.resolved);
    assert_eq!(
        a.replies
            .iter()
            .map(|r| r.text.as_str())
            .collect::<Vec<_>>(),
        vec!["first reply", "second reply"],
    );
}
