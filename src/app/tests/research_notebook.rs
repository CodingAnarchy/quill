//! The author's research / worldbuilding notebook (#932): an author-maintained,
//! bible-like journal the AI's trackers *cite* but never write. These hermetic
//! tests (#531) cover the three load-bearing properties — the notebook is an
//! author-editable file that persists, a tracker entry can carry a readable
//! citation to a research note, and the AI-write path never touches the notebook
//! (the no-AI-write invariant that keeps this inside the AI boundary).

use super::*;
use crate::ai::bible;
use crate::mode::Action;
use std::fs;

/// The `research:view` run-action opens the notebook, seeding an empty template on
/// first use; the author's edits then persist across a reopen — it's a plain
/// author-editable file.
#[test]
fn research_notebook_is_author_editable_and_persists() {
    let (mut app, dir) = two_chapter_app("research-persist");
    let path = bible::research_path(&dir);
    assert!(!path.exists(), "no notebook until it's opened");

    // Opening it seeds the template and opens an editable pane.
    app.run_action(Action::ViewResearch, false);
    assert!(path.exists(), "opening seeds the notebook");
    assert!(app.split_view().is_some(), "opens in an editable pane");

    // The author writes a research note; the file persists it verbatim.
    fs::write(
        &path,
        "# Research notebook\n\n## Feudal Timeline\n\nLord Grey ruled 1432-1456.\n",
    )
    .unwrap();

    // Reopen the project from disk: the author's note is still there, and its
    // heading is a note a tracker can cite.
    let reopened = crate::project::Project::open(&dir).unwrap();
    let raw = fs::read_to_string(bible::research_path(&reopened.root)).unwrap();
    assert!(raw.contains("Lord Grey ruled 1432-1456"), "note persisted");
    assert_eq!(
        bible::research_notes(&reopened.root),
        vec!["Feudal Timeline"]
    );

    // Opening again never clobbers the author's writing.
    app.run_action(Action::ViewResearch, false);
    let after = fs::read_to_string(&path).unwrap();
    assert!(
        after.contains("Lord Grey ruled 1432-1456"),
        "reopen preserves it"
    );
}

/// A tracker entry can carry a readable citation to a research note the author has
/// written; the notebook's `## ` headings are the notes a tracker may cite, and the
/// seed template's example heading is not one (so a fresh notebook fabricates
/// nothing).
#[test]
fn a_tracker_entry_can_cite_a_research_note() {
    let (_app, dir) = two_chapter_app("research-cite");
    fs::create_dir_all(dir.join("bible")).unwrap();
    fs::write(
        bible::research_path(&dir),
        "# Research notebook\n\n## Feudal Timeline\n\nLord Grey ruled 1432-1456.\n",
    )
    .unwrap();

    // The author's note is an available citation target; the citation reads cleanly
    // and is recognizable to a reader.
    assert_eq!(bible::research_notes(&dir), vec!["Feudal Timeline"]);
    let cited = format!(
        "Lord Grey ruled 1432-1456 ({})",
        bible::cite_research("Feudal Timeline")
    );
    assert_eq!(
        cited,
        "Lord Grey ruled 1432-1456 (per research note 'Feudal Timeline')"
    );
    assert!(bible::has_research_citation(&cited));
    assert!(!bible::has_research_citation("Lord Grey ruled 1432-1456"));
}

/// The write path grounds a generated tracker entry against the author's notebook
/// (#932): a bullet naming an authored research note gets a readable citation
/// appended — the AI *cites* the note.
#[test]
fn the_write_path_cites_an_authored_research_note() {
    let (mut app, dir) = two_chapter_app("research-write-cite");
    fs::create_dir_all(dir.join("bible")).unwrap();
    fs::write(
        bible::research_path(&dir),
        "# Research notebook\n\n## Feudal Timeline\n\nLord Grey ruled 1432-1456.\n",
    )
    .unwrap();

    // A character whose goal names the authored note by title.
    app.apply_characters(Ok(vec![Character {
        name: "Lord Grey".into(),
        goals: vec!["restore the Feudal Timeline".into()],
        ..Default::default()
    }]));

    let tracker =
        fs::read_to_string(dir.join("bible").join("characters").join("lord-grey.md")).unwrap();
    assert!(
        tracker.contains("restore the Feudal Timeline (per research note 'Feudal Timeline')"),
        "the tracker cites the authored note: {tracker}"
    );
}

/// The no-AI-write invariant (#932): running the AI character pass — a full
/// bible-write path — writes the tracker files but never touches the author's
/// research notebook. The AI cites the notebook; it never authors it.
#[test]
fn the_ai_write_path_never_touches_the_research_notebook() {
    let (mut app, dir) = two_chapter_app("research-no-ai-write");

    // The author owns the notebook; seed it with their prose.
    let path = bible::research_path(&dir);
    let authored = "# Research notebook\n\n## Feudal Timeline\n\nLord Grey ruled 1432-1456.\n";
    fs::create_dir_all(dir.join("bible")).unwrap();
    fs::write(&path, authored).unwrap();

    // Run a full character bible-write pass (the AI-write path, `stage_bible`) —
    // including a character whose entry *cites* the notebook, so the citing path is
    // exercised too.
    app.apply_characters(Ok(vec![Character {
        name: "Lord Grey".into(),
        goals: vec!["restore the Feudal Timeline".into()],
        ..Default::default()
    }]));

    // The tracker file was written…
    assert!(
        dir.join("bible")
            .join("characters")
            .join("lord-grey.md")
            .exists(),
        "the AI wrote the character tracker"
    );
    // …but the author's research notebook is byte-for-byte untouched.
    let after = fs::read_to_string(&path).unwrap();
    assert_eq!(
        after, authored,
        "the AI-write path never touches the notebook"
    );
}
