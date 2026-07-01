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

/// Drive a structured Q&A reply (JSON with citations) through `apply_qa_result`
/// like the async path does, recording the question first as `ask_ai` would.
fn answer_with_citations(app: &mut App, question: &str, json: &str) {
    let backend = MockBackend {
        result: Ok(json.to_string()),
    };
    let req = ai::build_qa_request(app.manuscript_text(), question, &app.ai.model);
    app.pending_qa_question = Some(question.to_string());
    app.apply_qa_result(backend.complete(&req));
}

#[test]
fn qa_answer_with_citations_renders_jumpable_entries() {
    let (mut app, _dir) = two_chapter_app("ai-cite");
    answer_with_citations(
        &mut app,
        "where is the locket?",
        r#"{"answer":"The locket appears in chapter Two.","citations":["Two"]}"#,
    );
    match &app.mode {
        Mode::Findings(f) => {
            // The answer prose leads as non-jumpable context …
            let text: String = f.items.iter().map(|i| i.summary.clone()).collect();
            assert!(text.contains("appears in chapter Two"), "{text}");
            let context = &f.items[f.selected.min(1).saturating_sub(1)];
            assert!(
                context.chapters.is_empty(),
                "answer rows aren't jump targets"
            );
            // … and the selected entry is the jumpable citation for the cited chapter.
            let jump = &f.items[f.selected];
            assert_eq!(jump.chapters, vec!["Two".to_string()]);
            assert!(jump.summary.contains("Two"), "{}", jump.summary);
        }
        _ => panic!("citations should open a jumpable findings list"),
    }
}

#[test]
fn qa_citation_jump_opens_the_cited_chapter() {
    let (mut app, _dir) = two_chapter_app("ai-cite-jump");
    answer_with_citations(
        &mut app,
        "where is the locket?",
        r#"{"answer":"See chapter Two.","citations":["Two"]}"#,
    );
    app.on_key(key(KeyCode::Enter)); // jump to the cited chapter
    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(app.project.active_index(), 1, "jumped to chapter Two");
}

#[test]
fn qa_answer_without_citations_stays_a_plain_overlay() {
    let (mut app, _dir) = two_chapter_app("ai-cite-none");
    answer_with_citations(
        &mut app,
        "is there a dragon?",
        r#"{"answer":"The manuscript never mentions a dragon.","citations":[]}"#,
    );
    assert!(app.overlay.is_some(), "the answer still shows");
    assert!(
        !matches!(app.mode, Mode::Findings(_)),
        "no citations → no jumpable list"
    );
}

#[test]
fn qa_log_records_asks_and_is_browsable() {
    let (mut app, _dir) = two_chapter_app("ai-log");
    answer_with_citations(
        &mut app,
        "who is in chapter one?",
        r#"{"answer":"Mara, in chapter One.","citations":["One"]}"#,
    );
    answer_with_citations(
        &mut app,
        "and chapter two?",
        r#"{"answer":"Nobody new.","citations":["Two"]}"#,
    );
    assert_eq!(app.qa_log.len(), 2, "both asks are logged");
    assert_eq!(app.qa_log[0].question, "who is in chapter one?");
    assert_eq!(app.qa_log[1].answer.citations, vec!["Two".to_string()]);

    // Browsing the history opens a jumpable list of every past ask's citations,
    // newest first, labeled with the question.
    app.open_qa_history();
    match &app.mode {
        Mode::Findings(f) => {
            assert_eq!(f.items.len(), 2, "one row per cited chapter, both asks");
            assert!(
                f.items[0].summary.contains("and chapter two?"),
                "newest ask first: {}",
                f.items[0].summary
            );
            assert_eq!(f.items[0].chapters, vec!["Two".to_string()]);
        }
        _ => panic!("history should open a jumpable findings list"),
    }
    app.on_key(key(KeyCode::Enter)); // jump from the history
    assert_eq!(
        app.project.active_index(),
        1,
        "jumped to the cited chapter Two"
    );
}

