//! On-demand scope-to-selection for the continuity & setting passes (#1054): an
//! author scopes a pass to an editor selection / chapter range so revising one
//! scene re-verifies just that scene. Narrows the input only — same finding kind,
//! no prose, cheaper. Hermetic throughout: a canned mock backend drives the pass,
//! so no test makes a live LLM call, hits the network, downloads a model, or reads
//! the keychain (project rule #531).

use super::*;

use std::fs;
use std::sync::Mutex;

use crate::ai::Backend;
use crate::mode::{AiAction, FindingKind};

/// A backend that returns a canned reply with no network, recording the last
/// request's cached context so a test can assert exactly which chapters were sent.
struct MockBackend {
    reply: String,
    seen_context: Mutex<Option<String>>,
}

impl MockBackend {
    fn ok(reply: &str) -> Self {
        Self {
            reply: reply.to_string(),
            seen_context: Mutex::new(None),
        }
    }
}

impl Backend for MockBackend {
    fn complete(&self, req: &ai::Request) -> Result<ai::Response, ai::AiError> {
        *self.seen_context.lock().unwrap() = Some(req.cached_context.clone());
        Ok(ai::Response {
            text: self.reply.clone(),
            usage: ai::Usage::default(),
        })
    }
}

/// Write a prior timeline bible so a scoped continuity pass has a bible to revise
/// (the incremental-revise path #90 the scoped pass reuses).
fn write_timeline_bible(dir: &std::path::Path) {
    let bible = dir.join("bible");
    fs::create_dir_all(&bible).unwrap();
    fs::write(
        bible.join("timeline.md"),
        "---\ntitle: Timeline\n---\n\n<!-- generated:start -->\n\
         ## Timeline (narrative order)\n- **One:** alpha event\n<!-- generated:end -->\n",
    )
    .unwrap();
}

/// A continuity JSON reply carrying one finding on `chapter` — the same schema a
/// full pass returns, so a scoped run yields the same kind of findings.
fn continuity_json(chapter: &str, detail: &str) -> String {
    format!(
        r#"{{"events":[],"findings":[{{"kind":"object","detail":"{detail}","chapters":["{chapter}"]}}]}}"#
    )
}

// --- Scope resolution from the editor ---------------------------------------

#[test]
fn a_caret_in_the_single_chapter_view_scopes_to_the_active_chapter() {
    // No continuous view, no selection — just the cursor sitting in a chapter. The
    // on-demand scope is "this scene": the active chapter, not the whole book.
    let (mut app, _dir) = two_chapter_app("scope-caret-single");
    app.on_key(ctrl('n')); // active → Two
    let scope = app
        .selection_pass_scope()
        .expect("the cursor's chapter scopes");
    assert_eq!(scope, vec!["two.md".to_string()]);
}

#[test]
fn a_selection_in_the_continuous_view_scopes_to_the_spanned_chapters() {
    // In the continuous view the buffer concatenates several chapters. A Visual
    // selection wholly inside the first chapter scopes to just it — the whole point
    // of narrowing a pass to one scene mid-manuscript.
    let (mut app, _dir) = three_chapter_app("scope-visual-cont"); // alpha / beta / gamma
    app.on_key(ctrl('l')); // enter continuous view
    assert!(app.manuscript.is_some());

    // Select the first few bytes (inside chapter c0), then resolve the scope while
    // still in Visual — exactly what `open_ai_palette` does before the palette opens.
    let spans = app.manuscript.as_ref().unwrap().segment_spans();
    let (_idx, c0_start, c0_end) = spans[0];
    app.mode = Mode::Visual {
        anchor: c0_start,
        linewise: false,
    };
    let buf = app.edit_buffer_mut();
    buf.cursor = buf.cursor_at_byte((c0_start + 1).min(c0_end.saturating_sub(1)));

    let scope = app.selection_pass_scope().expect("the selection scopes");
    assert_eq!(
        scope,
        vec!["c0.md".to_string()],
        "a selection inside the first loaded chapter scopes to it alone"
    );
}

