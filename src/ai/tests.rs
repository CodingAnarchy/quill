//! Unit tests for the shared `ai` helpers: schema builders (`object_schema`,
//! `chapters_finding`), settings/backend resolution, the `parse_json`
//! malformed-input matrix (#313/#319), and answer parsing.
//! Extracted from `mod.rs` to keep that file under the ~1,000-line convention.

use super::*;

/// The `[ai]` section wrapper — used by the tests that parse a config string
/// directly. (Loading from disk goes through `config::load_section`, #430.)
#[derive(Deserialize, Default)]
struct ConfigFile {
    #[serde(default)]
    ai: Settings,
}

#[test]
fn object_schema_builds_the_envelope() {
    let s = object_schema(
        vec![("name", string_field()), ("tags", string_array())],
        &["name"],
    );
    assert_eq!(s["type"], "object");
    assert_eq!(s["additionalProperties"], false);
    assert_eq!(s["required"], serde_json::json!(["name"]));
    assert_eq!(
        s["properties"]["name"],
        serde_json::json!({ "type": "string" })
    );
    assert_eq!(
        s["properties"]["tags"],
        serde_json::json!({ "type": "array", "items": { "type": "string" } })
    );
}

#[test]
fn object_schema_omits_required_when_empty() {
    // Matches a hand-written all-optional object that left the key out, so
    // constrained decoding is unchanged (#429).
    let s = object_schema(vec![("note", string_field())], &[]);
    assert!(s.get("required").is_none(), "no empty `required` key: {s}");
}

#[test]
fn chapters_finding_is_two_strings_plus_chapters() {
    let f = chapters_finding("kind", "detail");
    assert_eq!(
        f["required"],
        serde_json::json!(["kind", "detail", "chapters"])
    );
    assert_eq!(
        f["properties"]["kind"],
        serde_json::json!({ "type": "string" })
    );
    assert_eq!(
        f["properties"]["detail"],
        serde_json::json!({ "type": "string" })
    );
    assert_eq!(
        f["properties"]["chapters"],
        serde_json::json!({ "type": "array", "items": { "type": "string" } })
    );
    assert_eq!(f["additionalProperties"], false);
}

#[test]
fn settings_default_to_ai_off() {
    let s = Settings::default();
    assert!(!s.enabled);
    assert_eq!(s.model, "claude-opus-4-8");
    assert!(s.check_ready().is_err());
}

#[test]
fn settings_parse_from_config_toml() {
    let cfg: ConfigFile =
        toml::from_str("[ai]\nenabled = true\nmodel = \"claude-haiku-4-5\"\napi_key = \"sk-x\"\n")
            .unwrap();
    assert!(cfg.ai.enabled);
    assert_eq!(cfg.ai.model, "claude-haiku-4-5");
    assert_eq!(cfg.ai.resolved_key().as_deref(), Some("sk-x"));
    cfg.ai.check_ready().unwrap(); // enabled + key + online → ready
}

#[test]
fn check_ready_reports_why_not() {
    let mut s = Settings {
        enabled: true,
        ..Settings::default()
    };
    s.api_key = Some("k".into());
    s.check_ready().unwrap(); // Claude + key → ready
    s.api_key = None;
    assert!(s.check_ready().is_err()); // Claude, no key → not ready
}

#[test]
fn local_backend_is_ready_without_a_key_and_offline_forces_it() {
    // Local needs no key.
    let s = Settings {
        enabled: true,
        backend: BackendKind::Local,
        ..Settings::default()
    };
    assert_eq!(s.backend(), BackendKind::Local);
    s.check_ready().unwrap();
    // Offline forces local even with backend = claude and no key.
    let s = Settings {
        enabled: true,
        offline: true,
        ..Settings::default()
    };
    assert_eq!(s.backend(), BackendKind::Local);
    s.check_ready().unwrap();
    // Disabled is never ready, regardless of backend.
    let s = Settings {
        backend: BackendKind::Local,
        ..Settings::default()
    };
    assert!(s.check_ready().is_err());
}

#[test]
fn backend_parses_from_config() {
    let cfg: ConfigFile = toml::from_str("[ai]\nenabled = true\nbackend = \"local\"\n").unwrap();
    assert_eq!(cfg.ai.backend, BackendKind::Local);
}

#[test]
fn build_qa_request_carries_manuscript_and_question() {
    let req = build_qa_request(
        "# One\n\nThe locket gleamed.".into(),
        "where is the locket?",
        "claude-opus-4-8",
    );
    assert!(req.cached_context.contains("locket gleamed"));
    assert_eq!(req.user, "where is the locket?");
    assert_eq!(req.model, "claude-opus-4-8");
    assert!(req.system.is_some());
    // The reply is now schema-constrained so it carries jumpable citations (#609).
    let schema = req.json_schema.expect("Q&A is now structured");
    assert_eq!(
        schema["required"],
        serde_json::json!(["answer", "citations"])
    );
}

