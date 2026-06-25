//! Tests for the AI features: ask the manuscript, trackers (character /
//! timeline / setting), scaffolding, naming, style, submission, cost & triage.

use super::*;
use crate::ai;
use crate::ai::naming::{self, Naming};
use crate::ai::router::Policy;
use crate::ai::scaffold::{self, Scaffold};
use crate::app::palette::AiStart;
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
            Ok(text) => Ok(ai::Response { text: text.clone(), usage: Default::default() }),
            Err(()) => Err(ai::AiError::Network { status: None, message: "boom".into() }),
        }
    }
}

#[test]
fn ask_with_ai_off_explains_how_to_enable() {
    let (mut app, _dir) = two_chapter_app("ai-off");
    app.ai = ai::Settings::default(); // off, deterministic regardless of env/config
    app.ask_ai("who is the protagonist?");
    assert!(!app.ai_task.is_busy(), "nothing should be sent when AI is off");
    assert!(app.overlay.is_none());
    assert!(app.message.as_deref().unwrap_or("").contains("off"), "{:?}", app.message);
}

#[test]
#[cfg(not(feature = "local"))]
fn offline_mode_selects_the_local_backend() {
    let (mut app, _dir) = two_chapter_app("ai-offline");
    app.ai = ai::Settings { enabled: true, offline: true, ..ai::Settings::default() };
    // Offline forces the local backend, even with a key present.
    app.ai.api_key = Some("k".into());
    assert_eq!(app.ai.backend(), ai::BackendKind::Local);
    app.ask_ai("anything?");
    // This build has no `local` feature, so it can't run — nothing is sent and the
    // message explains how to enable it (rather than silently using Claude).
    assert!(!app.ai_task.is_busy());
    assert!(app.message.as_deref().unwrap_or("").contains("local"), "{:?}", app.message);
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
    let backend = MockBackend { result: Ok("Chapter One is about a locket.".into()) };
    let req = ai::build_qa_request(app.manuscript_text(), "what is chapter one about?", &app.ai.model);
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
    assert!(app.message.as_deref().unwrap_or("").contains("boom"), "{:?}", app.message);
}

// --- AI: character tracking + story bible -----------------------------------

#[test]
fn enrich_mentions_counts_names_and_last_chapter() {
    let (app, _dir) = two_chapter_app("char-mentions"); // "first chapter" / "second chapter"
    let mut c = Character { name: "chapter".into(), ..Default::default() };
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
    assert!(!matches!(app.mode, Mode::BibleReview(_)), "no conflicts → no review");
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
    let (app, _dir, file) = conflict_app("char-conflict");
    assert!(matches!(app.mode, Mode::BibleReview(_)), "should queue a review");
    assert!(
        fs::read_to_string(&file).unwrap().contains("blue eyes"),
        "the hand edit must survive until resolved"
    );
}

#[test]
fn review_overwrite_writes_the_proposed_version() {
    let (mut app, _dir, file) = conflict_app("char-resolve-o");
    app.on_key(key(KeyCode::Char('o')));
    assert!(matches!(app.mode, Mode::Insert), "single conflict resolved → done");
    assert!(fs::read_to_string(&file).unwrap().contains("hazel eyes"), "proposed written");
}

#[test]
fn review_keep_preserves_the_authors_version() {
    let (mut app, _dir, file) = conflict_app("char-resolve-k");
    app.on_key(key(KeyCode::Char('k')));
    assert!(matches!(app.mode, Mode::Insert));
    assert!(fs::read_to_string(&file).unwrap().contains("blue eyes"), "kept the hand edit");
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
    assert!(saved.contains("hazel eyes") && saved.contains('!'), "edited proposal saved");
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
}

#[test]
fn marking_a_finding_in_the_active_chapter_updates_the_buffer() {
    let (mut app, _dir) = two_chapter_app("mark-active"); // active is "One"
    app.apply_scaffold(Ok(Scaffold {
        outline: vec![],
        gaps: vec![scaffold::Gap { detail: "weak open".into(), chapter: "One".into() }],
    }));
    app.on_key(key(KeyCode::Char('i')));
    assert!(app.buffer.text().contains("xxx"), "marker shows in the active buffer");
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
}

#[test]
fn naming_findings_open_the_overlay() {
    let (mut app, _dir) = two_chapter_app("naming");
    app.apply_naming(Ok(Naming {
        findings: vec![naming::NameFinding {
            name: "Mara".into(),
            variants: vec!["Marah".into()],
            chapters: vec!["Two".into()],
        }],
    }));
    assert!(matches!(app.mode, Mode::Findings(_)));
}

#[test]
fn naming_with_no_variants_summarizes() {
    let (mut app, _dir) = two_chapter_app("naming-clean");
    app.apply_naming(Ok(Naming { findings: vec![] }));
    assert!(app.overlay.is_some());
    assert!(!matches!(app.mode, Mode::Findings(_)));
}