#[test]
fn no_selection_and_capturing_the_palette_scope_reflects_the_cursor() {
    // Opening the AI palette captures the on-demand scope from the live editor
    // state; a plain caret captures the active chapter as the scope.
    let (mut app, _dir) = two_chapter_app("scope-capture");
    app.open_ai_palette();
    assert_eq!(
        app.pass_scope,
        Some(vec!["one.md".to_string()]),
        "the palette captured the cursor's chapter as the scope"
    );
}

// --- A scoped pass narrows input, keeps the finding kind ---------------------

#[test]
fn a_scoped_continuity_pass_sends_only_the_scoped_chapter() {
    // A scope of chapter Two must send only Two's prose, never One's — the input
    // narrowing that makes the pass cheap and focused.
    let (mut app, dir) = two_chapter_app("scope-req-continuity");
    write_timeline_bible(&dir);
    app.pass_scope = Some(vec!["two.md".to_string()]);

    let (req, _parse) = app
        .scoped_pass_request(AiAction::Continuity)
        .expect("a scoped continuity request builds");
    assert!(
        req.cached_context.contains("second chapter"),
        "the scoped chapter is sent: {}",
        req.cached_context
    );
    assert!(
        !req.cached_context.contains("first chapter"),
        "the out-of-scope chapter is not sent: {}",
        req.cached_context
    );
}

#[test]
fn a_scoped_pass_produces_the_same_kind_of_finding_as_the_full_pass() {
    // End to end through a mock backend: a scoped continuity pass parses to the same
    // Continuity finding kind a full pass yields — scoping narrows input, never the
    // output contract.
    let (mut app, dir) = two_chapter_app("scope-samekind");
    write_timeline_bible(&dir);
    app.pass_scope = Some(vec!["two.md".to_string()]);

    let (req, to_findings) = app
        .scoped_pass_request(AiAction::Continuity)
        .expect("a scoped continuity request builds");
    let backend = MockBackend::ok(&continuity_json("Two", "the locket changes color"));
    let findings = backend
        .complete(&req)
        .map(|r| to_findings(&r.text, AiAction::Continuity, Some(0)))
        .expect("the mock completes");

    // Only the scoped chapter was in the request.
    let context = backend
        .seen_context
        .lock()
        .unwrap()
        .clone()
        .unwrap_or_default();
    assert!(
        context.contains("second chapter") && !context.contains("first chapter"),
        "only the scoped chapter reached the model: {context}"
    );

    app.apply_scoped_pass(Ok(findings));
    match &app.mode {
        Mode::Findings(f) => {
            assert_eq!(f.items.len(), 1);
            assert_eq!(
                f.items[0].kind,
                FindingKind::Continuity,
                "the same finding kind the full pass produces"
            );
            assert_eq!(f.items[0].action, Some(AiAction::Continuity));
            assert_eq!(f.items[0].chapters, ["Two"]);
        }
        _ => panic!("a scoped pass surfaces findings like any pass"),
    }
    assert_eq!(app.triage.len(), 1, "the finding enters revision triage");
    assert_eq!(app.triage[0].kind, FindingKind::Continuity);
}

#[test]
fn a_scoped_setting_pass_also_narrows_and_keeps_its_finding_kind() {
    // The setting pass scopes the same way, into its own RuleFinding kind.
    let (mut app, dir) = two_chapter_app("scope-setting");
    let bible = dir.join("bible");
    fs::create_dir_all(&bible).unwrap();
    fs::write(
        bible.join("setting.md"),
        "---\ntitle: Setting\n---\n\n<!-- generated:start -->\n## Magic system\n<!-- generated:end -->\n",
    )
    .unwrap();
    app.pass_scope = Some(vec!["two.md".to_string()]);

    let (req, to_findings) = app
        .scoped_pass_request(AiAction::Setting)
        .expect("a scoped setting request builds");
    assert!(
        req.cached_context.contains("second chapter")
            && !req.cached_context.contains("first chapter"),
        "scoped to Two only: {}",
        req.cached_context
    );
    let reply = r#"{"categories":[],"gazetteer":[],"findings":[{"rule":"magic needs a focus","violation":"casts barehanded","chapters":["Two"]}]}"#;
    let backend = MockBackend::ok(reply);
    let findings = backend
        .complete(&req)
        .map(|r| to_findings(&r.text, AiAction::Setting, Some(0)))
        .unwrap();
    app.apply_scoped_pass(Ok(findings));
    match &app.mode {
        Mode::Findings(f) => {
            assert_eq!(f.items[0].kind, FindingKind::Setting);
            assert_eq!(f.items[0].action, Some(AiAction::Setting));
        }
        _ => panic!("expected setting findings"),
    }
}

