//! AI generation: scaffolding, naming, the command palette, brainstorm, and drafting.

use super::*;
use crate::ai::naming::{self, Naming};
use crate::ai::scaffold::{self, Scaffold};
use std::fs;

// --- AI: scaffolding (outline + gaps + xxx insertion) ----------------------

#[test]
fn scaffold_writes_outline_and_opens_gap_findings() {
    let (mut app, dir) = two_chapter_app("scaffold");
    let s = Scaffold {
        outline: vec![scaffold::OutlineItem {
            chapter: "One".into(),
            beats: vec!["a beat".into()],
        }],
        gaps: vec![scaffold::Gap {
            detail: "no clear goal".into(),
            chapter: "Two".into(),
        }],
    };
    app.apply_scaffold(Ok(s));
    let file = fs::read_to_string(dir.join("bible/outline.md")).expect("outline written");
    assert!(file.contains("## One") && file.contains("a beat"));
    assert!(file.contains("## Gaps"));
    assert!(
        matches!(app.mode, Mode::Findings(_)),
        "gap findings overlay opens"
    );
}

#[test]
fn marking_a_finding_inserts_an_xxx_todo_in_a_nonactive_chapter() {
    let (mut app, dir) = two_chapter_app("mark-todo"); // active is "One"
    app.apply_scaffold(Ok(Scaffold {
        outline: vec![],
        gaps: vec![scaffold::Gap {
            detail: "needs a hook".into(),
            chapter: "Two".into(),
        }],
    }));
    let before = app.count_todo();
    app.on_key(key(KeyCode::Char('i'))); // mark xxx in "Two" (non-active → file)
    assert!(
        app.count_todo() > before,
        "the report's xxx count should rise"
    );
    assert!(
        fs::read_to_string(dir.join("two.md"))
            .unwrap()
            .contains("xxx"),
        "marker on disk"
    );
}

#[test]
fn marking_a_finding_in_the_active_chapter_updates_the_buffer() {
    let (mut app, _dir) = two_chapter_app("mark-active"); // active is "One"
    app.apply_scaffold(Ok(Scaffold {
        outline: vec![],
        gaps: vec![scaffold::Gap {
            detail: "weak open".into(),
            chapter: "One".into(),
        }],
    }));
    app.on_key(key(KeyCode::Char('i')));
    assert!(
        app.buffer.text().contains("xxx"),
        "marker shows in the active buffer"
    );
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
    fs::write(
        dir.join("bible/characters/mara.md"),
        "---\nname: Mara Vance\n---\n",
    )
    .unwrap();

    let e = app.known_entities();
    assert!(e.contains(&"Aldermoor".to_string()));
    assert!(e.contains(&"lightsteel".to_string()));
    assert!(e.contains(&"Mara Vance".to_string()));
    assert!(
        !e.contains(&"nope".to_string()),
        "only the Gazetteer section is read"
    );
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
        Mode::Palette(p) => assert_eq!(p.items.len(), 33, "all AI actions listed"),
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
fn applying_word_search_results_opens_a_pickable_list() {
    let (mut app, _dir) = two_chapter_app("thesaurus-apply");
    let words = crate::ai::thesaurus::parse(
        r#"{"words":[
            {"text":"melancholy","definition":"a pensive sadness","syllables":4},
            {"text":"wistful","definition":"gently sad and longing"}
        ]}"#,
    )
    .unwrap();
    app.apply_thesaurus(Ok(words));
    match &app.mode {
        Mode::Palette(p) => {
            assert_eq!(p.items.len(), 2, "one row per candidate word");
            assert!(matches!(
                &p.items[0].command,
                crate::mode::Command::InsertWord(w) if w == "melancholy"
            ));
        }
        _ => panic!("candidates should show as a pickable list"),
    }
}