// --- AI: command palette ----------------------------------------------------

#[test]
fn palette_opens_with_the_ai_actions() {
    let (mut app, _dir) = two_chapter_app("palette");
    app.open_ai_palette();
    match &app.mode {
        Mode::Palette(p) => assert_eq!(p.items.len(), 20, "all AI actions listed"),
        _ => panic!("expected the palette to open"),
    }
}

#[test]
fn word_search_prompt_is_prefilled_with_the_word_under_the_cursor() {
    let (mut app, _dir) = two_chapter_app("thesaurus-prefill");
    app.start_thesaurus(); // cursor is at the start of "first chapter"
    match &app.mode {
        Mode::Prompt(s) => {
            assert!(matches!(s.pending, Pending::Thesaurus));
            assert_eq!(s.input, "first", "prefilled with the word under the cursor");
        }
        _ => panic!("word search should open a prompt"),
    }
}

#[test]
fn applying_word_search_results_opens_the_overlay() {
    let (mut app, _dir) = two_chapter_app("thesaurus-apply");
    let words = crate::ai::thesaurus::parse(
        r#"{"words":[{"text":"melancholy","definition":"a pensive sadness","syllables":4}]}"#,
    )
    .unwrap();
    app.apply_thesaurus(Ok(words));
    assert!(app.overlay.is_some(), "candidates are shown in the AI overlay");
}

#[test]
fn editorial_read_flags_land_in_the_findings_list_tagged_by_lens() {
    let (mut app, _dir) = two_chapter_app("editorial");
    let notes = crate::ai::editorial::parse(
        r#"{"notes":[
            {"focus":"structural","chapters":["One"],"concern":"the opening lacks stakes"},
            {"focus":"line","chapters":["Two"],"location":"could of","concern":"usage (could of -> could have)"}
        ]}"#,
    )
    .unwrap();
    app.apply_editorial(Ok(notes));

    match &app.mode {
        Mode::Findings(f) => {
            let summaries: Vec<&str> = f.items.iter().map(|i| i.summary.as_str()).collect();
            assert!(summaries.iter().any(|s| s.starts_with("structural:") && s.contains("stakes")), "{summaries:?}");
            assert!(
                summaries.iter().any(|s| s.starts_with("line:") && s.contains('\u{201c}') && s.contains("could of")),
                "line flag pinpoints a quoted span: {summaries:?}"
            );
        }
        _ => panic!("editorial flags should open the findings list"),
    }
}

#[test]
fn palette_down_moves_the_selection() {
    let (mut app, _dir) = two_chapter_app("palette-nav");
    app.open_ai_palette();
    app.on_key(key(KeyCode::Down));
    match &app.mode {
        Mode::Palette(p) => assert_eq!(p.selected, 1),
        _ => panic!("expected the palette"),
    }
}

#[test]
fn palette_enter_on_ask_opens_the_question_prompt() {
    let (mut app, _dir) = two_chapter_app("palette-ask");
    app.open_ai_palette(); // "Ask the manuscript" is the first item
    app.on_key(key(KeyCode::Enter));
    assert!(matches!(app.mode, Mode::Prompt(_)), "Ask opens the question prompt");
}

#[test]
fn palette_esc_closes_to_insert() {
    let (mut app, _dir) = two_chapter_app("palette-esc");
    app.open_ai_palette();
    app.on_key(key(KeyCode::Esc));
    assert!(matches!(app.mode, Mode::Insert));
}

// --- #255: brainstorm (generative, from a premise) --------------------------

#[test]
fn brainstorm_writes_an_off_manuscript_doc() {
    let (mut app, dir) = two_chapter_app("brainstorm");
    let before = fs::read_to_string(dir.join("one.md")).unwrap();
    app.apply_brainstorm(Ok(ai::brainstorm::Brainstorm {
        premises: vec!["a quiet town with a buried secret".into()],
        what_ifs: vec!["what if the keeper knew the victim".into()],
        themes: vec!["isolation".into()],
        conflicts: vec!["truth vs. self-preservation".into()],
    }));
    let doc = fs::read_to_string(dir.join("bible/brainstorm.md")).unwrap();
    assert!(doc.contains("## Premises") && doc.contains("buried secret"));
    assert!(doc.contains("## What if") && doc.contains("## Central conflicts"));
    assert!(doc.contains("Generated by Quill"), "marked AI-generated");
    assert_eq!(fs::read_to_string(dir.join("one.md")).unwrap(), before, "chapter prose untouched");
}

#[test]
fn brainstorm_action_opens_a_premise_prompt() {
    let (mut app, _dir) = two_chapter_app("brainstorm-prompt");
    app.run_ai_action(crate::mode::AiAction::Brainstorm, false);
    assert!(matches!(app.mode, Mode::Prompt(_)), "asks for a premise");
}

