//! Character tracking: a structured pass over the manuscript that extracts a
//! profile per character (traits, relationships, goals, per-chapter arc) and
//! flags drift, written to the human-readable story bible
//! (`bible/characters/<slug>.md`). The first feature to use structured outputs;
//! its on-disk format is reused by the later trackers. See `docs/ai-features.md`.

use std::collections::HashMap;
use std::fmt::Write;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::{bible, incremental, AiError, Request, MAX_OUTPUT_TOKENS};

/// One character's profile. The serde fields the model fills define the JSON
/// schema; `mentions`/`last_chapter` are enriched locally after extraction.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
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
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
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
    const fn is_empty(&self) -> bool {
        self.patterns.is_empty()
            && self.register.is_none()
            && self.tics.is_empty()
            && self.rhythm.is_none()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Relationship {
    pub name: String,
    pub relation: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct ArcBeat {
    pub chapter: String,
    pub state: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
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
    super::object_schema(vec![("characters", items_schema())], &["characters"])
}

/// The array-of-character-profiles schema on its own, so other passes (the notes
/// import, #283) can compose it without the `{ "characters": … }` wrapper.
///
/// **Every field is `required`** (like the other bible trackers). Structured
/// outputs compile the schema into a constrained-decoding grammar, and an
/// `additionalProperties: false` object with many *optional* fields expands to an
/// any-subset-in-any-order grammar the API rejects with "Schema is too complex".
/// Leaving only `name` required is what broke the character rebuild — don't
/// reintroduce optional fields here. The model emits empty strings/arrays for
/// what it has nothing for; every field is `#[serde(default)]`, so that parses
/// identically to an omitted one.
pub fn items_schema() -> Value {
    let string_array = super::string_array();
    json!({
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
                            "required": ["patterns", "register", "tics", "rhythm"],
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
                    "required": [
                        "name", "aliases", "role", "physical", "voice",
                        "personality", "relationships", "goals", "arc", "drift"
                    ],
                    "additionalProperties": false
                }
    })
}

/// Build the character-extraction request: the manuscript as cached context, the
/// schema attached so the reply is typed JSON.
pub fn build_request(manuscript: String, model: &str) -> Request {
    super::manuscript_request(
        SYSTEM,
        "Extract the character profiles as specified.",
        manuscript,
        model,
        schema(),
    )
}

const CONCEPT_SYSTEM: &str = "You are a story-bible assistant helping an author \
start a novel from a concept, before any draft exists. From the concept or \
premise, INVENT a small starter cast of distinct, plausible characters who could \
populate this story. For each, draft a profile: name, physical traits, an \
actionable voice profile (speech patterns, vocabulary/register, verbal tics, \
sentence rhythm), personality, relationships, goals, and a short arc sketch — a \
few beats labeled by story phase (e.g. \"Opening\", \"Midpoint\", \"Climax\") \
since there are no chapters yet. These are first-draft suggestions the author will \
reshape, not canon. Do NOT write prose or scenes. Leave `drift` empty — there's no \
manuscript to contradict.";

/// Build a generative request (#256): draft a starter cast from a concept /
/// premise (no manuscript), into the same schema the extractive pass returns.
pub fn build_concept_request(concept: &str, model: &str) -> Request {
    super::manuscript_request(
        CONCEPT_SYSTEM,
        "Draft the starter character profiles as specified.",
        format!("Concept / premise:\n{concept}"),
        model,
        schema(),
    )
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
pub fn build_incremental_request(
    prior_bible: &str,
    changed_chapters: &str,
    model: &str,
) -> Request {
    incremental::revise_request(
        REVISE_SYSTEM,
        schema(),
        MAX_OUTPUT_TOKENS,
        prior_bible,
        changed_chapters,
        model,
    )
}

/// Parse a structured reply (`{ "characters": [...] }`) into profiles.
pub fn parse(text: &str) -> Result<Vec<Character>, AiError> {
    #[derive(Deserialize)]
    struct Reply {
        characters: Vec<Character>,
    }
    let reply: Reply = super::parse_json(text, "character")?;
    Ok(reply.characters)
}

// --- On-disk story-bible format --------------------------------------------

/// Render a character's bible file (front matter + generated block + the
/// author's `notes`, via [`bible::render`]). `today` is the generation date.
pub fn render_profile(c: &Character, notes: &str, today: &str) -> String {
    let mut fm = format!("name: {}\n", bible::yaml(&c.name));
    if !c.aliases.is_empty() {
        let aliases: Vec<String> = c.aliases.iter().map(|a| bible::yaml(a)).collect();
        let _ = writeln!(fm, "aliases: [{}]", aliases.join(", "));
    }
    if let Some(role) = &c.role {
        let _ = writeln!(fm, "role: {}", bible::yaml(role));
    }
    let _ = writeln!(fm, "mentions: {}", c.mentions);
    if let Some(last) = &c.last_chapter {
        let _ = writeln!(fm, "last_chapter: {}", bible::yaml(last));
    }
    bible::render(&fm, &generated_block(c), notes, today)
}

/// The regenerated body (everything between the generated markers).
fn generated_block(c: &Character) -> String {
    let mut s = String::new();
    if let Some(role) = &c.role {
        let _ = writeln!(s, "**Role:** {role}\n");
    }
    bible::bullet_section(&mut s, "Physical", &c.physical);
    if let Some(voice) = c.voice.as_ref().filter(|v| !v.is_empty()) {
        s.push_str("## Voice\n");
        let labeled = |s: &mut String, label: &str, items: &[String]| {
            if !items.is_empty() {
                let _ = writeln!(s, "- **{label}:** {}", items.join("; "));
            }
        };
        labeled(&mut s, "Speech patterns", &voice.patterns);
        if let Some(register) = &voice.register {
            let _ = writeln!(s, "- **Register:** {register}");
        }
        labeled(&mut s, "Verbal tics", &voice.tics);
        if let Some(rhythm) = &voice.rhythm {
            let _ = writeln!(s, "- **Rhythm:** {rhythm}");
        }
        s.push('\n');
    }
    bible::bullet_section(&mut s, "Personality", &c.personality);
    if !c.relationships.is_empty() {
        s.push_str("## Relationships\n");
        for r in &c.relationships {
            let _ = writeln!(s, "- {} — {}", r.name, r.relation);
        }
        s.push('\n');
    }
    bible::bullet_section(&mut s, "Goals", &c.goals);
    if !c.arc.is_empty() {
        s.push_str("## Arc\n");
        for beat in &c.arc {
            let _ = writeln!(s, "- **{}:** {}", beat.chapter, beat.state);
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
            let _ = writeln!(s, "- {}{}", d.detail, where_);
        }
        s.push('\n');
    }
    s
}

// --- Relationship index (aggregation, #707) --------------------------------

/// A resolved relationship edge in the index: the target as the author would see
/// it (the profile's canonical name when the reference resolves to a tracked
/// character, otherwise the raw reference), whether that target has its own
/// profile, and the relation label.
struct Edge<'a> {
    /// The canonical display name of the target (a tracked character's `name`, or
    /// the raw reference when it doesn't resolve to one).
    target: &'a str,
    relation: &'a str,
    /// Whether `target` is one of the tracked characters (so the index can link
    /// mutual pairs and separate off-cast references).
    tracked: bool,
}

/// Case-insensitive resolver from a relationship reference (a name or alias) to
/// the canonical profile name it belongs to. Built once per index render so the
/// aggregation is O(edges), not O(edges × characters).
fn name_resolver(chars: &[Character]) -> HashMap<String, &str> {
    let mut map = HashMap::new();
    for c in chars {
        // Later profiles don't clobber earlier ones for a shared alias; the first
        // (name before alias, earlier character before later) wins, which keeps
        // the canonical name stable.
        map.entry(c.name.to_lowercase()).or_insert(c.name.as_str());
        for alias in &c.aliases {
            map.entry(alias.to_lowercase()).or_insert(c.name.as_str());
        }
    }
    map
}

/// Resolve one character's relationships into display edges, canonicalizing each
/// target to a tracked profile's name where the reference matches.
fn edges_of<'a>(c: &'a Character, resolver: &HashMap<String, &'a str>) -> Vec<Edge<'a>> {
    c.relationships
        .iter()
        .map(|r| match resolver.get(&r.name.to_lowercase()) {
            Some(&canonical) => Edge {
                target: canonical,
                relation: &r.relation,
                tracked: true,
            },
            None => Edge {
                target: &r.name,
                relation: &r.relation,
                tracked: false,
            },
        })
        .collect()
}

/// Render the aggregated relationship index (#707): a single `bible/relationships.md`
/// rolling up the `relationships` every character profile already carries into one
/// cross-cast view — no new AI call, pure aggregation of extracted facts.
///
/// Each tracked character gets a section listing their outgoing relationships.
/// When the target is itself a tracked character and names this character back,
/// the edge is annotated as mutual with the reciprocal label. References the
/// character doesn't list back but that name *them* are surfaced under "Also
/// referenced by", so a one-sided edge is visible from both ends.
pub fn render_index(chars: &[Character], today: &str) -> String {
    let resolver = name_resolver(chars);
    // Resolve every character's edges once, indexed by canonical name, so both the
    // outgoing list and the reciprocity / back-reference lookups read the same
    // resolved data.
    let edges: Vec<(&str, Vec<Edge>)> = chars
        .iter()
        .map(|c| (c.name.as_str(), edges_of(c, &resolver)))
        .collect();

    // Alphabetical, case-insensitive, so the index is stable regardless of the
    // order the pass returned characters in.
    let mut order: Vec<usize> = (0..chars.len()).collect();
    order.sort_by_key(|&i| chars[i].name.to_lowercase());

    let mut body = String::new();
    let mut any = false;
    for &i in &order {
        let name = chars[i].name.as_str();
        let outgoing = &edges[i].1;
        let back = back_references(name, &edges);
        if outgoing.is_empty() && back.is_empty() {
            continue;
        }
        any = true;
        let _ = writeln!(body, "## {name}");
        for edge in outgoing {
            let mutual = edge
                .tracked
                .then(|| reciprocal(name, edge.target, &edges))
                .flatten();
            match mutual {
                Some(back_relation) => {
                    let _ = writeln!(
                        body,
                        "- {} — {} (mutual: {})",
                        edge.target, edge.relation, back_relation
                    );
                }
                None => {
                    let _ = writeln!(body, "- {} — {}", edge.target, edge.relation);
                }
            }
        }
        // Incoming edges this character doesn't list back — a one-sided reference
        // the author can see from this end too.
        let names_back: Vec<&str> = outgoing.iter().map(|e| e.target).collect();
        let unlisted: Vec<&(&str, &str)> = back
            .iter()
            .filter(|(from, _)| !names_back.contains(from))
            .collect();
        if !unlisted.is_empty() {
            let refs: Vec<String> = unlisted
                .iter()
                .map(|(from, relation)| format!("{from} ({relation})"))
                .collect();
            let _ = writeln!(body, "- _Also referenced by:_ {}", refs.join(", "));
        }
        body.push('\n');
    }
    if !any {
        body.push_str("_No relationships recorded yet._\n\n");
    }

    let front = "title: Relationship index\n".to_string();
    // The index is fully derived from the profiles, so it carries no author Notes
    // block to preserve — an empty notes block keeps the bible-file shape uniform.
    bible::render(&front, &body, "", today)
}

/// The reciprocal relation label `target` records for `source`, if `target` is a
/// tracked character that names `source` back — used to mark a mutual edge.
fn reciprocal<'a>(source: &str, target: &str, edges: &'a [(&str, Vec<Edge>)]) -> Option<&'a str> {
    edges
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(target))
        .and_then(|(_, es)| {
            es.iter()
                .find(|e| e.target.eq_ignore_ascii_case(source))
                .map(|e| e.relation)
        })
}

