//! Structural edits made through the editor keep `book.toml`'s `[quill]`
//! per-project config overrides (#710): adding, renaming, and reordering
//! chapters, and setting a goal, all rewrite the manifest, and none of them may
//! erase the overrides — which the config loader must still read afterwards.

use super::*;
use crate::config::{load_snippets, set_config_dir, EditorConfig};
use crate::project::GoalField;

/// A two-chapter project whose manifest carries `[quill]` overrides: an editor
/// key, snippets, and a nested table.
fn overrides_app(name: &str) -> (App, TempProject) {
    isolate(&format!("{name}-cfg"));
    let dir = app_scratch_dir(name);
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Book\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n\n\
         [quill.editor]\ntypewriter = true\n\n\
         [quill.snippets]\nsig = \"— A.\"\n\n\
         [quill.theme.colors]\nheading = \"#ff8800\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "first chapter\n").unwrap();
    fs::write(dir.join("two.md"), "second chapter\n").unwrap();
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

/// The `[quill]` table as written to disk, panicking if the file is invalid.
fn quill_table(dir: &Path) -> toml::Value {
    let text = fs::read_to_string(Project::manifest_path(dir)).unwrap();
    let doc: toml::Table =
        toml::from_str(&text).unwrap_or_else(|e| panic!("invalid book.toml: {e}\n{text}"));
    doc.get("quill")
        .cloned()
        .unwrap_or_else(|| panic!("[quill] erased:\n{text}"))
}

fn assert_overrides_kept(dir: &Path) {
    let quill = quill_table(dir);
    assert_eq!(quill["editor"]["typewriter"].as_bool(), Some(true));
    assert_eq!(quill["snippets"]["sig"].as_str(), Some("— A."));
    assert_eq!(
        quill["theme"]["colors"]["heading"].as_str(),
        Some("#ff8800")
    );
}

#[test]
fn chapter_structure_edits_keep_the_overrides() {
    let (mut app, dir) = overrides_app("overrides-structure");
    app.on_key(ctrl('g')); // chapter navigator, selection on chapter 0

    app.on_key(key(KeyCode::Char('a'))); // add a chapter after the selection
    type_str(&mut app, "Interlude");
    app.on_key(key(KeyCode::Enter));
    assert_overrides_kept(&dir);

    app.on_key(key(KeyCode::Char('r'))); // rename the selection (the new chapter)
    while matches!(&app.mode, Mode::Prompt(p) if !p.input.is_empty()) {
        app.on_key(key(KeyCode::Backspace));
    }
    type_str(&mut app, "Uno");
    app.on_key(key(KeyCode::Enter));
    assert_overrides_kept(&dir);

    app.on_key(key(KeyCode::Char('J'))); // move it down
    assert_overrides_kept(&dir);

    let titles: Vec<String> = Project::open(&dir)
        .unwrap()
        .manifest
        .chapters
        .iter()
        .map(|c| c.title.clone())
        .collect();
    assert_eq!(titles, ["One", "Two", "Uno"]);

    // The config loader still sees the project's overrides after the rewrites.
    assert!(EditorConfig::load().typewriter);
    let (snippets, _) = load_snippets();
    assert!(snippets.iter().any(|s| s.name == "sig" && s.text == "— A."));
    set_config_dir(None);
}

#[test]
fn setting_a_goal_keeps_the_overrides() {
    let (mut app, dir) = overrides_app("overrides-goal");
    app.project
        .set_goal(GoalField::Words, Some(80_000))
        .unwrap();
    assert_overrides_kept(&dir);
    assert!(EditorConfig::load().typewriter);
    set_config_dir(None);
}