#[test]
fn qa_history_is_empty_before_any_ask() {
    let (mut app, _dir) = two_chapter_app("ai-log-empty");
    app.open_qa_history();
    assert!(!matches!(app.mode, Mode::Findings(_)), "nothing to browse");
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("No questions"),
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

/// A continuity result with a single event of the given description and no
/// findings — for the revise-delta tests, where a second pass rewrites the same
/// clean (AI-written) timeline with different content.
fn cont_event(description: &str) -> Continuity {
    Continuity {
        events: vec![continuity::Event {
            description: description.into(),
            chapter: "One".into(),
            story_order: 1,
        }],
        findings: Vec::new(),
    }
}

#[test]
fn clean_revise_surfaces_the_delta() {
    let (mut app, dir) = two_chapter_app("cont-delta");
    // First pass establishes a clean, AI-written timeline (no findings).
    app.apply_continuity(Ok(cont_event("alpha event")));
    assert!(dir.join("bible/timeline.md").exists());

    // A second pass rewrites the same file with different content. The file was
    // AI-written (not hand-edited), so it's a clean write — and the author should
    // see what changed (#607).
    app.apply_continuity(Ok(cont_event("beta event")));
    let panel = app
        .overlay_lines()
        .expect("a clean revise that changed the tracker opens the delta overlay");
    let text: String = panel
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        text.starts_with("Updated timeline: +"),
        "the panel leads with a `+N/-M` delta summary: {text:?}"
    );
    assert!(
        text.contains("+- **One:** beta event") && text.contains("-- **One:** alpha event"),
        "the diff shows the change from alpha to beta: {text:?}"
    );
}

#[test]
fn revise_with_findings_puts_the_delta_summary_in_the_message() {
    let (mut app, _dir) = two_chapter_app("cont-delta-findings");
    // A clean first pass, then a revise that both changes the tracker and
    // surfaces a finding. Findings lead (the overlay), but the `+N/-M` summary
    // rides the message line so the delta isn't lost.
    app.apply_continuity(Ok(cont_event("alpha event")));
    app.apply_continuity(Ok(Continuity {
        events: vec![continuity::Event {
            description: "beta event".into(),
            chapter: "One".into(),
            story_order: 1,
        }],
        findings: vec![continuity::Finding {
            kind: "object".into(),
            detail: "the locket changes color".into(),
            chapters: vec!["One".into()],
        }],
    }));
    assert!(
        matches!(app.mode, Mode::Findings(_)),
        "findings still lead the review"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .starts_with("Updated timeline: +"),
        "the delta summary rides the message line: {:?}",
        app.message
    );
}

#[test]
fn no_change_revise_shows_no_delta() {
    let (mut app, _dir) = two_chapter_app("cont-nodelta");
    app.apply_continuity(Ok(cont_event("alpha event")));

    // Re-run the identical pass: the file is unchanged, so there's no delta to
    // show — the historic clean-write summary, not a diff.
    app.apply_continuity(Ok(cont_event("alpha event")));
    let text = app
        .overlay_lines()
        .expect("the clean-write summary still shows")
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !text.contains("Updated") && !text.contains("+beta"),
        "a no-change write surfaces no delta: {text:?}"
    );
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

// --- Persistent revision triage (#591) -------------------------------------

/// A continuity result carrying a single finding with the given detail.
fn one_finding(detail: &str) -> Continuity {
    cont_with(vec![continuity::Finding {
        kind: "object".into(),
        detail: detail.into(),
        chapters: vec!["One".into()],
    }])
}

#[test]
fn dismissed_finding_stays_dismissed_after_reload() {
    let (mut app, dir) = two_chapter_app("triage-persist-dismiss");
    app.apply_continuity(Ok(one_finding("the locket changes color")));
    assert!(matches!(app.mode, Mode::Findings(_)));
    app.on_key(key(KeyCode::Char('d'))); // dismiss the only finding
    assert!(
        app.triage.is_empty(),
        "dismissed leaves the session triage list"
    );

    // Reopen the project (a new session): the dismissed finding must not return.
    let reopened = open_test_app(&dir);
    assert!(
        reopened.triage.is_empty(),
        "a dismissed finding stays dismissed across sessions"
    );
    drop(dir);
}

