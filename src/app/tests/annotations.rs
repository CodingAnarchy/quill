//! Inline author annotations (#302): a note anchored to a chapter byte range,
//! marked in the editor, re-anchored as the prose is edited, listed/navigable,
//! and kept out of the chapter `.md` (so exports stay clean).

use super::*;
use crate::annotation::Remapper;
use crate::buffer::Cursor;
use crate::mode::{AnnoShow, AnnoSort, Mode};
use crate::project::Project;

fn ctrl(c: char) -> KeyEvent {
    KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
}

#[test]
fn annotation_persists_in_a_side_file_and_round_trips() {
    let (mut app, dir) = two_chapter_app("anno-roundtrip");
    // "first chapter\n" — annotate "first".
    app.create_annotation(0, 5, "check the opening".into());
    assert_eq!(app.annotations.len(), 1);

    app.on_key(ctrl('s')); // save the chapter

    // The chapter file stays byte-for-byte clean — the note never enters the
    // prose, which is why exports (they read the chapter bodies) never carry it.
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        "first chapter\n"
    );
    // The note lives in the side file instead, keyed by chapter.
    let side = fs::read_to_string(dir.join("annotations.toml")).unwrap();
    assert!(
        side.contains("check the opening") && side.contains("one.md"),
        "{side}"
    );

    // It survives a reload of the project.
    let reopened = Project::open(&dir).unwrap();
    let live = reopened.annotations_for("one.md");
    assert_eq!(live.len(), 1);
    assert_eq!(
        (live[0].start, live[0].end, live[0].note.as_str()),
        (0, 5, "check the opening")
    );
}

#[test]
fn annotating_via_the_prompt_anchors_the_word_at_the_cursor() {
    let (mut app, _dir) = two_chapter_app("anno-prompt");
    // Cursor inside "chapter" (bytes 6..13 of "first chapter").
    app.buffer.cursor = Cursor { line: 0, col: 8 };
    app.start_annotation();
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "the note prompt opened"
    );

    type_str(&mut app, "is this the right word");
    app.on_key(key(KeyCode::Enter));

    assert_eq!(app.annotations.len(), 1);
    let a = &app.annotations[0];
    assert_eq!(
        (a.start, a.end),
        (6, 13),
        "the whole word under the cursor is anchored"
    );
    assert_eq!(a.note, "is this the right word");
}

#[test]
fn an_empty_note_creates_nothing() {
    let (mut app, _dir) = two_chapter_app("anno-empty");
    app.start_annotation_range(0, 5);
    app.on_key(key(KeyCode::Enter)); // confirm with no text
    assert!(app.annotations.is_empty(), "a blank note is discarded");
}

#[test]
fn anchor_slides_when_text_is_inserted_before_it() {
    let (mut app, _dir) = two_chapter_app("anno-shift");
    app.create_annotation(6, 13, "note on chapter".into()); // "chapter"

    // Type three characters at the very start, before the anchored span.
    app.buffer.cursor = Cursor { line: 0, col: 0 };
    type_str(&mut app, "XYZ");
    app.on_idle(); // re-anchoring runs off the keystroke path

    let a = &app.annotations[0];
    assert_eq!(
        (a.start, a.end),
        (9, 16),
        "the note slid right by the inserted text"
    );
}

#[test]
fn deleting_the_anchored_text_drops_the_note() {
    let (mut app, _dir) = two_chapter_app("anno-drop");
    app.create_annotation(0, 5, "note".into()); // "first"

    // Erase the whole anchored word.
    app.buffer.cursor = Cursor { line: 0, col: 5 };
    for _ in 0..5 {
        app.on_key(key(KeyCode::Backspace));
    }
    app.on_idle();

    assert!(
        app.annotations.is_empty(),
        "a note whose text is gone is dropped"
    );
}

