//! AI prose & off-manuscript passes: style, voice, submission, marketing, social, ad creative, and the local analysis report.

use super::*;
use std::fs;

// --- AI: prose & style ------------------------------------------------------

#[test]
fn style_findings_open_the_overlay() {
    let (mut app, _dir) = two_chapter_app("style");
    app.apply_style(Ok(ai::style::Style {
        findings: vec![ai::style::StyleFinding {
            kind: crate::ai::style::StyleKind::Pov,
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
            kind: crate::ai::voice::VoiceKind::Dialogue,
            character: "Mara".into(),
            line: String::new(),
            detail: "too formal for her clipped register".into(),
            chapters: vec!["Two".into()],
        }],
    }));
    match &app.mode {
        Mode::Findings(f) => {
            assert!(
                f.items[0].summary.contains("Mara"),
                "names the character: {}",
                f.items[0].summary
            );
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
    assert!(
        !app.ai_task.is_busy(),
        "nothing sent without voice profiles to ground on"
    );
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("character pass first"),
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
    fs::write(
        chars.join("mara.md"),
        "name: Mara\n\n## Physical\n- green eyes\n",
    )
    .unwrap();
    app.analyze_voice();
    assert!(!app.ai_task.is_busy());
    assert!(
        app.message
            .as_deref()
            .unwrap_or("")
            .contains("character pass first"),
        "{:?}",
        app.message
    );
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
    assert!(fs::read_to_string(dir.join("submission/synopsis.md"))
        .unwrap()
        .contains("confronts her brother"));
    assert!(fs::read_to_string(dir.join("submission/blurb.md"))
        .unwrap()
        .contains("reckoning"));
    assert!(fs::read_to_string(dir.join("submission/query.md"))
        .unwrap()
        .contains("Dear Agent"));
    assert!(app.overlay.is_some(), "summary overlay");
    assert!(
        !matches!(app.mode, Mode::BibleReview(_)),
        "no conflicts on a fresh write"
    );
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
    assert!(
        text.contains("One") && text.contains("Two"),
        "both chapters listed: {text}"
    );
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
    assert!(
        app.overlay.is_none(),
        "a non-scroll key dismisses the report"
    );
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
fn s_saves_the_analysis_report_to_a_dated_file() {
    let (mut app, dir) = two_chapter_app("analysis-save");
    app.on_key(ctrl('t'));
    let on_screen = app.overlay.as_ref().unwrap().plain_text();

    app.on_key(key(KeyCode::Char('s'))); // save the snapshot
    assert!(
        app.overlay.is_some(),
        "saving leaves the report open to keep reading"
    );

    let path = dir.join("export").join(format!(
        "analysis-{}.md",
        crate::date::iso(crate::date::today())
    ));
    let written = fs::read_to_string(&path).expect("the report file was written");
    assert_eq!(written, on_screen, "the file matches what's on screen");
    assert!(
        written.contains("analysis"),
        "it's the analysis report: {written}"
    );
    assert!(
        app.message.as_deref().unwrap().contains("export/analysis-"),
        "the status names the saved file: {:?}",
        app.message
    );
}

#[test]
fn s_saves_the_progress_report_to_a_dated_file() {
    let (mut app, dir) = two_chapter_app("progress-save");
    app.open_progress_report();
    let on_screen = app.overlay.as_ref().unwrap().plain_text();

    app.on_key(key(KeyCode::Char('s')));

    let path = dir.join("export").join(format!(
        "progress-{}.md",
        crate::date::iso(crate::date::today())
    ));
    let written = fs::read_to_string(&path).expect("the progress report was written");
    assert_eq!(written, on_screen, "the file matches what's on screen");
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
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        before,
        "chapter prose untouched"
    );
}

// --- #236: marketing copy ---------------------------------------------------

#[test]
fn marketing_writes_blurbs_and_taglines_off_manuscript() {
    let (mut app, dir) = two_chapter_app("marketing");
    let before = fs::read_to_string(dir.join("one.md")).unwrap();
    app.apply_marketing(Ok(ai::marketing::Marketing {
        blurbs: vec![
            "A locket. A reckoning.".into(),
            "She came home to bury a secret.".into(),
        ],
        taglines: vec!["Some doors stay shut for a reason.".into()],
    }));
    let blurbs = fs::read_to_string(dir.join("marketing/blurbs.md")).unwrap();
    assert!(blurbs.contains("## Variant 1") && blurbs.contains("## Variant 2"));
    assert!(blurbs.contains("A reckoning"));
    assert!(fs::read_to_string(dir.join("marketing/taglines.md"))
        .unwrap()
        .contains("- Some doors stay shut"));
    assert!(app.overlay.is_some(), "summary overlay");
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        before,
        "chapter prose untouched"
    );
}

#[test]
fn marketing_with_no_taglines_writes_only_blurbs() {
    let (mut app, dir) = two_chapter_app("marketing-blurbs-only");
    app.apply_marketing(Ok(ai::marketing::Marketing {
        blurbs: vec!["Only a blurb.".into()],
        taglines: vec![],
    }));
    assert!(dir.join("marketing/blurbs.md").exists());
    assert!(
        !dir.join("marketing/taglines.md").exists(),
        "no empty taglines file"
    );
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
    assert!(fs::read_to_string(dir.join("marketing/social/x.md"))
        .unwrap()
        .contains("reckoning"));
    assert!(fs::read_to_string(dir.join("marketing/social/facebook.md"))
        .unwrap()
        .contains("bury a secret"));
    assert!(
        fs::read_to_string(dir.join("marketing/social/instagram.md"))
            .unwrap()
            .contains("#booklover")
    );
    assert!(app.overlay.is_some(), "summary overlay");
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        before,
        "chapter prose untouched"
    );
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
    assert!(
        !dir.join("marketing/social/facebook.md").exists(),
        "no empty facebook file"
    );
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
    assert!(fs::read_to_string(dir.join("marketing/ads/amazon.md"))
        .unwrap()
        .contains("Unearth the secret"));
    assert!(fs::read_to_string(dir.join("marketing/ads/meta.md"))
        .unwrap()
        .contains("## Suggested targeting"));
    assert!(fs::read_to_string(dir.join("marketing/ads/x.md"))
        .unwrap()
        .contains("## Ad image"));
    assert!(app.overlay.is_some(), "summary overlay");
    assert_eq!(
        fs::read_to_string(dir.join("one.md")).unwrap(),
        before,
        "chapter prose untouched"
    );
}

#[test]
fn ad_creative_picks_up_existing_marketing_copy() {
    let (app, dir) = two_chapter_app("ad-creative-context");
    let mk = dir.join("marketing");
    fs::create_dir_all(&mk).unwrap();
    fs::write(mk.join("blurbs.md"), "---\ntitle: x\n---\n\n<!-- generated:start -->\nA locket and a lie.\n<!-- generated:end -->\n").unwrap();
    let ctx = app.marketing_copy_context();
    assert!(
        ctx.contains("A locket and a lie"),
        "reads existing marketing copy: {ctx:?}"
    );
}
