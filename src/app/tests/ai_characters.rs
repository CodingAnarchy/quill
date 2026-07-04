//! AI character tracking + story bible: mention enrichment, writing character
//! profiles, and the hand-edit conflict review (overwrite / keep / edit).

use super::*;
use std::fs;

// --- AI: character tracking + story bible -----------------------------------

#[test]
fn enrich_mentions_counts_names_and_last_chapter() {
    let (app, _dir) = two_chapter_app("char-mentions"); // "first chapter" / "second chapter"
    let mut c = Character {
        name: "chapter".into(),
        ..Default::default()
    };
    app.enrich_mentions(&mut c);
    assert_eq!(c.mentions, 2); // once in each chapter body
    assert_eq!(c.last_chapter.as_deref(), Some("Two"));
}

#[test]
fn character_pass_writes_bible_files_and_summarizes() {
    let (mut app, dir) = two_chapter_app("char-write");
    let chars = vec![Character {
        name: "Mara Vance".into(),
        goals: vec!["escape".into()],
        ..Default::default()
    }];
    app.apply_characters(Ok(chars));

    let file = dir.join("bible/characters/mara-vance.md");
    let content = fs::read_to_string(&file).expect("profile written");
    assert!(content.contains("name: Mara Vance"));
    assert!(content.contains("escape"));
    assert!(app.overlay.is_some(), "summary overlay should open");
    assert!(
        !matches!(app.mode, Mode::BibleReview(_)),
        "no conflicts → no review"
    );
}

#[test]
fn written_profile_carries_a_provenance_line() {
    // A generated bible entry records a readable provenance line naming its source
    // chapters and the AI-generated state (#832); the two-chapter fixture makes the
    // source `ch. 1–2`.
    let (mut app, dir) = two_chapter_app("char-provenance");
    app.apply_characters(Ok(vec![Character {
        name: "Mara Vance".into(),
        goals: vec!["escape".into()],
        ..Default::default()
    }]));

    let content =
        fs::read_to_string(dir.join("bible/characters/mara-vance.md")).expect("profile written");
    assert!(
        content.contains("> Generated from ch. 1–2 · AI-generated"),
        "provenance line present: {content}"
    );
    assert_eq!(
        crate::ai::bible::provenance(&content).as_deref(),
        Some("> Generated from ch. 1–2 · AI-generated"),
        "reads as AI-generated while unedited"
    );

    // A hand-edit of the generated body flips the reported state (#832).
    let edited = content.replace("escape", "flee");
    assert_eq!(
        crate::ai::bible::provenance(&edited).as_deref(),
        Some("> Generated from ch. 1–2 · edited by you")
    );
}

#[test]
fn failed_bible_write_is_reported_not_counted_as_success() {
    let (mut app, dir) = two_chapter_app("char-write-fail");
    // Make `bible` a regular file so the `bible/characters` dir can't be created
    // — every profile write then fails.
    fs::write(dir.join("bible"), "x").unwrap();

    app.apply_characters(Ok(vec![Character {
        name: "Mara".into(),
        ..Default::default()
    }]));

    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Couldn't write"),
        "a failed write is surfaced: {:?}",
        app.message
    );
    assert!(
        app.overlay.is_none(),
        "no success summary when the write failed"
    );
    assert!(
        !matches!(app.mode, Mode::BibleReview(_)),
        "a write failure isn't a conflict"
    );
}

/// Set up a project with a clean, then hand-edited, Mara profile; a second pass
/// with new content must not overwrite it.
fn conflict_app(name: &str) -> (App, TempProject, PathBuf) {
    let (mut app, dir) = two_chapter_app(name);
    app.apply_characters(Ok(vec![Character {
        name: "Mara".into(),
        physical: vec!["green eyes".into()],
        ..Default::default()
    }]));
    let file = dir.join("bible/characters/mara.md");
    let edited = fs::read_to_string(&file)
        .unwrap()
        .replace("green eyes", "blue eyes");
    fs::write(&file, edited).unwrap();
    // Second pass, different content → conflict.
    app.apply_characters(Ok(vec![Character {
        name: "Mara".into(),
        physical: vec!["hazel eyes".into()],
        ..Default::default()
    }]));
    (app, dir, file)
}

#[test]
fn hand_edited_profile_enters_review_without_clobbering() {
    let (app, _dir, file) = conflict_app("char-conflict");
    assert!(
        matches!(app.mode, Mode::BibleReview(_)),
        "should queue a review"
    );
    assert!(
        fs::read_to_string(&file).unwrap().contains("blue eyes"),
        "the hand edit must survive until resolved"
    );
}

#[test]
fn review_overwrite_writes_the_proposed_version() {
    let (mut app, _dir, file) = conflict_app("char-resolve-o");
    app.on_key(key(KeyCode::Char('o')));
    assert!(
        matches!(app.mode, Mode::Insert),
        "single conflict resolved → done"
    );
    assert!(
        fs::read_to_string(&file).unwrap().contains("hazel eyes"),
        "proposed written"
    );
}

#[test]
fn review_keep_preserves_the_authors_version() {
    let (mut app, _dir, file) = conflict_app("char-resolve-k");
    app.on_key(key(KeyCode::Char('k')));
    assert!(matches!(app.mode, Mode::Insert));
    assert!(
        fs::read_to_string(&file).unwrap().contains("blue eyes"),
        "kept the hand edit"
    );
}

#[test]
fn review_edit_then_save_writes_the_edited_version() {
    let (mut app, _dir, file) = conflict_app("char-resolve-e");
    app.on_key(key(KeyCode::Char('e'))); // load proposed into the scratch editor
    assert!(matches!(&app.mode, Mode::BibleReview(r) if r.editing.is_some()));
    app.edit_buffer_mut().insert_char('!'); // tweak the proposed version
    app.on_key(ctrl('s')); // save
    assert!(matches!(app.mode, Mode::Insert));
    let saved = fs::read_to_string(&file).unwrap();
    assert!(
        saved.contains("hazel eyes") && saved.contains('!'),
        "edited proposal saved"
    );
}