#[test]
fn review_list_jumps_to_and_deletes_annotations() {
    let (mut app, _dir) = two_chapter_app("anno-review");
    app.create_annotation(0, 5, "first note".into());
    app.create_annotation(6, 13, "second note".into());

    app.open_annotation_review();
    assert!(matches!(app.mode, Mode::Annotations { selected: 0, .. }));

    // Move to the second annotation and jump — the cursor lands on its start.
    app.on_key(key(KeyCode::Down));
    app.on_key(key(KeyCode::Enter));
    assert_eq!(
        app.buffer.cursor_byte(),
        6,
        "Enter jumps the cursor to the annotation"
    );

    // Reopen and delete the first; one remains.
    app.open_annotation_review();
    app.on_key(key(KeyCode::Char('d')));
    assert_eq!(app.annotations.len(), 1);
    assert_eq!(app.annotations[0].note, "second note");
}

#[test]
fn annotations_can_be_created_and_anchored_in_the_continuous_view() {
    let (mut app, dir) = three_chapter_app("anno-continuous");
    app.toggle_continuous();
    assert!(app.manuscript.is_some(), "in the continuous view");

    // Annotate "alpha" (buffer bytes 0..5) — the first loaded chapter.
    app.start_annotation_range(0, 5);
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "the note prompt opens in continuous view"
    );
    type_str(&mut app, "first note");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.annotations.len(), 1);
    assert_eq!(
        app.annotations[0].chapter, "c0.md",
        "tagged to the chapter that owns the span"
    );

    // Saving writes it to that chapter's side file, translated chapter-local.
    app.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    let reopened = Project::open(&dir).unwrap();
    let live = reopened.annotations_for("c0.md");
    assert_eq!(live.len(), 1);
    assert_eq!(
        (live[0].start, live[0].end),
        (0, 5),
        "stored chapter-local, not buffer-absolute"
    );
}

#[test]
fn an_edit_re_anchors_a_later_chapters_note_in_the_continuous_view() {
    let (mut app, dir) = three_chapter_app("anno-continuous-edit");
    app.toggle_continuous();
    // "beta" is the second loaded chapter, after "alpha\n" (6 bytes).
    let beta_start = app.manuscript.as_ref().unwrap().segment_spans()[1].1;
    app.start_annotation_range(beta_start, beta_start + 4); // "beta"
    type_str(&mut app, "n");
    app.on_key(key(KeyCode::Enter));
    assert_eq!(app.annotations[0].chapter, "c1.md");

    // Type three chars at the very start (chapter 0); the note in chapter 1 slides.
    app.edit_buffer_mut().cursor = Cursor { line: 0, col: 0 };
    type_str(&mut app, "XYZ");
    app.on_idle();
    assert_eq!(
        app.annotations[0].start,
        beta_start + 3,
        "the later note re-anchored"
    );

    // It still persists chapter-local (unshifted) to its own chapter on save.
    app.on_key(KeyEvent::new(KeyCode::Char('s'), KeyModifiers::CONTROL));
    let reopened = Project::open(&dir).unwrap();
    let c1 = reopened.annotations_for("c1.md");
    assert_eq!(
        (c1[0].start, c1[0].end),
        (0, 4),
        "chapter-local offset unchanged by ch 0's edit"
    );
}

#[test]
fn undo_and_redo_move_annotations_with_the_text() {
    let (mut app, _dir) = two_chapter_app("anno-undo");
    // "first chapter\n" — annotate "chapter" (bytes 6..13).
    app.create_annotation(6, 13, "note on chapter".into());

    // Type three chars at the very start; on idle the note re-anchors right.
    app.buffer.cursor = Cursor { line: 0, col: 0 };
    type_str(&mut app, "XYZ");
    app.on_idle();
    assert_eq!(
        (app.annotations[0].start, app.annotations[0].end),
        (9, 16),
        "slid right by the insert"
    );

    // Undo the insertion → the note returns to its original span (#370).
    app.on_key(ctrl('z'));
    assert_eq!(
        (app.annotations[0].start, app.annotations[0].end),
        (6, 13),
        "undo restores the anchor"
    );

    // Redo → it follows the text forward again.
    app.on_key(ctrl('y'));
    assert_eq!(
        (app.annotations[0].start, app.annotations[0].end),
        (9, 16),
        "redo re-applies"
    );
}

/// The annotation list's `(selected, filter, typing)` state.
fn anno_state(app: &App) -> (usize, String, bool) {
    match &app.mode {
        Mode::Annotations {
            selected,
            filter,
            typing,
            ..
        } => (*selected, filter.clone(), *typing),
        _ => panic!("expected the annotation list"),
    }
}

