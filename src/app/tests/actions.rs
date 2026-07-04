//! Tests for the unified action registry and the run-action menu (#225): the
//! registry enumerates every invokable action with a stable name, the menu lists
//! them, and `run_action` routes each through its existing code path while
//! surfacing why an unavailable action can't run.

use super::*;
use crate::ai;
use crate::ai::keychain::Key;
use crate::mode::{Action, AiAction, ExportFormat, PaletteRow, Pending};

/// Every registry action has a distinct stable name — workflows (#89) reference
/// these, so a collision would make one unaddressable.
#[test]
fn action_names_are_unique_and_stable() {
    let mut names: Vec<&str> = Action::ALL.iter().map(|a| a.name()).collect();
    let count = names.len();
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), count, "action names must be unique");
    // A couple of the names the ticket calls out by hand — these are the contract.
    assert!(names.contains(&"save"));
    assert!(names.contains(&"export:docx"));
    assert!(names.contains(&"report:progress"));
    assert!(names.contains(&"ai:characters"));
    assert!(names.contains(&"triage"));
}

#[test]
fn action_palette_lists_every_registry_action() {
    let (mut app, _dir) = two_chapter_app("action-palette");
    app.open_action_palette();
    match &app.mode {
        Mode::Palette(p) => assert_eq!(p.items.len(), Action::ALL.len(), "all actions listed"),
        _ => panic!("expected the run-action palette to open"),
    }
}

/// Each group's actions must be contiguous in `ALL` — otherwise the menu would
/// emit a second header for a re-opened group. Guards future `ALL` additions.
#[test]
fn action_groups_are_contiguous_in_the_registry() {
    let mut seen: Vec<&str> = Vec::new();
    let mut prev: Option<&str> = None;
    for a in Action::ALL {
        let g = a.group();
        if prev != Some(g) {
            assert!(
                !seen.contains(&g),
                "group '{g}' is split across the registry — keep each group's actions together"
            );
            seen.push(g);
            prev = Some(g);
        }
    }
}

#[test]
fn the_action_palette_groups_actions_under_collapsible_headers() {
    let (mut app, _dir) = two_chapter_app("palette-groups");
    app.open_action_palette();
    let Mode::Palette(p) = &app.mode else {
        panic!("expected the run-action palette");
    };
    let rows = p.rows();
    // The first visible row is a group header.
    assert!(
        matches!(
            rows[0],
            PaletteRow::Header {
                group: "Project",
                ..
            }
        ),
        "first row is the Project header"
    );
    // The AI passes gather under one heading; each group emits exactly one header.
    let headers: Vec<&str> = rows
        .iter()
        .filter_map(|r| match r {
            PaletteRow::Header { group, .. } => Some(*group),
            PaletteRow::Item(_) => None,
        })
        .collect();
    assert!(headers.contains(&"AI") && headers.contains(&"Export"));
    let mut unique = headers.clone();
    unique.sort_unstable();
    unique.dedup();
    assert_eq!(unique.len(), headers.len(), "one header per group");
}

#[test]
fn collapsing_a_group_hides_its_items_but_keeps_the_header() {
    let (mut app, _dir) = two_chapter_app("palette-collapse");
    app.open_action_palette();
    let before = match &app.mode {
        Mode::Palette(p) => p.rows().len(),
        _ => unreachable!(),
    };
    // The Project header is selected first; ← collapses it.
    app.on_key(key(KeyCode::Left));
    let Mode::Palette(p) = &app.mode else {
        panic!();
    };
    assert!(p.collapsed.contains("Project"), "group collapsed");
    assert!(p.rows().len() < before, "the group's items are hidden");
    assert!(
        matches!(
            p.rows()[p.selected],
            PaletteRow::Header {
                group: "Project",
                collapsed: true
            }
        ),
        "the header stays, still selected"
    );
}

#[test]
fn enter_on_a_header_toggles_it_rather_than_running_an_action() {
    let (mut app, _dir) = two_chapter_app("palette-toggle");
    app.open_action_palette();
    app.on_key(key(KeyCode::Enter)); // on the Project header
    assert!(
        matches!(app.mode, Mode::Palette(_)),
        "Enter on a header keeps the palette open"
    );
    let Mode::Palette(p) = &app.mode else {
        panic!();
    };
    assert!(p.collapsed.contains("Project"), "collapsed by Enter");
    app.on_key(key(KeyCode::Enter)); // toggles back open
    let Mode::Palette(p) = &app.mode else {
        panic!();
    };
    assert!(!p.collapsed.contains("Project"), "Enter expands it again");
}

