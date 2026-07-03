//! Series export (#332), scoped / sample export (#584), and the progress report's
//! series total (#333): compiling every book in order, chapter / part / range
//! samples, the export menu's scopes, and the series word-count line.

use super::*;
use crate::project::Project;
use std::fmt::Write;
use std::io::{Cursor, Read};

// --- Series export (#332) ---------------------------------------------------

#[test]
fn exporting_the_series_compiles_every_book_in_order() {
    use crate::mode::{ExportFormat, ExportScope};
    let (mut app, dir) = series_app("export-series");
    app.run_export(ExportFormat::Markdown, ExportScope::Series);

    // The file is named for the series; it carries both books and their prose.
    let out = fs::read_to_string(dir.join("export/the-saga.md")).unwrap();
    assert!(out.starts_with("The Saga"), "{out}");
    let one = out.find("# Book One").expect("book one");
    let two = out.find("# Book Two").expect("book two");
    assert!(one < two, "books in reading order");
    // Prose from the active book and the disk-read inactive book both appear.
    assert!(
        out.contains("alpha one") && out.contains("alpha two"),
        "active book prose"
    );
    assert!(out.contains("beta one"), "inactive book prose");
}

#[test]
fn exporting_just_the_active_book_is_scoped_to_it() {
    use crate::mode::{ExportFormat, ExportScope};
    let (mut app, dir) = series_app("export-book");
    app.run_export(ExportFormat::Markdown, ExportScope::ActiveBook);
    // Named for the active book; carries its prose, not the other book's.
    let out = fs::read_to_string(dir.join("export/book-one.md")).unwrap();
    assert!(
        out.contains("alpha one") && !out.contains("beta one"),
        "{out}"
    );
}

#[test]
fn the_export_menu_offers_series_and_book_scopes() {
    let (mut app, _dir) = series_app("export-menu");
    app.open_export_palette();
    let Mode::Palette(p) = &app.mode else {
        panic!("export palette")
    };
    assert!(p.items.iter().any(|i| i.label.starts_with("Series → ")));
    assert!(p.items.iter().any(|i| i.label.starts_with("This book → ")));
}

#[test]
fn a_standalone_export_menu_lists_the_preview_then_each_format_once() {
    let (mut app, _dir) = two_chapter_app("export-single");
    app.open_export_palette();
    let Mode::Palette(p) = &app.mode else {
        panic!("export palette")
    };
    // The style setter and preview lead (#569), then the seven whole-book formats
    // once each (no series/book scope duplication for a standalone novel).
    assert_eq!(p.items[0].label, "Set export style…");
    assert_eq!(p.items[1].label, "Preview style…");
    assert_eq!(p.items[2].label, "Manuscript (Markdown)");
    assert_eq!(p.items[8].label, "PDF (print interior)");
    // The sample-export entries (#584) follow under collapsed groups, so the
    // everyday whole-book formats still lead the menu unchanged.
    assert_eq!(p.items[2].group, None, "whole-book formats are ungrouped");
}

// --- Scoped / sample export (#584) ------------------------------------------