#[test]
fn rerun_does_not_resurface_a_dismissed_finding() {
    let (mut app, dir) = two_chapter_app("triage-persist-rerun");
    app.apply_continuity(Ok(one_finding("the locket changes color")));
    app.on_key(key(KeyCode::Char('d'))); // dismiss it
    assert!(matches!(app.mode, Mode::Insert), "list emptied and closed");

    // Re-run the same pass: it re-produces the identical finding, which must be
    // filtered out rather than popping the triage overlay again.
    app.apply_continuity(Ok(one_finding("the locket changes color")));
    assert!(
        app.triage.is_empty(),
        "a re-produced dismissed finding does not re-surface"
    );
    assert!(
        !matches!(app.mode, Mode::Findings(_)),
        "no overlay for an already-dismissed finding"
    );
    drop(dir);
}

#[test]
fn open_triage_list_survives_a_restart() {
    let (mut app, dir) = two_chapter_app("triage-persist-survive");
    app.apply_continuity(Ok(one_finding("the locket changes color")));
    assert_eq!(app.triage.len(), 1);
    app.on_key(key(KeyCode::Esc)); // leave the finding open (neither dismiss nor mark)

    // A new session restores the still-open finding as a standing to-do.
    let mut reopened = open_test_app(&dir);
    assert_eq!(
        reopened.triage.len(),
        1,
        "an un-triaged finding survives a restart"
    );
    reopened.open_triage();
    assert!(
        matches!(reopened.mode, Mode::Findings(_)),
        "the restored finding opens in triage"
    );
    drop(dir);
}