#[test]
fn a_flat_palette_has_no_group_headers() {
    let (mut app, _dir) = two_chapter_app("palette-flat");
    app.open_ai_palette(); // the AI palette is ungrouped
    let Mode::Palette(p) = &app.mode else {
        panic!();
    };
    assert!(
        p.rows().iter().all(|r| matches!(r, PaletteRow::Item(_))),
        "a flat palette renders no headers"
    );
    assert_eq!(p.rows().len(), p.items.len());
}

#[test]
fn ctrl_k_opens_the_action_palette() {
    let (mut app, _dir) = two_chapter_app("action-ctrl-k");
    app.on_key(ctrl('k'));
    assert!(
        matches!(app.mode, Mode::Palette(_)),
        "Ctrl-k opens the run-action menu"
    );
}

#[test]
fn run_save_action_writes_the_chapter() {
    let (mut app, dir) = two_chapter_app("action-save");
    app.on_key(key(KeyCode::Char('X'))); // edit chapter one
    assert!(app.buffer.dirty);
    app.run_action(Action::Save, false);
    assert!(!app.buffer.dirty, "save clears the dirty flag");
    let saved = fs::read_to_string(dir.join("one.md")).unwrap();
    assert!(saved.starts_with('X'), "edit saved, got: {saved:?}");
}

#[test]
fn run_progress_report_action_opens_the_overlay() {
    let (mut app, _dir) = two_chapter_app("action-report");
    app.run_action(Action::ProgressReport, false);
    assert!(app.overlay.is_some(), "the progress report overlay opens");
}

#[test]
fn run_writing_history_action_opens_the_overlay() {
    // The view reads the recorded history log; seed one in the project's .quill
    // so the overlay has content to render (#586).
    let (mut app, _dir) = two_chapter_app("action-history");
    crate::history::persist(
        &app.project.root,
        &crate::history::Snapshot {
            date: "2026-06-16".into(),
            words: 1000,
            added: 400,
            removed: 10,
            ..Default::default()
        },
    )
    .ok();
    app.run_action(Action::WritingHistory, false);
    assert!(app.overlay.is_some(), "the writing-history overlay opens");
}

#[test]
fn run_export_action_compiles_the_manuscript() {
    let (mut app, dir) = two_chapter_app("action-export");
    app.run_action(Action::Export(ExportFormat::Markdown), false);
    assert!(
        app.message.as_deref().unwrap_or("").contains("Exported to"),
        "{:?}",
        app.message
    );
    assert!(
        dir.join("export").join("book.md").exists(),
        "export written"
    );
}

/// Export needs the per-chapter files; in the continuous view the window is
/// unsaved, so the action reports why instead of running.
#[test]
fn export_action_unavailable_in_the_continuous_view() {
    let (mut app, dir) = two_chapter_app("action-export-continuous");
    app.toggle_continuous();
    assert!(app.manuscript.is_some());
    app.run_action(Action::Export(ExportFormat::Markdown), false);
    assert!(
        app.message.as_deref().unwrap_or("").contains("continuous"),
        "explains the continuous-view guard, got: {:?}",
        app.message
    );
    assert!(
        !dir.join("export").join("book.md").exists(),
        "nothing exported"
    );
}

/// A network AI action with the backend off opens the in-app enable on-ramp
/// (#602) rather than a dead-end message, and sends nothing before the author acts.
#[test]
fn ai_action_when_ai_is_off_offers_to_enable() {
    let (mut app, _dir) = two_chapter_app("action-ai-off");
    app.ai = ai::Settings::default(); // off, deterministic regardless of env/config
    app.run_action(Action::Ai(AiAction::Characters), false);
    assert!(!app.ai_task.is_busy(), "nothing sent when AI is off");
    match &app.mode {
        Mode::Prompt(s) => assert!(
            matches!(
                s.pending,
                Pending::ConfirmEnableAi {
                    resume: Some(AiAction::Characters)
                }
            ),
            "the enable prompt carries the action to resume"
        ),
        _ => panic!("expected the enable-AI prompt to open"),
    }
}

/// Declining the enable prompt leaves AI off and writes nothing — the opt-in
/// default is preserved (#602).
#[test]
fn declining_the_enable_prompt_leaves_ai_off() {
    let (mut app, _dir) = two_chapter_app("action-ai-enable-decline");
    isolate("enable-decline");
    app.ai = ai::Settings::default();
    app.run_action(Action::Ai(AiAction::Characters), false);
    app.on_key(key(KeyCode::Char('n')));
    assert!(!app.ai.enabled, "AI stays off after declining");
    assert!(!app.ai_task.is_busy(), "nothing sent");
    // No config file written (or, if one exists, no enabled = true).
    let cfg = crate::config::config_path().and_then(|p| std::fs::read_to_string(p).ok());
    assert!(
        cfg.is_none_or(|t| !t.contains("enabled = true")),
        "nothing enabled in the config"
    );
}