/// A standalone book with four body chapters split across two parts, on disk —
/// for the sample-export tests (single chapter, a part, a contiguous range).
fn parted_app(name: &str) -> (App, TempProject) {
    let dir = crate::testutil::scratch_dir("books", name);
    let bodies = ["alpha\n", "bravo\n", "charlie\n", "delta\n"];
    let mut manifest = String::from("title = \"Book\"\n\n");
    for (i, body) in bodies.iter().enumerate() {
        write!(
            manifest,
            "[[chapters]]\ntitle = \"Ch{i}\"\nfile = \"c{i}.md\"\n\n"
        )
        .unwrap();
        fs::write(dir.join(format!("c{i}.md")), body).unwrap();
    }
    // Two parts: chapters 1–2 and 3–4 (1-based body positions, #185).
    manifest.push_str(
        "[[parts]]\nname = \"Part One\"\nchapters = [1, 2]\n\n\
         [[parts]]\nname = \"Part Two\"\nchapters = [3, 4]\n",
    );
    fs::write(Project::manifest_path(&dir), manifest).unwrap();
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

#[test]
fn exporting_a_single_chapter_writes_only_that_chapter() {
    use crate::mode::{ExportFormat, ExportScope};
    let (mut app, dir) = parted_app("export-one-chapter");
    app.run_export(ExportFormat::Markdown, ExportScope::Chapter(2));
    // Named for the chapter, distinct from the whole-book export's `book.md`.
    let out = fs::read_to_string(dir.join("export/book-ch2.md")).unwrap();
    assert!(out.contains("charlie"), "the chosen chapter's prose: {out}");
    assert!(
        !out.contains("alpha") && !out.contains("delta"),
        "no other chapter: {out}"
    );
}

#[test]
fn exporting_a_range_writes_only_those_chapters_to_a_distinct_file() {
    use crate::mode::{ExportFormat, ExportScope};
    let (mut app, dir) = parted_app("export-range");
    app.run_export(
        ExportFormat::Markdown,
        ExportScope::Range { start: 0, end: 1 },
    );
    // 1-based, inclusive in the filename — and beside, not over, the full export.
    let out = fs::read_to_string(dir.join("export/book-chapters-1-2.md")).unwrap();
    assert!(out.contains("alpha") && out.contains("bravo"), "{out}");
    assert!(
        !out.contains("charlie") && !out.contains("delta"),
        "only the range: {out}"
    );
    // The whole-book export still writes its own file untouched.
    app.run_export(ExportFormat::Markdown, ExportScope::ActiveBook);
    let full = fs::read_to_string(dir.join("export/book.md")).unwrap();
    assert!(full.contains("alpha") && full.contains("delta"), "{full}");
}

#[test]
fn exporting_a_part_writes_only_its_chapters() {
    use crate::mode::{ExportFormat, ExportScope};
    let (mut app, dir) = parted_app("export-part");
    app.run_export(
        ExportFormat::Markdown,
        ExportScope::Part {
            name: "Part Two".into(),
            start: 2,
            end: 3,
        },
    );
    let out = fs::read_to_string(dir.join("export/book-part-two.md")).unwrap();
    assert!(out.contains("charlie") && out.contains("delta"), "{out}");
    assert!(
        !out.contains("alpha") && !out.contains("bravo"),
        "only the part: {out}"
    );
}

#[test]
fn the_export_menu_offers_chapter_part_and_range_samples() {
    let (mut app, _dir) = parted_app("export-samples-menu");
    app.open_export_palette();
    let Mode::Palette(p) = &app.mode else {
        panic!("export palette")
    };
    let labels: Vec<&str> = p.items.iter().map(|i| i.label.as_str()).collect();
    assert!(
        labels.iter().any(|l| l.starts_with("Chapter: Ch0 → ")),
        "a per-chapter sample"
    );
    assert!(
        labels.iter().any(|l| l.starts_with("Part: Part One → ")),
        "a per-part sample"
    );
    assert!(
        labels
            .iter()
            .any(|l| l.starts_with("First three chapters → ")),
        "a submission-range sample"
    );
    // The sample entries sit under collapsed groups; the whole-book formats don't.
    assert!(
        p.items
            .iter()
            .filter(|i| i.group.is_some())
            .all(|i| p.collapsed.contains(i.group.unwrap())),
        "sample groups start collapsed"
    );
}

// --- Progress report: series total (#333) -----------------------------------

fn report_text(app: &App) -> String {
    app.report_lines()
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn the_progress_report_shows_a_series_total() {
    let (app, _dir) = series_app("report-series");
    let text = report_text(&app);
    assert!(
        text.contains("series:") && text.contains("6 words across 2 books"),
        "{text}"
    );
}

#[test]
fn a_standalone_report_has_no_series_line() {
    let (app, _dir) = two_chapter_app("report-single");
    assert!(!report_text(&app).contains("series:"));
}

// --- Per-book publishing metadata (#810) ------------------------------------

/// Read one part out of a built `.docx` (a zip) as UTF-8 text — for asserting on
/// the emitted `word/document.xml` copyright blocks.
fn docx_part(bytes: &[u8], name: &str) -> String {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes.to_vec())).expect("valid docx zip");
    let mut file = zip.by_name(name).expect("part present");
    let mut text = String::new();
    file.read_to_string(&mut text).expect("utf-8 part");
    text
}

