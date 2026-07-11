//! Line-level, speaker-attributed voice check (#1050): deepens the #183 voice pass
//! so a *specific* dialogue line that reads out of character for its speaker given
//! the scene lands as a finding naming the speaker + chapter. Findings only — it
//! flags the line, it never rewrites it. Hermetic throughout: a canned mock backend
//! drives the pass, so no test makes a live LLM call, hits the network, downloads a
//! model, or reads the keychain (project rule #531). The pass grounds on the
//! character bible's voice profiles (#183/#74), which the tests seed on disk.

use super::*;

use std::sync::Mutex;

use crate::ai::voice::{self, Voice, VoiceFinding};
use crate::ai::Backend;
use crate::mode::{Action, AiAction, FindingKind};

/// A backend that returns a canned reply (or an error) with no network — lets the
/// voice pass be driven deterministically and offline. It records the last request
/// it saw, so a test can assert the voice profiles reached the model and that the
/// prompt forbids a rewrite.
struct MockBackend {
    reply: Result<String, ()>,
    /// The user message of the last request seen, so a test can assert the voice
    /// profiles reached the model (`Request` isn't `Clone`, so we capture the field).
    seen_user: Mutex<Option<String>>,
}

impl MockBackend {
    fn ok(reply: &str) -> Self {
        Self {
            reply: Ok(reply.to_string()),
            seen_user: Mutex::new(None),
        }
    }
}

impl Backend for MockBackend {
    fn complete(&self, req: &ai::Request) -> Result<ai::Response, ai::AiError> {
        *self.seen_user.lock().unwrap() = Some(req.user.clone());
        match &self.reply {
            Ok(text) => Ok(ai::Response {
                text: text.clone(),
                usage: ai::Usage::default(),
            }),
            Err(()) => Err(ai::AiError::Network {
                status: None,
                message: "boom".into(),
            }),
        }
    }
}

/// Seed a character voice profile on disk so the pass has voice profiles to ground
/// on (its precondition is a `## Voice` section, not merely a character file). Mara
/// is clipped and formal, so a slangy line is out of character for her.
fn seed_voice_profile(dir: &Path) {
    let chars = dir.join("bible").join("characters");
    fs::create_dir_all(&chars).unwrap();
    fs::write(
        chars.join("mara.md"),
        "name: Mara\n\n## Voice\n- clipped, dry, formal; never uses slang\n",
    )
    .unwrap();
}

/// Drive the voice pass core with a mock backend the way the async worker path does:
/// read the seeded voice profiles, build the request from the manuscript + those
/// profiles, complete it, parse, and apply the result. Returns the mock so the
/// caller can inspect the request it saw.
fn run_voice(app: &mut App, dir: &Path, reply: &str) -> MockBackend {
    let backend = MockBackend::ok(reply);
    let notes = crate::ai::bible::raw_dir(dir, "characters");
    let req = voice::build_request(app.manuscript_text(), &notes, &app.ai.model);
    let parsed = backend.complete(&req).and_then(|r| voice::parse(&r.text));
    app.apply_voice(parsed);
    backend
}

// --- A line-level dialogue flag names the speaker + chapter -------------------

#[test]
fn an_out_of_character_line_lands_naming_the_speaker_and_chapter() {
    let (mut app, dir) = two_chapter_app("voice-line");
    seed_voice_profile(&dir);
    let mock = run_voice(
        &mut app,
        &dir,
        r#"{"findings":[
            {"kind":"dialogue","character":"Mara","line":"Yeah, whatever, dude.",
             "detail":"slangy — her profile is clipped and formal","chapters":["Two"]}
        ]}"#,
    );

    // The voice profiles reached the model (grounding), and the manuscript is the
    // cached part.
    let seen_user = mock.seen_user.lock().unwrap().clone().unwrap_or_default();
    assert!(
        seen_user.contains("clipped, dry, formal"),
        "voice profiles ground the pass: {seen_user}"
    );

    match &app.mode {
        Mode::Findings(f) => {
            assert_eq!(f.items.len(), 1);
            let item = &f.items[0];
            // Names the speaker.
            assert!(
                item.summary.contains("Mara"),
                "names the speaker: {}",
                item.summary
            );
            // Quotes the specific spoken line so the author lands on it.
            assert!(
                item.summary.contains("Yeah, whatever, dude."),
                "quotes the flagged line: {}",
                item.summary
            );
            // Explains what's off, referencing the voice profile.
            assert!(
                item.summary.contains("slangy"),
                "the why-note: {}",
                item.summary
            );
            // Anchored to its chapter.
            assert_eq!(item.chapters, ["Two"], "names the chapter");
            // Classified under the voice kind (reuses #183's kind/action — no new
            // enum entry), so the health dashboard groups it with the voice pass.
            assert_eq!(item.kind, FindingKind::Voice);
            assert_eq!(item.action, Some(AiAction::Voice));
        }
        _ => panic!("expected the findings overlay"),
    }
    // It feeds revision triage like every other analysis pass.
    assert_eq!(app.triage.len(), 1);
    assert_eq!(app.triage[0].kind, FindingKind::Voice);
}