/// Confirming the enable prompt writes `[ai] enabled = true` to the hermetic
/// config, applies it live, and — with no key set — chains into the Set-key flow
/// (#602). No live AI call is made.
#[test]
fn confirming_the_enable_prompt_writes_the_flag_and_chains_to_the_key() {
    let (mut app, _dir) = two_chapter_app("action-ai-enable-confirm");
    isolate("enable-confirm");
    app.ai = ai::Settings::default();
    app.run_action(Action::Ai(AiAction::Characters), false);
    app.on_key(key(KeyCode::Char('y')));
    // The flag is spliced into the config and applied to the running app.
    assert!(app.ai.enabled, "AI enabled live");
    let text = std::fs::read_to_string(crate::config::config_path().unwrap()).unwrap();
    assert!(text.contains("enabled = true"), "flag persisted: {text}");
    // No key is set (tests never touch the keychain), so it chains to the key flow.
    match &app.mode {
        Mode::Prompt(s) => assert!(
            matches!(s.pending, Pending::SetApiKey(Key::Anthropic)),
            "chains into the Set-key prompt"
        ),
        _ => panic!("expected the set-key prompt"),
    }
    assert!(!app.ai_task.is_busy(), "no AI call made");
}

/// The plain "Enable AI assistance" run-action opens the same enable prompt with
/// no pass to resume, and is listed only while AI is off (#602).
#[test]
fn enable_ai_action_is_registered_and_gated() {
    assert!(matches!(
        Action::from_name("ai:enable"),
        Some(Action::EnableAi)
    ));
    assert_eq!(Action::EnableAi.group(), "AI");

    let (mut app, _dir) = two_chapter_app("action-enable-plain");
    app.ai = ai::Settings::default(); // off → available
    assert!(app.action_status(Action::EnableAi).is_none());
    app.run_action(Action::EnableAi, false);
    match &app.mode {
        Mode::Prompt(s) => assert!(
            matches!(s.pending, Pending::ConfirmEnableAi { resume: None }),
            "plain action enables without resuming a pass"
        ),
        _ => panic!("expected the enable-AI prompt"),
    }
    // Once AI is on, the plain action is no longer offered.
    app.ai.enabled = true;
    assert!(app.action_status(Action::EnableAi).is_some());
}

/// Triage is local aggregation, so it stays available with the AI backend off —
/// it just reports an empty list rather than blocking on AI readiness.
#[test]
fn triage_action_available_when_ai_is_off() {
    let (mut app, _dir) = two_chapter_app("action-triage-off");
    app.ai = ai::Settings::default();
    app.run_action(Action::Ai(AiAction::Triage), false);
    assert!(!app.ai_task.is_busy());
    assert!(
        app.message.as_deref().unwrap_or("").contains("No findings"),
        "triage runs locally and reports its empty state, got: {:?}",
        app.message
    );
}

// --- Storing API keys in the OS keychain (#530) ----------------------------

/// The set-key actions are always available — they configure the key, so they
/// can't depend on a key already being set, and they don't require AI enabled.
#[test]
fn set_key_actions_are_always_available() {
    let (mut app, _dir) = two_chapter_app("action-set-key-avail");
    app.ai = ai::Settings::default(); // AI off
    assert!(app.action_status(Action::SetClaudeKey).is_none());
    assert!(app.action_status(Action::SetImageKey).is_none());
}

/// Running the set-key action opens a text prompt for the key — it never writes
/// anything until the author enters a value.
#[test]
fn set_claude_key_action_opens_a_prompt() {
    let (mut app, _dir) = two_chapter_app("action-set-key-prompt");
    app.run_action(Action::SetClaudeKey, false);
    match &app.mode {
        Mode::Prompt(s) => {
            assert!(matches!(s.pending, Pending::SetApiKey(Key::Anthropic)));
            assert!(s.input.is_empty(), "no key prefilled");
        }
        _ => panic!("expected the set-key prompt to open"),
    }
}

// --- Exporting the story bible (#708) --------------------------------------

