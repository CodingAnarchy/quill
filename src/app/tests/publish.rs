//! Export-artifact actions: exporting the story bible (#708), the export
//! preflight / publish health check (#815), and the one-click "Prepare for
//! KDP / IngramSpark" publish bundle (#816) — the preflight's fail/warn report
//! and the bundle that sets print defaults, runs the preflight, and emits the
//! print PDF + reflowable EPUB (blocking on a failing preflight).

use super::*;
use crate::ai;
use crate::ai::bible::render;
use crate::mode::Action;

// --- Exporting the story bible (#708) --------------------------------------

/// The export-bible action is registered, needs no AI, and (unlike the manuscript
/// exports) works from the continuous view — it reads `bible/` from disk, not the
/// buffer.
#[test]
fn export_bible_action_is_registered_and_always_available() {
    assert!(matches!(
        Action::from_name("export:bible"),
        Some(Action::ExportBible)
    ));
    assert_eq!(Action::ExportBible.group(), "Export");
    let (mut app, _dir) = two_chapter_app("action-export-bible-avail");
    app.ai = ai::Settings::default(); // AI off — still available (no request)
    assert!(app.action_status(Action::ExportBible).is_none());
}

/// End-to-end: seeding a bible and dispatching the action writes a clean, ordered
/// `export/story-bible.md` with the generated/notes markers and front matter
/// stripped.
#[test]
fn export_bible_action_writes_a_clean_document() {
    let (mut app, dir) = two_chapter_app("action-export-bible-run");
    let bible = dir.join("bible");
    fs::create_dir_all(bible.join("characters")).unwrap();
    fs::write(
        bible.join("characters").join("mara.md"),
        render(
            "name: Mara\n",
            "## Physical\n- green eyes\n",
            "## Notes\n\nThe lead.\n",
            "2026-06-19",
        ),
    )
    .unwrap();
    fs::write(
        bible.join("setting.md"),
        render(
            "title: Setting\n",
            "## Gazetteer\n- Aldermoor\n",
            "## Notes\n",
            "2026-06-19",
        ),
    )
    .unwrap();

    app.run_action(Action::ExportBible, false);

    let written =
        fs::read_to_string(dir.join("export").join("story-bible.md")).expect("bible written");
    assert!(written.contains("## Characters"));
    assert!(written.contains("green eyes"));
    assert!(written.contains("The lead."));
    assert!(written.contains("Aldermoor"));
    assert!(!written.contains("generated:start"), "markers stripped");
    assert!(!written.contains("digest:"), "front matter stripped");
    assert!(
        written.find("## Characters").unwrap() < written.find("## Setting").unwrap(),
        "sections in order"
    );
    assert!(
        app.message.as_deref().unwrap_or("").contains("story-bible"),
        "reports the written path, got: {:?}",
        app.message
    );
}

// --- Export preflight / publish health check (#815) ------------------------

/// The preflight action is registered under Export and, like the manuscript
/// exports, is available from the single-chapter view without AI.
#[test]
fn preflight_action_is_registered_under_export() {
    assert!(matches!(
        Action::from_name("export:preflight"),
        Some(Action::PreflightExport)
    ));
    assert_eq!(Action::PreflightExport.group(), "Export");
    let (mut app, _dir) = two_chapter_app("action-preflight-avail");
    app.ai = ai::Settings::default(); // AI off — still available (no request)
    assert!(app.action_status(Action::PreflightExport).is_none());
}

/// A book with known issues — no ISBN, no cover, and an empty chapter — produces
/// the expected fail/warn lines, shown in the overlay and saved to
/// `export/preflight-<date>.md`.
#[test]
fn preflight_flags_a_book_with_known_issues() {
    let dir = app_scratch_dir("action-preflight-flawed");
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Flawed\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Empty\"\nfile = \"empty.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "The night was dark and long.\n").unwrap();
    fs::write(dir.join("empty.md"), "").unwrap(); // an empty body chapter
    let mut app = open_test_app(&dir);
    let _dir = TempProject::new(dir.clone());

    app.run_action(Action::PreflightExport, false);

    // The overlay shows the report.
    let shown = app
        .overlay_lines()
        .expect("preflight overlay is open")
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(shown.contains("FAIL  no ISBN"), "no-ISBN fails:\n{shown}");
    assert!(
        shown.contains("FAIL  no cover image"),
        "no-cover fails:\n{shown}"
    );
    assert!(
        shown.contains("empty chapter(s): Empty"),
        "the empty chapter is named:\n{shown}"
    );

    // And it's saved as an auditable artifact under export/.
    let saved = fs::read_dir(dir.join("export"))
        .expect("export dir")
        .filter_map(Result::ok)
        .find(|e| e.file_name().to_string_lossy().starts_with("preflight-"))
        .expect("a preflight-<date>.md was written");
    let written = fs::read_to_string(saved.path()).unwrap();
    assert!(written.contains("# Export preflight — Flawed"));
    assert!(written.contains("FAIL  no ISBN"));
    assert!(written.contains("empty chapter(s): Empty"));
}

