//! Tests for the `src/project.rs` module, split out to keep the module under the
//! ~1,000-line convention (#404). Pure move — no behavior change.

use super::*;
use crate::buffer::Buffer;
use std::fmt::Write;
use std::time::Instant;

fn temp_dir(name: &str) -> PathBuf {
    crate::testutil::scratch_dir("proj", name)
}

#[test]
fn accepts_words_into_the_project_dictionary() {
    let dir = temp_dir("dict");
    let mut p = Project::create(&dir).unwrap();

    assert!(!p.is_accepted("Vashti"));
    assert!(p.accept_word("Vashti").unwrap(), "newly added");
    assert!(
        p.is_accepted("Vashti") && p.is_accepted("vashti"),
        "matched case-insensitively"
    );
    assert!(
        !p.accept_word("vashti").unwrap(),
        "deduped case-insensitively"
    );

    // Persisted to dictionary.txt (original case) and respected after reload.
    let file = fs::read_to_string(dir.join(DICTIONARY_NAME)).unwrap();
    assert!(file.contains("Vashti"), "{file}");
    assert!(
        Project::open(&dir).unwrap().is_accepted("VASHTI"),
        "loaded on reopen"
    );

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn is_accepted_short_circuits_and_matches_case_insensitively() {
    // The empty-dictionary case (no custom words) accepts nothing — the common
    // idle-pass path, guarded at zero cost (#780).
    let dir = temp_dir("dict-empty");
    let mut p = Project::create(&dir).unwrap();
    assert!(p.dictionary.is_empty());
    assert!(!p.is_accepted("anything"));
    assert!(!p.is_accepted("MiXeD"));

    // With entries, matching is case-insensitive for both lowercase (borrow-lookup)
    // and uppercase-bearing (lowercased) words, unchanged from before.
    p.accept_word("Vashti").unwrap();
    assert!(p.is_accepted("vashti"), "already-lowercase, no alloc path");
    assert!(p.is_accepted("VASHTI"), "uppercase, lowercased path");
    assert!(p.is_accepted("VaShTi"), "mixed case");
    assert!(!p.is_accepted("other"));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn accepting_a_word_preserves_existing_dictionary_entries() {
    let dir = temp_dir("dict-append");
    let mut p = Project::create(&dir).unwrap();
    fs::write(dir.join(DICTIONARY_NAME), "Aerendil\n").unwrap();
    p.dictionary = load_dictionary(&dir); // re-read what we just wrote
    p.accept_word("Vashti").unwrap();
    let file = fs::read_to_string(dir.join(DICTIONARY_NAME)).unwrap();
    assert!(
        file.contains("Aerendil") && file.contains("Vashti"),
        "both kept: {file}"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn reload_dictionary_picks_up_an_external_edit() {
    // The in-app view/edit path (#935) writes `dictionary.txt` directly, then asks
    // the project to re-read it. `reload_dictionary` reflects an added word and
    // drops a removed one, without reopening the project.
    let dir = temp_dir("dict-reload");
    let mut p = Project::create(&dir).unwrap();
    assert!(p.dictionary.is_empty());

    // A hand-edit adds two words at the dictionary path the app exposes.
    fs::write(p.dictionary_path(), "Vashti\nAerendil\n").unwrap();
    assert!(!p.is_accepted("Vashti"), "not yet loaded");
    p.reload_dictionary();
    assert!(
        p.is_accepted("Vashti") && p.is_accepted("Aerendil"),
        "the external edit loaded"
    );

    // Removing a word from the file and reloading drops it from the set.
    fs::write(p.dictionary_path(), "Vashti\n").unwrap();
    p.reload_dictionary();
    assert!(p.is_accepted("Vashti"));
    assert!(!p.is_accepted("Aerendil"), "the removed word is gone");
    fs::remove_dir_all(&dir).ok();
}

/// Write an `n`-chapter project corpus (each chapter has front matter and a
/// ~2.7 KB body) under `dir`, returning the manifest text for the caller to
/// write. Shared by the open-performance guards.
fn write_chapter_project(dir: &Path, n: usize, title: &str) -> String {
    let mut manifest = format!("title = \"{title}\"\n\n");
    for i in 0..n {
        let file = format!("ch{i}.md");
        write!(
            manifest,
            "[[chapters]]\ntitle = \"Ch{i}\"\nfile = \"{file}\"\n\n"
        )
        .unwrap();
        let mut body = String::from("---\nstatus: draft\npov: Mara\n---\n# Chapter\n\n");
        for _ in 0..50 {
            body.push_str("The night was dark and the road was very long indeed.\n");
        }
        fs::write(dir.join(&file), body).unwrap();
    }
    manifest
}

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
fn goals_round_trip_through_the_manifest() {
    let dir = temp_dir("goals");
    let mut project = Project::create(&dir).unwrap();
    assert!(project.manifest.goals.is_none());
    assert_eq!(project.manifest.todo_marker(), "xxx"); // default

    project.manifest.start_date = Some("2026-01-11".into());
    project.manifest.goals = Some(Goals {
        words: 150_000,
        chapters: 60,
        words_per_page: 250,
        daily: None,
        deadline: None,
    });
    project.save_manifest().unwrap();

    // The `[goals]` table serializes after the scalars and before
    // `[[chapters]]` (valid TOML).
    let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
    assert!(
        toml.find("[goals]").unwrap() < toml.find("[[chapters]]").unwrap(),
        "goals must precede chapters:\n{toml}"
    );

    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.manifest.start_date.as_deref(), Some("2026-01-11"));
    assert_eq!(reopened.manifest.goals.unwrap().words, 150_000);

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn set_goal_splices_into_goals_preserving_comments_and_clears_optional_keys() {
    // A hand-maintained manifest with a comment and another section.
    let dir = temp_dir("set-goal-splice");
    let mut project = Project::create(&dir).unwrap();
    let path = Project::manifest_path(&dir);
    fs::write(
        &path,
        "# my book\ntitle = \"Book\"\nauthor = \"Ada\"\n\n\
         [analysis]\nstopwords_add = [\"foo\"]\n\n\
         [[chapters]]\ntitle = \"Chapter 1\"\nfile = \"chapter-1.md\"\n",
    )
    .unwrap();

    // Set each goal; every write is a targeted splice that leaves the rest intact.
    project.set_goal(GoalField::Words, Some(80_000)).unwrap();
    project.set_goal(GoalField::Chapters, Some(30)).unwrap();
    project.set_goal(GoalField::Daily, Some(1_000)).unwrap();

    let toml = fs::read_to_string(&path).unwrap();
    assert!(toml.contains("# my book"), "comment preserved:\n{toml}");
    assert!(
        toml.contains("author = \"Ada\""),
        "scalar preserved:\n{toml}"
    );
    assert!(
        toml.contains("[analysis]") && toml.contains("stopwords_add = [\"foo\"]"),
        "other section preserved:\n{toml}"
    );
    assert!(
        toml.contains("words = 80000")
            && toml.contains("chapters = 30")
            && toml.contains("daily = 1000"),
        "goals spliced:\n{toml}"
    );

    // Clearing the optional daily goal removes the key, not writes a zero.
    project.set_goal(GoalField::Daily, None).unwrap();
    let toml = fs::read_to_string(&path).unwrap();
    assert!(!toml.contains("daily"), "daily key removed:\n{toml}");
    assert_eq!(project.goals().unwrap().daily, None);

    // Everything still round-trips on a fresh open.
    let reopened = Project::open(&dir).unwrap();
    let g = reopened.goals().unwrap();
    assert_eq!((g.words, g.chapters, g.daily), (80_000, 30, None));

    fs::remove_dir_all(&dir).ok();
}

#[test]
fn set_deadline_splices_an_iso_string_and_clears_to_remove_the_key() {
    // A hand-maintained manifest with a comment and another section (#814).
    let dir = temp_dir("set-deadline-splice");
    let mut project = Project::create(&dir).unwrap();
    let path = Project::manifest_path(&dir);
    fs::write(
        &path,
        "# my book\ntitle = \"Book\"\nauthor = \"Ada\"\n\n\
         [[chapters]]\ntitle = \"Chapter 1\"\nfile = \"chapter-1.md\"\n",
    )
    .unwrap();

    let deadline = crate::date::parse("2026-12-31").unwrap();
    project.set_deadline(Some(deadline)).unwrap();
    let toml = fs::read_to_string(&path).unwrap();
    // Persisted as a quoted ISO string (like `start_date`), the rest intact.
    assert!(
        toml.contains("deadline = \"2026-12-31\""),
        "deadline spliced as a quoted string:\n{toml}"
    );
    assert!(toml.contains("# my book") && toml.contains("author = \"Ada\""));

    // It round-trips: a fresh open parses the same date back.
    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.goals().unwrap().deadline_date(), Some(deadline));

    // Clearing removes the key rather than writing an empty string.
    project.set_deadline(None).unwrap();
    let toml = fs::read_to_string(&path).unwrap();
    assert!(!toml.contains("deadline"), "deadline key removed:\n{toml}");
    assert_eq!(project.goals().unwrap().deadline, None);

    fs::remove_dir_all(&dir).ok();
}

// --- Writes derived from a failed read must abort, not destroy data (#565) ---
//
// Each splice-then-write path reads an expected-existing file first. A transient
// read failure must abort the write rather than write content derived from `""`
// (which would durably destroy the manifest / a chapter / the dictionary while
// reporting success). To simulate an unreadable-but-present file hermetically we
// replace the target path with a directory: `read_to_string` then fails with a
// non-NotFound error — clearly "read failed", not "file is empty" — and the
// sibling file we assert on is left byte-for-byte.

#[test]
fn setting_a_goal_aborts_when_the_manifest_cant_be_read() {
    let dir = temp_dir("goal-read-fail");
    let mut project = Project::create(&dir).unwrap();
    let path = Project::manifest_path(&dir);
    // A real, populated manifest, then make it unreadable by swapping the file
    // for a directory of the same name.
    let before = fs::read_to_string(&path).unwrap();
    assert!(before.contains("[[chapters]]"), "real manifest:\n{before}");
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();

    let result = project.set_goal(GoalField::Words, Some(80_000));
    assert!(
        result.is_err(),
        "a failed manifest read must abort the write"
    );

    // The directory (our stand-in for the unreadable file) is untouched: nothing
    // was spliced onto an empty base.
    assert!(path.is_dir(), "the manifest was not overwritten");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn replacing_a_chapter_body_aborts_when_the_file_cant_be_read() {
    let dir = temp_dir("chapter-read-fail");
    let mut project = Project::create(&dir).unwrap();
    // Add a second, non-active chapter (the on-disk replace path) with real prose.
    project.add_chapter(1, "Two").unwrap();
    let file = project.manifest.chapters[1].file.clone();
    let chapter_path = dir.join(&file);
    fs::write(&chapter_path, "the original prose stays\n").unwrap();
    let before = fs::read_to_string(&chapter_path).unwrap();

    // Make the chapter file unreadable (swap it for a directory).
    fs::remove_file(&chapter_path).unwrap();
    fs::create_dir(&chapter_path).unwrap();

    let result = project.replace_chapter_body(1, &[(0, 3)], "X");
    assert!(
        result.is_err(),
        "a failed chapter read must abort the write"
    );
    assert!(chapter_path.is_dir(), "the chapter was not overwritten");

    // Swap the real prose back; it is exactly what it was — never blanked.
    fs::remove_dir(&chapter_path).unwrap();
    fs::write(&chapter_path, &before).unwrap();
    assert_eq!(fs::read_to_string(&chapter_path).unwrap(), before);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn editing_chapter_front_matter_aborts_when_the_file_cant_be_read() {
    let dir = temp_dir("front-matter-read-fail");
    let mut project = Project::create(&dir).unwrap();
    project.add_chapter(1, "Two").unwrap();
    let chapter_path = dir.join(&project.manifest.chapters[1].file);
    fs::write(&chapter_path, "---\npov: Mara\n---\nprose here\n").unwrap();

    fs::remove_file(&chapter_path).unwrap();
    fs::create_dir(&chapter_path).unwrap();

    let result = project.edit_chapter_front_matter(1, |fm| fm.set_pov("Vashti"));
    assert!(
        result.is_err(),
        "a failed front-matter read must abort the write"
    );
    assert!(chapter_path.is_dir(), "the chapter was not overwritten");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn accepting_a_word_aborts_when_the_dictionary_cant_be_read() {
    let dir = temp_dir("dict-read-fail");
    let mut project = Project::create(&dir).unwrap();
    let path = dir.join(DICTIONARY_NAME);
    fs::write(&path, "Aerendil\nEowyn\n").unwrap();
    project.dictionary = load_dictionary(&dir);

    // Make the dictionary unreadable.
    fs::remove_file(&path).unwrap();
    fs::create_dir(&path).unwrap();

    let result = project.accept_word("Vashti");
    assert!(
        result.is_err(),
        "a failed dictionary read must abort the write, not shrink the list"
    );
    // The in-memory set is unchanged (the insert only happens after a good write),
    // and the directory stand-in was not overwritten with just the new word.
    assert!(!project.is_accepted("Vashti"), "set not mutated on failure");
    assert!(path.is_dir(), "the dictionary was not overwritten");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn parts_round_trip_and_map_chapters_to_ranges() {
    let dir = temp_dir("parts");
    let mut project = Project::create(&dir).unwrap(); // 1 starter chapter
    assert!(project.manifest.parts.is_none());
    assert_eq!(project.manifest.part_label(), "part"); // default
    assert_eq!(project.manifest.part_of(0), None); // no parts → no mapping

    // Four body chapters for the two parts to cover.
    project.add_chapter(1, "Two").unwrap();
    project.add_chapter(2, "Three").unwrap();
    project.add_chapter(3, "Four").unwrap();
    project.manifest.part_label = Some("act".into());
    project.manifest.parts = Some(vec![
        Part {
            name: Some("The Gathering".into()),
            chapters: [1, 2],
            goal: Some(20_000),
        },
        Part {
            name: None,
            chapters: [3, 4],
            goal: None,
        },
    ]);
    project.save_manifest().unwrap();

    // `[[parts]]` serializes after `part_label` (a scalar) and before
    // `[[chapters]]` (valid TOML: scalars precede arrays-of-tables).
    let toml = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
    assert!(
        toml.find("part_label").unwrap() < toml.find("[[parts]]").unwrap(),
        "part_label must precede [[parts]]:\n{toml}"
    );
    assert!(
        toml.find("[[parts]]").unwrap() < toml.find("[[chapters]]").unwrap(),
        "parts must precede chapters:\n{toml}"
    );

    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.manifest.part_label(), "act");
    let parts = reopened.manifest.parts.as_ref().unwrap();
    assert_eq!(parts[0].name.as_deref(), Some("The Gathering"));
    assert_eq!(parts[0].chapters, [1, 2]);
    assert_eq!(parts[0].goal, Some(20_000), "part goal round-trips (#146)");
    assert_eq!(parts[1].goal, None);
    // Chapter index 0 (chapter 1) is in part 0; index 2 (chapter 3) in part 1.
    assert_eq!(reopened.manifest.part_of(0), Some(0));
    assert_eq!(reopened.manifest.part_of(2), Some(1));
    // A chapter outside every range maps to no part.
    assert_eq!(reopened.manifest.part_of(9), None);

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

#[test]
fn add_rename_swap_remove_persist() {
    let dir = temp_dir("structure");
    let mut project = Project::create(&dir).unwrap(); // "Chapter 1"

    // Add a second chapter after the first; file is created with a fresh name.
    project.add_chapter(1, "Second").unwrap();
    assert_eq!(project.manifest.chapters.len(), 2);
    assert_ne!(
        project.manifest.chapters[0].file,
        project.manifest.chapters[1].file
    );
    assert!(project.chapter_path(1).exists());

    // Rename and reorder.
    project.rename_chapter(0, "First").unwrap();
    project.swap_chapters(0, 1).unwrap();
    let order: Vec<&str> = project
        .manifest
        .chapters
        .iter()
        .map(|c| c.title.as_str())
        .collect();
    assert_eq!(order, vec!["Second", "First"]);

    // Reopen: every change persisted.
    let reopened = Project::open(&dir).unwrap();
    let order: Vec<String> = reopened
        .manifest
        .chapters
        .iter()
        .map(|c| c.title.clone())
        .collect();
    assert_eq!(order, vec!["Second".to_string(), "First".to_string()]);

    // Remove the first; its file moves to unused/ rather than vanishing.
    let removed_file = reopened.manifest.chapters[0].file.clone();
    let mut reopened = reopened;
    reopened.remove_chapter(0).unwrap();
    assert_eq!(reopened.manifest.chapters.len(), 1);
    assert!(dir.join("unused").join(&removed_file).exists());
    assert!(!dir.join(&removed_file).exists());

    fs::remove_dir_all(&dir).ok();
}

/// Importing a manuscript replaces the starter chapter with the imported set:
/// one file per chapter in reading order, the manifest rebuilt, and the title /
/// author applied. The success path is what the atomicity fix must preserve.
#[test]
fn import_chapters_replaces_the_starter_set() {
    let dir = temp_dir("import-ok");
    let mut project = Project::create(&dir).unwrap(); // one starter chapter
    let starter = project.manifest.chapters[0].file.clone();

    let chapters = vec![
        ("One".to_string(), "first body\n".to_string()),
        ("Two".to_string(), "second body\n".to_string()),
        ("Three".to_string(), "third body\n".to_string()),
    ];
    project
        .import_chapters(Some("Imported"), Some("A. Author"), &chapters)
        .unwrap();

    assert_eq!(project.manifest.title, "Imported");
    assert_eq!(project.manifest.author.as_deref(), Some("A. Author"));
    let titles: Vec<&str> = project
        .manifest
        .chapters
        .iter()
        .map(|c| c.title.as_str())
        .collect();
    assert_eq!(titles, vec!["One", "Two", "Three"]);
    assert_eq!(
        fs::read_to_string(project.chapter_path(1)).unwrap(),
        "second body\n"
    );
    // The starter chapter's file (chapter-1.md) is reused, not orphaned; no
    // leftover starter beyond the imported set, and no staging temp files.
    assert!(!dir.join("unused").join(&starter).exists());
    let strays: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(strays.is_empty(), "staging temp left behind: {strays:?}");

    // Persisted across a reopen.
    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.manifest.chapters.len(), 3);
    fs::remove_dir_all(&dir).ok();
}

/// The atomicity guarantee (#600): if a write partway through an import fails,
/// the original chapters and manifest survive byte-for-byte. We seed a project,
/// capture its on-disk state, then import a fresh manuscript with the third
/// staging write armed to fail before its rename — the originals must be intact.
#[test]
fn a_failed_import_write_preserves_the_originals() {
    let dir = temp_dir("import-fail");
    let mut project = Project::create(&dir).unwrap();
    // Give the project real content to lose: rename the starter and add two more.
    project.rename_chapter(0, "Original One").unwrap();
    project.add_chapter(1, "Original Two").unwrap();
    project.add_chapter(2, "Original Three").unwrap();
    for i in 0..3 {
        let path = project.chapter_path(i);
        fs::write(&path, format!("original body {}\n", i + 1)).unwrap();
    }
    project.save_manifest().unwrap();

    // Snapshot every chapter file and the manifest as written on disk.
    let manifest_path = Project::manifest_path(&dir);
    let manifest_before = fs::read_to_string(&manifest_path).unwrap();
    let files_before: Vec<(String, String)> = project
        .manifest
        .chapters
        .iter()
        .map(|c| {
            (
                c.file.clone(),
                fs::read_to_string(dir.join(&c.file)).unwrap(),
            )
        })
        .collect();

    // Import four new chapters, but fail the third staging write before its
    // rename — mid-operation, after some new files are staged.
    let incoming = vec![
        ("New One".to_string(), "doomed one\n".to_string()),
        ("New Two".to_string(), "doomed two\n".to_string()),
        ("New Three".to_string(), "doomed three\n".to_string()),
        ("New Four".to_string(), "doomed four\n".to_string()),
    ];
    crate::atomic::fail_point::arm_pre_rename_failure_on_nth(3);
    let result = project.import_chapters(Some("Doomed"), Some("Nobody"), &incoming);
    assert!(
        result.is_err(),
        "the interrupted import must report failure"
    );

    // The manifest on disk is untouched, and every original chapter file is
    // byte-for-byte intact — no originals deleted, no half-rebuilt manifest.
    assert_eq!(
        fs::read_to_string(&manifest_path).unwrap(),
        manifest_before,
        "the manifest must survive a failed import"
    );
    for (file, body) in &files_before {
        assert_eq!(
            fs::read_to_string(dir.join(file)).unwrap(),
            *body,
            "original chapter {file} must survive a failed import"
        );
    }
    // No staging temp files were left behind.
    let strays: Vec<_> = fs::read_dir(&dir)
        .unwrap()
        .filter_map(std::result::Result::ok)
        .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
        .collect();
    assert!(strays.is_empty(), "staging temp left behind: {strays:?}");

    // And a reopen still sees the originals — the project remains usable.
    let reopened = Project::open(&dir).unwrap();
    let titles: Vec<&str> = reopened
        .manifest
        .chapters
        .iter()
        .map(|c| c.title.as_str())
        .collect();
    assert_eq!(
        titles,
        vec!["Original One", "Original Two", "Original Three"]
    );
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

/// Performance guard (run with `cargo test --release -- --ignored
/// --nocapture`). Opening a project reads and parses every chapter file once
/// (word count + front matter); even a large novel must open near-instantly.
/// Catches a regression to per-chapter re-reads or O(n²) work at open.
#[test]
#[ignore = "release-only perf guard; run via cargo perf"]
fn open_is_fast_for_a_large_novel() {
    let dir = temp_dir("perf-open");
    fs::create_dir_all(&dir).unwrap();
    let manifest = write_chapter_project(&dir, 150, "Big");
    fs::write(Project::manifest_path(&dir), manifest).unwrap();

    let t = Instant::now();
    let project = Project::open(&dir).unwrap();
    let dt = t.elapsed();

    println!("open 150 chapters (front matter + word counts) = {dt:?}");
    assert_eq!(project.manifest.chapters.len(), 150);
    assert!(dt.as_millis() < 250, "open took {dt:?}");

    fs::remove_dir_all(&dir).ok();
}

/// Performance guard (`cargo perf`). Opening a series-sized project (many
/// books' worth of chapters) stays within budget — the per-chapter cache read
/// is the only work proportional to the project, and it's bounded. See
/// `docs/performance.md`. Catches a regression to O(n²) work at open or to
/// re-reading files after the initial cache pass.
#[test]
#[ignore = "release-only perf guard; run via cargo perf"]
fn open_is_fast_for_a_series_sized_project() {
    let n = 1500;
    let dir = temp_dir("perf-open-series");
    fs::create_dir_all(&dir).unwrap();
    let manifest = write_chapter_project(&dir, n, "Series");
    fs::write(Project::manifest_path(&dir), manifest).unwrap();

    let t = Instant::now();
    let project = Project::open(&dir).unwrap();
    let dt = t.elapsed();

    println!("open {n} chapters (series-sized) = {dt:?}");
    assert_eq!(project.manifest.chapters.len(), n);
    assert!(dt.as_millis() < 500, "open took {dt:?}");

    fs::remove_dir_all(&dir).ok();
}
