use std::fs;
use std::path::Path;
use std::sync::Arc;

use quill_core::testutil::scratch_dir;

use super::*;

/// A two-chapter novel with a bible (two story pages, one character) and a
/// findings store holding two open findings and one dismissed one.
fn project_with_bible(dir: &Path) -> Arc<QuillProject> {
    fs::write(
        dir.join("book.toml"),
        "title = \"Book\"\n\n[[chapters]]\ntitle = \"Arrival\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"The Storm\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "Mara lands at dusk.\n").unwrap();
    fs::write(
        dir.join("two.md"),
        "🙂 The locket was gold, she said, and warm.\n",
    )
    .unwrap();
    let bible = dir.join("bible");
    fs::create_dir_all(bible.join("characters")).unwrap();
    fs::write(
        bible.join("setting.md"),
        "---\ndigest: x\n---\n# Setting\n\nA lighthouse.\n",
    )
    .unwrap();
    fs::write(bible.join("research-notes.md"), "Tides.\n").unwrap();
    fs::write(
        bible.join("characters/mara.md"),
        "---\nname: \"Mara Vance\"\n---\nEyes: grey.\n",
    )
    .unwrap();
    fs::write(bible.join(".state.toml"), "x = 1\n").unwrap();
    fs::write(
        bible.join("findings.toml"),
        r#"[[findings]]
summary = "object: the locket — \"was gold\" here, silver in Arrival"
kind = "continuity"
chapters = ["the storm", "Arrival"]

[[findings]]
summary = "pacing: a quick landing"
kind = "pacing"
chapters = ["Arrival"]

[[findings]]
summary = "dismissed one"
kind = "style"
chapters = ["Arrival"]
dismissed = true
"#,
    )
    .unwrap();
    QuillProject::open(&dir.to_string_lossy()).unwrap()
}

#[test]
fn lists_the_bible_by_section_with_readable_titles() {
    let dir = scratch_dir("ffi-bible", "pages");
    let project = project_with_bible(&dir);
    let pages = project.bible_pages().unwrap();
    let listed: Vec<(&str, &str)> = pages
        .iter()
        .map(|p| (p.section.as_str(), p.title.as_str()))
        .collect();
    assert_eq!(
        listed,
        [
            ("Story", "Research notes"),
            ("Story", "Setting"),
            ("Characters", "Mara Vance"),
        ]
    );
    assert_eq!(
        project.bible_page("characters/mara.md").unwrap(),
        "Eyes: grey.\n"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_page_outside_the_bible_is_refused() {
    let dir = scratch_dir("ffi-bible", "outside");
    let project = project_with_bible(&dir);
    for path in ["../book.toml", "/etc/hosts", "findings.toml", "../one.md"] {
        assert!(project.bible_page(path).is_err(), "{path}");
    }
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn findings_come_in_manuscript_order_and_open_their_chapter() {
    let dir = scratch_dir("ffi-bible", "findings");
    let project = project_with_bible(&dir);
    let findings = project.findings().unwrap();
    let summaries: Vec<&str> = findings.iter().map(|f| f.summary.as_str()).collect();
    assert_eq!(
        summaries.len(),
        2,
        "the dismissed one is hidden: {summaries:?}"
    );
    // "Arrival" (chapter 0) first; "the storm" matches "The Storm" ignoring case.
    assert_eq!(findings[0].chapter_index, Some(0));
    assert_eq!(findings[0].kind, "Pacing & tension");
    assert_eq!(findings[1].chapter_index, Some(1));
    assert_eq!(findings[1].kind, "Continuity");

    // The quoted phrase is found in the chapter, after the emoji (2 units).
    let span = project
        .finding_passage(1, &findings[1].summary)
        .unwrap()
        .unwrap();
    assert_eq!((span.start, span.length), (14, 8));
    assert_eq!(project.finding_passage(0, "no quotes here").unwrap(), None);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn reading_never_writes_under_the_bible() {
    let dir = scratch_dir("ffi-bible", "readonly");
    let project = project_with_bible(&dir);
    let snapshot = || {
        let mut files: Vec<(String, Vec<u8>)> = Vec::new();
        for sub in ["", "characters/"] {
            for e in fs::read_dir(dir.join("bible").join(sub)).unwrap().flatten() {
                if e.file_type().unwrap().is_file() {
                    files.push((e.path().display().to_string(), fs::read(e.path()).unwrap()));
                }
            }
        }
        files.sort();
        files
    };
    let before = snapshot();
    project.bible_pages().unwrap();
    project.bible_page("setting.md").unwrap();
    let findings = project.findings().unwrap();
    project.finding_passage(1, &findings[1].summary).unwrap();
    assert_eq!(snapshot(), before);
    fs::remove_dir_all(&dir).ok();
}