#[test]
fn slash_filters_annotations_by_note_text_and_enter_jumps() {
    // "first chapter\n": "first" = 0..5, "chapter" = 6..13.
    let (mut app, _dir) = two_chapter_app("anno-filter-jump");
    app.create_annotation(0, 5, "opening beat".into());
    app.create_annotation(6, 13, "closing beat".into());
    app.open_annotation_review();
    assert!(matches!(app.mode, Mode::Annotations { selected: 0, .. }));

    // `/` then "closing" narrows to the second note; the selection snaps to it.
    app.on_key(key(KeyCode::Char('/')));
    assert!(anno_state(&app).2);
    type_str(&mut app, "closing");
    assert_eq!(anno_state(&app).0, 1, "selection follows the only match");

    // Enter leaves the input sub-mode, then acts on the match: jump to byte 6.
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Enter));
    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(app.buffer.cursor_byte(), 6);
}

#[test]
fn annotation_filter_matches_the_anchored_snippet_too() {
    // The snippet ("chapter") should match even when the note text doesn't.
    let (mut app, _dir) = two_chapter_app("anno-filter-snippet");
    app.create_annotation(0, 5, "note A".into()); // anchors "first"
    app.create_annotation(6, 13, "note B".into()); // anchors "chapter"
    app.open_annotation_review();

    app.on_key(key(KeyCode::Char('/')));
    type_str(&mut app, "chapter"); // only the second note's snippet matches
    assert_eq!(anno_state(&app).0, 1);
}

#[test]
fn deleting_a_filtered_annotation_removes_the_right_one() {
    let (mut app, _dir) = two_chapter_app("anno-filter-delete");
    app.create_annotation(0, 5, "keep me".into());
    app.create_annotation(6, 13, "drop me".into());
    app.open_annotation_review();

    app.on_key(key(KeyCode::Char('/')));
    type_str(&mut app, "drop");
    app.on_key(key(KeyCode::Enter)); // leave input, keep the filter
    app.on_key(key(KeyCode::Char('d'))); // delete the filtered match

    assert_eq!(app.annotations.len(), 1);
    assert_eq!(app.annotations[0].note, "keep me");
}

#[test]
fn annotation_esc_clears_the_filter_then_closes() {
    let (mut app, _dir) = two_chapter_app("anno-filter-esc");
    app.create_annotation(0, 5, "alpha".into());
    app.create_annotation(6, 13, "beta".into());
    app.open_annotation_review();

    app.on_key(key(KeyCode::Char('/')));
    type_str(&mut app, "beta");
    app.on_key(key(KeyCode::Esc)); // leave input, keep filter
    assert_eq!(anno_state(&app).1, "beta");

    app.on_key(key(KeyCode::Esc)); // clear filter; list restored
    assert_eq!(anno_state(&app).1, "");

    app.on_key(key(KeyCode::Esc)); // close
    assert!(matches!(app.mode, Mode::Insert));
}

// --- Annotation threads: reply + resolve (#889) -------------------------------

/// The review list's resolved/open filter state.
fn anno_show(app: &App) -> AnnoShow {
    match &app.mode {
        Mode::Annotations { show, .. } => *show,
        _ => panic!("expected the annotation list"),
    }
}

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

// --- Batched remap keeps positions identical + is not quadratic (#893) --------

#[test]
fn a_reply_thread_and_resolved_flag_survive_an_edit_burst() {
    // Threads ride along with the anchor through the idle remap: the replies and the
    // resolved flag aren't disturbed when the note re-anchors (#889 + #893).
    let (mut app, _dir) = two_chapter_app("anno-thread-remap");
    app.create_annotation(6, 13, "note on chapter".into()); // "chapter"
    app.open_annotation_review();
    app.on_key(key(KeyCode::Char('a')));
    type_str(&mut app, "keep me");
    app.on_key(key(KeyCode::Enter));
    app.on_key(key(KeyCode::Char('r'))); // resolve
    app.on_key(key(KeyCode::Esc)); // back to editing

    app.buffer.cursor = Cursor { line: 0, col: 0 };
    type_str(&mut app, "XYZ");
    app.on_idle();

    let a = &app.annotations[0];
    assert_eq!((a.start, a.end), (9, 16), "re-anchored past the insert");
    assert_eq!(a.replies.len(), 1, "reply preserved through the remap");
    assert_eq!(a.replies[0].text, "keep me");
    assert!(a.resolved, "resolved flag preserved through the remap");
}

