//! Character tracking: a structured pass over the manuscript that extracts a
//! profile per character (traits, relationships, goals, per-chapter arc) and
//! flags drift, written to the human-readable story bible
//! (`bible/characters/<slug>.md`). The first feature to use structured outputs;
//! its on-disk format is reused by the later trackers. See `docs/ai-features.md`.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{bible, incremental, AiError, Request, MAX_OUTPUT_TOKENS};

/// One character's profile. The serde fields the model fills define the JSON
/// schema; `mentions`/`last_chapter` are enriched locally after extraction.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Character {
    pub name: String,
    #[serde(default)]
    pub aliases: Vec<String>,
    #[serde(default)]
    pub role: Option<String>,
    #[serde(default)]
    pub physical: Vec<String>,
    #[serde(default)]
    pub voice: Option<VoiceProfile>,
    #[serde(default)]
    pub personality: Vec<String>,
    #[serde(default)]
    pub relationships: Vec<Relationship>,
    #[serde(default)]
    pub goals: Vec<String>,
    #[serde(default)]
    pub arc: Vec<ArcBeat>,
    #[serde(default)]
    pub drift: Vec<Drift>,
    /// Locally computed: total name/alias mentions across the manuscript.
    #[serde(default)]
    pub mentions: usize,
    /// Locally computed: the last chapter the character is mentioned in.
    #[serde(default)]
    pub last_chapter: Option<String>,
}

/// An actionable voice profile (#183): the speech fingerprint the voice-analysis
/// pass checks dialogue and narration against, richer than a one-line summary.
/// Every field is optional so a thin character still parses.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct VoiceProfile {
    /// How they build sentences — habits of phrasing, syntax, directness.
    #[serde(default)]
    pub patterns: Vec<String>,
    /// Vocabulary and register (formal/colloquial, jargon, era, profanity).
    #[serde(default)]
    pub register: Option<String>,
    /// Verbal tics or catchphrases that recur in their speech.
    #[serde(default)]
    pub tics: Vec<String>,
    /// Typical sentence rhythm and length (clipped, run-on, measured).
    #[serde(default)]
    pub rhythm: Option<String>,
}

impl VoiceProfile {
    /// Whether the profile carries anything worth rendering.
    fn is_empty(&self) -> bool {
        self.patterns.is_empty() && self.register.is_none() && self.tics.is_empty() && self.rhythm.is_none()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Relationship {
    pub name: String,
    pub relation: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct ArcBeat {
    pub chapter: String,
    pub state: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq)]
pub struct Drift {
    pub detail: String,
    #[serde(default)]
    pub chapters: Vec<String>,
}

const SYSTEM: &str = "You are a story-bible assistant. Read the novel manuscript \
and extract a profile for every named character with a meaningful presence. Be \
faithful to the text — do not invent details. For each character capture physical \
traits; an actionable voice profile (speech patterns, vocabulary and register, \
verbal tics or catchphrases, and typical sentence rhythm/length — the speech \
fingerprint, not a one-line label); personality, relationships, goals, and a \
per-chapter arc (the chapter title and the character's state or change there). In \
`drift`, record only genuine contradictions across the manuscript (e.g. eye color \
stated differently in two chapters), each with the chapters involved. Reference \
chapters by their title.";

/// The JSON Schema for a character-extraction reply (matches the model-provided
/// fields of [`Character`], wrapped in a `characters` array).
pub fn schema() -> Value {
    let string_array = json!({ "type": "array", "items": { "type": "string" } });
    json!({
        "type": "object",
        "properties": {
            "characters": {
                "type": "array",
                "items": {
                    "type": "object",
                    "properties": {
                        "name": { "type": "string" },
                        "aliases": string_array,
                        "role": { "type": "string" },
                        "physical": string_array,
                        "voice": {
                            "type": "object",
                            "properties": {
                                "patterns": string_array,
                                "register": { "type": "string" },
                                "tics": string_array,
                                "rhythm": { "type": "string" }
                            },
                            "additionalProperties": false
                        },
                        "personality": string_array,
                        "relationships": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "name": { "type": "string" },
                                    "relation": { "type": "string" }
                                },
                                "required": ["name", "relation"],
                                "additionalProperties": false
                            }
                        },
                        "goals": string_array,
                        "arc": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "chapter": { "type": "string" },
                                    "state": { "type": "string" }
                                },
                                "required": ["chapter", "state"],
                                "additionalProperties": false
                            }
                        },
                        "drift": {
                            "type": "array",
                            "items": {
                                "type": "object",
                                "properties": {
                                    "detail": { "type": "string" },
                                    "chapters": string_array
                                },
                                "required": ["detail", "chapters"],
                                "additionalProperties": false
                            }
                        }
                    },
                    "required": ["name"],
                    "additionalProperties": false
                }
            }
        },
        "required": ["characters"],
        "additionalProperties": false
    })
}

