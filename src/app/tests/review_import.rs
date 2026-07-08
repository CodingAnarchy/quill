//! Importing a marked-up review copy (#886): the return leg of the review-copy
//! export (#834). A `.docx` carrying the reviewer's Word comments is read, each
//! comment's commented phrase is re-anchored into the active chapter's prose as a
//! live annotation, and comments whose phrase no longer matches (the prose
//! drifted) are reported rather than silently lost. The author reviews the resolve
//! first, then merges.

use super::*;
use crate::export::docx::{to_docx_review, ChapterNotes, ReviewNote};
use crate::export::{Chapter, Document, Part, PubMeta};
use crate::mode::{Mode, Pending};
use crate::project::Project;

/// Build a review-copy `.docx` with one chapter carrying `notes` as Word comments
/// (the #834 writer), so an import test round-trips real comment plumbing rather
/// than a hand-rolled fixture. `body` is the chapter prose the annotations were
/// left on.
fn review_docx(body: &str, notes: Vec<ReviewNote>) -> Vec<u8> {
    let doc = Document {
        title: "Salt and Iron".into(),
        subtitle: None,
        author: None,
        series: None,
        publishing: PubMeta::default(),
        epigraph: None,
        half_title: false,
        series_books: Vec::new(),
        front: vec![],
        parts: vec![Part {
            name: None,
            chapters: vec![Chapter {
                title: "One".into(),
                epigraph: None,
                body: body.into(),
                role: None,
            }],
        }],
        back: vec![],
    };
    let chapter_notes: Vec<ChapterNotes> = vec![notes];
    to_docx_review(&doc, &chapter_notes).unwrap()
}