/// The old per-edit remap: walk every annotation once per edit, dropping any whose
/// text is consumed. The batched [`Remapper`] must produce identical positions.
fn remap_naive(anns: &mut Vec<(usize, usize)>, edits: &[(usize, usize, usize)]) {
    for &e in edits {
        anns.retain_mut(|r| match crate::annotation::remap(*r, e) {
            Some((s, en)) => {
                *r = (s, en);
                true
            }
            None => false,
        });
    }
}

#[test]
fn batched_remap_matches_the_per_edit_walk_on_many_annotations_and_edits() {
    // Many annotations spread across a buffer, many edits: the one-pass Remapper
    // must land every note exactly where the old walk-per-edit did (#893).
    let mut seed = 0x2545_F491_4F6C_DD1Du64;
    let mut rng = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..40 {
        let mut anns: Vec<(usize, usize)> = (0..200)
            .map(|i| {
                let s = i * 3;
                (s, s + 2)
            })
            .collect();
        let edits: Vec<(usize, usize, usize)> = (0..150)
            .map(|_| {
                let start = (rng() % 600) as usize;
                let old_len = (rng() % 4) as usize;
                let new_len = (rng() % 4) as usize;
                (start, start + old_len, start + new_len)
            })
            .collect();

        let mut expected = anns.clone();
        remap_naive(&mut expected, &edits);

        let mapper = Remapper::new(&edits);
        anns.retain_mut(|r| match mapper.remap_range(r.0, r.1) {
            Some((s, e)) => {
                *r = (s, e);
                true
            }
            None => false,
        });

        assert_eq!(anns, expected, "batched remap diverged for edits {edits:?}");
    }
}

#[test]
#[ignore = "release-only perf guard; run via cargo perf"]
fn batched_remap_is_not_quadratic_in_annotations_times_edits() {
    use std::time::Instant;

    // The defect (#893) was the per-annotation cost: the old code walked *every*
    // annotation once per edit, so re-anchoring N notes through E edits cost N·E.
    // The Remapper builds the composed edit map once, then answers each annotation
    // with a binary search — so the annotation pass is linear in N, independent of
    // the edit count. Measure a fixed edit burst against a growing annotation set
    // and confirm the remap pass scales with N, not N·E.
    let remap_pass = |n_anns: usize, edits: &[(usize, usize, usize)]| -> std::time::Duration {
        let span = n_anns * 6 + 1;
        let mut anns: Vec<(usize, usize)> = (0..n_anns).map(|i| (i * 6, i * 6 + 3)).collect();
        let mapper = Remapper::new(edits);
        // Time only the per-annotation remap — the part that used to be O(edits ×
        // annotations). Building the map is done once, up front.
        let t = Instant::now();
        anns.retain_mut(|r| match mapper.remap_range(r.0.min(span), r.1.min(span)) {
            Some((s, e)) => {
                *r = (s, e);
                true
            }
            None => false,
        });
        t.elapsed()
    };

    // A fixed 2,000-edit burst spread across the buffer.
    let edits: Vec<(usize, usize, usize)> = (0..2000)
        .map(|i| {
            let start = (i * 13) % 60_001;
            (start, start + 1, start + 2)
        })
        .collect();

    let _ = remap_pass(1000, &edits); // warm
    let small = remap_pass(5_000, &edits);
    let big = remap_pass(20_000, &edits);
    println!("remap pass: 5k anns = {small:?}, 20k anns = {big:?} (edits fixed at 2000)");

    // 4× the annotations must not cost anywhere near 4× · (edit growth) — with the
    // old N·E walk, holding E fixed, 4× N is 4× work; the guard is that 20k stays a
    // small multiple of 5k (linear), and both are well within a frame. A quadratic
    // regression (re-introducing the per-edit walk) would make `big` blow past this.
    assert!(big.as_millis() < 16, "remap pass took {big:?}");
    assert!(
        big < small * 8,
        "remap should scale ~linearly with annotations: 5k={small:?}, 20k={big:?}"
    );
}