/// Build the character-extraction request: the manuscript as cached context, the
/// schema attached so the reply is typed JSON.
pub fn build_request(manuscript: String, model: &str) -> Request {
    Request {
        system: Some(SYSTEM.to_string()),
        cached_context: manuscript,
        user: "Extract the character profiles as specified.".to_string(),
        model: model.to_string(),
        max_tokens: MAX_OUTPUT_TOKENS,
        json_schema: Some(schema()),
    }
}

const REVISE_SYSTEM: &str = "You are a story-bible assistant maintaining an \
existing character bible. You are given the current bible (built from the whole \
manuscript) and only the chapters that have changed since. Revise the bible: \
update characters the changed chapters affect (traits, voice, relationships, \
goals, and the per-chapter arc beats for those chapters), add characters newly \
introduced, and drop ones the changes remove. Preserve everything the changed \
chapters don't touch — arc beats and relationships established in unchanged \
chapters — exactly as the existing bible records them. Re-evaluate `drift` across \
the whole bible. Do not invent details. Reference chapters by title. Return the \
complete revised set of characters.";

/// Build an incremental revision request (#90): revise the existing character
/// bible from only the changed chapters.
pub fn build_incremental_request(prior_bible: &str, changed_chapters: &str, model: &str) -> Request {
    incremental::revise_request(REVISE_SYSTEM, schema(), MAX_OUTPUT_TOKENS, prior_bible, changed_chapters, model)
}

/// Parse a structured reply (`{ "characters": [...] }`) into profiles.
pub fn parse(text: &str) -> Result<Vec<Character>, AiError> {
    #[derive(Deserialize)]
    struct Reply {
        characters: Vec<Character>,
    }
    let reply: Reply =
        serde_json::from_str(text).map_err(|e| AiError::Api(format!("bad character JSON: {e}")))?;
    Ok(reply.characters)
}

// --- On-disk story-bible format --------------------------------------------

/// Render a character's bible file (front matter + generated block + the
/// author's `notes`, via [`bible::render`]). `today` is the generation date.
pub fn render_profile(c: &Character, notes: &str, today: &str) -> String {
    let mut fm = format!("name: {}\n", bible::yaml(&c.name));
    if !c.aliases.is_empty() {
        let aliases: Vec<String> = c.aliases.iter().map(|a| bible::yaml(a)).collect();
        fm.push_str(&format!("aliases: [{}]\n", aliases.join(", ")));
    }
    if let Some(role) = &c.role {
        fm.push_str(&format!("role: {}\n", bible::yaml(role)));
    }
    fm.push_str(&format!("mentions: {}\n", c.mentions));
    if let Some(last) = &c.last_chapter {
        fm.push_str(&format!("last_chapter: {}\n", bible::yaml(last)));
    }
    bible::render(&fm, &generated_block(c), notes, today)
}