/// A two-book series whose `[[books]]` entries carry the given per-book
/// publishing fields, for the #810 per-book copyright tests. Each `book_meta` is
/// the extra TOML lines (ISBN/rights/…) for that book, appended to its title
/// before its chapters. The active book (book one) starts hoisted, as always.
fn series_pub_app(name: &str, book_meta: [&str; 2]) -> (App, TempProject) {
    let dir = crate::testutil::scratch_dir("books", name);
    for sub in ["b1", "b2"] {
        fs::create_dir_all(dir.join(sub)).unwrap();
    }
    fs::write(dir.join("b1/one.md"), "alpha one\n").unwrap();
    fs::write(dir.join("b2/one.md"), "beta one\n").unwrap();
    let mut manifest = String::from("title = \"The Saga\"\nauthor = \"A. Writer\"\n\n");
    write!(
        manifest,
        "[[books]]\ntitle = \"Book One\"\n{}\n",
        book_meta[0]
    )
    .unwrap();
    manifest.push_str("[[books.chapters]]\ntitle = \"Dawn\"\nfile = \"b1/one.md\"\n\n");
    write!(
        manifest,
        "[[books]]\ntitle = \"Book Two\"\n{}\n",
        book_meta[1]
    )
    .unwrap();
    manifest.push_str("[[books.chapters]]\ntitle = \"Dusk\"\nfile = \"b2/one.md\"\n");
    fs::write(Project::manifest_path(&dir), manifest).unwrap();
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

#[test]
fn series_export_stamps_each_book_its_own_copyright() {
    use crate::mode::{ExportFormat, ExportScope};
    // Two books, each with its own distinct ISBN and rights line in `[[books]]`.
    let (mut app, dir) = series_pub_app(
        "export-per-book-copyright",
        [
            "isbn = \"978-0-00-000000-0\"\nrights = \"© 2024 A. Writer\"",
            "isbn = \"978-1-11-111111-1\"\nrights = \"© 2026 B. Author\"",
        ],
    );
    app.run_export(ExportFormat::Docx, ExportScope::Series);
    let xml = docx_part(
        &fs::read(dir.join("export/the-saga.docx")).unwrap(),
        "word/document.xml",
    );
    // Each book's OWN ISBN and rights reach the document — not the active book's
    // reused for both. In reading order, book two's follow book one's.
    for needle in [
        "© 2024 A. Writer",
        "ISBN: 978-0-00-000000-0",
        "© 2026 B. Author",
        "ISBN: 978-1-11-111111-1",
    ] {
        assert!(xml.contains(needle), "missing {needle} in:\n{xml}");
    }
    let first = xml.find("ISBN: 978-0-00-000000-0").expect("book one isbn");
    let second = xml.find("ISBN: 978-1-11-111111-1").expect("book two isbn");
    assert!(
        first < second,
        "each book's own copyright, in reading order"
    );
}

#[test]
fn a_book_with_no_metadata_falls_back_to_the_series_metadata() {
    use crate::mode::{ExportFormat, ExportScope};
    // Book one (the active book) carries the metadata; book two carries none, so
    // its divider copyright falls back to the series/active-book metadata.
    let (mut app, dir) = series_pub_app(
        "export-fallback-copyright",
        [
            "isbn = \"978-0-00-000000-0\"\nrights = \"© 2024 A. Writer\"",
            "",
        ],
    );
    app.run_export(ExportFormat::Docx, ExportScope::Series);
    let xml = docx_part(
        &fs::read(dir.join("export/the-saga.docx")).unwrap(),
        "word/document.xml",
    );
    // No stray second ISBN — the metadata-less book does not invent one, and it
    // isn't left blank either: it falls back to the series/active-book copyright.
    // The series ISBN appears on the front-matter copyright page and under each
    // book (book one's own, book two's fallback), so it repeats across the books.
    assert!(
        xml.matches("ISBN: 978-0-00-000000-0").count() >= 2,
        "the metadata-less book falls back to the series ISBN under its title:\n{xml}"
    );
    assert!(
        !xml.contains("ISBN: 978-1-11-111111-1"),
        "no other book's ISBN leaks onto the fallback book:\n{xml}"
    );
    // Book two's own title still leads its (fallback) copyright, in reading order.
    let two = xml.find("Book Two").expect("book two title");
    let rights_after_two = xml[two..].find("© 2024 A. Writer");
    assert!(
        rights_after_two.is_some(),
        "the fallback copyright is reprinted under book two:\n{xml}"
    );
}

#[test]
fn a_standalone_export_copyright_is_unchanged() {
    use crate::mode::{ExportFormat, ExportScope};
    // A standalone novel (no `[[books]]`) with publishing metadata renders exactly
    // one copyright block, stamped with its single manifest's metadata — the
    // per-book series plumbing leaves the single-book path untouched.
    let dir = crate::testutil::scratch_dir("books", "export-standalone-copyright");
    fs::write(dir.join("one.md"), "alpha\n").unwrap();
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Solo\"\nauthor = \"A. Writer\"\n\
         isbn = \"978-0-00-000000-0\"\nrights = \"© 2024 A. Writer\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    let mut app = open_test_app(&dir);
    let _guard = TempProject::new(dir.clone());
    app.run_export(ExportFormat::Docx, ExportScope::ActiveBook);
    let xml = docx_part(
        &fs::read(dir.join("export/solo.docx")).unwrap(),
        "word/document.xml",
    );
    assert!(
        xml.contains("978-0-00-000000-0") && xml.contains("© 2024 A. Writer"),
        "the standalone book's own copyright:\n{xml}"
    );
    // Exactly one copyright page — no per-book series reprint on a standalone.
    assert_eq!(
        xml.matches("ISBN: 978-0-00-000000-0").count(),
        1,
        "one copyright block on a standalone:\n{xml}"
    );
}