/// The export-bible action is registered, needs no AI, and (unlike the manuscript
/// exports) works from the continuous view — it reads `bible/` from disk, not the
/// buffer.
#[test]
fn export_bible_action_is_registered_and_always_available() {
    assert!(matches!(
        Action::from_name("export:bible"),
        Some(Action::ExportBible)
    ));
    assert_eq!(Action::ExportBible.group(), "Export");
    let (mut app, _dir) = two_chapter_app("action-export-bible-avail");
    app.ai = ai::Settings::default(); // AI off — still available (no request)
    assert!(app.action_status(Action::ExportBible).is_none());
}

/// End-to-end: seeding a bible and dispatching the action writes a clean, ordered
/// `export/story-bible.md` with the generated/notes markers and front matter
/// stripped.
#[test]
fn export_bible_action_writes_a_clean_document() {
    use crate::ai::bible::render;

    let (mut app, dir) = two_chapter_app("action-export-bible-run");
    let bible = dir.join("bible");
    fs::create_dir_all(bible.join("characters")).unwrap();
    fs::write(
        bible.join("characters").join("mara.md"),
        render(
            "name: Mara\n",
            "## Physical\n- green eyes\n",
            "## Notes\n\nThe lead.\n",
            "2026-06-19",
        ),
    )
    .unwrap();
    fs::write(
        bible.join("setting.md"),
        render(
            "title: Setting\n",
            "## Gazetteer\n- Aldermoor\n",
            "## Notes\n",
            "2026-06-19",
        ),
    )
    .unwrap();

    app.run_action(Action::ExportBible, false);

    let written =
        fs::read_to_string(dir.join("export").join("story-bible.md")).expect("bible written");
    assert!(written.contains("## Characters"));
    assert!(written.contains("green eyes"));
    assert!(written.contains("The lead."));
    assert!(written.contains("Aldermoor"));
    assert!(!written.contains("generated:start"), "markers stripped");
    assert!(!written.contains("digest:"), "front matter stripped");
    assert!(
        written.find("## Characters").unwrap() < written.find("## Setting").unwrap(),
        "sections in order"
    );
    assert!(
        app.message.as_deref().unwrap_or("").contains("story-bible"),
        "reports the written path, got: {:?}",
        app.message
    );
}

// --- Export preflight / publish health check (#815) ------------------------

/// The preflight action is registered under Export and, like the manuscript
/// exports, is available from the single-chapter view without AI.
#[test]
fn preflight_action_is_registered_under_export() {
    assert!(matches!(
        Action::from_name("export:preflight"),
        Some(Action::PreflightExport)
    ));
    assert_eq!(Action::PreflightExport.group(), "Export");
    let (mut app, _dir) = two_chapter_app("action-preflight-avail");
    app.ai = ai::Settings::default(); // AI off — still available (no request)
    assert!(app.action_status(Action::PreflightExport).is_none());
}

/// A book with known issues — no ISBN, no cover, and an empty chapter — produces
/// the expected fail/warn lines, shown in the overlay and saved to
/// `export/preflight-<date>.md`.
#[test]
fn preflight_flags_a_book_with_known_issues() {
    let dir = app_scratch_dir("action-preflight-flawed");
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Flawed\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Empty\"\nfile = \"empty.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "The night was dark and long.\n").unwrap();
    fs::write(dir.join("empty.md"), "").unwrap(); // an empty body chapter
    let mut app = open_test_app(&dir);
    let _dir = TempProject::new(dir.clone());

    app.run_action(Action::PreflightExport, false);

    // The overlay shows the report.
    let shown = app
        .overlay_lines()
        .expect("preflight overlay is open")
        .iter()
        .map(|l| {
            l.spans
                .iter()
                .map(|s| s.content.as_ref())
                .collect::<String>()
        })
        .collect::<Vec<_>>()
        .join("\n");
    assert!(shown.contains("FAIL  no ISBN"), "no-ISBN fails:\n{shown}");
    assert!(
        shown.contains("FAIL  no cover image"),
        "no-cover fails:\n{shown}"
    );
    assert!(
        shown.contains("empty chapter(s): Empty"),
        "the empty chapter is named:\n{shown}"
    );

    // And it's saved as an auditable artifact under export/.
    let saved = fs::read_dir(dir.join("export"))
        .expect("export dir")
        .filter_map(Result::ok)
        .find(|e| e.file_name().to_string_lossy().starts_with("preflight-"))
        .expect("a preflight-<date>.md was written");
    let written = fs::read_to_string(saved.path()).unwrap();
    assert!(written.contains("# Export preflight — Flawed"));
    assert!(written.contains("FAIL  no ISBN"));
    assert!(written.contains("empty chapter(s): Empty"));
}