/// A fully-specified book with a high-resolution cover and prose in every chapter
/// passes the preflight — no failures.
#[test]
fn preflight_passes_a_clean_book() {
    let dir = app_scratch_dir("action-preflight-clean");
    fs::write(dir.join("cover.png"), png_bytes(3000, 4500)).unwrap();
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Clean\"\n\
         isbn = \"978-1-234-56789-7\"\n\
         publisher = \"Quill Press\"\n\
         pub_date = \"2026\"\n\
         language = \"en-US\"\n\
         rights = \"© 2026 A. Writer.\"\n\n\
         [style]\ncover_image = \"cover.png\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "A balanced *italic* opening.\n").unwrap();
    fs::write(dir.join("two.md"), "More prose to fill the chapter.\n").unwrap();
    let mut app = open_test_app(&dir);
    let _dir = TempProject::new(dir.clone());

    app.run_action(Action::PreflightExport, false);

    let saved = fs::read_dir(dir.join("export"))
        .expect("export dir")
        .filter_map(Result::ok)
        .find(|e| e.file_name().to_string_lossy().starts_with("preflight-"))
        .expect("a preflight report was written");
    let written = fs::read_to_string(saved.path()).unwrap();
    assert!(
        !written.contains("FAIL"),
        "a clean book has no failures:\n{written}"
    );
    assert!(
        written.contains("0 failure(s)"),
        "summary is clean:\n{written}"
    );
}

// --- One-click "Prepare for KDP / IngramSpark" publish bundle (#816) --------

/// Both publish bundles are registered under Export, and — like the manuscript
/// exports and the preflight — available from the single-chapter view without AI.
#[test]
fn prepare_actions_are_registered_under_export() {
    assert!(matches!(
        Action::from_name("export:kdp"),
        Some(Action::PrepareForKdp)
    ));
    assert!(matches!(
        Action::from_name("export:ingramspark"),
        Some(Action::PrepareForIngramSpark)
    ));
    assert_eq!(Action::PrepareForKdp.group(), "Export");
    assert_eq!(Action::PrepareForIngramSpark.group(), "Export");
    let (mut app, _dir) = two_chapter_app("action-kdp-avail");
    app.ai = ai::Settings::default(); // AI off — still available (no request)
    assert!(app.action_status(Action::PrepareForKdp).is_none());
    assert!(app.action_status(Action::PrepareForIngramSpark).is_none());
}

/// The bundle needs the per-chapter files; in the continuous view the window is
/// unsaved, so the action reports why instead of running or writing anything.
#[test]
fn prepare_unavailable_in_the_continuous_view() {
    let (mut app, dir) = two_chapter_app("action-kdp-continuous");
    app.toggle_continuous();
    assert!(app.manuscript.is_some());
    app.run_action(Action::PrepareForKdp, false);
    assert!(
        app.message.as_deref().unwrap_or("").contains("continuous"),
        "explains the continuous-view guard, got: {:?}",
        app.message
    );
    assert!(
        !dir.join("export").join("book.print.pdf").exists(),
        "nothing exported"
    );
}

/// A clean book: "Prepare for KDP" sets the KDP print defaults on book.toml (6x9
/// trim, 0.125" bleed, a default language), runs the preflight, and emits *both*
/// the print PDF and the reflowable EPUB to export/.
#[test]
fn prepare_for_kdp_sets_defaults_and_emits_both_artifacts() {
    let dir = app_scratch_dir("action-kdp-clean");
    fs::write(dir.join("cover.png"), png_bytes(3000, 4500)).unwrap();
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Ready\"\n\
         isbn = \"978-1-234-56789-7\"\n\
         publisher = \"Quill Press\"\n\
         pub_date = \"2026\"\n\
         rights = \"© 2026 A. Writer.\"\n\n\
         [style]\ncover_image = \"cover.png\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "A balanced opening chapter.\n").unwrap();
    fs::write(dir.join("two.md"), "More prose to fill the chapter.\n").unwrap();
    let mut app = open_test_app(&dir);
    let _dir = TempProject::new(dir.clone());

    app.run_action(Action::PrepareForKdp, false);

    // Both publishing artifacts landed together.
    let export = dir.join("export");
    assert!(
        export.join("ready.print.pdf").exists(),
        "print PDF written: {:?}",
        app.message
    );
    assert!(
        export.join("ready.epub").exists(),
        "reflowable EPUB written: {:?}",
        app.message
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Prepared for KDP"),
        "reports the bundle, got: {:?}",
        app.message
    );

    // The KDP-sane print defaults were persisted to book.toml.
    let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
    assert!(toml.contains("trim = \"6x9\""), "trim default set:\n{toml}");
    assert!(toml.contains("bleed = 0.125"), "bleed default set:\n{toml}");
    assert!(
        toml.contains("language = \"en\""),
        "a default language was filled in:\n{toml}"
    );

    // The preflight ran and its report was saved as an auditable artifact.
    let saved = fs::read_dir(&export)
        .expect("export dir")
        .filter_map(Result::ok)
        .find(|e| e.file_name().to_string_lossy().starts_with("preflight-"))
        .expect("a preflight report was written");
    let written = fs::read_to_string(saved.path()).unwrap();
    assert!(
        written.contains("0 failure(s)"),
        "clean preflight:\n{written}"
    );
}

