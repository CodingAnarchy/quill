//! AI Q&A plus the character / continuity / setting trackers, and the mock backend harness.

use super::*;
use std::fs;

// --- AI: "Ask the manuscript" ----------------------------------------------

/// A backend that returns a canned answer (or an error) with no network — lets
/// the Q&A flow be tested deterministically and offline.
struct MockBackend {
    result: Result<String, ()>,
}

impl Backend for MockBackend {
    fn complete(&self, _req: &ai::Request) -> Result<ai::Response, ai::AiError> {
        match &self.result {
            Ok(text) => Ok(ai::Response {
                text: text.clone(),
                usage: Default::default(),
            }),
            Err(()) => Err(ai::AiError::Network {
                status: None,
                message: "boom".into(),
            }),
        }
    }
}

#[test]
fn ask_with_ai_off_explains_how_to_enable() {
    let (mut app, _dir) = two_chapter_app("ai-off");
    app.ai = ai::Settings::default(); // off, deterministic regardless of env/config
    app.ask_ai("who is the protagonist?");
    assert!(
        !app.ai_task.is_busy(),
        "nothing should be sent when AI is off"
    );
    assert!(app.overlay.is_none());
    assert!(
        app.message.as_deref().unwrap_or("").contains("off"),
        "{:?}",
        app.message
    );
}

#[test]
#[cfg(not(feature = "local"))]
fn offline_mode_selects_the_local_backend() {
    let (mut app, _dir) = two_chapter_app("ai-offline");
    app.ai = ai::Settings {
        enabled: true,
        offline: true,
        ..ai::Settings::default()
    };
    // Offline forces the local backend, even with a key present.
    app.ai.api_key = Some("k".into());
    assert_eq!(app.ai.backend(), ai::BackendKind::Local);
    app.ask_ai("anything?");
    // This build has no `local` feature, so it can't run — nothing is sent and the
    // message explains how to enable it (rather than silently using Claude).
    assert!(!app.ai_task.is_busy());
    assert!(
        app.message.as_deref().unwrap_or("").contains("local"),
        "{:?}",
        app.message
    );
}

#[test]
fn manuscript_text_includes_every_chapter() {
    let (app, _dir) = two_chapter_app("ai-manuscript");
    let m = app.manuscript_text();
    assert!(m.contains("# One") && m.contains("first chapter"), "{m}");
    assert!(m.contains("# Two") && m.contains("second chapter"), "{m}");
}

#[test]
fn qa_answer_opens_the_overlay() {
    let (mut app, _dir) = two_chapter_app("ai-answer");
    // Drive the Q&A core with a mock backend (no network): build the request from
    // the manuscript, complete it, and apply the result like the async path does.
    let backend = MockBackend {
        result: Ok("Chapter One is about a locket.".into()),
    };
    let req = ai::build_qa_request(
        app.manuscript_text(),
        "what is chapter one about?",
        &app.ai.model,
    );
    app.apply_qa_result(backend.complete(&req));

    let panel = app.overlay_lines().expect("answer overlay should open");
    let text: String = panel
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
        .collect();
    assert!(text.contains("about a locket"), "{text}");
}

#[test]
fn qa_error_sets_a_message_not_a_panel() {
    let (mut app, _dir) = two_chapter_app("ai-err");
    let backend = MockBackend { result: Err(()) };
    let req = ai::build_qa_request(app.manuscript_text(), "q", &app.ai.model);
    app.apply_qa_result(backend.complete(&req));
    assert!(app.overlay.is_none());
    assert!(
        app.message.as_deref().unwrap_or("").contains("boom"),
        "{:?}",
        app.message
    );
}

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

// --- AI: timeline & continuity ---------------------------------------------

#[test]
fn continuity_writes_timeline_and_opens_findings() {
    let (mut app, dir) = two_chapter_app("cont-write");
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "object".into(),
        detail: "the locket changes color".into(),
        chapters: vec!["One".into()],
    }])));
    let tl = fs::read_to_string(dir.join("bible/timeline.md")).expect("timeline written");
    assert!(tl.contains("alpha event"));
    assert!(
        matches!(app.mode, Mode::Findings(_)),
        "findings overlay should open"
    );
}

