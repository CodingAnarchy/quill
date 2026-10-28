use std::fs;
use std::path::Path;

use quill_core::annotation::Live;
use quill_core::project::Project;
use quill_core::testutil::scratch_dir;

use super::*;

const FRONT: &str = "---\nstatus: draft\nsummary: \"The 🙂 chapter\"\npov: Mara\n---\n";

fn project(dir: &Path, body: &str) -> Arc<QuillProject> {
    fs::write(
        dir.join("book.toml"),
        "title = \"Book\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), format!("{FRONT}{body}")).unwrap();
    QuillProject::open(&dir.to_string_lossy()).unwrap()
}

#[test]
fn saving_keeps_front_matter_byte_for_byte() {
    let dir = scratch_dir("ffi-session", "front");
    let session = project(&dir, "The boat came in.\n")
        .edit_chapter(0)
        .unwrap();
    assert_eq!(session.text(), "The boat came in.\n");

    // Insert " late" before the period, as the text view would report it.
    session.apply_edit(16, 16, " late");
    assert!(session.is_dirty());
    session.save().unwrap();
    assert!(!session.is_dirty());

    let on_disk = fs::read_to_string(dir.join("one.md")).unwrap();
    assert_eq!(on_disk, format!("{FRONT}The boat came in late.\n"));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn edits_after_an_emoji_land_where_the_phone_put_them() {
    let dir = scratch_dir("ffi-session", "emoji");
    let session = project(&dir, "🙂 cafe\u{301} soon\n")
        .edit_chapter(0)
        .unwrap();
    // UTF-16: 🙂 = 0..2, space = 2, "café" = 3..8 (e + combining = 2 units),
    // space = 8, "soon" = 9..13.
    assert_eq!(session.utf16_length(), 14);
    session.apply_edit(9, 13, "now");
    assert_eq!(session.text(), "🙂 cafe\u{301} now\n");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_word_count_is_live_and_the_list_updates_on_save() {
    let dir = scratch_dir("ffi-session", "words");
    let project = project(&dir, "One two three.\n");
    let session = Arc::clone(&project).edit_chapter(0).unwrap();
    session.apply_edit(14, 14, " Four five.");
    assert_eq!(session.word_count(), 5);
    assert_eq!(project.chapters().unwrap()[0].words, 3, "not saved yet");
    session.save().unwrap();
    assert_eq!(project.chapters().unwrap()[0].words, 5);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_annotation_stays_on_its_phrase() {
    let dir = scratch_dir("ffi-session", "annotation");
    let body = "The lamp went out. She climbed.\n";
    {
        drop(project(&dir, body)); // write the files, then release the lock
        let mut p = Project::open(&dir).unwrap();
        let start = body.find("She climbed").unwrap();
        let note = Live {
            chapter: "one.md".into(),
            start,
            end: start + "She climbed".len(),
            note: "too quick?".into(),
            replies: Vec::new(),
            resolved: false,
            reviewer: None,
        };
        p.set_annotations([("one.md", std::slice::from_ref(&note))])
            .unwrap();
    }
    let project = QuillProject::open(&dir.to_string_lossy()).unwrap();
    let session = Arc::clone(&project).edit_chapter(0).unwrap();
    // Insert a word earlier in the paragraph, before the noted phrase.
    session.apply_edit(4, 4, "old ");
    session.save().unwrap();

    let reopened = Project::open(&dir).unwrap();
    let notes = reopened.annotations_for("one.md");
    let text = fs::read_to_string(dir.join("one.md")).unwrap();
    let body = text.strip_prefix(FRONT).unwrap();
    assert_eq!(&body[notes[0].start..notes[0].end], "She climbed");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn styles_are_reported_in_utf16() {
    let dir = scratch_dir("ffi-session", "styles");
    let session = project(&dir, "# Night\n\n🙂 a *dark* sky\n")
        .edit_chapter(0)
        .unwrap();
    let spans = session.styles(0, session.utf16_length());
    let heading = spans.iter().find(|s| s.kind == StyleKind::Heading).unwrap();
    assert_eq!((heading.start, heading.length), (0, 8)); // with its newline
                                                         // "*dark*" starts after "# Night\n\n" (9) + "🙂 a " (5 units) = 14.
    let em = spans
        .iter()
        .find(|s| s.kind == StyleKind::Emphasis)
        .unwrap();
    assert_eq!((em.start, em.length), (14, 6));
    let markers: Vec<(u32, u32)> = spans
        .iter()
        .filter(|s| s.kind == StyleKind::Marker)
        .map(|s| (s.start, s.length))
        .collect();
    assert_eq!(markers, [(14, 1), (19, 1)]);

    // After an edit, the styles follow it.
    session.apply_edit(15, 19, "bright");
    let em = session
        .styles(0, session.utf16_length())
        .into_iter()
        .find(|s| s.kind == StyleKind::Emphasis)
        .unwrap();
    assert_eq!((em.start, em.length), (14, 8));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn saving_an_unchanged_chapter_writes_nothing() {
    let dir = scratch_dir("ffi-session", "clean");
    let session = project(&dir, "Same.\n").edit_chapter(0).unwrap();
    fs::write(dir.join("one.md"), "changed elsewhere\n").unwrap();
    session.save().unwrap();
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        "changed elsewhere\n"
    );
    fs::remove_dir_all(&dir).ok();
}

/// The phone's per-keystroke work (#1232): apply the edit, restyle the edited
/// paragraph, and read the live count. It must stay far inside a frame on a
/// 10,000-word chapter, like the desktop's keystroke path.
#[test]
#[ignore = "timing guard: run with `cargo perf` (release build)"]
fn a_keystroke_is_fast_in_a_long_chapter() {
    use std::time::Instant;

    use quill_core::testutil::perf_budget_us;

    let dir = scratch_dir("ffi-session", "perf");
    let paragraph = "She walked the *long* road home, past the **dark** water and the gulls. ";
    let body: String = (0..800)
        .map(|i| format!("{}\n\n", paragraph.repeat(1 + i % 2)))
        .collect();
    let session = project(&dir, &body).edit_chapter(0).unwrap();
    assert!(session.word_count() > 10_000, "{}", session.word_count());

    // Type 200 characters in the middle of the chapter.
    let mut at = session.utf16_length() / 2;
    let start = Instant::now();
    for _ in 0..200 {
        session.apply_edit(at, at, "a");
        at += 1;
        let _ = session.styles(at.saturating_sub(80), at + 80);
        let _ = session.word_count();
    }
    let per = start.elapsed() / 200;
    println!("phone keystroke (edit + paragraph styles + count) on 10k+ words = {per:?}");
    assert!(
        per.as_micros() < perf_budget_us(1_000),
        "a keystroke took {per:?}"
    );
    fs::remove_dir_all(&dir).ok();
}