// --- Multi-reviewer triage: sort, filter, jump-to-open (#933) -----------------

use crate::annotation::Live;

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

// --- Next / previous OPEN annotation motion (`]a` / `[a`, #1009) --------------

/// A `Live` note in `file` at `[start, end)` with the given resolved state — the
/// seed for the open-annotation motion tests.
fn note(file: &str, start: usize, end: usize, resolved: bool) -> Live {
    Live {
        chapter: file.into(),
        start,
        end,
        note: "n".into(),
        replies: Vec::new(),
        resolved,
        reviewer: None,
    }
}

#[test]
fn next_open_annotation_steps_only_over_open_notes() {
    // "first chapter\n" — three notes: open @0, resolved @6, open @9. `]a` skips
    // the resolved one.
    let (mut app, _dir) = two_chapter_app("anno-next-open");
    app.annotations = vec![
        note("one.md", 0, 5, false),
        note("one.md", 6, 8, true),
        note("one.md", 9, 12, false),
    ];
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.jump_to_open_annotation(true);
    assert_eq!(
        app.buffer.cursor_byte(),
        9,
        "skipped the resolved note at 6"
    );
    // Nothing open beyond 9 in this chapter or a later one — reports the end.
    app.jump_to_open_annotation(true);
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("No next open annotation"),
        "{:?}",
        app.message
    );
}

#[test]
fn previous_open_annotation_walks_back_over_open_notes() {
    let (mut app, _dir) = two_chapter_app("anno-prev-open");
    app.annotations = vec![
        note("one.md", 0, 5, false),
        note("one.md", 6, 8, true),
        note("one.md", 9, 12, false),
    ];
    app.buffer.cursor = app.buffer.cursor_at_byte(app.buffer.len_bytes());
    app.jump_to_open_annotation(false);
    assert_eq!(app.buffer.cursor_byte(), 9, "nearest open note going back");
    app.jump_to_open_annotation(false);
    assert_eq!(
        app.buffer.cursor_byte(),
        0,
        "skipped the resolved note at 6"
    );
}

#[test]
fn next_open_annotation_crosses_into_the_next_chapter() {
    // Chapter 0's only open note is behind the cursor; chapter 1 has an open note.
    let (mut app, _dir) = two_chapter_app("anno-cross-next");
    app.annotations = vec![note("one.md", 0, 5, false)];
    // Chapter 1's notes live in the project's persisted set (an unloaded chapter).
    app.project
        .set_chapter_annotations("two.md", &[note("two.md", 7, 13, false)])
        .unwrap();
    // Past chapter 0's single note.
    app.buffer.cursor = app.buffer.cursor_at_byte(app.buffer.len_bytes());
    app.jump_to_open_annotation(true);
    assert_eq!(app.project.active_index(), 1, "crossed into chapter 1");
    assert_eq!(
        app.buffer.cursor_byte(),
        7,
        "landed on chapter 1's open note"
    );
}

#[test]
fn open_annotation_crossing_skips_a_chapter_with_only_resolved_notes() {
    // Chapter 1 has only a resolved note; the forward crossing skips it. (A three-
    // chapter project so there's a chapter 2 to land in.)
    let dir = app_scratch_dir("anno-skip-resolved");
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n\n\
         [[chapters]]\ntitle = \"Three\"\nfile = \"three.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "first chapter\n").unwrap();
    fs::write(dir.join("two.md"), "second chapter\n").unwrap();
    fs::write(dir.join("three.md"), "third chapter\n").unwrap();
    let mut app = open_test_app(&dir);
    let _tmp = TempProject::new(dir);

    app.annotations = vec![note("one.md", 0, 5, false)];
    app.project
        .set_chapter_annotations("two.md", &[note("two.md", 0, 6, true)])
        .unwrap();
    app.project
        .set_chapter_annotations("three.md", &[note("three.md", 2, 7, false)])
        .unwrap();
    app.buffer.cursor = app.buffer.cursor_at_byte(app.buffer.len_bytes());
    app.jump_to_open_annotation(true);
    assert_eq!(
        app.project.active_index(),
        2,
        "skipped chapter 1's resolved-only notes"
    );
    assert_eq!(
        app.buffer.cursor_byte(),
        2,
        "landed on chapter 2's open note"
    );
}

