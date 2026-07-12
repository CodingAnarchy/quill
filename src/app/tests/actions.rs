//! Tests for the unified action registry and the run-action menu (#225): the
//! registry enumerates every invokable action with a stable name, the menu lists
//! them, and `run_action` routes each through its existing code path while
//! surfacing why an unavailable action can't run.

use super::*;
use crate::ai;
use crate::ai::keychain::Key;
use crate::mode::{Action, AiAction, Command, ExportFormat, PaletteRow, Pending};

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

/// The run-action palette lists exactly the *applicable* actions (#944): every
/// item is one whose `action_status` is `None`, and inapplicable ones are absent.
#[test]
fn action_palette_lists_only_applicable_actions() {
    let (mut app, _dir) = two_chapter_app("action-palette");
    app.open_action_palette();
    match &app.mode {
        Mode::Palette(p) => {
            let applicable = Action::ALL
                .iter()
                .filter(|&&a| app.action_status(a).is_none())
                .count();
            assert_eq!(
                p.items.len(),
                applicable,
                "only applicable actions are listed"
            );
            assert!(
                p.items.iter().all(|i| match i.command {
                    Command::Run(a) => app.action_status(a).is_none(),
                    _ => false,
                }),
                "every listed action is runnable right now"
            );
        }
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
fn bible_and_reference_views_group_under_bible_not_ai_or_view() {
    // The bible browser and research notebook are local reference-file views (not AI
    // operations), and the corkboards are structural views — all four live under the
    // "Bible" group, out of "AI"/"View" (#590 follow-up).
    for a in [
        Action::BrowseBible,
        Action::ViewResearch,
        Action::Corkboard,
        Action::ManuscriptCorkboard,
    ] {
        assert_eq!(a.group(), "Bible", "{a:?} should be in the Bible group");
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
    // Groups open collapsed by default now (#943); expand Project first (→), so
    // there's something for ← to collapse back.
    app.on_key(key(KeyCode::Right));
    let before = match &app.mode {
        Mode::Palette(p) => {
            assert!(!p.collapsed.contains("Project"), "→ expanded Project");
            p.rows().len()
        }
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
    // Project opens collapsed (#943), so the first Enter expands it.
    app.on_key(key(KeyCode::Enter)); // on the Project header
    assert!(
        matches!(app.mode, Mode::Palette(_)),
        "Enter on a header keeps the palette open"
    );
    let Mode::Palette(p) = &app.mode else {
        panic!();
    };
    assert!(!p.collapsed.contains("Project"), "Enter expands it");
    app.on_key(key(KeyCode::Enter)); // toggles back collapsed
    let Mode::Palette(p) = &app.mode else {
        panic!();
    };
    assert!(p.collapsed.contains("Project"), "Enter collapses it again");
}

/// The stable names of the actions the run-action palette currently offers — the
/// searchable set, independent of which groups are collapsed. Names (not `Action`,
/// which isn't `PartialEq`) so tests can assert membership.
fn palette_action_names(app: &App) -> Vec<&'static str> {
    let Mode::Palette(p) = &app.mode else {
        panic!("expected the run-action palette");
    };
    p.items
        .iter()
        .filter_map(|i| match i.command {
            Command::Run(a) => Some(a.name()),
            _ => None,
        })
        .collect()
}

// --- #944: hide inapplicable actions ---------------------------------------

/// Inapplicable actions are *absent* from the palette, not tagged "unavailable"
/// (#944): with AI off, the AI passes and a series action drop out, while an
/// applicable action (Enable AI) and the always-shown `SwitchBackend` remain.
#[test]
fn the_action_palette_hides_inapplicable_actions() {
    let (mut app, _dir) = two_chapter_app("palette-hide-inapplicable");
    app.ai = ai::Settings::default(); // AI off, deterministic
    assert!(app.project.book_count() <= 1, "single-book fixture");
    app.open_action_palette();
    let actions = palette_action_names(&app);
    let has = |a: Action| actions.contains(&a.name());

    // Inapplicable with AI off / on a single book — hidden.
    assert!(
        !has(Action::Ai(AiAction::Characters)),
        "an AI pass is hidden while AI is off"
    );
    assert!(
        !has(Action::RefreshSeriesBible),
        "a series action is hidden on a single book"
    );
    // Applicable — present.
    assert!(has(Action::EnableAi), "Enable AI is shown while AI is off");
    assert!(has(Action::Save), "an always-applicable action is shown");
    // The backend switch has status None but is a special-case that must survive
    // the filter (#944/#371).
    assert!(has(Action::SwitchBackend), "SwitchBackend is still listed");
    // No listed action carries the old "unavailable" tag.
    let Mode::Palette(p) = &app.mode else {
        unreachable!();
    };
    assert!(
        p.items.iter().all(|i| !i.desc.contains("unavailable")),
        "no dead 'unavailable' rows remain"
    );
}

/// An action reappears once it becomes applicable: Enable AI is shown while AI is
/// off and gone once AI is on, and an AI pass makes the opposite transition —
/// filtering follows the live `action_status` (#944).
#[test]
fn hidden_actions_reappear_when_they_become_applicable() {
    let (mut app, _dir) = two_chapter_app("palette-reappear");
    app.ai = ai::Settings::default(); // off
    app.open_action_palette();
    let off = palette_action_names(&app);
    assert!(
        off.contains(&Action::EnableAi.name()),
        "Enable AI shown while off"
    );

    // Turn AI on: Enable AI drops out, and the local AI actions stay/come in.
    app.ai.enabled = true;
    app.open_action_palette();
    let on = palette_action_names(&app);
    assert!(
        !on.contains(&Action::EnableAi.name()),
        "Enable AI is hidden once AI is enabled"
    );
    assert!(
        on.contains(&Action::Ai(AiAction::Triage).name()),
        "a local AI action is shown with AI on"
    );
}

// --- #943: remember collapse state + open collapsed ------------------------

/// A fresh run-action palette opens with every group collapsed (#943): the
/// visible rows are all group headers, no items.
#[test]
fn the_action_palette_opens_with_every_group_collapsed() {
    let (mut app, _dir) = two_chapter_app("palette-open-collapsed");
    app.open_action_palette();
    let Mode::Palette(p) = &app.mode else {
        panic!("expected the run-action palette");
    };
    // Every group present in the items is in the collapsed set.
    let groups: std::collections::HashSet<&str> = p.items.iter().filter_map(|i| i.group).collect();
    assert!(!groups.is_empty(), "the palette has groups");
    assert!(
        groups.iter().all(|g| p.collapsed.contains(g)),
        "every group opens collapsed"
    );
    // So the visible rows are headers only — a short, scannable menu.
    assert!(
        p.rows()
            .iter()
            .all(|r| matches!(r, PaletteRow::Header { .. })),
        "the initial view is group headers, not the full action list"
    );
}

/// Collapse state persists across opens (#943): a group the author expands stays
/// expanded the next time the palette opens, rather than resetting to collapsed.
#[test]
fn collapse_state_persists_across_opens() {
    let (mut app, _dir) = two_chapter_app("palette-persist");
    app.open_action_palette();
    // Expand the Project group (selected header, →), then close the palette.
    app.on_key(key(KeyCode::Right));
    let Mode::Palette(p) = &app.mode else {
        panic!();
    };
    assert!(!p.collapsed.contains("Project"), "expanded Project");
    app.on_key(key(KeyCode::Esc));

    // Re-open: Project is remembered expanded, not reset to collapsed.
    app.open_action_palette();
    let Mode::Palette(p) = &app.mode else {
        panic!();
    };
    assert!(
        !p.collapsed.contains("Project"),
        "the expanded group is remembered on the next open"
    );

    // Collapse it again and confirm the collapse is likewise remembered.
    app.on_key(key(KeyCode::Left));
    app.on_key(key(KeyCode::Esc));
    app.open_action_palette();
    let Mode::Palette(p) = &app.mode else {
        panic!();
    };
    assert!(
        p.collapsed.contains("Project"),
        "re-collapsing is remembered too"
    );
}

/// Collapsing a group never hides an action from a filter (#943): `items` — the
/// set a typed filter searches — holds every applicable action regardless of the
/// collapse state, so a match can still surface it.
#[test]
fn collapse_state_does_not_remove_items_from_the_searchable_set() {
    let (mut app, _dir) = two_chapter_app("palette-filter");
    app.ai = ai::Settings::default();
    app.open_action_palette(); // all collapsed by default
    let collapsed_items = palette_action_names(&app);
    // Expand every group and re-read: the item set is identical.
    let groups: Vec<&'static str> = {
        let Mode::Palette(p) = &mut app.mode else {
            panic!();
        };
        let gs: Vec<&'static str> = p.collapsed.iter().copied().collect();
        for g in &gs {
            p.set_collapsed(g, false);
        }
        gs
    };
    assert!(!groups.is_empty(), "there were collapsed groups");
    let expanded_items = palette_action_names(&app);
    assert_eq!(
        collapsed_items, expanded_items,
        "the searchable item set is the same whether groups are collapsed or not"
    );
    // And it holds the applicable actions a filter would match.
    assert!(expanded_items.contains(&Action::Save.name()));
    assert!(expanded_items.contains(&Action::EnableAi.name()));
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

/// The run-action palette carries each action's shortcut on the item (#1103): an
/// action with a binding gets its key, a palette-only action gets `None`. The item
/// is the single carrier the render reads, so this pins what the cheat sheet shows.
#[test]
fn palette_items_carry_the_action_shortcut() {
    let (mut app, _dir) = two_chapter_app("action-palette-shortcut");
    app.open_action_palette();
    let Mode::Palette(p) = &app.mode else {
        panic!("the run-action palette opens");
    };
    let shortcut_of = |name: &str| {
        p.items
            .iter()
            .find(|i| match i.command {
                Command::Run(a) => a.name() == name,
                _ => false,
            })
            .map(|i| i.shortcut)
    };
    // An action with a dedicated binding carries it; a palette-only one carries none.
    assert_eq!(shortcut_of("save"), Some(Some("Ctrl-S")));
    assert_eq!(shortcut_of("report:progress"), Some(Some("Ctrl-R")));
    assert_eq!(shortcut_of("goals:set"), Some(None));
    assert_eq!(shortcut_of("goto:line"), Some(None));
}

/// The shortcut the palette shows is the *real* dispatch binding (#1103): the
/// `Ctrl-`_letter_ dispatch resolves through `Action::from_ctrl_key`, and every key
/// it names round-trips back to that same key through `Action::shortcut` — so a
/// shown shortcut can't drift from what actually runs. This is the single-source
/// guarantee.
#[test]
fn shortcut_matches_the_dispatch_binding() {
    for c in 'a'..='z' {
        if let Some(action) = Action::from_ctrl_key(c) {
            let want = format!("Ctrl-{}", c.to_ascii_uppercase());
            assert_eq!(
                action.shortcut(),
                Some(want.as_str()),
                "Ctrl-{c} dispatches {action:?}, so its shown shortcut must be {want}"
            );
        }
    }
    // The one non-letter Ctrl binding: Ctrl-\ toggles composition mode.
    assert!(
        matches!(Action::from_ctrl_key('\\'), Some(Action::ToggleComposition)),
        "Ctrl-\\ resolves to the composition toggle"
    );
    assert_eq!(Action::ToggleComposition.shortcut(), Some("Ctrl-\\"));
    // A key with no action resolves to nothing (Ctrl-B browses projects — no Action).
    assert!(Action::from_ctrl_key('b').is_none());

    // The other direction: every `Ctrl-`_letter_ shortcut a registry action *shows*
    // must round-trip back to that key through `from_ctrl_key` (the dispatch table),
    // so nothing advertises a Ctrl-letter binding that dispatch doesn't honor.
    for &a in Action::ALL {
        if let Some(sc) = a.shortcut() {
            if let Some(letter) = sc.strip_prefix("Ctrl-").and_then(|s| {
                let mut it = s.chars();
                match (it.next(), it.next()) {
                    (Some(c), None) if c.is_ascii_alphabetic() => Some(c.to_ascii_lowercase()),
                    _ => None,
                }
            }) {
                assert!(
                    Action::from_ctrl_key(letter).is_some(),
                    "{a:?} shows {sc}, but Ctrl-{letter} maps to no action in dispatch"
                );
            }
        }
    }
}

/// F1 is the shown shortcut for the user's guide (#1103) and really opens it — the
/// one function-key binding, dispatched in `on_key` rather than `on_key_ctrl`.
#[test]
fn f1_shortcut_opens_the_user_guide() {
    let (mut app, _dir) = two_chapter_app("action-f1-guide");
    assert_eq!(Action::UserGuide.shortcut(), Some("F1"));
    app.on_key(key(KeyCode::F(1)));
    assert!(app.overlay.is_some(), "F1 opened the user's guide overlay");
}

/// Pressing an action's shown shortcut actually runs that action (#1103) — the
/// live proof the cheat sheet doesn't lie. Ctrl-S saves; Ctrl-R opens the progress
/// report. Driven through the real top-level key path, not `dispatch_action`.
#[test]
fn pressing_a_shown_shortcut_runs_the_action() {
    let (mut app, _dir) = two_chapter_app("action-shortcut-live");
    // Ctrl-S (Save.shortcut()) clears the dirty flag, exactly like the palette's Save.
    app.on_key(key(KeyCode::Char('X')));
    assert!(app.buffer.dirty);
    assert_eq!(Action::Save.shortcut(), Some("Ctrl-S"));
    app.on_key(ctrl('s'));
    assert!(
        !app.buffer.dirty,
        "Ctrl-S saved, like the shown shortcut says"
    );
    // Ctrl-R (ProgressReport.shortcut()) opens the same overlay the action does.
    assert_eq!(Action::ProgressReport.shortcut(), Some("Ctrl-R"));
    app.on_key(ctrl('r'));
    assert!(app.overlay.is_some(), "Ctrl-R opened the progress report");
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

/// Review-copy export (#834): registered under Export, and dispatching it writes
/// a `.docx` whose section carries line numbering and whose body carries the
/// author's annotation as an anchored Word comment (with a `comments.xml` part).
#[test]
fn review_copy_action_writes_a_numbered_docx_with_annotation_comments() {
    use std::io::{Cursor, Read};

    assert!(matches!(
        Action::from_name("export:review"),
        Some(Action::ReviewCopy)
    ));
    assert_eq!(Action::ReviewCopy.group(), "Export");

    let (mut app, dir) = two_chapter_app("action-review-copy");
    // Chapter one is "first chapter\n"; annotate "first" (bytes 0..5).
    app.create_annotation(0, 5, "check the opening".into());

    app.run_action(Action::ReviewCopy, false);
    assert!(
        app.message.as_deref().unwrap_or("").contains("Exported to"),
        "{:?}",
        app.message
    );
    let path = dir.join("export").join("book-review.review.docx");
    assert!(path.exists(), "review copy written");

    let read_part = |name: &str| {
        let bytes = fs::read(&path).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(bytes)).unwrap();
        let mut file = zip.by_name(name).unwrap();
        let mut s = String::new();
        file.read_to_string(&mut s).unwrap();
        s
    };
    let doc = read_part("word/document.xml");
    assert!(
        doc.contains("<w:lnNumType"),
        "line numbering on the section: {doc}"
    );
    assert!(
        doc.contains("<w:commentRangeStart") && doc.contains("<w:commentReference"),
        "the annotation is anchored as an in-body comment: {doc}"
    );
    let comments = read_part("word/comments.xml");
    assert!(
        comments.contains("check the opening"),
        "the note reaches comments.xml: {comments}"
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
