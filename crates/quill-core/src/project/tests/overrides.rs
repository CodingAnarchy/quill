//! Tests that a manifest save keeps `book.toml`'s unmodeled top-level keys: the
//! `[quill]` per-project config overrides (#710) and any additive key a newer 1.x
//! wrote (#1097). Covers a plain save, a structural edit, a hand edit made after
//! open, a multi-book series save, convert-to-series, the config loader reading
//! the overrides back, a manifest with nothing unknown staying byte-for-byte, and
//! a file that no longer parses being left alone.

use super::*;
use crate::config::{load_snippets, set_config_dir, set_project_dir, EditorConfig};

/// `[quill]` overrides with a scalar key, a snippets table, and a nested table,
/// plus an unknown top-level scalar and an unknown table from a "newer" Quill.
const EXTRAS: &str = "\
[quill.editor]
typewriter = true

[quill.snippets]
sig = \"— A.\"
hr = \"* * *\"

[quill.theme.colors]
heading = \"#ff8800\"

[future_table]
enabled = true
";

/// A standalone manifest with two chapters and the [`EXTRAS`] appended, plus an
/// unknown top-level scalar ahead of the tables.
fn standalone_with_extras(dir: &Path) {
    fs::write(
        Project::manifest_path(dir),
        format!(
            "title = \"Book\"\nfuture_scalar = \"newer\"\n\n\
             [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
             [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n\n{EXTRAS}"
        ),
    )
    .unwrap();
    fs::write(dir.join("one.md"), "first\n").unwrap();
    fs::write(dir.join("two.md"), "second\n").unwrap();
}

/// The written `book.toml`, parsed (panicking with the text if it's invalid).
fn written(dir: &Path) -> toml::Table {
    let text = fs::read_to_string(Project::manifest_path(dir)).unwrap();
    toml::from_str(&text).unwrap_or_else(|e| panic!("invalid book.toml: {e}\n{text}"))
}

/// Every key from [`EXTRAS`] and the unknown scalar are present in `doc`.
fn assert_extras_kept(doc: &toml::Table) {
    let quill = &doc["quill"];
    assert_eq!(quill["editor"]["typewriter"].as_bool(), Some(true), "{doc}");
    assert_eq!(quill["snippets"]["sig"].as_str(), Some("— A."), "{doc}");
    assert_eq!(quill["snippets"]["hr"].as_str(), Some("* * *"), "{doc}");
    assert_eq!(
        quill["theme"]["colors"]["heading"].as_str(),
        Some("#ff8800"),
        "{doc}"
    );
    assert_eq!(
        doc["future_table"]["enabled"].as_bool(),
        Some(true),
        "{doc}"
    );
    assert_eq!(doc["future_scalar"].as_str(), Some("newer"), "{doc}");
}

#[test]
fn a_plain_save_keeps_quill_overrides_and_unknown_keys() {
    let dir = temp_dir("extras-plain");
    standalone_with_extras(&dir);

    Project::open(&dir).unwrap().save_manifest().unwrap();

    let doc = written(&dir);
    assert_extras_kept(&doc);
    assert_eq!(doc["title"].as_str(), Some("Book"));
    assert_eq!(doc["chapters"].as_array().map(Vec::len), Some(2));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn structural_edits_keep_quill_overrides() {
    let dir = temp_dir("extras-structural");
    standalone_with_extras(&dir);
    let mut project = Project::open(&dir).unwrap();

    project.rename_chapter(0, "Uno").unwrap();
    assert_extras_kept(&written(&dir));
    project.swap_chapters(0, 1).unwrap();
    assert_extras_kept(&written(&dir));
    project.add_chapter(1, "Interlude").unwrap();
    assert_extras_kept(&written(&dir));

    let reopened = Project::open(&dir).unwrap();
    let titles: Vec<&str> = reopened
        .manifest
        .chapters
        .iter()
        .map(|c| c.title.as_str())
        .collect();
    assert_eq!(titles, ["Two", "Interlude", "Uno"]);
    fs::remove_dir_all(&dir).ok();
}

// The unknown keys are read from disk at save time, not cached at open: an
// override the author adds by hand while Quill is running survives the next save.
#[test]
fn a_hand_edit_made_after_open_survives_the_next_save() {
    let dir = temp_dir("extras-hand-edit");
    standalone_with_extras(&dir);
    let project = Project::open(&dir).unwrap();

    let path = Project::manifest_path(&dir);
    let mut text = fs::read_to_string(&path).unwrap();
    text.push_str("\n[quill.backup]\ninterval_minutes = 7\n");
    fs::write(&path, text).unwrap();

    project.save_manifest().unwrap();
    let doc = written(&dir);
    assert_eq!(
        doc["quill"]["backup"]["interval_minutes"].as_integer(),
        Some(7)
    );
    assert_extras_kept(&doc);
    fs::remove_dir_all(&dir).ok();
}

// After a save, the config loader still sees the project's overrides.
#[test]
fn the_overrides_still_load_after_a_save() {
    let dir = temp_dir("extras-load");
    let cfg = temp_dir("extras-load-cfg");
    standalone_with_extras(&dir);
    set_config_dir(Some(cfg.clone()));
    set_project_dir(Some(dir.clone()));

    let mut project = Project::open(&dir).unwrap();
    project.rename_chapter(1, "Deux").unwrap();

    let editor = EditorConfig::load();
    let (snippets, warnings) = load_snippets();
    set_project_dir(None);
    set_config_dir(None);

    assert!(
        editor.flags.typewriter,
        "[quill.editor] typewriter still applies"
    );
    assert!(warnings.is_empty(), "{warnings:?}");
    let names: Vec<&str> = snippets.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(names, ["hr", "sig"]);
    fs::remove_dir_all(&dir).ok();
    fs::remove_dir_all(&cfg).ok();
}

// No `[quill]` and no unknown keys: the save is exactly the typed serialization —
// no format churn for every existing project.
#[test]
fn a_manifest_with_nothing_unknown_saves_byte_for_byte_as_before() {
    let dir = temp_dir("extras-none");
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\nauthor = \"A\"\n\n[goals]\nwords = 90000\n\n\
         [[parts]]\nname = \"I\"\nchapters = [1, 1]\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "first\n").unwrap();
    let project = Project::open(&dir).unwrap();

    project.save_manifest().unwrap();
    let text = fs::read_to_string(Project::manifest_path(&dir)).unwrap();
    assert_eq!(text, toml::to_string_pretty(&project.manifest).unwrap());
    fs::remove_dir_all(&dir).ok();
}

/// A two-book series manifest with the [`EXTRAS`] at the series root.
fn series_with_extras(dir: &Path) {
    fs::write(
        Project::manifest_path(dir),
        format!(
            "title = \"Saga\"\nfuture_scalar = \"newer\"\n\n\
             [[books]]\ntitle = \"Book One\"\n\n\
             [[books.chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
             [[books]]\ntitle = \"Book Two\"\n\n\
             [[books.chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n\n{EXTRAS}"
        ),
    )
    .unwrap();
    fs::write(dir.join("one.md"), "first\n").unwrap();
    fs::write(dir.join("two.md"), "second\n").unwrap();
}

#[test]
fn a_series_save_keeps_quill_overrides() {
    let dir = temp_dir("extras-series");
    series_with_extras(&dir);
    let mut project = Project::open(&dir).unwrap();
    assert_eq!(project.book_count(), 2);

    project.rename_chapter(0, "Uno").unwrap();
    let doc = written(&dir);
    assert_extras_kept(&doc);
    assert_eq!(doc["books"].as_array().map(Vec::len), Some(2));

    let reopened = Project::open(&dir).unwrap();
    assert_eq!(reopened.book_count(), 2);
    assert_eq!(reopened.manifest.chapters[0].title, "Uno");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn convert_to_series_keeps_quill_overrides() {
    let dir = temp_dir("extras-convert");
    standalone_with_extras(&dir);
    let mut project = Project::open(&dir).unwrap();

    project.convert_to_series("Saga").unwrap();
    let doc = written(&dir);
    assert_extras_kept(&doc);
    assert_eq!(doc["title"].as_str(), Some("Saga"));
    assert!(doc.contains_key("books"), "{doc}");

    // Saving again in the series shape still carries them.
    project.save_manifest().unwrap();
    assert_extras_kept(&written(&dir));
    assert_eq!(Project::open(&dir).unwrap().book_count(), 1);
    fs::remove_dir_all(&dir).ok();
}

// A `book.toml` hand-broken after open can't have its unknown keys recovered, so
// the save refuses rather than overwrite the author's half-finished edit.
#[test]
fn a_manifest_that_no_longer_parses_is_not_overwritten() {
    let dir = temp_dir("extras-broken");
    standalone_with_extras(&dir);
    let project = Project::open(&dir).unwrap();

    let path = Project::manifest_path(&dir);
    let broken = "title = \"Book\"\n[quill.editor\n";
    fs::write(&path, broken).unwrap();

    let err = project.save_manifest().unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(fs::read_to_string(&path).unwrap(), broken);
    fs::remove_dir_all(&dir).ok();
}