// --- Cost reflects the reduced scope ----------------------------------------

/// A three-chapter project with substantial, distinct prose per chapter and a
/// small prior timeline bible — so scoping to one chapter is a clear input saving
/// over the whole-book pass (the bible overhead is dwarfed by the chapter prose).
fn big_book_with_bible(name: &str) -> (App, TempProject) {
    let dir = app_scratch_dir(name);
    let mut manifest = String::from("title = \"Book\"\n\n");
    for i in 0..3 {
        write!(
            manifest,
            "[[chapters]]\ntitle = \"C{i}\"\nfile = \"c{i}.md\"\n\n"
        )
        .unwrap();
        // A few hundred chars of distinct prose per chapter.
        let body = format!("Chapter {i} prose. ").repeat(30);
        fs::write(dir.join(format!("c{i}.md")), format!("{body}\n")).unwrap();
    }
    fs::write(Project::manifest_path(&dir), manifest).unwrap();
    write_timeline_bible(&dir);
    let app = open_test_app(&dir);
    (app, TempProject::new(dir))
}

#[test]
fn a_scoped_pass_costs_less_than_the_full_pass() {
    // The pre-flight estimate must reflect the narrowed input: a scope of one
    // chapter estimates a smaller context than the whole-book pass, so the author
    // sees the cheaper figure before running (#1054).
    let (mut app, _dir) = big_book_with_bible("scope-cost");
    let action = AiAction::Continuity;

    let full = app.pass_context_len(action, false);
    app.pass_scope = Some(vec!["c1.md".to_string()]);
    let scoped = app.pass_context_len(action, false);
    assert!(
        scoped < full,
        "a scoped pass estimates a smaller context than the full pass: scoped {scoped} vs full {full}"
    );

    // And that translates to a lower dollar estimate at the same model / output bound.
    let model = &app.ai.model;
    let full_cost = ai::cost::estimate(full, ai::MAX_OUTPUT_TOKENS, model, false).dollars;
    let scoped_cost = ai::cost::estimate(scoped, ai::MAX_OUTPUT_TOKENS, model, false).dollars;
    assert!(
        scoped_cost < full_cost,
        "the scoped estimate is cheaper: {scoped_cost} vs {full_cost}"
    );
}

// --- Whole-book behavior is unchanged with no scope -------------------------

#[test]
fn no_scope_runs_the_whole_book_as_before() {
    // With no captured scope, the continuity estimate is the whole manuscript and
    // `analyze_continuity` runs the unchanged full/whole-book pass — the feature is
    // strictly additive.
    let (mut app, _dir) = two_chapter_app("scope-none");
    assert!(app.pass_scope.is_none());
    assert_eq!(
        app.pass_context_len(AiAction::Continuity, false),
        app.manuscript_text().len(),
        "no scope → the whole manuscript is estimated, exactly as before"
    );

    // Dispatching with no scope goes through the book bible pass, not the scoped
    // path, and spawns a request (no scope was consumed).
    app.analyze_continuity(false);
    assert!(app.pass_scope.is_none(), "nothing to consume");
}

#[test]
fn dispatching_a_scoped_pass_consumes_the_scope() {
    // Once a scoped pass is dispatched, the scope is taken — so a later pass with no
    // fresh selection reverts to the whole book rather than reusing a stale scope.
    let (mut app, dir) = two_chapter_app("scope-consumed");
    write_timeline_bible(&dir);
    app.pass_scope = Some(vec!["two.md".to_string()]);
    app.analyze_continuity(false);
    assert!(
        app.pass_scope.is_none(),
        "the scope is consumed on dispatch, so it can't leak into the next pass"
    );
}
