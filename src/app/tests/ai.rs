//! Tests for the AI features: ask the manuscript, trackers (character /
//! timeline / setting), scaffolding, naming, style, submission, cost & triage.

use super::*;
use crate::ai;
use crate::ai::naming::{self, Naming};
use crate::ai::scaffold::{self, Scaffold};
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
            Ok(text) => Ok(ai::Response { text: text.clone(), usage: Default::default() }),
            Err(()) => Err(ai::AiError::Api("boom".into())),
        }
    }
}

#[test]
fn ask_with_ai_off_explains_how_to_enable() {
    let (mut app, dir) = two_chapter_app("ai-off");
    app.ai = ai::Settings::default(); // off, deterministic regardless of env/config
    app.ask_ai("who is the protagonist?");
    assert!(!app.ai_task.is_busy(), "nothing should be sent when AI is off");
    assert!(app.overlay.is_none());
    assert!(app.message.as_deref().unwrap_or("").contains("off"), "{:?}", app.message);
    fs::remove_dir_all(&dir).ok();
}

#[test]
#[cfg(not(feature = "local"))]
fn offline_mode_selects_the_local_backend() {
    let (mut app, dir) = two_chapter_app("ai-offline");
    app.ai = ai::Settings { enabled: true, offline: true, ..ai::Settings::default() };
    // Offline forces the local backend, even with a key present.
    app.ai.api_key = Some("k".into());
    assert_eq!(app.ai.backend(), ai::BackendKind::Local);
    app.ask_ai("anything?");
    // This build has no `local` feature, so it can't run — nothing is sent and the
    // message explains how to enable it (rather than silently using Claude).
    assert!(!app.ai_task.is_busy());
    assert!(app.message.as_deref().unwrap_or("").contains("local"), "{:?}", app.message);
    fs::remove_dir_all(&dir).ok();
}


#[test]
fn manuscript_text_includes_every_chapter() {
    let (app, dir) = two_chapter_app("ai-manuscript");
    let m = app.manuscript_text();
    assert!(m.contains("# One") && m.contains("first chapter"), "{m}");
    assert!(m.contains("# Two") && m.contains("second chapter"), "{m}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn qa_answer_opens_the_overlay() {
    let (mut app, dir) = two_chapter_app("ai-answer");
    // Drive the Q&A core with a mock backend (no network): build the request from
    // the manuscript, complete it, and apply the result like the async path does.
    let backend = MockBackend { result: Ok("Chapter One is about a locket.".into()) };
    let req = ai::build_qa_request(app.manuscript_text(), "what is chapter one about?", &app.ai.model);
    app.apply_qa_result(backend.complete(&req));

    let panel = app.overlay_lines().expect("answer overlay should open");
    let text: String = panel
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.to_string()))
        .collect();
    assert!(text.contains("about a locket"), "{text}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn qa_error_sets_a_message_not_a_panel() {
    let (mut app, dir) = two_chapter_app("ai-err");
    let backend = MockBackend { result: Err(()) };
    let req = ai::build_qa_request(app.manuscript_text(), "q", &app.ai.model);
    app.apply_qa_result(backend.complete(&req));
    assert!(app.overlay.is_none());
    assert!(app.message.as_deref().unwrap_or("").contains("boom"), "{:?}", app.message);
    fs::remove_dir_all(&dir).ok();
}

// --- AI: character tracking + story bible -----------------------------------