#[test]
fn open_annotation_motion_reports_when_none_open_remain() {
    // A single resolved note: nothing open in either direction from anywhere.
    let (mut app, _dir) = two_chapter_app("anno-none-open");
    app.annotations = vec![note("one.md", 3, 8, true)];
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    let before = app.buffer.cursor_byte();
    app.jump_to_open_annotation(true);
    assert_eq!(app.buffer.cursor_byte(), before, "cursor unmoved");
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("No next open annotation"),
        "{:?}",
        app.message
    );
    app.jump_to_open_annotation(false);
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("No previous open annotation"),
        "{:?}",
        app.message
    );
}

#[test]
fn open_annotation_motion_works_without_opening_the_review_overlay() {
    let (mut app, _dir) = two_chapter_app("anno-no-overlay");
    app.mode = Mode::Insert;
    app.annotations = vec![note("one.md", 0, 5, false), note("one.md", 8, 12, false)];
    app.buffer.cursor = app.buffer.cursor_at_byte(0);
    app.jump_to_open_annotation(true);
    assert!(
        matches!(app.mode, Mode::Insert),
        "stayed in the editor — no review overlay"
    );
    assert_eq!(app.buffer.cursor_byte(), 8, "moved to the next open note");
}

#[test]
fn crossing_open_annotation_pushes_the_jumplist_so_back_jump_returns() {
    let (mut app, _dir) = two_chapter_app("anno-jumpback");
    app.annotations = vec![note("one.md", 0, 5, false)];
    app.project
        .set_chapter_annotations("two.md", &[note("two.md", 0, 6, false)])
        .unwrap();
    app.buffer.cursor = app.buffer.cursor_at_byte(app.buffer.len_bytes());
    app.jump_to_open_annotation(true); // crosses to chapter 1
    assert_eq!(app.project.active_index(), 1, "crossed forward");
    app.jump_back();
    assert_eq!(
        app.project.active_index(),
        0,
        "back-jump returns to chapter 0"
    );
}

// --- Re-anchor before a keystroke-path persist (continuous-view drift) ---------
//
// Annotations are buffer-absolute offsets, reconciled with the current
// buffer/window only by `sync_annotations` on the idle tick. Any persist on the
// KEYSTROKE path (a reslide, entering/leaving the continuous view, a chapter
// switch) fires after the buffer/window/text has already moved, so it must
// re-anchor the live offsets to the current buffer first — otherwise it writes a
// stale offset to disk. These three tests pin the three ways that used to drift.