#[test]
fn empty_premise_sends_nothing() {
    let (mut app, _dir) = two_chapter_app("brainstorm-empty");
    app.ai = ai::Settings::default();
    app.ai.enabled = true;
    app.ai.api_key = Some("k".into());
    app.brainstorm("   ");
    assert!(!app.ai_task.is_busy(), "an empty premise sends no request");
}

// --- #256: draft characters from a concept ----------------------------------

#[test]
fn draft_characters_action_opens_a_concept_prompt() {
    let (mut app, _dir) = two_chapter_app("draft-chars-prompt");
    app.run_ai_action(crate::mode::AiAction::DraftCharacters, false);
    assert!(matches!(app.mode, Mode::Prompt(_)), "asks for a concept");
}

#[test]
fn empty_concept_sends_nothing() {
    let (mut app, _dir) = two_chapter_app("draft-chars-empty");
    app.ai = ai::Settings::default();
    app.ai.enabled = true;
    app.ai.api_key = Some("k".into());
    app.draft_characters("  ");
    assert!(!app.ai_task.is_busy(), "an empty concept sends no request");
}

// --- #257: draft setting from a concept -------------------------------------

#[test]
fn draft_setting_action_opens_a_concept_prompt() {
    let (mut app, _dir) = two_chapter_app("draft-setting-prompt");
    app.run_ai_action(crate::mode::AiAction::DraftSetting, false);
    assert!(matches!(app.mode, Mode::Prompt(_)), "asks for a concept");
}

#[test]
fn drafted_setting_writes_to_the_bible_off_manuscript() {
    // Reuses apply_setting, so a drafted world lands in bible/setting.md (#77
    // format), marked generated, with no chapter touched.
    let (mut app, dir) = two_chapter_app("draft-setting-write");
    let before = fs::read_to_string(dir.join("one.md")).unwrap();
    app.apply_setting(Ok(ai::setting::Setting {
        categories: vec![ai::setting::Category {
            name: "Geography & locations".into(),
            entries: vec![ai::setting::Entry {
                name: "Saltmere".into(),
                detail: "a fog-bound fishing town".into(),
            }],
        }],
        gazetteer: vec!["Saltmere".into()],
        findings: vec![],
    }));
    let bible = fs::read_to_string(dir.join("bible/setting.md")).unwrap();
    assert!(bible.contains("Geography & locations") && bible.contains("Saltmere"));
    assert!(bible.contains("Generated by Quill"), "marked AI-generated");
    assert_eq!(fs::read_to_string(dir.join("one.md")).unwrap(), before, "chapter prose untouched");
}

#[test]
fn drafted_characters_write_to_the_bible_off_manuscript() {
    // The generative pass reuses apply_characters, so a drafted cast lands in
    // bible/characters/ in the #74 format, marked generated, no chapter touched.
    let (mut app, dir) = two_chapter_app("draft-chars-write");
    let before = fs::read_to_string(dir.join("one.md")).unwrap();
    app.apply_characters(Ok(vec![ai::character::Character {
        name: "Mara Vance".into(),
        goals: vec!["find the locket's owner".into()],
        ..Default::default()
    }]));
    let profile = fs::read_to_string(dir.join("bible/characters/mara-vance.md")).unwrap();
    assert!(profile.contains("name: Mara Vance"));
    assert!(profile.contains("## Goals") && profile.contains("locket"));
    assert!(profile.contains("Generated by Quill"), "marked AI-generated");
    assert_eq!(fs::read_to_string(dir.join("one.md")).unwrap(), before, "chapter prose untouched");
}

// --- AI: prose & style ------------------------------------------------------

#[test]
fn style_findings_open_the_overlay() {
    let (mut app, _dir) = two_chapter_app("style");
    app.apply_style(Ok(ai::style::Style {
        findings: vec![ai::style::StyleFinding {
            kind: "POV".into(),
            detail: "head-hop in the duel scene".into(),
            chapters: vec!["Two".into()],
        }],
    }));
    assert!(matches!(app.mode, Mode::Findings(_)));
}

// --- #183: character voice analysis -----------------------------------------

#[test]
fn voice_findings_open_the_overlay_and_name_the_character() {
    let (mut app, _dir) = two_chapter_app("voice");
    app.apply_voice(Ok(ai::voice::Voice {
        findings: vec![ai::voice::VoiceFinding {
            kind: "dialogue".into(),
            character: "Mara".into(),
            detail: "too formal for her clipped register".into(),
            chapters: vec!["Two".into()],
        }],
    }));
    match &app.mode {
        Mode::Findings(f) => {
            assert!(f.items[0].summary.contains("Mara"), "names the character: {}", f.items[0].summary);
            assert!(f.items[0].summary.contains("dialogue"));
        }
        _ => panic!("expected the findings overlay"),
    }
    // Voice findings feed revision triage like the other passes.
    assert_eq!(app.triage.len(), 1);
}

