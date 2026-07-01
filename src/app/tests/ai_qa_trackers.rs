//! AI Q&A plus the character / continuity / setting trackers, and the mock backend harness.

use super::*;
use crate::mode::AiAction;
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

// --- Plot-thread / arc tracker (#606) --------------------------------------

/// A plot result with the given dangling-thread findings and one always-present
/// thread, so the tracker file always has content to write.
fn plot_with(findings: Vec<plot::ThreadFinding>) -> Plot {
    Plot {
        threads: vec![plot::Thread {
            name: "The stolen locket".into(),
            status: "open".into(),
            setup: "Mara pockets it in ch. 1".into(),
            payoff: String::new(),
            last_seen: "One".into(),
            notes: String::new(),
        }],
        findings,
    }
}

#[test]
fn plot_writes_tracker_and_opens_dangling_findings() {
    let (mut app, dir) = two_chapter_app("plot-write");
    app.apply_plot(Ok(plot_with(vec![plot::ThreadFinding {
        thread: "The stolen locket".into(),
        issue: "introduced in ch. 1, never resolved".into(),
        chapters: vec!["One".into()],
    }])));
    let file = fs::read_to_string(dir.join("bible/plot.md")).expect("plot tracker written");
    assert!(file.contains("title: Plot threads"));
    assert!(file.contains("### The stolen locket — open"));
    assert!(file.contains("## Dangling threads"));
    assert!(
        matches!(app.mode, Mode::Findings(_)),
        "dangling-thread findings overlay opens"
    );
}

#[test]
fn plot_with_no_dangling_threads_summarizes() {
    let (mut app, dir) = two_chapter_app("plot-clean");
    app.apply_plot(Ok(plot_with(vec![])));
    assert!(dir.join("bible/plot.md").exists());
    assert!(
        app.overlay.is_some(),
        "summary overlay when nothing dangles"
    );
    assert!(!matches!(app.mode, Mode::Findings(_)));
}

#[test]
fn plot_findings_flow_through_triage_deduped() {
    let (mut app, _dir) = two_chapter_app("plot-triage");
    let finding = plot::ThreadFinding {
        thread: "Who burned the archive".into(),
        issue: "opened in ch. 3, never answered".into(),
        chapters: vec!["Two".into()],
    };
    app.apply_plot(Ok(plot_with(vec![finding.clone()])));
    // A re-run with the same dangling finding must not double-count in triage.
    app.apply_plot(Ok(plot_with(vec![finding])));
    app.open_triage();
    match &app.mode {
        Mode::Findings(f) => assert_eq!(f.items.len(), 1, "one dangling finding, deduped"),
        _ => panic!("expected the triage findings overlay"),
    }
}

#[test]
fn plot_pass_scopes_incrementally_and_round_trips_its_bible_key() {
    let (mut app, dir) = two_chapter_app("plot-incr");
    // The tracker's bible key is the single source shared by the state file, the
    // incremental cache, and the series rollup (#563).
    let key = AiAction::Plot.bible_key().expect("plot is a bible pass");
    assert_eq!(key, "plot");
    // Seed a tracker file + recorded state so the up-to-date / incremental paths
    // are reachable, then edit one chapter and confirm only it is in the delta.
    fs::create_dir_all(dir.join("bible")).unwrap();
    fs::write(
        dir.join("bible/plot.md"),
        "---\ntitle: Plot threads\n---\n\n<!-- generated:start -->\nthreads\n<!-- generated:end -->\n",
    )
    .unwrap();
    ai::incremental::State::record(&dir, key, "2026-06-21", app.chapter_digests()).unwrap();
    // Nothing changed → the pass is up to date and sends nothing.
    assert_eq!(app.pass_context_len(AiAction::Plot, false), 0, "up to date");

    app.on_key(ctrl('n')); // active → Two
    type_str(&mut app, "NEWWORD");
    app.on_key(ctrl('s'));
    let state = ai::incremental::State::load(&dir);
    let scope = ai::incremental::decide(state.built_from(key), &app.chapter_digests(), 5);
    assert_eq!(
        scope,
        ai::incremental::Scope::Incremental(vec!["two.md".into()]),
        "only the edited chapter is in the plot delta"
    );
}
