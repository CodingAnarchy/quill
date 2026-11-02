use std::fs;
use std::path::PathBuf;

use quill_core::export::assemble::{self, Assembly, Format};
use quill_core::project::Project;
use quill_core::testutil::scratch_dir;

use super::*;

/// A two-chapter novel with emphasis and a scene break.
fn novel(name: &str) -> PathBuf {
    let dir = scratch_dir("ffi-export", name);
    fs::write(
        dir.join("book.toml"),
        "title = \"The Lamp Room\"\nauthor = \"Mara Vey\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .expect("manifest");
    fs::write(
        dir.join("one.md"),
        "The *sea* came in.\n\n* * *\n\nThen dark.\n",
    )
    .expect("one");
    fs::write(dir.join("two.md"), "Morning.\n").expect("two");
    dir
}

#[test]
fn every_kind_exports_a_file_named_for_the_book() {
    let dir = novel("kinds");
    let project = QuillProject::open(&dir.to_string_lossy()).expect("open");
    let out = scratch_dir("ffi-export", "kinds-out");
    for (kind, ext, magic) in [
        (ExportKind::Manuscript, "docx", &b"PK"[..]),
        (ExportKind::Epub, "epub", &b"PK"[..]),
        (ExportKind::Pdf, "pdf", &b"%PDF"[..]),
        (ExportKind::PrintPdf, "print.pdf", &b"%PDF"[..]),
    ] {
        let result = project
            .export(kind, &out.to_string_lossy())
            .expect("export");
        assert!(
            result.path.ends_with(&format!("The Lamp Room.{ext}")),
            "{}",
            result.path
        );
        let bytes = fs::read(&result.path).expect("read the file");
        assert!(bytes.starts_with(magic), "{ext} starts with its magic");
    }
}

#[test]
fn the_phones_file_matches_what_the_desktop_exports() {
    // The desktop compiles with the same core assembly and renderer, so with
    // no live chapter the bytes match (EPUB is left out: it stamps the time).
    let dir = novel("matches");
    let quill = QuillProject::open(&dir.to_string_lossy()).expect("open");
    let out = scratch_dir("ffi-export", "matches-out");
    let phone = quill
        .export(ExportKind::Manuscript, &out.to_string_lossy())
        .expect("export");
    drop(quill);

    let project = Project::open(&dir).expect("open");
    let (style, _) = assemble::resolve_style(&project);
    let doc = Assembly::new(&project, None, &style).document(None);
    let (desktop, _) = assemble::render(&project, &doc, Format::Manuscript);
    assert_eq!(
        fs::read(&phone.path).expect("read"),
        desktop.expect("render")
    );
}

#[test]
fn a_missing_chapter_is_named_with_what_to_do() {
    let dir = novel("missing");
    fs::remove_file(dir.join("two.md")).expect("remove");
    let project = QuillProject::open(&dir.to_string_lossy()).expect("open");
    let out = scratch_dir("ffi-export", "missing-out");
    let err = project
        .export(ExportKind::Epub, &out.to_string_lossy())
        .expect_err("refused");
    let message = err.to_string();
    assert!(
        message.contains("\u{201c}Two\u{201d} (two.md)"),
        "{message}"
    );
    assert!(message.contains("History"), "says what to do: {message}");
}

#[test]
fn a_short_story_exports_as_itself() {
    let dir = scratch_dir("ffi-export", "story");
    let path = crate::create_project(
        "Low Tide",
        crate::ProjectForm::ShortStory,
        &dir.to_string_lossy(),
    )
    .expect("create");
    let project = QuillProject::open(&path).expect("open");
    let out = scratch_dir("ffi-export", "story-out");
    let result = project
        .export(ExportKind::Manuscript, &out.to_string_lossy())
        .expect("export");
    assert!(result.path.ends_with("Low Tide.docx"), "{}", result.path);
}