/// An author's explicit language / trim are honored, not overwritten — the
/// defaults only fill gaps. IngramSpark drives the same bundle as KDP.
#[test]
fn prepare_for_ingramspark_keeps_the_authors_language_and_emits() {
    let dir = app_scratch_dir("action-ingram-clean");
    fs::write(dir.join("cover.png"), png_bytes(3000, 4500)).unwrap();
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Ready Two\"\n\
         isbn = \"978-1-234-56789-7\"\n\
         publisher = \"Quill Press\"\n\
         pub_date = \"2026\"\n\
         language = \"fr\"\n\
         rights = \"© 2026 A. Writer.\"\n\n\
         [style]\ncover_image = \"cover.png\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "Assez de prose pour le chapitre.\n").unwrap();
    let mut app = open_test_app(&dir);
    let _dir = TempProject::new(dir.clone());

    app.run_action(Action::PrepareForIngramSpark, false);

    let export = dir.join("export");
    assert!(
        export.join("ready-two.print.pdf").exists(),
        "print PDF: {:?}",
        app.message
    );
    assert!(
        export.join("ready-two.epub").exists(),
        "EPUB: {:?}",
        app.message
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Prepared for IngramSpark"),
        "reports IngramSpark, got: {:?}",
        app.message
    );
    let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
    assert!(
        toml.contains("language = \"fr\""),
        "the author's language is untouched:\n{toml}"
    );
}

/// A book that fails the preflight (no ISBN) is *not* exported — the bundle
/// surfaces the failure count and points at the report, emitting no artifacts.
#[test]
fn prepare_blocks_export_when_preflight_fails() {
    let dir = app_scratch_dir("action-kdp-fails");
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Flawed\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "The night was dark and long.\n").unwrap();
    let mut app = open_test_app(&dir);
    let _dir = TempProject::new(dir.clone());

    app.run_action(Action::PrepareForKdp, false);

    // The failure is surfaced and nothing was emitted.
    let msg = app.message.clone().unwrap_or_default();
    assert!(
        msg.contains("failure(s)") && msg.contains("nothing exported"),
        "surfaces the preflight failures, got: {msg}"
    );
    let export = dir.join("export");
    assert!(
        !export.join("flawed.print.pdf").exists(),
        "no print PDF on a failing preflight"
    );
    assert!(
        !export.join("flawed.epub").exists(),
        "no EPUB on a failing preflight"
    );
    // But the preflight report was still saved, so the author can fix against it.
    let saved = fs::read_dir(&export)
        .expect("export dir")
        .filter_map(Result::ok)
        .find(|e| e.file_name().to_string_lossy().starts_with("preflight-"));
    let written = fs::read_to_string(saved.expect("preflight saved").path()).unwrap();
    assert!(
        written.contains("FAIL  no ISBN"),
        "the failure is recorded:\n{written}"
    );
}

/// The publish bundle targets a standalone novel: a multi-book series has no single
/// top-level [print]/language table to set, so the action explains that per book.
#[test]
fn prepare_is_unavailable_for_a_multi_book_series() {
    let dir = app_scratch_dir("action-kdp-series");
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Series\"\n\n\
         [[books]]\ntitle = \"Book One\"\n\n\
         [[books.chapters]]\ntitle = \"One\"\nfile = \"b1-one.md\"\n\n\
         [[books]]\ntitle = \"Book Two\"\n\n\
         [[books.chapters]]\ntitle = \"One\"\nfile = \"b2-one.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("b1-one.md"), "Prose one.\n").unwrap();
    fs::write(dir.join("b2-one.md"), "Prose two.\n").unwrap();
    let mut app = open_test_app(&dir);
    let _dir = TempProject::new(dir.clone());
    assert!(app.project.book_count() > 1);

    assert!(
        app.action_status(Action::PrepareForKdp).is_some(),
        "the series case is gated at status"
    );
    app.run_action(Action::PrepareForKdp, false);
    assert!(
        !dir.join("export").exists(),
        "nothing exported for a series"
    );
}

/// A tiny solid-red PNG of `w`×`h` — a real raster the cover DPI check decodes.
fn png_bytes(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 0, 255]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}