/// A fully-specified book with a high-resolution cover and prose in every chapter
/// passes the preflight — no failures.
#[test]
fn preflight_passes_a_clean_book() {
    let dir = app_scratch_dir("action-preflight-clean");
    fs::write(dir.join("cover.png"), png_bytes(3000, 4500)).unwrap();
    fs::write(
        Project::manifest_path(&dir),
        "title = \"Clean\"\n\
         isbn = \"978-1-234-56789-7\"\n\
         publisher = \"Quill Press\"\n\
         pub_date = \"2026\"\n\
         language = \"en-US\"\n\
         rights = \"© 2026 A. Writer.\"\n\n\
         [style]\ncover_image = \"cover.png\"\n\n\
         [[chapters]]\ntitle = \"One\"\nfile = \"one.md\"\n\n\
         [[chapters]]\ntitle = \"Two\"\nfile = \"two.md\"\n",
    )
    .unwrap();
    fs::write(dir.join("one.md"), "A balanced *italic* opening.\n").unwrap();
    fs::write(dir.join("two.md"), "More prose to fill the chapter.\n").unwrap();
    let mut app = open_test_app(&dir);
    let _dir = TempProject::new(dir.clone());

    app.run_action(Action::PreflightExport, false);

    let saved = fs::read_dir(dir.join("export"))
        .expect("export dir")
        .filter_map(Result::ok)
        .find(|e| e.file_name().to_string_lossy().starts_with("preflight-"))
        .expect("a preflight report was written");
    let written = fs::read_to_string(saved.path()).unwrap();
    assert!(
        !written.contains("FAIL"),
        "a clean book has no failures:\n{written}"
    );
    assert!(
        written.contains("0 failure(s)"),
        "summary is clean:\n{written}"
    );
}

/// A tiny solid-red PNG of `w`×`h` — a real raster the cover DPI check decodes.
fn png_bytes(w: u32, h: u32) -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(w, h, image::Rgba([255, 0, 0, 255]));
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(img)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

/// A blank entry cancels — nothing is stored, no follow-up prompt.
#[test]
fn entering_a_blank_key_cancels() {
    let (mut app, _dir) = two_chapter_app("action-set-key-blank");
    app.run_action(Action::SetClaudeKey, false);
    app.on_key(key(KeyCode::Enter));
    assert!(
        app.message.as_deref().unwrap_or("").contains("Cancelled"),
        "blank entry cancels, got: {:?}",
        app.message
    );
    assert!(!matches!(app.mode, Mode::Prompt(_)), "prompt closed");
}

/// In test builds the keychain is a no-op (#531), so entering a key reports the
/// failure and points the author at the env-var escape hatch rather than writing
/// to (or reading from) the developer's real keychain. This proves the suite is
/// hermetic against the OS store while still exercising the entry path.
#[test]
fn entering_a_key_in_tests_never_touches_the_real_keychain() {
    let (mut app, _dir) = two_chapter_app("action-set-key-hermetic");
    app.run_action(Action::SetImageKey, false);
    for c in "sk-test-123".chars() {
        app.on_key(key(KeyCode::Char(c)));
    }
    app.on_key(key(KeyCode::Enter));
    let msg = app.message.as_deref().unwrap_or("");
    assert!(
        msg.contains("FAL_KEY"),
        "points to the env-var escape hatch when the keychain can't be written, got: {msg:?}"
    );
    // The config file must not have been written with the secret either.
    if let Some(path) = crate::config::config_path() {
        if let Ok(text) = std::fs::read_to_string(path) {
            assert!(
                !text.contains("sk-test-123"),
                "secret never lands in config"
            );
        }
    }
}

/// Stripping the plaintext key removes the `api_key` line from the config section
/// and reloads the settings, so the running app stops reading the stale key.
#[test]
fn strip_plaintext_key_removes_it_from_the_config() {
    let (mut app, _dir) = two_chapter_app("action-strip-key");
    isolate("strip-key"); // point config resolution at a scratch dir for this test
    let path = crate::config::config_path().expect("config path in test");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "[ai]\nenabled = true\napi_key = \"sk-plaintext\"\n").unwrap();
    app.ai = ai::Settings::load();
    assert_eq!(app.ai.api_key.as_deref(), Some("sk-plaintext"));

    app.strip_plaintext_key(Key::Anthropic);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        !text.contains("sk-plaintext"),
        "plaintext key stripped: {text}"
    );
    assert!(text.contains("enabled = true"), "other keys kept: {text}");
    assert!(
        app.ai.api_key.is_none(),
        "settings reloaded without the key"
    );
}