#[test]
fn continuity_with_no_findings_summarizes() {
    let (mut app, dir) = two_chapter_app("cont-clean");
    app.apply_continuity(Ok(cont_with(vec![])));
    assert!(dir.join("bible/timeline.md").exists());
    assert!(app.overlay.is_some(), "summary overlay");
    assert!(!matches!(app.mode, Mode::Findings(_)));
}

#[test]
fn findings_dismiss_removes_and_closes_when_empty() {
    let (mut app, _dir) = two_chapter_app("cont-dismiss");
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "age".into(),
        detail: "Mara is 30 then 28".into(),
        chapters: vec!["One".into()],
    }])));
    assert!(matches!(app.mode, Mode::Findings(_)));
    app.on_key(key(KeyCode::Char('d'))); // dismiss the only finding
    assert!(matches!(app.mode, Mode::Insert), "empty list closes");
}

#[test]
fn findings_jump_activates_the_chapter() {
    let (mut app, _dir) = two_chapter_app("cont-jump");
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "location".into(),
        detail: "two places at once".into(),
        chapters: vec!["Two".into()],
    }])));
    app.on_key(key(KeyCode::Enter)); // jump to the finding's chapter
    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(app.project.active_index(), 1, "jumped to chapter Two");
}

#[test]
fn hand_edited_timeline_reviews_then_shows_findings() {
    let (mut app, dir) = two_chapter_app("cont-conflict");
    // First pass: clean timeline (with a finding so findings are pending later too).
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "object".into(),
        detail: "first".into(),
        chapters: vec!["One".into()],
    }])));
    let tl = dir.join("bible/timeline.md");
    // Hand-edit the generated block.
    let edited = fs::read_to_string(&tl)
        .unwrap()
        .replace("alpha event", "TAMPERED");
    fs::write(&tl, edited).unwrap();

    // Second pass with findings → timeline conflict first.
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "object".into(),
        detail: "second".into(),
        chapters: vec!["One".into()],
    }])));
    assert!(
        matches!(app.mode, Mode::BibleReview(_)),
        "timeline conflict reviewed first"
    );

    app.on_key(key(KeyCode::Char('k'))); // keep mine → drains queue → findings open
    assert!(
        matches!(app.mode, Mode::Findings(_)),
        "findings follow the review"
    );
}

// --- AI: setting & worldbuilding -------------------------------------------

#[test]
fn setting_writes_bible_and_opens_rule_findings() {
    let (mut app, dir) = two_chapter_app("setting-write");
    let s = Setting {
        categories: vec![setting::Category {
            name: "Magic system".into(),
            entries: vec![setting::Entry {
                name: "Casting".into(),
                detail: "needs a focus".into(),
            }],
        }],
        gazetteer: vec!["Aldermoor".into()],
        findings: vec![setting::RuleFinding {
            rule: "magic needs a focus".into(),
            violation: "casts barehanded".into(),
            chapters: vec!["Two".into()],
        }],
    };
    app.apply_setting(Ok(s));
    let file = fs::read_to_string(dir.join("bible/setting.md")).expect("setting written");
    assert!(file.contains("## Magic system"));
    assert!(file.contains("## Gazetteer"));
    assert!(
        matches!(app.mode, Mode::Findings(_)),
        "rule findings overlay opens"
    );
}

#[test]
fn setting_with_no_violations_summarizes() {
    let (mut app, dir) = two_chapter_app("setting-clean");
    let s = Setting {
        categories: vec![setting::Category {
            name: "Geography".into(),
            entries: vec![],
        }],
        gazetteer: vec![],
        findings: vec![],
    };
    app.apply_setting(Ok(s));
    assert!(dir.join("bible/setting.md").exists());
    assert!(app.overlay.is_some(), "summary overlay");
    assert!(!matches!(app.mode, Mode::Findings(_)));
}