#[test]
fn voice_pass_needs_the_character_bible() {
    let (mut app, _dir) = two_chapter_app("voice-no-bible");
    app.ai = ai::Settings::default();
    app.ai.enabled = true;
    app.ai.api_key = Some("k".into());
    app.analyze_voice(); // no bible/characters yet
    assert!(!app.ai_task.is_busy(), "nothing sent without voice profiles to ground on");
    assert!(
        app.message.as_deref().unwrap_or("").contains("character pass first"),
        "{:?}",
        app.message
    );
}

#[test]
fn voice_pass_needs_voice_profiles_not_just_a_character_file() {
    let (mut app, dir) = two_chapter_app("voice-no-profiles");
    app.ai = ai::Settings::default();
    app.ai.enabled = true;
    app.ai.api_key = Some("k".into());
    // A character file with no `## Voice` section (e.g. a pre-#183 bible) doesn't
    // satisfy the precondition — the pass can't ground on it.
    let chars = dir.join("bible").join("characters");
    fs::create_dir_all(&chars).unwrap();
    fs::write(chars.join("mara.md"), "name: Mara\n\n## Physical\n- green eyes\n").unwrap();
    app.analyze_voice();
    assert!(!app.ai_task.is_busy());
    assert!(app.message.as_deref().unwrap_or("").contains("character pass first"), "{:?}", app.message);
}

#[test]
fn style_with_no_findings_summarizes() {
    let (mut app, _dir) = two_chapter_app("style-clean");
    app.apply_style(Ok(ai::style::Style { findings: vec![] }));
    assert!(app.overlay.is_some());
    assert!(!matches!(app.mode, Mode::Findings(_)));
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
}

// --- Local analysis report --------------------------------------------------

#[test]
fn ctrl_t_opens_the_analysis_report_with_a_row_per_chapter() {
    let (mut app, _dir) = two_chapter_app("analysis-open");
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
}

#[test]
fn a_non_scroll_key_closes_the_analysis_report() {
    let (mut app, _dir) = two_chapter_app("analysis-close");
    app.viewport_height = 5;
    app.on_key(ctrl('t'));
    app.on_key(key(KeyCode::Down)); // scroll a bit first
    assert!(app.overlay_scroll() > 0);
    app.on_key(key(KeyCode::Char('x'))); // not a scroll key → close
    assert!(app.overlay.is_none(), "a non-scroll key dismisses the report");
    assert_eq!(app.overlay_scroll(), 0, "scroll resets on close");
}

#[test]
fn the_analysis_report_scrolls_and_clamps() {
    let (mut app, _dir) = two_chapter_app("analysis-scroll");
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
}

// --- #236: marketing copy ---------------------------------------------------

#[test]
fn marketing_writes_blurbs_and_taglines_off_manuscript() {
    let (mut app, dir) = two_chapter_app("marketing");
    let before = fs::read_to_string(dir.join("one.md")).unwrap();
    app.apply_marketing(Ok(ai::marketing::Marketing {
        blurbs: vec!["A locket. A reckoning.".into(), "She came home to bury a secret.".into()],
        taglines: vec!["Some doors stay shut for a reason.".into()],
    }));
    let blurbs = fs::read_to_string(dir.join("marketing/blurbs.md")).unwrap();
    assert!(blurbs.contains("## Variant 1") && blurbs.contains("## Variant 2"));
    assert!(blurbs.contains("A reckoning"));
    assert!(fs::read_to_string(dir.join("marketing/taglines.md")).unwrap().contains("- Some doors stay shut"));
    assert!(app.overlay.is_some(), "summary overlay");
    assert_eq!(fs::read_to_string(dir.join("one.md")).unwrap(), before, "chapter prose untouched");
}

#[test]
fn marketing_with_no_taglines_writes_only_blurbs() {
    let (mut app, dir) = two_chapter_app("marketing-blurbs-only");
    app.apply_marketing(Ok(ai::marketing::Marketing {
        blurbs: vec!["Only a blurb.".into()],
        taglines: vec![],
    }));
    assert!(dir.join("marketing/blurbs.md").exists());
    assert!(!dir.join("marketing/taglines.md").exists(), "no empty taglines file");
}

// --- #237: social posts -----------------------------------------------------