#[test]
fn enrich_mentions_counts_names_and_last_chapter() {
    let (app, dir) = two_chapter_app("char-mentions"); // "first chapter" / "second chapter"
    let mut c = Character { name: "chapter".into(), ..Default::default() };
    app.enrich_mentions(&mut c);
    assert_eq!(c.mentions, 2); // once in each chapter body
    assert_eq!(c.last_chapter.as_deref(), Some("Two"));
    fs::remove_dir_all(&dir).ok();
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
    assert!(!matches!(app.mode, Mode::BibleReview(_)), "no conflicts → no review");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn failed_bible_write_is_reported_not_counted_as_success() {
    let (mut app, dir) = two_chapter_app("char-write-fail");
    // Make `bible` a regular file so the `bible/characters` dir can't be created
    // — every profile write then fails.
    fs::write(dir.join("bible"), "x").unwrap();

    app.apply_characters(Ok(vec![Character { name: "Mara".into(), ..Default::default() }]));

    assert!(
        app.message.as_deref().unwrap_or("").contains("Couldn't write"),
        "a failed write is surfaced: {:?}",
        app.message
    );
    assert!(app.overlay.is_none(), "no success summary when the write failed");
    assert!(!matches!(app.mode, Mode::BibleReview(_)), "a write failure isn't a conflict");
    fs::remove_dir_all(&dir).ok();
}

/// Set up a project with a clean, then hand-edited, Mara profile; a second pass
/// with new content must not overwrite it.
fn conflict_app(name: &str) -> (App, PathBuf, PathBuf) {
    let (mut app, dir) = two_chapter_app(name);
    app.apply_characters(Ok(vec![Character {
        name: "Mara".into(),
        physical: vec!["green eyes".into()],
        ..Default::default()
    }]));
    let file = dir.join("bible/characters/mara.md");
    let edited = fs::read_to_string(&file).unwrap().replace("green eyes", "blue eyes");
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
    let (app, dir, file) = conflict_app("char-conflict");
    assert!(matches!(app.mode, Mode::BibleReview(_)), "should queue a review");
    assert!(
        fs::read_to_string(&file).unwrap().contains("blue eyes"),
        "the hand edit must survive until resolved"
    );
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn review_overwrite_writes_the_proposed_version() {
    let (mut app, dir, file) = conflict_app("char-resolve-o");
    app.on_key(key(KeyCode::Char('o')));
    assert!(matches!(app.mode, Mode::Insert), "single conflict resolved → done");
    assert!(fs::read_to_string(&file).unwrap().contains("hazel eyes"), "proposed written");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn review_keep_preserves_the_authors_version() {
    let (mut app, dir, file) = conflict_app("char-resolve-k");
    app.on_key(key(KeyCode::Char('k')));
    assert!(matches!(app.mode, Mode::Insert));
    assert!(fs::read_to_string(&file).unwrap().contains("blue eyes"), "kept the hand edit");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn review_edit_then_save_writes_the_edited_version() {
    let (mut app, dir, file) = conflict_app("char-resolve-e");
    app.on_key(key(KeyCode::Char('e'))); // load proposed into the scratch editor
    assert!(matches!(&app.mode, Mode::BibleReview(r) if r.editing.is_some()));
    app.edit_buffer_mut().insert_char('!'); // tweak the proposed version
    app.on_key(ctrl('s')); // save
    assert!(matches!(app.mode, Mode::Insert));
    let saved = fs::read_to_string(&file).unwrap();
    assert!(saved.contains("hazel eyes") && saved.contains('!'), "edited proposal saved");
    fs::remove_dir_all(&dir).ok();
}

// --- AI: timeline & continuity ---------------------------------------------

fn cont_with(findings: Vec<continuity::Finding>) -> Continuity {
    Continuity {
        events: vec![continuity::Event {
            description: "alpha event".into(),
            chapter: "One".into(),
            story_order: 1,
        }],
        findings,
    }
}

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
    assert!(matches!(app.mode, Mode::Findings(_)), "findings overlay should open");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn continuity_with_no_findings_summarizes() {
    let (mut app, dir) = two_chapter_app("cont-clean");
    app.apply_continuity(Ok(cont_with(vec![])));
    assert!(dir.join("bible/timeline.md").exists());
    assert!(app.overlay.is_some(), "summary overlay");
    assert!(!matches!(app.mode, Mode::Findings(_)));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn findings_dismiss_removes_and_closes_when_empty() {
    let (mut app, dir) = two_chapter_app("cont-dismiss");
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "age".into(),
        detail: "Mara is 30 then 28".into(),
        chapters: vec!["One".into()],
    }])));
    assert!(matches!(app.mode, Mode::Findings(_)));
    app.on_key(key(KeyCode::Char('d'))); // dismiss the only finding
    assert!(matches!(app.mode, Mode::Insert), "empty list closes");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn findings_jump_activates_the_chapter() {
    let (mut app, dir) = two_chapter_app("cont-jump");
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "location".into(),
        detail: "two places at once".into(),
        chapters: vec!["Two".into()],
    }])));
    app.on_key(key(KeyCode::Enter)); // jump to the finding's chapter
    assert!(matches!(app.mode, Mode::Insert));
    assert_eq!(app.project.active_index(), 1, "jumped to chapter Two");
    fs::remove_dir_all(&dir).ok();
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
    let edited = fs::read_to_string(&tl).unwrap().replace("alpha event", "TAMPERED");
    fs::write(&tl, edited).unwrap();

    // Second pass with findings → timeline conflict first.
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "object".into(),
        detail: "second".into(),
        chapters: vec!["One".into()],
    }])));
    assert!(matches!(app.mode, Mode::BibleReview(_)), "timeline conflict reviewed first");

    app.on_key(key(KeyCode::Char('k'))); // keep mine → drains queue → findings open
    assert!(matches!(app.mode, Mode::Findings(_)), "findings follow the review");
    fs::remove_dir_all(&dir).ok();
}