#[test]
fn empty_word_search_results_report_instead_of_opening_a_list() {
    let (mut app, _dir) = two_chapter_app("thesaurus-empty");
    app.apply_thesaurus(Ok(vec![]));
    assert!(
        !matches!(app.mode, Mode::Palette(_)),
        "no list opens for zero candidates"
    );
    assert!(
        app.message.is_some(),
        "the author is told to loosen the constraints"
    );
}

#[test]
fn picking_a_word_places_it_at_the_cursor() {
    let (mut app, _dir) = two_chapter_app("thesaurus-place");
    let before = app.buffer.text();
    app.insert_word("melancholy");
    let after = app.buffer.text();
    assert!(
        after.contains("melancholy") && !before.contains("melancholy"),
        "the chosen word is placed verbatim at the cursor"
    );
}

// --- AI: name generator (#610) ---------------------------------------------

#[test]
fn name_generator_opens_the_kind_picker() {
    let (mut app, _dir) = two_chapter_app("namegen-kinds");
    app.start_namegen();
    match &app.mode {
        Mode::Palette(p) => assert_eq!(
            p.items.len(),
            crate::ai::namegen::Kind::ALL.len(),
            "character / place / term"
        ),
        _ => panic!("name generator should open the kind picker"),
    }
}

#[test]
fn picking_a_kind_opens_the_constraints_prompt() {
    let (mut app, _dir) = two_chapter_app("namegen-prompt");
    app.prompt_namegen(crate::ai::namegen::Kind::Place);
    match &app.mode {
        Mode::Prompt(s) => assert!(
            matches!(s.pending, Pending::NameGen(crate::ai::namegen::Kind::Place)),
            "the prompt carries the chosen kind"
        ),
        _ => panic!("picking a kind should open the constraints prompt"),
    }
}

#[test]
fn applying_name_results_opens_a_pickable_list() {
    let (mut app, _dir) = two_chapter_app("namegen-apply");
    let names = crate::ai::namegen::parse(
        r#"{"names":[
            {"text":"Aldric","note":"old ruler","syllables":2,"origin":"Old English"},
            {"text":"Bramwen","note":"raven-fair"}
        ]}"#,
    )
    .unwrap();
    app.apply_namegen(Ok(names));
    match &app.mode {
        Mode::Palette(p) => {
            assert_eq!(p.items.len(), 2, "one row per candidate name");
            assert!(matches!(
                &p.items[0].command,
                crate::mode::Command::InsertName(n) if n == "Aldric"
            ));
        }
        _ => panic!("candidates should show as a pickable list"),
    }
}

#[test]
fn empty_name_results_report_instead_of_opening_a_list() {
    let (mut app, _dir) = two_chapter_app("namegen-empty");
    app.apply_namegen(Ok(vec![]));
    assert!(
        !matches!(app.mode, Mode::Palette(_)),
        "no list opens for zero candidates"
    );
    assert!(
        app.message.is_some(),
        "the author is told to loosen constraints"
    );
}

#[test]
fn picking_a_name_places_it_at_the_cursor() {
    let (mut app, _dir) = two_chapter_app("namegen-place");
    let before = app.buffer.text();
    app.insert_name("Aldric");
    let after = app.buffer.text();
    assert!(
        after.contains("Aldric") && !before.contains("Aldric"),
        "the chosen name is placed verbatim at the cursor"
    );
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
            assert!(
                summaries
                    .iter()
                    .any(|s| s.starts_with("structural:") && s.contains("stakes")),
                "{summaries:?}"
            );
            assert!(
                summaries.iter().any(|s| s.starts_with("line:")
                    && s.contains('\u{201c}')
                    && s.contains("could of")),
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
    assert!(
        matches!(app.mode, Mode::Prompt(_)),
        "Ask opens the question prompt"
    );
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
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        before,
        "chapter prose untouched"
    );
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
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        before,
        "chapter prose untouched"
    );
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
    assert!(
        profile.contains("Generated by Quill"),
        "marked AI-generated"
    );
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        before,
        "chapter prose untouched"
    );
}