#[test]
fn social_writes_a_file_per_platform_off_manuscript() {
    let (mut app, dir) = two_chapter_app("social");
    let before = fs::read_to_string(dir.join("one.md")).unwrap();
    app.apply_social(Ok(ai::social::Social {
        x: vec!["A locket. A reckoning. #fantasy".into()],
        facebook: vec!["She came home to bury a secret, and dug up something worse.".into()],
        instagram: vec!["Some doors stay shut for a reason.\n\n#booklover #fantasy".into()],
    }));
    assert!(fs::read_to_string(dir.join("marketing/social/x.md")).unwrap().contains("reckoning"));
    assert!(fs::read_to_string(dir.join("marketing/social/facebook.md")).unwrap().contains("bury a secret"));
    assert!(fs::read_to_string(dir.join("marketing/social/instagram.md")).unwrap().contains("#booklover"));
    assert!(app.overlay.is_some(), "summary overlay");
    assert_eq!(fs::read_to_string(dir.join("one.md")).unwrap(), before, "chapter prose untouched");
}

#[test]
fn social_skips_a_platform_with_no_posts() {
    let (mut app, dir) = two_chapter_app("social-partial");
    app.apply_social(Ok(ai::social::Social {
        x: vec!["just one platform".into()],
        facebook: vec![],
        instagram: vec![],
    }));
    assert!(dir.join("marketing/social/x.md").exists());
    assert!(!dir.join("marketing/social/facebook.md").exists(), "no empty facebook file");
    assert!(!dir.join("marketing/social/instagram.md").exists());
}

// --- #251: ad creative ------------------------------------------------------

#[test]
fn ad_creative_writes_a_file_per_platform_off_manuscript() {
    let (mut app, dir) = two_chapter_app("ad-creative");
    let before = fs::read_to_string(dir.join("one.md")).unwrap();
    let ad = |h: &str| ai::ad_creative::Ad {
        headlines: vec![h.into()],
        body: vec!["A locket. A reckoning.".into()],
        targeting: vec!["fantasy readers".into()],
        image_brief: "a fog-bound coast at dawn".into(),
    };
    app.apply_ad_creative(Ok(ai::ad_creative::AdCreative {
        amazon: ad("Unearth the secret"),
        meta: ad("She came home to bury it"),
        x: ad("Some doors stay shut"),
    }));
    assert!(fs::read_to_string(dir.join("marketing/ads/amazon.md")).unwrap().contains("Unearth the secret"));
    assert!(fs::read_to_string(dir.join("marketing/ads/meta.md")).unwrap().contains("## Suggested targeting"));
    assert!(fs::read_to_string(dir.join("marketing/ads/x.md")).unwrap().contains("## Ad image"));
    assert!(app.overlay.is_some(), "summary overlay");
    assert_eq!(fs::read_to_string(dir.join("one.md")).unwrap(), before, "chapter prose untouched");
}

#[test]
fn ad_creative_picks_up_existing_marketing_copy() {
    let (app, dir) = two_chapter_app("ad-creative-context");
    let mk = dir.join("marketing");
    fs::create_dir_all(&mk).unwrap();
    fs::write(mk.join("blurbs.md"), "---\ntitle: x\n---\n\n<!-- generated:start -->\nA locket and a lie.\n<!-- generated:end -->\n").unwrap();
    let ctx = app.marketing_copy_context();
    assert!(ctx.contains("A locket and a lie"), "reads existing marketing copy: {ctx:?}");
}

// --- #238: character portraits ----------------------------------------------

fn image_on(app: &mut App) {
    app.image = ai::image::ImageSettings { enabled: true, api_key: Some("k".into()), ..Default::default() };
}

fn write_character(dir: &std::path::Path, slug: &str, name: &str) {
    let chars = dir.join("bible").join("characters");
    fs::create_dir_all(&chars).unwrap();
    fs::write(chars.join(format!("{slug}.md")), format!("---\nname: {name}\n---\n\n## Physical\n- green eyes\n")).unwrap();
}

#[test]
fn portrait_pass_needs_image_generation_enabled() {
    let (mut app, dir) = two_chapter_app("portrait-off");
    app.image = ai::image::ImageSettings::default(); // off
    write_character(&dir, "mara", "Mara");
    app.start_portrait();
    assert!(!matches!(app.mode, Mode::Prompt(_)), "no confirm when image gen is off");
    assert!(
        app.message.as_deref().unwrap_or("").contains("Image generation is off"),
        "{:?}",
        app.message
    );
}

#[test]
fn portrait_pass_needs_the_character_bible() {
    let (mut app, _dir) = two_chapter_app("portrait-no-bible");
    image_on(&mut app);
    app.start_portrait(); // no bible/characters yet
    assert!(!matches!(app.mode, Mode::Prompt(_)));
    assert!(app.message.as_deref().unwrap_or("").contains("character pass first"), "{:?}", app.message);
}

#[test]
fn portrait_pass_opens_a_character_picker() {
    let (mut app, dir) = two_chapter_app("portrait-picker");
    image_on(&mut app);
    write_character(&dir, "mara", "Mara");
    write_character(&dir, "tomas", "Tomas");
    app.start_portrait();
    match &app.mode {
        // "All characters (2)" plus one item per character (#244).
        Mode::Palette(p) => {
            assert_eq!(p.title, "Character portrait");
            assert_eq!(p.items.len(), 3);
            assert!(p.items[0].label.contains("All characters (2)"), "{}", p.items[0].label);
        }
        _ => panic!("expected the character picker"),
    }
}