/// Every tracked character that names `name`, as `(their name, the relation they
/// record)` — the incoming edges, so a one-sided reference shows from both ends.
fn back_references<'a>(name: &str, edges: &'a [(&str, Vec<Edge>)]) -> Vec<(&'a str, &'a str)> {
    let mut out = Vec::new();
    for (from, es) in edges {
        if from.eq_ignore_ascii_case(name) {
            continue;
        }
        for e in es {
            if e.target.eq_ignore_ascii_case(name) {
                out.push((*from, e.relation));
            }
        }
    }
    out
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
            relationships: vec![Relationship {
                name: "Tomas".into(),
                relation: "estranged brother".into(),
            }],
            goals: vec!["find the locket's owner".into()],
            arc: vec![ArcBeat {
                chapter: "The Find".into(),
                state: "discovers the locket".into(),
            }],
            drift: vec![Drift {
                detail: "eyes green vs grey".into(),
                chapters: vec!["The Find".into(), "The Reckoning".into()],
            }],
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
        assert!(matches!(
            parse("not json"),
            Err(AiError::ModelNonCompliance(_))
        ));
    }

    #[test]
    fn concept_request_carries_the_concept_not_a_manuscript() {
        let req = build_concept_request("a heist on a generation ship", "claude-opus-4-8");
        assert!(req.cached_context.contains("generation ship"));
        assert!(
            req.system.as_deref().unwrap().contains("INVENT"),
            "asks to invent, not extract"
        );
        assert!(
            req.json_schema.is_some(),
            "same schema as the extractive pass"
        );
    }

    #[test]
    fn incremental_request_carries_the_bible_and_only_changed_chapters() {
        let req = build_incremental_request(
            "EXISTING BIBLE",
            "# Two\n\nchanged prose",
            "claude-opus-4-8",
        );
        assert!(
            req.cached_context.contains("EXISTING BIBLE"),
            "feeds the prior bible as cache"
        );
        assert!(
            req.cached_context.contains("changed prose"),
            "feeds the changed chapter"
        );
        assert!(
            req.system.as_deref().unwrap().contains("existing"),
            "asks to revise, not regenerate"
        );
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

    // --- Relationship index (#707) -----------------------------------------

    /// A minimal character with just a name and a set of outgoing relationships,
    /// so the index tests read as edge lists.
    fn with_rels(name: &str, rels: &[(&str, &str)]) -> Character {
        Character {
            name: name.into(),
            relationships: rels
                .iter()
                .map(|(n, r)| Relationship {
                    name: (*n).into(),
                    relation: (*r).into(),
                })
                .collect(),
            ..Character::default()
        }
    }

    #[test]
    fn index_aggregates_each_characters_edges() {
        // Two characters, one edge each, both pointing at off-cast names: each gets
        // its own section with its outgoing edge, sorted alphabetically.
        let chars = vec![
            with_rels("Mara", &[("the locket", "obsessed with")]),
            with_rels("Ada", &[("her past", "haunted by")]),
        ];
        let index = render_index(&chars, "2026-06-19");
        // Alphabetical: Ada before Mara.
        let ada = index.find("## Ada").unwrap();
        let mara = index.find("## Mara").unwrap();
        assert!(ada < mara, "sections are alphabetical");
        assert!(index.contains("- her past — haunted by"));
        assert!(index.contains("- the locket — obsessed with"));
    }

    #[test]
    fn index_marks_reciprocal_edges_as_mutual() {
        // Mara → Tomas and Tomas → Mara: each edge is annotated with the other's
        // relation label.
        let chars = vec![
            with_rels("Mara", &[("Tomas", "estranged brother")]),
            with_rels("Tomas", &[("Mara", "sister he abandoned")]),
        ];
        let index = render_index(&chars, "2026-06-19");
        assert!(index.contains("- Tomas — estranged brother (mutual: sister he abandoned)"));
        assert!(index.contains("- Mara — sister he abandoned (mutual: estranged brother)"));
    }

    #[test]
    fn index_surfaces_one_sided_edges_from_both_ends() {
        // Mara names Tomas, but Tomas's profile doesn't name Mara back. Tomas's
        // section still shows the incoming reference so the edge is visible both ways.
        let chars = vec![
            with_rels("Mara", &[("Tomas", "estranged brother")]),
            with_rels("Tomas", &[]),
        ];
        let index = render_index(&chars, "2026-06-19");
        assert!(index.contains("## Mara"));
        assert!(index.contains("- Tomas — estranged brother"));
        assert!(
            !index.contains("- Tomas — estranged brother (mutual"),
            "not mutual — Tomas doesn't list Mara back"
        );
        assert!(index.contains("## Tomas"));
        assert!(
            index.contains("- _Also referenced by:_ Mara (estranged brother)"),
            "Tomas's section surfaces the incoming edge"
        );
    }

    #[test]
    fn index_resolves_references_through_aliases() {
        // Mara refers to "Tom", which is Tomas's alias; the edge canonicalizes to
        // Tomas and pairs up as mutual.
        let mut tomas = with_rels("Tomas", &[("Mara", "sister")]);
        tomas.aliases = vec!["Tom".into()];
        let chars = vec![with_rels("Mara", &[("Tom", "brother")]), tomas];
        let index = render_index(&chars, "2026-06-19");
        // Displayed under the canonical name, marked mutual.
        assert!(index.contains("- Tomas — brother (mutual: sister)"));
        assert!(index.contains("- Mara — sister (mutual: brother)"));
    }

    #[test]
    fn index_handles_a_cast_with_no_relationships() {
        let chars = vec![with_rels("Mara", &[]), with_rels("Ada", &[])];
        let index = render_index(&chars, "2026-06-19");
        assert!(index.contains("_No relationships recorded yet._"));
        // A clean, non-hand-edited bible file even when empty.
        assert!(!bible::is_hand_edited(&index));
    }

    #[test]
    fn index_is_a_wellformed_bible_file() {
        let chars = vec![with_rels("Mara", &[("Tomas", "brother")])];
        let index = render_index(&chars, "2026-06-19");
        assert!(index.contains("title: Relationship index"));
        assert!(index.contains("generated: 2026-06-19"));
        // No author notes to preserve, but the round-trip stays clean.
        assert!(!bible::is_hand_edited(&index));
    }
}