/// A five-chapter project (`c0..c4.md`), each a single line, for the
/// window-paging tests: the continuous window holds three chapters, so walking
/// down the manuscript pages it more than once in a single input burst.
fn five_chapter_app(name: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    let mut manifest = String::from("title = \"Book\"\n\n");
    for (i, body) in ["alpha\n", "beta\n", "gamma\n", "delta\n", "epsilon\n"]
        .iter()
        .enumerate()
    {
        write!(
            manifest,
            "[[chapters]]\ntitle = \"C{i}\"\nfile = \"c{i}.md\"\n\n"
        )
        .unwrap();
        fs::write(dir.join(format!("c{i}.md")), body).unwrap();
    }
    fs::write(Project::manifest_path(&dir), manifest).unwrap();
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

#[test]
fn a_second_reslide_in_one_burst_persists_the_correct_offset() {
    // #4: in the continuous view, a note on a chapter that pages out of the window
    // during a single input burst must keep its correct on-disk offset. The window
    // holds three chapters; three Downs walk the cursor down far enough to reslide
    // twice. Before the fix, the first reslide left `self.annotations` at the old
    // window's absolute offsets, so the second reslide's persist wrote those stale
    // offsets against the new window's bases — drifting chapter 1's note by len(c0).
    let (mut app, dir) = five_chapter_app("anno-double-reslide");
    app.toggle_continuous();
    assert_eq!(
        app.manuscript.as_ref().unwrap().loaded_chapters(),
        vec![0, 1, 2],
        "the window opens on the first three chapters"
    );

    // Annotate "beta" — the second loaded chapter (c1), buffer bytes [base, base+4).
    let beta_start = app.manuscript.as_ref().unwrap().segment_spans()[1].1;
    app.create_annotation(beta_start, beta_start + 4, "note on beta".into());
    assert_eq!(app.annotations[0].chapter, "c1.md");

    // Three Downs in one burst — no idle tick between them — crossing two reslides.
    for _ in 0..3 {
        app.on_key(key(KeyCode::Down));
    }
    // The window has paged past c1, so the idle re-anchor can't rescue its on-disk
    // offset (it's no longer loaded); the keystroke-path persists had to be right.

    let reopened = Project::open(&dir).unwrap();
    let c1 = reopened.annotations_for("c1.md");
    assert_eq!(c1.len(), 1);
    assert_eq!(
        (c1[0].start, c1[0].end),
        (0, 4),
        "chapter 1's note kept its chapter-local offset across two reslides"
    );
}

#[test]
fn entering_continuous_flushes_the_reanchored_offset() {
    // #11: single view — annotate, type before the note, let the idle tick
    // re-anchor it in memory (but not to disk), then Ctrl-L into the continuous
    // view WITHOUT a manual save. Entering flushes the prose; it must flush the
    // re-anchored annotation offset too, or the continuous view's reload overwrites
    // the correct in-memory offset with the stale on-disk one.
    let (mut app, _dir) = two_chapter_app("anno-enter-continuous-flush");
    // "first chapter\n" — annotate "chapter" (bytes 6..13).
    app.create_annotation(6, 13, "note on chapter".into());

    // Type three chars at the very start; on idle the note re-anchors to 9..16 in
    // memory — but the on-disk offset is still the pre-edit 6..13.
    app.buffer.cursor = Cursor { line: 0, col: 0 };
    type_str(&mut app, "XYZ");
    app.on_idle();
    assert_eq!(
        (app.annotations[0].start, app.annotations[0].end),
        (9, 16),
        "re-anchored in memory on idle"
    );

    // Ctrl-L into the continuous view — no manual Ctrl-S first.
    app.on_key(ctrl('l'));
    assert!(app.manuscript.is_some(), "entered the continuous view");
    // The continuous view reloads annotations from disk on the next idle tick.
    app.on_idle();

    let a = &app.annotations[0];
    assert_eq!(
        app.edit_buffer().byte_slice(a.start, a.end),
        "chapter",
        "the note still covers its phrase after the continuous reload"
    );
}

#[test]
fn leaving_continuous_persists_the_burst_delta() {
    // #15: in the continuous view, annotate a word, then in the SAME burst (no idle
    // tick) type before it and Ctrl-L back to single view. Leaving is a
    // keystroke-path save; it must fold the burst's pending edit into the offset it
    // flushes, or it writes the pre-edit offset and the note drifts by the delta.
    let dir = app_scratch_dir("anno-leave-continuous-delta");
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "alpha beta\n").unwrap();
    fs::write(dir.join("two.md"), "second chapter\n").unwrap();
    let mut app = open_test_app(&dir);
    let _tmp = TempProject::new(dir.clone());

    app.toggle_continuous();
    // Annotate "beta" in the first loaded chapter (buffer bytes 6..10).
    app.create_annotation(6, 10, "note on beta".into());
    assert_eq!(app.annotations[0].chapter, "one.md");

    // Same burst, no idle: type three chars at the very start, before the note.
    app.edit_buffer_mut().cursor = Cursor { line: 0, col: 0 };
    type_str(&mut app, "XYZ");
    // Ctrl-L leaves the continuous view straight away.
    app.on_key(ctrl('l'));
    assert!(app.manuscript.is_none(), "back in the single-chapter view");

    let reopened = Project::open(&dir).unwrap();
    let c0 = reopened.annotations_for("one.md");
    assert_eq!(c0.len(), 1);
    assert_eq!(
        (c0[0].start, c0[0].end),
        (9, 13),
        "the flushed offset reflects the burst's insert before the note"
    );
}