/// Write `bytes` to a unique temp `.docx` and return its path.
fn temp_docx(tag: &str, bytes: &[u8]) -> std::path::PathBuf {
    let path = std::env::temp_dir().join(format!("quill-review-{tag}-{}.docx", std::process::id()));
    fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn importing_review_comments_anchors_them_as_annotations() {
    isolate("review-import-anchor");
    // The active chapter's prose; the review copy's comments were left on phrases
    // within it, so they re-anchor exactly.
    let dir = make_project(
        "review-import-anchor-p",
        "Salt and Iron",
        "The sea was grey and it ended.\n",
    );
    let mut app = open_app(&dir);

    let bytes = review_docx(
        "The sea was grey and it ended.",
        vec![
            ReviewNote {
                text: "grey".into(),
                note: "vivid".into(),
            },
            ReviewNote {
                text: "it ended".into(),
                note: "abrupt".into(),
            },
        ],
    );
    let source = temp_docx("anchor", &bytes);
    let original = fs::read(&source).unwrap();

    app.begin_import_review_comments(source.to_str().unwrap());
    // The resolve opens the review-import preview before anything is merged.
    let matched = match &app.mode {
        Mode::ReviewImport(p) => {
            assert_eq!(p.matched.len(), 2, "both comments matched the prose");
            assert!(p.dropped.is_empty(), "nothing was dropped");
            p.matched.clone()
        }
        _ => panic!("the review-import preview should open"),
    };
    // No annotation is created until the merge is confirmed.
    assert!(app.annotations.is_empty(), "merge is deferred to confirm");

    app.confirm_review_import();
    assert!(!matches!(app.mode, Mode::ReviewImport(_)), "preview closed");
    assert_eq!(app.annotations.len(), 2, "both comments became annotations");

    // Each annotation is anchored to its phrase's exact byte span.
    let prose = "The sea was grey and it ended.\n";
    let span = |needle: &str| {
        let s = prose.find(needle).unwrap();
        (s, s + needle.len())
    };
    let by_note = |note: &str| {
        app.annotations
            .iter()
            .find(|a| a.note == note)
            .map(|a| (a.start, a.end))
    };
    assert_eq!(by_note("vivid"), Some(span("grey")));
    assert_eq!(by_note("abrupt"), Some(span("it ended")));

    // The matches carried the resolved spans through to the merge unchanged.
    assert!(matched
        .iter()
        .any(|(s, e, n, _)| (*s, *e) == span("grey") && n == "vivid"));

    // The imported annotations persist to the side file, keyed by chapter.
    app.persist_annotations();
    let side = fs::read_to_string(dir.join("annotations.toml")).unwrap();
    assert!(side.contains("vivid") && side.contains("one.md"), "{side}");

    // The source `.docx` is read-only.
    assert_eq!(
        fs::read(&source).unwrap(),
        original,
        "the source is left untouched"
    );
    let _ = fs::remove_file(&source);
}

#[test]
fn a_comment_that_no_longer_matches_the_prose_is_reported() {
    isolate("review-import-drift");
    // The prose has changed since the review copy went out: "grey" is now "gray",
    // so the comment on "grey" can't re-anchor.
    let dir = make_project(
        "review-import-drift-p",
        "Salt and Iron",
        "The sea was gray and it ended.\n",
    );
    let mut app = open_app(&dir);

    let bytes = review_docx(
        "The sea was grey and it ended.",
        vec![
            ReviewNote {
                text: "grey".into(),
                note: "vivid".into(),
            },
            ReviewNote {
                text: "it ended".into(),
                note: "abrupt".into(),
            },
        ],
    );
    let source = temp_docx("drift", &bytes);

    app.begin_import_review_comments(source.to_str().unwrap());
    match &app.mode {
        Mode::ReviewImport(p) => {
            assert_eq!(p.matched.len(), 1, "only the still-present phrase matched");
            assert_eq!(p.dropped.len(), 1, "the drifted phrase is reported");
            assert_eq!(p.dropped[0].0, "grey", "with the phrase that didn't match");
            assert_eq!(p.dropped[0].1, "vivid");
        }
        _ => panic!("the review-import preview should open"),
    }

    app.confirm_review_import();
    assert_eq!(app.annotations.len(), 1, "only the match was merged");
    let msg = app.message.clone().unwrap_or_default();
    assert!(
        msg.contains("1 unmatched"),
        "the drop is surfaced in the report: {msg}"
    );
    let _ = fs::remove_file(&source);
}

#[test]
fn an_imported_comment_carries_its_reviewer_onto_the_annotation() {
    // The reviewer identity a named `.docx` comment carries (#933) rides the merge
    // through to the created annotation and persists. Driven from the review-import
    // preview with a reviewer on the matched span (what `extract_comments` fills in
    // from `w:author`, covered end-to-end in the docx unit tests).
    use crate::mode::ReviewImport;

    isolate("review-import-reviewer");
    let dir = make_project(
        "review-import-reviewer-p",
        "Salt and Iron",
        "The sea was grey and it ended.\n",
    );
    let mut app = open_app(&dir);

    // "grey" is at bytes 12..16; attribute the comment to a named editor.
    app.mode = Mode::ReviewImport(Box::new(ReviewImport {
        source: std::path::PathBuf::from("/tmp/review.docx"),
        chapter: "one.md".into(),
        matched: vec![(12, 16, "vivid".into(), Some("Jane Editor".into()))],
        dropped: Vec::new(),
        scroll: 0,
    }));

    app.confirm_review_import();
    assert_eq!(app.annotations.len(), 1);
    assert_eq!(
        app.annotations[0].reviewer.as_deref(),
        Some("Jane Editor"),
        "the reviewer identity carries onto the annotation"
    );

    // It persists and reloads with the reviewer intact.
    app.persist_annotations();
    let reopened = Project::open(&dir).unwrap();
    let live = reopened.annotations_for("one.md");
    assert_eq!(live[0].reviewer.as_deref(), Some("Jane Editor"));
}

#[test]
fn a_docx_with_no_comments_is_a_clean_no_op() {
    isolate("review-import-empty");
    let dir = make_project("review-import-empty-p", "Salt and Iron", "Plain prose.\n");
    let mut app = open_app(&dir);

    // A review copy with no annotations carries no comments part at all.
    let bytes = review_docx("Plain prose.", vec![]);
    let source = temp_docx("empty", &bytes);

    app.begin_import_review_comments(source.to_str().unwrap());
    assert!(
        !matches!(app.mode, Mode::ReviewImport(_)),
        "no preview opens for a comment-less .docx"
    );
    assert!(app.annotations.is_empty(), "nothing imported");
    let msg = app.message.clone().unwrap_or_default();
    assert!(
        msg.contains("No review comments"),
        "reported as a no-op: {msg}"
    );
    let _ = fs::remove_file(&source);
}

#[test]
fn a_non_docx_source_is_rejected() {
    isolate("review-import-nondocx");
    let dir = make_project("review-import-nondocx-p", "Salt and Iron", "Prose.\n");
    let mut app = open_app(&dir);

    let source = std::env::temp_dir().join(format!("quill-review-not-{}.txt", std::process::id()));
    fs::write(&source, "not a docx").unwrap();
    app.begin_import_review_comments(source.to_str().unwrap());
    assert!(!matches!(app.mode, Mode::ReviewImport(_)));
    assert!(app
        .message
        .clone()
        .unwrap_or_default()
        .contains("Not a .docx"));
    let _ = fs::remove_file(&source);
}

#[test]
fn the_import_action_opens_the_docx_prompt() {
    isolate("review-import-action");
    let dir = make_project("review-import-action-p", "Salt and Iron", "Prose.\n");
    let mut app = open_app(&dir);

    app.run_action(crate::mode::Action::ImportReviewComments, false);
    assert!(
        matches!(&app.mode, Mode::Prompt(s) if matches!(s.pending, Pending::ImportReviewComments)),
        "the .docx path prompt opens"
    );
}

#[test]
fn importing_reloads_the_project_annotations() {
    // End-to-end: after a merge + save, reopening the project sees the imported
    // annotations on their phrases — the same side-file round-trip #302 guarantees.
    isolate("review-import-reload");
    let dir = make_project(
        "review-import-reload-p",
        "Salt and Iron",
        "The sea was grey and it ended.\n",
    );
    let mut app = open_app(&dir);
    let bytes = review_docx(
        "The sea was grey and it ended.",
        vec![ReviewNote {
            text: "grey".into(),
            note: "vivid".into(),
        }],
    );
    let source = temp_docx("reload", &bytes);
    app.begin_import_review_comments(source.to_str().unwrap());
    app.confirm_review_import();
    app.persist_annotations();

    let reopened = Project::open(&dir).unwrap();
    let live = reopened.annotations_for("one.md");
    assert_eq!(live.len(), 1);
    assert_eq!(live[0].note, "vivid");
    let s = "The sea was grey and it ended.\n".find("grey").unwrap();
    assert_eq!((live[0].start, live[0].end), (s, s + "grey".len()));
    let _ = fs::remove_file(&source);
}