#[test]
fn picking_all_confirms_the_whole_cast() {
    let (mut app, dir) = two_chapter_app("portrait-all");
    image_on(&mut app);
    write_character(&dir, "mara", "Mara");
    write_character(&dir, "tomas", "Tomas");
    app.confirm_portraits(crate::mode::PortraitChoice::All);
    match &app.mode {
        Mode::Prompt(p) => assert!(p.label.contains("2 character portraits"), "{}", p.label),
        _ => panic!("expected the cost confirm"),
    }
}

#[test]
fn picking_one_confirms_and_generates_only_that_character() {
    let (mut app, dir) = two_chapter_app("portrait-one");
    image_on(&mut app);
    write_character(&dir, "mara", "Mara");
    write_character(&dir, "tomas", "Tomas");
    // Sorted by file: mara.md (0), tomas.md (1) → pick the second.
    app.confirm_portraits(crate::mode::PortraitChoice::One(1));
    match &app.mode {
        Mode::Prompt(p) => assert!(p.label.contains("1 character portrait"), "{}", p.label),
        _ => panic!("expected the cost confirm"),
    }
    // Only the one picked target is staged, so generation won't touch the others.
    let pending = app.pending_portraits.as_ref().expect("a selection is staged");
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].0, "tomas", "the picked slug");
}

#[test]
fn cancelling_the_portrait_confirm_discards_the_selection() {
    let (mut app, dir) = two_chapter_app("portrait-cancel");
    image_on(&mut app);
    write_character(&dir, "mara", "Mara");
    app.confirm_portraits(crate::mode::PortraitChoice::All);
    assert!(app.pending_portraits.is_some());
    app.on_key(key(KeyCode::Esc)); // decline the confirm
    assert!(app.pending_portraits.is_none(), "the staged selection is dropped");
}

#[test]
fn image_results_summarize_saved_and_failed() {
    let (mut app, _dir) = two_chapter_app("image-apply");
    app.apply_images(Ok(ai::image::ImageReport {
        saved: vec!["marketing/images/characters/mara.jpg".into()],
        failed: vec!["Tomas: network error".into()],
    }));
    assert!(app.overlay.is_some(), "summary overlay opens");
}

#[test]
#[cfg(not(feature = "local"))]
fn image_pass_reports_when_the_local_backend_isnt_compiled() {
    let (mut app, dir) = two_chapter_app("image-local-uncompiled");
    app.image = ai::image::ImageSettings {
        enabled: true,
        backend: ai::image::ImageBackendKind::Local,
        ..Default::default()
    };
    write_character(&dir, "mara", "Mara");
    app.start_portrait();
    assert!(!matches!(app.mode, Mode::Prompt(_)), "no confirm; local model isn't built in");
    assert!(
        app.message.as_deref().unwrap_or("").contains("--features local"),
        "explains how to enable: {:?}",
        app.message
    );
}

#[test]
fn scene_pass_needs_image_generation_enabled() {
    let (mut app, _dir) = two_chapter_app("scene-off");
    app.image = ai::image::ImageSettings::default(); // off
    app.start_scene();
    assert!(!matches!(app.mode, Mode::Prompt(_)), "no confirm when image gen is off");
    assert!(app.message.as_deref().unwrap_or("").contains("Image generation is off"), "{:?}", app.message);
}

#[test]
fn scene_pass_confirms_for_the_active_chapter() {
    let (mut app, _dir) = two_chapter_app("scene-confirm");
    image_on(&mut app);
    // The active chapter ("One") has prose in two_chapter_app, so there's a scene.
    app.start_scene();
    match &app.mode {
        Mode::Prompt(p) => {
            assert!(p.label.contains("One"), "names the active chapter: {}", p.label);
            assert!(p.label.contains("fal.ai") && p.label.contains("billed"), "{}", p.label);
        }
        _ => panic!("expected the scene cost confirm"),
    }
}

#[test]
fn scene_pass_needs_chapter_text() {
    let (mut app, _dir) = two_chapter_app("scene-empty");
    image_on(&mut app);
    // Empty the active chapter's buffer so there's nothing to illustrate.
    app.buffer = crate::buffer::Buffer::from_text("");
    app.start_scene();
    assert!(!matches!(app.mode, Mode::Prompt(_)));
    assert!(app.message.as_deref().unwrap_or("").contains("no text to illustrate"), "{:?}", app.message);
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
    let (mut app, _dir) = two_chapter_app("cost-confirm");
    app.ai.cost_threshold = 0.0; // any non-empty manuscript exceeds it
    app.start_ai_action(crate::mode::AiAction::Continuity, false);
    match &app.mode {
        Mode::Prompt(s) => {
            assert!(matches!(s.pending, Pending::ConfirmAi { .. }), "cost confirm opens");
            assert!(s.label.contains("run?"), "{}", s.label);
        }
        _ => panic!("expected the cost confirm prompt"),
    }
}