// --- In-character dialogue yields no line-level flags ------------------------

#[test]
fn in_character_dialogue_yields_no_line_level_flags() {
    let (mut app, dir) = two_chapter_app("voice-line-clean");
    seed_voice_profile(&dir);
    // The model, biased toward precision, returns no findings for dialogue that reads
    // in character.
    run_voice(&mut app, &dir, r#"{"findings":[]}"#);
    // An empty result is a clean "no findings" state, not a findings list.
    assert!(app.overlay.is_some(), "a clean summary overlay opens");
    assert!(!matches!(app.mode, Mode::Findings(_)));
    assert!(app.triage.is_empty(), "nothing enters triage");
}

// --- Findings-only: it flags the line, never rewrites it ---------------------

#[test]
fn the_line_check_never_touches_the_manuscript() {
    let (mut app, dir) = two_chapter_app("voice-line-safe");
    seed_voice_profile(&dir);
    let before = fs::read_to_string(dir.join("two.md")).unwrap();
    run_voice(
        &mut app,
        &dir,
        r#"{"findings":[{"kind":"dialogue","character":"Mara","line":"Yo.",
            "detail":"out of register","chapters":["Two"]}]}"#,
    );
    assert_eq!(
        fs::read_to_string(dir.join("two.md")).unwrap(),
        before,
        "a voice line check flags, it never edits chapter prose"
    );
}

#[test]
fn the_prompt_and_schema_carry_no_replacement_line() {
    // The pass flags the line by construction: its prompt tells the model to observe
    // and quote the line for locating — never a rewrite — and its schema has no field
    // that could carry a replacement line, only observation fields.
    let req = voice::build_request("m".into(), "Mara: clipped", "claude-opus-4-8");
    let system = req.system.as_deref().unwrap_or("");
    assert!(system.contains("Do NOT rewrite"), "flags only: {system}");
    assert!(
        system.contains("do NOT put any corrected"),
        "the quoted line is for locating, not a rewrite: {system}"
    );
    // Line-level, speaker-attributed, scene-aware, precision-biased.
    assert!(system.contains("SPECIFIC line"), "{system}");
    assert!(system.contains("bias toward precision"), "{system}");

    let schema = voice::schema();
    let keys: std::collections::BTreeSet<&str> = schema["properties"]["findings"]["items"]
        ["properties"]
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        ["chapters", "character", "detail", "kind", "line"]
            .into_iter()
            .collect::<std::collections::BTreeSet<_>>(),
        "no field carries a replacement line — only observation fields: {keys:?}"
    );
}

// --- Allowlisted read-only analysis (reuses #183's action, no new enum entry) -

#[test]
fn voice_is_an_allowlisted_read_only_analysis_action() {
    // The line-level deepening rides the existing voice action: it produces findings,
    // no prose, so the assistant may run it, and it carries no free-text argument
    // (it grounds on the bible's voice profiles directly).
    let action = Action::Ai(AiAction::Voice);
    assert!(
        action.assistant_runnable(),
        "read-only voice analysis is allowlisted"
    );
    assert_eq!(
        action.text_arg(),
        None,
        "voice grounds on the bible, no free-text arg"
    );

    // It round-trips by its stable slug — the same registry action as #183.
    assert_eq!(action.name(), "ai:voice");
    assert!(matches!(
        Action::from_name("ai:voice"),
        Some(Action::Ai(AiAction::Voice))
    ));
}

// A typed-result construction exercising the line-level finding shape end-to-end,
// mirroring the accuracy/style pass tests (apply the parsed result directly).
#[test]
fn apply_voice_maps_the_line_level_finding_shape() {
    let (mut app, _dir) = two_chapter_app("voice-line-typed");
    app.apply_voice(Ok(Voice {
        findings: vec![VoiceFinding {
            kind: "dialogue".into(),
            character: "Ivo".into(),
            line: "I reckon we oughta skedaddle.".into(),
            detail: "his profile is precise and courtly, not folksy".into(),
            chapters: vec!["One".into()],
        }],
    }));
    match &app.mode {
        Mode::Findings(f) => {
            assert!(f.items[0].summary.contains("Ivo"));
            assert!(f.items[0].summary.contains("skedaddle"));
            assert_eq!(f.items[0].chapters, ["One"]);
        }
        _ => panic!("expected the findings overlay"),
    }
}
