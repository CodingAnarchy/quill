//! Tests for creating and opening a project: the starter manifest and chapter
//! file the create path writes (through the atomic layer), the built-in
//! templates, the create→open round trip, and every manifest field's parse /
//! round trip — goals scalars, style, book and publishing metadata, the
//! epigraph, and words-per-page. Also covers what `open` tolerates: a
//! chapterless manifest, unknown future keys, malformed TOML, and garbage
//! front matter.

use super::*;
use crate::buffer::Buffer;

#[test]
fn create_initializes_manifest_and_first_chapter() {
    let dir = temp_dir("create");
    let project = Project::create(&dir).unwrap();

    assert_eq!(project.manifest.chapters.len(), 1);
    assert_eq!(project.active_title(), Some("Chapter 1"));
    assert!(Project::manifest_path(&dir).exists());
    assert!(project.active_path().exists());

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn empty_template_matches_the_historic_default() {
    // The "Empty" template (index 0) reproduces `create` exactly: one blank
    // starter chapter, no parts (#611).
    let dir = temp_dir("tmpl-empty");
    let project = Project::create_with_template(&dir, ProjectTemplate::at(0)).unwrap();

    assert_eq!(project.manifest.chapters.len(), 1);
    assert_eq!(project.active_title(), Some("Chapter 1"));
    assert!(project.manifest.parts.is_none(), "no parts scaffolded");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn three_act_template_scaffolds_parts_and_chapter_stubs() {
    // Find the curated three-act template by name so the test tracks the table.
    let (index, template) = TEMPLATES
        .iter()
        .enumerate()
        .find(|(_, t)| t.name == "Three-act")
        .expect("a three-act template is offered");
    let dir = temp_dir("tmpl-three-act");
    let project = Project::create_with_template(&dir, ProjectTemplate::at(index)).unwrap();

    // Every stub across every part became an empty chapter, in order.
    let stub_titles: Vec<&str> = template
        .parts
        .iter()
        .flat_map(|p| p.chapters.iter().copied())
        .collect();
    let chapter_titles: Vec<&str> = project
        .manifest
        .chapters
        .iter()
        .map(|c| c.title.as_str())
        .collect();
    assert_eq!(chapter_titles, stub_titles);
    // The active chapter is the first stub, so the editor opens on it.
    assert_eq!(project.active_title(), Some(stub_titles[0]));

    // The `[[parts]]` skeleton names each part and spans its stubs, contiguously.
    let parts = project.manifest.parts.as_ref().expect("parts scaffolded");
    assert_eq!(parts.len(), template.parts.len());
    let mut next = 1;
    for (part, tmpl) in parts.iter().zip(template.parts) {
        assert_eq!(part.name.as_deref(), Some(tmpl.name));
        let end = next + tmpl.chapters.len() - 1;
        assert_eq!(part.chapters, [next, end]);
        next = end + 1;
    }

    // Every stub's file exists on disk and holds no prose (empty stubs).
    for i in 0..project.manifest.chapters.len() {
        let path = project.chapter_path(i);
        assert!(path.exists(), "{path:?} written");
        assert!(
            fs::read_to_string(&path).unwrap().trim().is_empty(),
            "stub {i} is empty"
        );
    }

    // The structure survives a round-trip through the manifest.
    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.manifest.parts, project.manifest.parts);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn create_then_open_round_trips_structure_and_text() {
    let dir = temp_dir("roundtrip");

    // Create, write some prose into the active chapter, save it.
    let project = Project::create(&dir).unwrap();
    let mut buf = Buffer::from_path(project.active_path()).unwrap();
    for c in "# Opening\n\nThe night was dark.".chars() {
        buf.insert_char(c);
    }
    buf.save().unwrap();

    // Reopen and confirm the structure (titles + files) and content
    // survived. Compare the persisted projection, not the whole manifest:
    // `reopened` has word counts from disk while `project`'s are still 0.
    let reopened = Project::open(&dir).unwrap();
    let structure = |p: &Project| -> Vec<(String, String)> {
        p.manifest
            .chapters
            .iter()
            .map(|c| (c.title.clone(), c.file.clone()))
            .collect()
    };
    assert_eq!(reopened.manifest.title, project.manifest.title);
    assert_eq!(structure(&reopened), structure(&project));
    let text = fs::read_to_string(reopened.active_path()).unwrap();
    assert!(text.contains("The night was dark."));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn open_parses_a_hand_written_manifest() {
    let dir = temp_dir("handwritten");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Red State Earth\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "first\n").unwrap();
    fs::write(dir.join("two.md"), "second\n").unwrap();

    let project = Project::open(&dir).unwrap();
    assert_eq!(project.manifest.title, "Red State Earth");
    assert_eq!(project.manifest.chapters.len(), 2);
    assert_eq!(project.active_index(), 0);
    assert_eq!(project.active_title(), Some("One"));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn daily_goal_parses_and_round_trips_and_is_optional() {
    // A [goals] block with a daily target parses it.
    let toml = "title = \"B\"\n\n[goals]\nwords = 90000\nchapters = 40\ndaily = 1000\n";
    let m: Manifest = toml::from_str(toml).unwrap();
    assert_eq!(m.goals.as_ref().unwrap().daily, Some(1000));

    // Round-trips when set...
    let out = toml::to_string_pretty(&m).unwrap();
    assert!(out.contains("daily = 1000"), "daily persists: {out}");

    // ...and is omitted entirely when unset (old manifests still parse).
    let none: Manifest =
        toml::from_str("title = \"B\"\n\n[goals]\nwords = 90000\nchapters = 40\n").unwrap();
    assert_eq!(none.goals.as_ref().unwrap().daily, None);
    assert!(
        !toml::to_string_pretty(&none).unwrap().contains("daily"),
        "no daily key when unset"
    );
}

#[test]
fn style_scene_break_parses_and_round_trips_and_is_optional() {
    // A [style] block parses into the export styling (#468).
    let m: Manifest =
        toml::from_str("title = \"B\"\n\n[style]\nscene_break = \"* * *\"\n").unwrap();
    assert_eq!(
        m.style.as_ref().unwrap().scene_break.as_deref(),
        Some("* * *")
    );

    // Round-trips when set...
    let out = toml::to_string_pretty(&m).unwrap();
    assert!(
        out.contains("[style]") && out.contains("scene_break = \"* * *\""),
        "{out}"
    );

    // ...and is omitted entirely when unset, so existing manifests are unchanged.
    let none: Manifest = toml::from_str("title = \"B\"\n").unwrap();
    assert!(none.style.is_none());
    assert!(
        !toml::to_string_pretty(&none).unwrap().contains("[style]"),
        "no [style] when unset"
    );
}

#[test]
fn book_metadata_persists_and_old_manifests_still_parse() {
    let dir = temp_dir("book-meta");

    // A manifest written before these fields existed still opens.
    let mut project = Project::create(&dir).unwrap();
    assert_eq!(project.manifest.subtitle, None);

    // Set the new fields and persist.
    project.manifest.subtitle = Some("A Novel".into());
    project.manifest.author = Some("A. Writer".into());
    project.manifest.series = Some("The Saga".into());
    project.save_manifest().unwrap();

    // The written TOML carries them, and they survive a reopen.
    let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
    assert!(toml.contains("author = \"A. Writer\""), "manifest:\n{toml}");
    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.manifest.subtitle.as_deref(), Some("A Novel"));
    assert_eq!(reopened.manifest.author.as_deref(), Some("A. Writer"));
    assert_eq!(reopened.manifest.series.as_deref(), Some("The Saga"));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn publishing_metadata_persists_and_round_trips() {
    // Publishing metadata (#583) — ISBN, publisher, publication date, language,
    // and rights — writes to the manifest and survives a reopen; absent fields
    // aren't written back, so old manifests still parse.
    let dir = temp_dir("publishing-meta");
    let mut project = Project::create(&dir).unwrap();
    assert_eq!(project.manifest.isbn, None);
    assert_eq!(project.manifest.language, None);

    project.manifest.isbn = Some("978-0-00-000000-0".into());
    project.manifest.publisher = Some("Quill & Co.".into());
    project.manifest.pub_date = Some("2026-06-30".into());
    project.manifest.language = Some("fr".into());
    project.manifest.rights = Some("© 2026 A. Writer".into());
    project.save_manifest().unwrap();

    let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
    assert!(toml.contains("isbn = "), "isbn written:\n{toml}");
    assert!(toml.contains("language = \"fr\""), "language written");

    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.manifest.isbn.as_deref(), Some("978-0-00-000000-0"));
    assert_eq!(reopened.manifest.publisher.as_deref(), Some("Quill & Co."));
    assert_eq!(reopened.manifest.pub_date.as_deref(), Some("2026-06-30"));
    assert_eq!(reopened.manifest.language.as_deref(), Some("fr"));
    assert_eq!(
        reopened.manifest.rights.as_deref(),
        Some("© 2026 A. Writer")
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn book_epigraph_persists_and_builds() {
    let dir = temp_dir("book-epigraph");
    let mut project = Project::create(&dir).unwrap();
    project.manifest.epigraph = Some("Call me Ishmael.".into());
    project.manifest.epigraph_by = Some("Melville".into());
    project.save_manifest().unwrap();

    let reopened = Project::open(&dir).unwrap();
    let ep = reopened.manifest.epigraph().unwrap();
    assert_eq!(ep.text, "Call me Ishmael.");
    assert_eq!(ep.attribution.as_deref(), Some("Melville"));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn words_per_page_defaults_to_a_standard_page() {
    let dir = temp_dir("wpp-default");
    std::fs::create_dir_all(&dir).unwrap();
    // A goals block that omits words_per_page (the common case).
    std::fs::write(
        Project::manifest_path(&dir),
        "title = \"B\"\n\n[goals]\nwords = 100000\nchapters = 40\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    std::fs::write(dir.join("one.md"), "body\n").unwrap();

    let project = Project::open(&dir).unwrap();
    assert_eq!(project.manifest.goals.unwrap().words_per_page, 250);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_multiline_book_epigraph_round_trips_through_the_manifest() {
    let dir = temp_dir("book-epigraph-multi");
    let mut project = Project::create(&dir).unwrap();
    project.manifest.epigraph = Some("Line one\nLine two\nLine three".into());
    project.save_manifest().unwrap();

    // toml serializes the multi-line string as a readable `"""` block.
    let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
    assert!(
        toml.contains("epigraph = \"\"\""),
        "expected a multi-line literal:\n{toml}"
    );

    let reopened = Project::open(&dir).unwrap();
    assert_eq!(
        reopened.manifest.epigraph().unwrap().text,
        "Line one\nLine two\nLine three"
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn open_repairs_a_chapterless_manifest() {
    let dir = temp_dir("empty-manifest");
    fs::create_dir_all(&dir).unwrap();
    fs::write(Project::manifest_path(&dir), "title = \"Empty\"\n").unwrap();

    let project = Project::open(&dir).unwrap();
    assert_eq!(project.manifest.chapters.len(), 1);
    assert!(project.active_path().exists());

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn open_survives_unknown_future_keys_end_to_end_and_stamps_the_version() {
    // COMPAT (#1097), end to end: a `book.toml` written by a *newer* Quill — with
    // unknown keys at the top level and inside a chapter entry — must open through
    // the real `Project::open` path with no error and no loss of the known fields.
    // On save the manifest is re-serialized from the typed struct with the file's
    // unknown *top-level* keys carried over; an unknown key nested inside a modeled
    // table (here a chapter entry) is still dropped (documented behavior).
    let dir = temp_dir("compat-unknown-keys");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        Project::manifest_path(&dir),
        "\
title = \"Forward Compat\"
author = \"A. Writer\"
future_top_level = \"a newer version wrote this\"

[[chapters]]
title = \"One\"
file = \"one.md\"
future_chapter_key = 99
",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "The prose.\n").unwrap();

    let project = Project::open(&dir).expect("unknown future keys must not fail the open");
    assert_eq!(project.manifest.title, "Forward Compat");
    assert_eq!(project.manifest.author.as_deref(), Some("A. Writer"));
    assert_eq!(project.manifest.chapters.len(), 1);
    // A pre-#1097 file has no version key; it reads as the current version and is
    // left untouched by migrate (byte-stable), so re-save doesn't add the key.
    assert_eq!(
        project.manifest.format_version(),
        CURRENT_FORMAT_VERSION,
        "an absent version reads as the current one"
    );

    // Re-save: the unknown top-level key survives, the nested one drops, known
    // data survives, and an untouched existing project stays byte-stable (no new
    // format_version key).
    project.save_manifest().unwrap();
    let written = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
    assert!(
        written.contains("future_top_level = \"a newer version wrote this\""),
        "an unknown top-level key is kept on re-save: {written}"
    );
    assert!(
        !written.contains("future_chapter_key"),
        "an unknown key inside a chapter entry is dropped (documented): {written}"
    );
    assert!(
        written.contains("title = \"Forward Compat\""),
        "known data kept: {written}"
    );
    assert!(
        !written.contains("format_version"),
        "an untouched existing project doesn't sprout a version key: {written}"
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn open_reports_a_malformed_manifest_without_panicking() {
    let dir = temp_dir("bad-manifest");
    fs::create_dir_all(&dir).unwrap();
    // Broken TOML — an unterminated array-of-tables header.
    fs::write(Project::manifest_path(&dir), "title = \"X\"\n[[chapters]\n").unwrap();

    let err = Project::open(&dir)
        .err()
        .expect("a malformed manifest is an error, not a panic");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert!(
        err.to_string().contains(MANIFEST_NAME),
        "clear message: {err}"
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn open_tolerates_garbage_front_matter() {
    let dir = temp_dir("bad-frontmatter");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        Project::manifest_path(&dir),
        "title = \"B\"\n\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    // A fenced block whose contents are not valid YAML at all.
    fs::write(
        dir.join("one.md"),
        "---\n: : : not yaml [}{\n\tstatus\n---\nThe prose body.\n",
    )
    .unwrap();

    // Opening caches each chapter's front matter — garbage must not panic; the
    // typed fields just come back empty, and the prose body is intact.
    let project = Project::open(&dir).expect("garbage front matter should not fail the open");
    assert_eq!(project.chapter_body(0), "The prose body.\n");
    assert!(project.manifest.chapters[0].status.is_none());

    fs::remove_dir_all(&dir).ok();
}

/// Creating a project writes its starter chapter file to disk (empty body),
/// alongside the manifest — the create path materializes the chapter, not just
/// the manifest entry.
#[test]
fn create_writes_the_starter_chapter_file() {
    let dir = temp_dir("starter-chapter");
    let project = Project::create(&dir).unwrap();

    let path = dir.join(&project.manifest.chapters[0].file);
    assert!(path.exists(), "starter chapter file must be created");
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        "",
        "starter chapter is created with an empty body"
    );

    fs::remove_dir_all(&dir).ok();
}

/// The starter chapter is written through the atomic-write layer, not a bare
/// `fs::write` — so a crash mid-write can never leave a truncated file (#661).
/// We prove the path by arming the atomic layer's pre-rename fault: if the
/// write goes through `atomic::write`, the injected failure surfaces and the
/// file is left uncreated; a bare `fs::write` would ignore the fault entirely.
#[test]
fn starter_chapter_is_written_atomically() {
    let dir = temp_dir("starter-atomic");
    fs::create_dir_all(&dir).unwrap();

    crate::atomic::fail_point::arm_pre_rename_failure();
    let result = Project::create(&dir);

    assert!(
        result.is_err(),
        "the injected atomic-write failure must surface, proving the atomic path"
    );
    assert!(
        !dir.join("chapter-1.md").exists(),
        "a failed atomic write leaves no partial starter chapter file"
    );
    let strays: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(strays.is_empty(), "staging temp left behind: {strays:?}");

    fs::remove_dir_all(&dir).ok();
}