// --- AI: setting & worldbuilding -------------------------------------------

#[test]
fn setting_writes_bible_and_opens_rule_findings() {
    let (mut app, dir) = two_chapter_app("setting-write");
    let s = Setting {
        categories: vec![setting::Category {
            name: "Magic system".into(),
            entries: vec![setting::Entry { name: "Casting".into(), detail: "needs a focus".into() }],
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
    assert!(matches!(app.mode, Mode::Findings(_)), "rule findings overlay opens");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn setting_with_no_violations_summarizes() {
    let (mut app, dir) = two_chapter_app("setting-clean");
    let s = Setting {
        categories: vec![setting::Category { name: "Geography".into(), entries: vec![] }],
        gazetteer: vec![],
        findings: vec![],
    };
    app.apply_setting(Ok(s));
    assert!(dir.join("bible/setting.md").exists());
    assert!(app.overlay.is_some(), "summary overlay");
    assert!(!matches!(app.mode, Mode::Findings(_)));
    fs::remove_dir_all(&dir).ok();
}

// --- AI: scaffolding (outline + gaps + xxx insertion) ----------------------

#[test]
fn scaffold_writes_outline_and_opens_gap_findings() {
    let (mut app, dir) = two_chapter_app("scaffold");
    let s = Scaffold {
        outline: vec![scaffold::OutlineItem { chapter: "One".into(), beats: vec!["a beat".into()] }],
        gaps: vec![scaffold::Gap { detail: "no clear goal".into(), chapter: "Two".into() }],
    };
    app.apply_scaffold(Ok(s));
    let file = fs::read_to_string(dir.join("bible/outline.md")).expect("outline written");
    assert!(file.contains("## One") && file.contains("a beat"));
    assert!(file.contains("## Gaps"));
    assert!(matches!(app.mode, Mode::Findings(_)), "gap findings overlay opens");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn marking_a_finding_inserts_an_xxx_todo_in_a_nonactive_chapter() {
    let (mut app, dir) = two_chapter_app("mark-todo"); // active is "One"
    app.apply_scaffold(Ok(Scaffold {
        outline: vec![],
        gaps: vec![scaffold::Gap { detail: "needs a hook".into(), chapter: "Two".into() }],
    }));
    let before = app.count_todo();
    app.on_key(key(KeyCode::Char('i'))); // mark xxx in "Two" (non-active → file)
    assert!(app.count_todo() > before, "the report's xxx count should rise");
    assert!(fs::read_to_string(dir.join("two.md")).unwrap().contains("xxx"), "marker on disk");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn marking_a_finding_in_the_active_chapter_updates_the_buffer() {
    let (mut app, dir) = two_chapter_app("mark-active"); // active is "One"
    app.apply_scaffold(Ok(Scaffold {
        outline: vec![],
        gaps: vec![scaffold::Gap { detail: "weak open".into(), chapter: "One".into() }],
    }));
    app.on_key(key(KeyCode::Char('i')));
    assert!(app.buffer.text().contains("xxx"), "marker shows in the active buffer");
    fs::remove_dir_all(&dir).ok();
}

// --- AI: naming & term consistency -----------------------------------------

#[test]
fn known_entities_reads_gazetteer_and_character_names() {
    let (app, dir) = two_chapter_app("entities");
    fs::create_dir_all(dir.join("bible/characters")).unwrap();
    fs::write(
        dir.join("bible/setting.md"),
        "---\n---\n\n## Gazetteer\n- Aldermoor\n- lightsteel\n\n## Other\n- nope\n",
    )
    .unwrap();
    fs::write(dir.join("bible/characters/mara.md"), "---\nname: Mara Vance\n---\n").unwrap();

    let e = app.known_entities();
    assert!(e.contains(&"Aldermoor".to_string()));
    assert!(e.contains(&"lightsteel".to_string()));
    assert!(e.contains(&"Mara Vance".to_string()));
    assert!(!e.contains(&"nope".to_string()), "only the Gazetteer section is read");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn naming_findings_open_the_overlay() {
    let (mut app, dir) = two_chapter_app("naming");
    app.apply_naming(Ok(Naming {
        findings: vec![naming::NameFinding {
            name: "Mara".into(),
            variants: vec!["Marah".into()],
            chapters: vec!["Two".into()],
        }],
    }));
    assert!(matches!(app.mode, Mode::Findings(_)));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn naming_with_no_variants_summarizes() {
    let (mut app, dir) = two_chapter_app("naming-clean");
    app.apply_naming(Ok(Naming { findings: vec![] }));
    assert!(app.overlay.is_some());
    assert!(!matches!(app.mode, Mode::Findings(_)));
    fs::remove_dir_all(&dir).ok();
}

// --- AI: command palette ----------------------------------------------------

#[test]
fn palette_opens_with_the_ai_actions() {
    let (mut app, dir) = two_chapter_app("palette");
    app.open_ai_palette();
    match &app.mode {
        Mode::Palette(p) => assert_eq!(p.items.len(), 9, "all AI actions listed"),
        _ => panic!("expected the palette to open"),
    }
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn palette_down_moves_the_selection() {
    let (mut app, dir) = two_chapter_app("palette-nav");
    app.open_ai_palette();
    app.on_key(key(KeyCode::Down));
    match &app.mode {
        Mode::Palette(p) => assert_eq!(p.selected, 1),
        _ => panic!("expected the palette"),
    }
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn palette_enter_on_ask_opens_the_question_prompt() {
    let (mut app, dir) = two_chapter_app("palette-ask");
    app.open_ai_palette(); // "Ask the manuscript" is the first item
    app.on_key(key(KeyCode::Enter));
    assert!(matches!(app.mode, Mode::Prompt(_)), "Ask opens the question prompt");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn palette_esc_closes_to_insert() {
    let (mut app, dir) = two_chapter_app("palette-esc");
    app.open_ai_palette();
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.mode, Mode::Insert));
    fs::remove_dir_all(&dir).ok();
}

// --- AI: prose & style ------------------------------------------------------

#[test]
fn style_findings_open_the_overlay() {
    let (mut app, dir) = two_chapter_app("style");
    app.apply_style(Ok(ai::style::Style {
        findings: vec![ai::style::StyleFinding {
            kind: "POV".into(),
            detail: "head-hop in the duel scene".into(),
            chapters: vec!["Two".into()],
        }],
    }));
    assert!(matches!(app.mode, Mode::Findings(_)));
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn style_with_no_findings_summarizes() {
    let (mut app, dir) = two_chapter_app("style-clean");
    app.apply_style(Ok(ai::style::Style { findings: vec![] }));
    assert!(app.overlay.is_some());
    assert!(!matches!(app.mode, Mode::Findings(_)));
    fs::remove_dir_all(&dir).ok();
}

// --- AI: submission material ------------------------------------------------

#[test]
fn submission_writes_three_artifacts_and_summarizes() {
    let (mut app, dir) = two_chapter_app("submission");
    app.apply_submission(Ok(ai::submission::Submission {
        synopsis: "Mara finds a locket and confronts her brother.".into(),
        blurb: "A locket. A secret. A reckoning.".into(),
        query: "Dear Agent, RED STATE EARTH is …".into(),
    }));
    assert!(fs::read_to_string(dir.join("submission/synopsis.md")).unwrap().contains("confronts her brother"));
    assert!(fs::read_to_string(dir.join("submission/blurb.md")).unwrap().contains("reckoning"));
    assert!(fs::read_to_string(dir.join("submission/query.md")).unwrap().contains("Dear Agent"));
    assert!(app.overlay.is_some(), "summary overlay");
    assert!(!matches!(app.mode, Mode::BibleReview(_)), "no conflicts on a fresh write");
    fs::remove_dir_all(&dir).ok();
}

// --- Local analysis report --------------------------------------------------

#[test]
fn ctrl_t_opens_the_analysis_report_with_a_row_per_chapter() {
    let (mut app, dir) = two_chapter_app("analysis-open");
    assert!(app.overlay.is_none());
    app.on_key(ctrl('t'));
    let lines = app.overlay_lines().expect("analysis report opens");
    let text: String = lines
        .iter()
        .flat_map(|l| l.spans.iter().map(|s| s.content.as_ref()))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(text.contains("Readability"), "{text}");
    assert!(text.contains("Sentence length"), "sentence section: {text}");
    assert!(text.contains("One") && text.contains("Two"), "both chapters listed: {text}");
    assert!(text.contains("Whole manuscript"), "overall row: {text}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_non_scroll_key_closes_the_analysis_report() {
    let (mut app, dir) = two_chapter_app("analysis-close");
    app.viewport_height = 5;
    app.on_key(ctrl('t'));
    app.on_key(key(KeyCode::Down)); // scroll a bit first
    assert!(app.overlay_scroll() > 0);
    app.on_key(key(KeyCode::Char('x'))); // not a scroll key → close
    assert!(app.overlay.is_none(), "a non-scroll key dismisses the report");
    assert_eq!(app.overlay_scroll(), 0, "scroll resets on close");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_analysis_report_scrolls_and_clamps() {
    let (mut app, dir) = two_chapter_app("analysis-scroll");
    app.viewport_height = 5; // tiny screen → 3-row interior
    app.on_key(ctrl('t'));
    let len = app.overlay_lines().unwrap().len() as u16;
    let max = len - 3; // content rows beyond the interior
    assert!(max > 0, "report is taller than the viewport");

    app.on_key(key(KeyCode::Down));
    assert_eq!(app.overlay_scroll(), 1, "Down scrolls one row");
    app.on_key(key(KeyCode::End));
    assert_eq!(app.overlay_scroll(), max, "End jumps to the bottom");
    app.on_key(key(KeyCode::Down));
    assert_eq!(app.overlay_scroll(), max, "can't scroll past the end");
    assert!(app.overlay.is_some(), "scroll keys don't close the overlay");
    app.on_key(key(KeyCode::Home));
    assert_eq!(app.overlay_scroll(), 0, "Home jumps back to the top");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn submission_never_touches_the_manuscript() {
    let (mut app, dir) = two_chapter_app("submission-safe");
    let before = fs::read_to_string(dir.join("one.md")).unwrap();
    app.apply_submission(Ok(ai::submission::Submission {
        synopsis: "S".into(),
        blurb: "B".into(),
        query: "Q".into(),
    }));
    assert_eq!(fs::read_to_string(dir.join("one.md")).unwrap(), before, "chapter prose untouched");
    fs::remove_dir_all(&dir).ok();
}

// --- AI: cost confirm & revision triage -------------------------------------

fn findings_len(app: &App) -> usize {
    match &app.mode {
        Mode::Findings(f) => f.items.len(),
        _ => panic!("expected the findings overlay"),
    }
}

#[test]
fn an_expensive_pass_asks_for_confirmation_first() {
    let (mut app, dir) = two_chapter_app("cost-confirm");
    app.ai.cost_threshold = 0.0; // any non-empty manuscript exceeds it
    app.start_ai_action(crate::mode::AiAction::Continuity);
    match &app.mode {
        Mode::Prompt(s) => {
            assert!(matches!(s.pending, Pending::ConfirmAi(_)), "cost confirm opens");
            assert!(s.label.contains("run?"), "{}", s.label);
        }
        _ => panic!("expected the cost confirm prompt"),
    }
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_cheap_pass_runs_without_confirmation() {
    let (mut app, dir) = two_chapter_app("cost-cheap");
    app.ai.cost_threshold = 1_000.0; // nothing this small could exceed it
    app.start_ai_action(crate::mode::AiAction::Style);
    // It dispatched straight through (no key configured → a status message),
    // never stopping at a confirm prompt.
    assert!(!matches!(app.mode, Mode::Prompt(_)), "no confirm for a cheap pass");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn confirming_an_expensive_pass_dispatches_it() {
    let (mut app, dir) = two_chapter_app("cost-yes");
    app.ai.cost_threshold = 0.0;
    app.start_ai_action(crate::mode::AiAction::Continuity);
    assert!(matches!(app.mode, Mode::Prompt(_)));
    app.on_key(key(KeyCode::Char('y'))); // confirm → dispatch (no key → message)
    assert!(!matches!(app.mode, Mode::Prompt(_)), "confirm closes the prompt");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn declining_an_expensive_pass_cancels_it() {
    let (mut app, dir) = two_chapter_app("cost-no");
    app.ai.cost_threshold = 0.0;
    app.start_ai_action(crate::mode::AiAction::Continuity);
    app.on_key(key(KeyCode::Char('n')));
    assert!(matches!(app.mode, Mode::Insert), "declining returns to editing");
    assert!(!app.ai_task.is_busy(), "nothing was sent");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn triage_accumulates_findings_across_passes_deduped() {
    let (mut app, dir) = two_chapter_app("triage-accum");
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "object".into(),
        detail: "the locket changes color".into(),
        chapters: vec!["One".into()],
    }])));
    app.apply_style(Ok(ai::style::Style {
        findings: vec![ai::style::StyleFinding {
            kind: "POV".into(),
            detail: "head-hop".into(),
            chapters: vec!["Two".into()],
        }],
    }));
    // A repeat of the continuity finding must not double-count.
    app.apply_continuity(Ok(cont_with(vec![continuity::Finding {
        kind: "object".into(),
        detail: "the locket changes color".into(),
        chapters: vec!["One".into()],
    }])));
    app.open_triage();
    assert_eq!(findings_len(&app), 2, "two distinct findings, deduped");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn triage_with_no_findings_shows_a_message() {
    let (mut app, dir) = two_chapter_app("triage-empty");
    app.open_triage();
    assert!(!matches!(app.mode, Mode::Findings(_)));
    assert!(app.message.as_deref().unwrap_or("").contains("No findings"), "{:?}", app.message);
    fs::remove_dir_all(&dir).ok();
}

// --- Incremental pass scoping (#90) ----------------------------------------

#[test]
fn an_up_to_date_bible_pass_makes_no_request() {
    let (mut app, dir) = two_chapter_app("incr-uptodate");
    // Record a baseline matching the current manuscript → nothing changed.
    ai::incremental::State::record(&dir, "characters", "2026-06-21", app.chapter_digests()).unwrap();
    app.analyze_characters();
    assert!(!app.ai_busy(), "an up-to-date bible spawns no request");
    assert!(app.message.as_deref().unwrap_or("").contains("current"), "{:?}", app.message);
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_edited_chapter_scopes_the_pass_to_just_that_chapter() {
    let (mut app, dir) = two_chapter_app("incr-scope");
    ai::incremental::State::record(&dir, "characters", "2026-06-21", app.chapter_digests()).unwrap();
    // Edit chapter Two and save; only it is now part of the delta.
    app.on_key(ctrl('n')); // active → Two
    type_str(&mut app, "NEWWORD");
    app.on_key(ctrl('s'));

    let state = ai::incremental::State::load(&dir);
    let scope = ai::incremental::decide(state.built_from("characters"), &app.chapter_digests(), 5);
    assert_eq!(scope, ai::incremental::Scope::Incremental(vec!["two.md".into()]));
    // The incremental context is only the changed chapter, not the whole book.
    let changed = app.chapters_text(&["two.md".to_string()]);
    assert!(changed.contains("NEWWORD") && !changed.contains("first chapter"), "{changed}");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn cost_context_is_the_whole_manuscript_first_then_zero_when_current() {
    let (app, dir) = two_chapter_app("incr-cost");
    let action = crate::mode::AiAction::Characters;
    assert_eq!(app.pass_context_len(action), app.manuscript_text().len(), "first run estimates the full manuscript");
    ai::incremental::State::record(&dir, "characters", "2026-06-21", app.chapter_digests()).unwrap();
    assert_eq!(app.pass_context_len(action), 0, "an up-to-date bible costs nothing to confirm");
    fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_clean_bible_pass_records_what_it_built_from() {
    let (mut app, dir) = two_chapter_app("incr-record");
    let built = app.chapter_digests();
    // Stand in for what analyze_characters sets just before spawning the request.
    app.pending_pass = Some(("characters".to_string(), built.clone()));
    app.apply_characters(Ok(vec![Character { name: "Mara".into(), ..Default::default() }]));
    let state = ai::incremental::State::load(&dir);
    assert_eq!(state.built_from("characters"), Some(&built), "the pass recorded its manuscript state");
    assert!(app.pending_pass.is_none(), "the snapshot was consumed");
    fs::remove_dir_all(&dir).ok();
}