#[test]
fn marking_xxx_resolves_and_drops_the_finding() {
    let (mut app, dir) = two_chapter_app("triage-persist-mark");
    app.apply_continuity(Ok(one_finding("the locket changes color")));
    assert!(matches!(app.mode, Mode::Findings(_)));
    app.on_key(key(KeyCode::Char('i'))); // mark xxx in the chapter
    let body = fs::read_to_string(dir.join("one.md")).unwrap();
    assert!(
        body.contains("xxx —"),
        "the marker is written (unchanged behavior)"
    );
    assert!(app.triage.is_empty(), "a resolved finding drops off triage");

    // It stays gone across a restart.
    let reopened = open_test_app(&dir);
    assert!(
        reopened.triage.is_empty(),
        "a resolved finding does not return"
    );
    drop(dir);
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

// --- Re-check a single finding (#608) --------------------------------------

/// A continuity UI finding on chapter `chapter`, tagged to the continuity pass so
/// it can be re-checked. Mirrors how `apply_continuity` builds a finding's summary
/// (`kind: detail`), so a re-run that re-parses to the same content matches it.
fn continuity_ui_finding(chapter: &str, detail: &str) -> crate::mode::Finding {
    crate::mode::Finding {
        summary: format!("object: {detail}"),
        chapters: vec![chapter.into()],
        book: None,
        action: Some(crate::mode::AiAction::Continuity),
    }
}

/// A continuity result carrying one finding of `detail` on `chapter` — the JSON a
/// scoped re-check parses, wrapped through the pass's own parser so its summary
/// exactly matches `continuity_ui_finding`.
fn continuity_json(chapter: &str, detail: &str) -> String {
    format!(
        r#"{{"events":[],"findings":[{{"kind":"object","detail":"{detail}","chapters":["{chapter}"]}}]}}"#
    )
}

#[test]
fn recheck_request_is_scoped_to_the_findings_chapters() {
    let (app, _dir) = two_chapter_app("recheck-scope");
    // A finding on chapter Two only. Its re-check request must carry Two's prose
    // and not One's — the whole point of a scoped re-check (#608).
    let finding = continuity_ui_finding("Two", "the locket changes color");
    let req = app
        .recheck_request_for(&finding)
        .expect("a continuity finding builds a re-check request");
    assert!(
        req.cached_context.contains("# Two") && req.cached_context.contains("second chapter"),
        "the finding's chapter is in the request: {}",
        req.cached_context
    );
    assert!(
        !req.cached_context.contains("first chapter"),
        "the other chapter is not sent: {}",
        req.cached_context
    );
}

#[test]
fn recheck_that_still_flags_keeps_the_finding() {
    let (mut app, _dir) = two_chapter_app("recheck-keep");
    // Surface a continuity finding into the overlay + triage the normal way.
    app.apply_continuity(Ok(one_finding("the locket changes color")));
    assert!(matches!(app.mode, Mode::Findings(_)));
    assert_eq!(app.triage.len(), 1);
    let finding = continuity_ui_finding("One", "the locket changes color");

    // The scoped re-run re-produces the same finding: the fix didn't hold. Parse
    // the mock response through the real re-check parser, so the fresh finding's
    // content key exactly matches the one under re-check.
    let fresh = app.recheck_findings_from(
        &finding,
        &continuity_json("One", "the locket changes color"),
    );
    assert_eq!(
        fresh,
        vec![finding.clone()],
        "parser reproduces the finding"
    );
    app.pending_recheck = Some(finding.clone());
    app.apply_recheck(Ok(fresh));

    assert!(app.triage.contains(&finding), "still-flagged finding stays");
    assert!(
        matches!(app.mode, Mode::Findings(f) if f.items.contains(&finding)),
        "it remains in the overlay"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Still flagged"),
        "{:?}",
        app.message
    );
}

#[test]
fn recheck_that_clears_drops_the_finding() {
    let (mut app, _dir) = two_chapter_app("recheck-clear");
    app.apply_continuity(Ok(one_finding("the locket changes color")));
    assert_eq!(app.triage.len(), 1);
    let finding = continuity_ui_finding("One", "the locket changes color");

    // The scoped re-run produces no findings: the fix resolved it.
    app.pending_recheck = Some(finding.clone());
    app.apply_recheck(Ok(Vec::new()));

    assert!(
        !app.triage.contains(&finding),
        "a cleared finding leaves triage"
    );
    assert!(
        matches!(app.mode, Mode::Insert),
        "the last overlay item cleared closes the overlay"
    );
    assert!(
        app.message.as_deref().unwrap_or("").contains("Cleared"),
        "{:?}",
        app.message
    );
}

#[test]
fn recheck_accumulates_new_findings_silently() {
    let (mut app, _dir) = two_chapter_app("recheck-new");
    app.apply_continuity(Ok(one_finding("the locket changes color")));
    let rechecked = continuity_ui_finding("One", "the locket changes color");
    // The scoped re-run clears the original but surfaces a *different* one in the
    // same chapter. The new one enters triage but doesn't re-open a fresh sweep.
    let fresh = vec![continuity_ui_finding("One", "the candle is lit then unlit")];
    app.pending_recheck = Some(rechecked.clone());
    app.apply_recheck(Ok(fresh.clone()));

    assert!(
        !app.triage.contains(&rechecked),
        "the verified finding cleared"
    );
    assert!(
        app.triage.contains(&fresh[0]),
        "the newly-surfaced finding accumulated into triage"
    );
}

#[test]
fn recheck_is_a_noop_for_a_finding_without_a_pass() {
    let (mut app, _dir) = two_chapter_app("recheck-noop");
    // A finding with no originating action (a Q&A citation, or one restored from
    // disk) can't be re-checked — a status, no request in flight.
    app.mode = Mode::Findings(Box::new(Findings {
        items: vec![crate::mode::Finding {
            summary: "cited: One".into(),
            chapters: vec!["One".into()],
            book: None,
            action: None,
        }],
        selected: 0,
    }));
    app.recheck_finding(0);
    assert!(!app.ai_task.is_busy(), "nothing is sent");
    assert!(app.pending_recheck.is_none());
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("Nothing to re-check"),
        "{:?}",
        app.message
    );
}