#[test]
fn retrieved_context_renders_passages_under_chapter_headings() {
    // Retrieved hits become the same `# Title\n\n<prose>` shape the whole-book
    // render uses, so the model cites the same chapter titles and hits jump (#831).
    let hits = vec![
        embed::Hit {
            passage: embed::Passage {
                title: "The Harbor".into(),
                file: "two.md".into(),
                text: "Ships crowded the harbor.".into(),
            },
            score: 0.9,
        },
        embed::Hit {
            passage: embed::Passage {
                title: "The Harbor".into(),
                file: "two.md".into(),
                text: "The tide came in.".into(),
            },
            score: 0.8,
        },
        embed::Hit {
            passage: embed::Passage {
                title: "The Locket".into(),
                file: "one.md".into(),
                text: "She held the locket.".into(),
            },
            score: 0.5,
        },
    ];
    let ctx = retrieved_context(&hits);
    // Both passages from the same chapter share one heading; the whole-book render
    // wraps under the same `# Title` heading so the citation titles match exactly.
    assert_eq!(ctx.matches("# The Harbor").count(), 1);
    assert!(ctx.contains("Ships crowded the harbor."));
    assert!(ctx.contains("The tide came in."));
    assert!(ctx.contains("# The Locket"));
    // A whole-book render feeds `build_qa_request` the same shape — one path.
    let req = build_qa_request(ctx, "where?", "m");
    assert!(req.cached_context.contains("# The Harbor"));
}

#[test]
fn parse_answer_reads_structured_reply_and_falls_back() {
    // Structured reply: answer prose plus cited chapter titles (#609).
    let a = parse_answer(r#"{"answer":"In Chapter One.","citations":["One","Two"]}"#);
    assert_eq!(a.answer, "In Chapter One.");
    assert_eq!(a.citations, vec!["One".to_string(), "Two".to_string()]);
    // A non-JSON reply (a mock, or a backend that ignores the schema) is taken
    // whole as the answer with no citations, so the flow never hard-fails.
    let plain = parse_answer("Just some prose.");
    assert_eq!(plain.answer, "Just some prose.");
    assert!(plain.citations.is_empty());
}

// --- parse_json malformed-input matrix (#313/#319) -----------------------
//
// `parse_json` centralizes every structured pass's malformed-reply handling
// (~7 passes delegate to it), but had no direct tests. It does a raw
// `serde_json::from_str` with NO fence/preamble stripping — matching
// `claude.rs::parse_response`, which concatenates the reply's text blocks
// verbatim. So the CURRENT contract is: only a bare JSON document parses; a
// markdown-fenced (```json … ```) or model-preamble-wrapped reply hard-fails
// as `ModelNonCompliance`. The passes request constrained JSON-schema decoding,
// so a compliant model returns bare JSON and this is the intended contract; the
// point of the matrix is that *every* malformed shape degrades gracefully to a
// `ModelNonCompliance` error (never a panic), which these assert.

/// A minimal target for the matrix — one required string field.
#[derive(serde::Deserialize, Debug, PartialEq)]
struct Probe {
    name: String,
}

fn parse_probe(text: &str) -> Result<Probe, AiError> {
    parse_json(text, "probe")
}

#[test]
fn parse_json_accepts_a_bare_valid_document() {
    let ok = parse_probe(r#"{"name":"Ada"}"#).expect("bare JSON parses");
    assert_eq!(ok.name, "Ada");
}

#[test]
fn parse_json_rejects_every_malformed_shape_as_model_non_compliance() {
    // Each of these is a reply a non-compliant model could send; every one must
    // surface as `ModelNonCompliance` (bad probe JSON), never a panic or a silent
    // default. This documents that fenced / preamble replies are NOT tolerated.
    let malformed = [
        ("markdown-fenced block", "```json\n{\"name\":\"Ada\"}\n```"),
        ("model preamble", "Here is the result: {\"name\":\"Ada\"}"),
        ("truncated / unterminated", r#"{"name":"Ada"#),
        ("wrong-typed field", r#"{"name":42}"#),
        ("empty input", ""),
        ("whitespace only", "   \n\t "),
        ("not JSON at all", "nope"),
    ];
    for (label, text) in malformed {
        match parse_probe(text) {
            Err(AiError::ModelNonCompliance(msg)) => {
                assert!(
                    msg.contains("bad probe JSON"),
                    "{label}: expected the uniform message, got {msg:?}"
                );
            }
            other => panic!("{label}: expected ModelNonCompliance, got {other:?}"),
        }
    }
}