#[test]
fn a_cheap_pass_runs_without_confirmation() {
    let (mut app, _dir) = two_chapter_app("cost-cheap");
    app.ai.cost_threshold = 1_000.0; // nothing this small could exceed it
    app.start_ai_action(crate::mode::AiAction::Style, false);
    // It dispatched straight through (no key configured → a status message),
    // never stopping at a confirm prompt.
    assert!(!matches!(app.mode, Mode::Prompt(_)), "no confirm for a cheap pass");
}

#[test]
fn confirming_an_expensive_pass_dispatches_it() {
    let (mut app, _dir) = two_chapter_app("cost-yes");
    app.ai.cost_threshold = 0.0;
    app.start_ai_action(crate::mode::AiAction::Continuity, false);
    assert!(matches!(app.mode, Mode::Prompt(_)));
    app.on_key(key(KeyCode::Char('y'))); // confirm → dispatch (no key → message)
    assert!(!matches!(app.mode, Mode::Prompt(_)), "confirm closes the prompt");
}

#[test]
fn declining_an_expensive_pass_cancels_it() {
    let (mut app, _dir) = two_chapter_app("cost-no");
    app.ai.cost_threshold = 0.0;
    app.start_ai_action(crate::mode::AiAction::Continuity, false);
    app.on_key(key(KeyCode::Char('n')));
    assert!(matches!(app.mode, Mode::Insert), "declining returns to editing");
    assert!(!app.ai_task.is_busy(), "nothing was sent");
}

#[test]
fn triage_accumulates_findings_across_passes_deduped() {
    let (mut app, _dir) = two_chapter_app("triage-accum");
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
}

#[test]
fn triage_with_no_findings_shows_a_message() {
    let (mut app, _dir) = two_chapter_app("triage-empty");
    app.open_triage();
    assert!(!matches!(app.mode, Mode::Findings(_)));
    assert!(app.message.as_deref().unwrap_or("").contains("No findings"), "{:?}", app.message);
}

// --- Incremental pass scoping (#90) ----------------------------------------

/// Write a character bible file so the "up to date" path (bible present + state
/// recorded) is reachable.
fn write_character_bible(dir: &std::path::Path) {
    let chars = dir.join("bible").join("characters");
    fs::create_dir_all(&chars).unwrap();
    fs::write(chars.join("mara.md"), "---\nname: Mara\n---\n\n<!-- generated:start -->\nprofile\n<!-- generated:end -->\n").unwrap();
}

#[test]
fn an_up_to_date_bible_pass_makes_no_request() {
    let (mut app, dir) = two_chapter_app("incr-uptodate");
    // A bible exists and its recorded state matches the manuscript → nothing to do.
    write_character_bible(&dir);
    ai::incremental::State::record(&dir, "characters", "2026-06-21", app.chapter_digests()).unwrap();
    app.analyze_characters(false);
    assert!(!app.ai_busy(), "an up-to-date bible spawns no request");
    assert!(app.message.as_deref().unwrap_or("").contains("current"), "{:?}", app.message);
}

#[test]
fn forcing_a_rebuild_overrides_an_up_to_date_bible() {
    let (app, dir) = two_chapter_app("incr-force");
    write_character_bible(&dir);
    ai::incremental::State::record(&dir, "characters", "2026-06-21", app.chapter_digests()).unwrap();
    let action = crate::mode::AiAction::Characters;
    assert_eq!(app.pass_context_len(action, false), 0, "up to date by default");
    assert_eq!(
        app.pass_context_len(action, true),
        app.manuscript_text().len(),
        "the palette's R (force) rebuilds from the whole manuscript anyway"
    );
}

#[test]
fn a_hand_deleted_bible_rebuilds_even_when_state_says_current() {
    let (app, dir) = two_chapter_app("incr-drift");
    // The state claims the bible is built from the current manuscript, but no
    // bible file exists on disk (deleted by hand) — rebuild rather than skip.
    ai::incremental::State::record(&dir, "characters", "2026-06-21", app.chapter_digests()).unwrap();
    assert_eq!(
        app.pass_context_len(crate::mode::AiAction::Characters, false),
        app.manuscript_text().len(),
        "a missing bible forces a full rebuild, not an up-to-date skip"
    );
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
}

