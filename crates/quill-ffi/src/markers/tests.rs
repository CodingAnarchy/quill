use std::fs;
use std::path::Path;
use std::sync::Arc;

use quill_core::testutil::scratch_dir;

use crate::StyleKind;

use super::*;

fn project(dir: &Path, marker: Option<&str>) -> Arc<QuillProject> {
    let marker = marker.map_or_else(String::new, |m| format!("todo_marker = \"{m}\"\n"));
    fs::write(
        dir.join("book.toml"),
        format!(
            "title = \"Book\"\n{marker}\n[[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
             [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n\n\
             [[chapters]]\ntitle = \"Three\"\nfile = \"three.md\"\n"
        ),
    )
    .unwrap();
    fs::write(dir.join("one.md"), "Clean prose.\n").unwrap();
    fs::write(
        dir.join("two.md"),
        "🙂 XXX: check the tide\nMore.\nxxx dates\n",
    )
    .unwrap();
    fs::write(dir.join("three.md"), "Nothing here.\n").unwrap();
    QuillProject::open(&dir.to_string_lossy()).unwrap()
}

#[test]
fn lists_markers_across_the_project_in_order() {
    let dir = scratch_dir("ffi-markers", "list");
    let project = project(&dir, None);
    let markers = project.markers().unwrap();
    let found: Vec<(u32, u32, &str)> = markers
        .iter()
        .map(|m| (m.chapter_index, m.start, m.line.as_str()))
        .collect();
    // Case-insensitive, like the desktop's count; UTF-16 offsets after 🙂.
    assert_eq!(
        found,
        [(1, 3, "🙂 XXX: check the tide"), (1, 29, "xxx dates")]
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_marker_typed_on_the_phone_joins_the_list_and_stands_out() {
    let dir = scratch_dir("ffi-markers", "typed");
    let project = project(&dir, Some("TK"));
    assert_eq!(project.todo_marker().unwrap(), "TK");
    let session = Arc::clone(&project).edit_chapter(0).unwrap();
    session.apply_edit(0, 0, "TK fix this. ");
    // The open chapter's list is live, before any save.
    assert_eq!(session.markers().len(), 1);
    let todo: Vec<(u32, u32)> = session
        .styles(0, session.utf16_length())
        .into_iter()
        .filter(|s| s.kind == StyleKind::Todo)
        .map(|s| (s.start, s.length))
        .collect();
    assert_eq!(todo, [(0, 2)]);
    // Saving updates the cached count, so the project-wide list sees it.
    assert!(project
        .markers()
        .unwrap()
        .iter()
        .all(|m| m.chapter_index != 0));
    session.save().unwrap();
    assert!(project
        .markers()
        .unwrap()
        .iter()
        .any(|m| m.chapter_index == 0));
    fs::remove_dir_all(&dir).ok();
}