/// The regenerated body (everything between the generated markers).
fn generated_block(c: &Character) -> String {
    let mut s = String::new();
    let bullets = |s: &mut String, heading: &str, items: &[String]| {
        if !items.is_empty() {
            s.push_str(&format!("## {heading}\n"));
            for it in items {
                s.push_str(&format!("- {it}\n"));
            }
            s.push('\n');
        }
    };

    if let Some(role) = &c.role {
        s.push_str(&format!("**Role:** {role}\n\n"));
    }
    bullets(&mut s, "Physical", &c.physical);
    if let Some(voice) = &c.voice {
        if !voice.is_empty() {
            s.push_str("## Voice\n");
            let labeled = |s: &mut String, label: &str, items: &[String]| {
                if !items.is_empty() {
                    s.push_str(&format!("- **{label}:** {}\n", items.join("; ")));
                }
            };
            labeled(&mut s, "Speech patterns", &voice.patterns);
            if let Some(register) = &voice.register {
                s.push_str(&format!("- **Register:** {register}\n"));
            }
            labeled(&mut s, "Verbal tics", &voice.tics);
            if let Some(rhythm) = &voice.rhythm {
                s.push_str(&format!("- **Rhythm:** {rhythm}\n"));
            }
            s.push('\n');
        }
    }
    bullets(&mut s, "Personality", &c.personality);
    if !c.relationships.is_empty() {
        s.push_str("## Relationships\n");
        for r in &c.relationships {
            s.push_str(&format!("- {} — {}\n", r.name, r.relation));
        }
        s.push('\n');
    }
    bullets(&mut s, "Goals", &c.goals);
    if !c.arc.is_empty() {
        s.push_str("## Arc\n");
        for beat in &c.arc {
            s.push_str(&format!("- **{}:** {}\n", beat.chapter, beat.state));
        }
        s.push('\n');
    }
    if !c.drift.is_empty() {
        s.push_str("## Drift flags\n");
        for d in &c.drift {
            let where_ = if d.chapters.is_empty() {
                String::new()
            } else {
                format!(" ({})", d.chapters.join(", "))
            };
            s.push_str(&format!("- {}{}\n", d.detail, where_));
        }
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Character {
        Character {
            name: "Mara Vance".into(),
            aliases: vec!["Mara".into()],
            role: Some("protagonist".into()),
            physical: vec!["green eyes".into()],
            voice: Some(VoiceProfile {
                patterns: vec!["answers a question with a question".into()],
                register: Some("dry, understated".into()),
                tics: vec!["\"suppose\"".into()],
                rhythm: Some("clipped".into()),
            }),
            personality: vec!["guarded".into()],
            relationships: vec![Relationship { name: "Tomas".into(), relation: "estranged brother".into() }],
            goals: vec!["find the locket's owner".into()],
            arc: vec![ArcBeat { chapter: "The Find".into(), state: "discovers the locket".into() }],
            drift: vec![Drift { detail: "eyes green vs grey".into(), chapters: vec!["The Find".into(), "The Reckoning".into()] }],
            mentions: 42,
            last_chapter: Some("The Reckoning".into()),
        }
    }

    #[test]
    fn parse_reads_the_characters_array() {
        let json = r#"{"characters":[{"name":"Mara","goals":["escape"]}]}"#;
        let chars = parse(json).unwrap();
        assert_eq!(chars.len(), 1);
        assert_eq!(chars[0].name, "Mara");
        assert_eq!(chars[0].goals, vec!["escape"]);
        assert!(chars[0].aliases.is_empty()); // serde default
    }

    #[test]
    fn parse_rejects_bad_json() {
        assert!(matches!(parse("not json"), Err(AiError::Api(_))));
    }

    #[test]
    fn incremental_request_carries_the_bible_and_only_changed_chapters() {
        let req = build_incremental_request("EXISTING BIBLE", "# Two\n\nchanged prose", "claude-opus-4-8");
        assert!(req.cached_context.contains("EXISTING BIBLE"), "feeds the prior bible as cache");
        assert!(req.cached_context.contains("changed prose"), "feeds the changed chapter");
        assert!(req.system.as_deref().unwrap().contains("existing"), "asks to revise, not regenerate");
        assert!(req.json_schema.is_some(), "still constrained to the schema");
    }

    #[test]
    fn render_profile_has_all_blocks_and_preserves_notes() {
        let notes = "## Notes\n\nMara is based on my aunt.\n";
        let file = render_profile(&sample(), notes, "2026-06-19");
        assert!(file.contains("name: Mara Vance"));
        assert!(file.contains("aliases: [Mara]"));
        assert!(file.contains("mentions: 42"));
        assert!(file.contains("## Voice"));
        assert!(file.contains("**Speech patterns:** answers a question with a question"));
        assert!(file.contains("**Register:** dry, understated"));
        assert!(file.contains("**Rhythm:** clipped"));
        assert!(file.contains("## Arc"));
        assert!(file.contains("## Drift flags"));
        assert!(file.contains("based on my aunt"));
        // Round-trip: the notes come back out unchanged (bible-format behavior).
        assert_eq!(bible::extract_notes(&file).trim(), notes.trim());
        assert!(!bible::is_hand_edited(&file));
    }
}