#[test]
fn cost_context_is_the_whole_manuscript_first_then_zero_when_current() {
    let (app, dir) = two_chapter_app("incr-cost");
    let action = crate::mode::AiAction::Characters;
    assert_eq!(app.pass_context_len(action, false), app.manuscript_text().len(), "first run estimates the full manuscript");
    write_character_bible(&dir);
    ai::incremental::State::record(&dir, "characters", "2026-06-21", app.chapter_digests()).unwrap();
    assert_eq!(app.pass_context_len(action, false), 0, "an up-to-date bible costs nothing to confirm");
}

#[test]
fn a_clean_bible_pass_records_what_it_built_from() {
    let (mut app, dir) = two_chapter_app("incr-record");
    let built = app.chapter_digests();
    // Stand in for what analyze_characters sets just before spawning the request.
    app.pending_pass = Some(("characters".to_string(), built.clone(), false));
    app.apply_characters(Ok(vec![Character { name: "Mara".into(), ..Default::default() }]));
    let state = ai::incremental::State::load(&dir);
    assert_eq!(state.built_from("characters"), Some(&built), "the pass recorded its manuscript state");
    assert!(app.pending_pass.is_none(), "the snapshot was consumed");
}

// --- AI backend switch (#371) ----------------------------------------------

#[test]
fn offline_pin_blocks_the_backend_switch() {
    let (mut app, _dir) = two_chapter_app("be-offline");
    app.ai.enabled = true;
    app.ai.offline = true;
    let before = app.ai.backend;
    app.toggle_ai_backend();
    assert_eq!(app.ai.backend, before, "the offline pin leaves the configured backend untouched");
    assert!(app.message.as_deref().unwrap_or("").contains("Offline mode pins"), "{:?}", app.message);
    assert_eq!(app.ai_backend_label(), "local (offline pinned)");
}

#[cfg(not(feature = "local"))]
#[test]
fn switching_to_local_without_the_feature_explains_how() {
    let (mut app, _dir) = two_chapter_app("be-nolocal");
    app.ai.enabled = true;
    app.ai.backend = ai::BackendKind::Claude;
    app.toggle_ai_backend(); // would flip to local, but it isn't compiled in
    assert_eq!(app.ai.backend, ai::BackendKind::Claude, "stays on Claude rather than pretending to switch");
    let msg = app.message.as_deref().unwrap_or("");
    assert!(msg.contains("--features local"), "{msg}");
}

#[cfg(feature = "local")]
#[test]
fn switching_flips_between_claude_and_local() {
    let (mut app, _dir) = two_chapter_app("be-flip");
    app.ai.enabled = true;
    app.ai.backend = ai::BackendKind::Claude;

    app.toggle_ai_backend();
    assert_eq!(app.ai.backend, ai::BackendKind::Local);
    assert_eq!(app.ai_backend_label(), "local (on-device)");

    app.toggle_ai_backend();
    assert_eq!(app.ai.backend, ai::BackendKind::Claude);
    assert_eq!(app.ai_backend_label(), "Claude (cloud)");
}

// --- Hybrid routing (#372) -------------------------------------------------

#[test]
fn offline_routes_costed_actions_on_device() {
    let (mut app, _dir) = two_chapter_app("route-offline");
    app.ai.enabled = true;
    app.ai.offline = true; // the hard pin
    assert!(
        matches!(app.ai_action_start(AiAction::Continuity, false), AiStart::Run(ai::BackendKind::Local)),
        "offline pins every pass on-device",
    );
}

#[test]
fn ask_policy_confirms_when_over_the_threshold() {
    let (mut app, _dir) = two_chapter_app("route-ask");
    app.ai.enabled = true;
    app.ai.routing = Policy::Ask;
    app.ai.cost_threshold = 0.0; // any non-empty manuscript is "over"
    assert!(
        matches!(app.ai_action_start(AiAction::Continuity, false), AiStart::Confirm { .. }),
        "the ask policy surfaces the choice over the threshold",
    );
}

#[test]
fn the_default_policy_keeps_the_plain_cost_confirm() {
    let (mut app, _dir) = two_chapter_app("route-default");
    app.ai.enabled = true; // routing defaults to Configured
    app.ai.cost_threshold = 0.0;
    match app.ai_action_start(AiAction::Continuity, false) {
        AiStart::Confirm { offer_local, label } => {
            assert!(!offer_local, "the default policy doesn't offer a local detour");
            assert!(label.contains("run?"), "{label}");
        }
        _ => panic!("expected the historical cost confirm"),
    }
}

#[cfg(feature = "local")]
#[test]
fn prefer_local_runs_a_fitting_pass_on_device() {
    let (mut app, _dir) = two_chapter_app("route-prefer-local");
    app.ai.enabled = true;
    app.ai.routing = Policy::PreferLocal;
    assert!(
        matches!(app.ai_action_start(AiAction::Continuity, false), AiStart::Run(ai::BackendKind::Local)),
        "prefer-local keeps a small pass on-device",
    );
}
