//! Reference-lookup pair — the in-context thesaurus / word search (#290/#653)
//! and the name generator (#610). Both are deliberate, author-invoked lookups
//! that return a pick-list of candidate words or names; the author chooses one
//! and Quill places it verbatim. No manuscript prose is written — this stays
//! within the AI boundary (reference, not co-writing).

use super::super::*;

/// A candidate word's one-line description for the pick list (#653): its
/// definition, any syllable / origin / rarity tags, and a trailing usage note —
/// everything the read-only overlay used to show, folded onto the picker row.
/// Mirrors [`namegen_desc`] (#610).
fn thesaurus_desc(word: &ai::thesaurus::Word) -> String {
    let mut tags: Vec<String> = Vec::new();
    if let Some(s) = word.syllables {
        tags.push(format!("{s} syl"));
    }
    if let Some(o) = word
        .origin
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        tags.push(o.to_string());
    }
    if let Some(r) = word
        .rarity
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        tags.push(r.to_string());
    }
    let mut desc = word.definition.trim().to_string();
    if !tags.is_empty() {
        let tail = format!("({})", tags.join(" \u{b7} "));
        desc = if desc.is_empty() {
            tail
        } else {
            format!("{desc}  {tail}")
        };
    }
    if let Some(note) = word
        .note
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        desc = if desc.is_empty() {
            note.to_string()
        } else {
            format!("{desc} \u{2014} {note}")
        };
    }
    desc
}

/// The name-generator kind picker's row title (#610) — the capitalized kind.
fn kind_title(kind: ai::namegen::Kind) -> String {
    let label = kind.label();
    let mut chars = label.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().collect::<String>() + chars.as_str()
    })
}

/// A candidate name's one-line description for the pick list (#610): its note,
/// with any syllable / origin / pronunciation tags appended.
fn namegen_desc(name: &ai::namegen::Name) -> String {
    let mut tags: Vec<String> = Vec::new();
    if let Some(s) = name.syllables {
        tags.push(format!("{s} syl"));
    }
    if let Some(o) = name
        .origin
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        tags.push(o.to_string());
    }
    if let Some(p) = name
        .pronunciation
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        tags.push(format!("/{p}/"));
    }
    let note = name.note.trim();
    match (note.is_empty(), tags.is_empty()) {
        (true, true) => String::new(),
        (false, true) => note.to_string(),
        (true, false) => tags.join(" \u{b7} "),
        (false, false) => format!("{note}  ({})", tags.join(" \u{b7} ")),
    }
}

impl App {
    // --- In-context thesaurus / word search (#290) ------------------------

    /// Open the word-search prompt, prefilled with the word under the cursor so the
    /// common case is "land on a word, invoke, hit enter (or add filters)".
    pub(crate) fn start_thesaurus(&mut self) {
        let buffer = self.edit_buffer();
        let prefill = buffer
            .text_object(TextObject::Word, false)
            .map(|(lo, hi)| buffer.byte_slice(lo, hi))
            .map(|w| w.trim().to_string())
            .filter(|w| !w.is_empty())
            .unwrap_or_default();
        self.open_prompt(
            Pending::Thesaurus,
            "Word search — word/phrase, then any filters (e.g. \u{201c}sad; 2 syllables, Anglo-Saxon; 8\u{201d}): ",
            &prefill,
            None,
        );
    }

    /// Run the word search: the typed request plus the current line as context
    /// (for fit only). Off the editor thread; the candidates arrive in an overlay.
    pub(crate) fn thesaurus(&mut self, request: &str) {
        let request = request.trim().to_string();
        if request.is_empty() {
            self.message = Some("Word search: nothing to look up.".into());
            return;
        }
        let buffer = self.edit_buffer();
        let context = buffer.line(buffer.cursor.line);
        let model = self.ai.model.clone();
        self.spawn_outcome("\u{27f3} Searching words\u{2026}", move |backend| {
            let req = ai::thesaurus::build_request(&request, &context, &model);
            Outcome::Thesaurus(
                backend
                    .complete(&req)
                    .and_then(|r| ai::thesaurus::parse(&r.text)),
            )
        });
    }

    /// Show the word-search candidates as a pickable list (#653): each row is a
    /// candidate word/phrase with its definition and any syllable/origin/rarity
    /// tags; Enter places the chosen word at the cursor (mirroring the name
    /// generator, #610). The author picks and Quill places — no prose is written,
    /// only the one candidate the author chose for the slot they singled out. Empty
    /// result → a status telling the author to loosen the constraints.
    pub(crate) fn apply_thesaurus(
        &mut self,
        result: Result<Vec<ai::thesaurus::Word>, ai::AiError>,
    ) {
        let Some(words) = self.ok_or_report(result) else {
            return;
        };
        let words: Vec<ai::thesaurus::Word> = words
            .into_iter()
            .filter(|w| !w.text.trim().is_empty())
            .collect();
        if words.is_empty() {
            self.message = Some("No matches — try loosening the constraints.".into());
            return;
        }
        let items = words
            .iter()
            .map(|w| {
                PaletteItem::new(
                    w.text.clone(),
                    thesaurus_desc(w),
                    Command::InsertWord(w.text.clone()),
                )
            })
            .collect();
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Pick a word to place",
            items,
            selected: 0,
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// Place the chosen candidate word/phrase at the cursor (#653), through the
    /// normal edit path (so it works in both views) as one undo group. Verbatim:
    /// smart typography is bypassed, the word lands exactly as chosen. Mirrors
    /// [`insert_name`](App::insert_name) / [`insert_snippet`](App::insert_snippet).
    pub(crate) fn insert_word(&mut self, word: &str) {
        self.edit_buffer_mut().seal_undo_group();
        for c in word.chars() {
            self.edit_insert_raw(c);
        }
        self.edit_buffer_mut().seal_undo_group();
        self.message = Some(format!("Placed \u{201c}{word}\u{201d}."));
    }

    // --- Name generator (#610) --------------------------------------------

    /// Open the name-generator kind picker: character, place, or invented term.
    /// Choosing one opens the constraints prompt — the deliberate, two-step lookup
    /// the AI boundary allows (an option list the author picks from, never prose).
    pub(crate) fn start_namegen(&mut self) {
        let items = ai::namegen::Kind::ALL
            .iter()
            .map(|&k| {
                PaletteItem::new(
                    kind_title(k),
                    format!("Generate {} names by constraints", k.label()),
                    Command::PickNameKind(k),
                )
            })
            .collect();
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Name generator — what needs a name?",
            items,
            selected: 0,
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// After the kind is chosen, prompt for the free-text constraints (origin,
    /// syllables, tone, era) before running the lookup.
    pub(crate) fn prompt_namegen(&mut self, kind: ai::namegen::Kind) {
        self.open_prompt(
            Pending::NameGen(kind),
            format!(
                "Name a {} \u{2014} constraints (e.g. \u{201c}Norse, 2 syllables, harsh, medieval\u{201d}): ",
                kind.label()
            ),
            "",
            None,
        );
    }

    /// Run the name generator: the chosen kind plus the typed constraints. Off the
    /// editor thread; the candidates arrive as a pickable list overlay. No
    /// manuscript is attached — it's a small, off-manuscript option-list lookup.
    pub(crate) fn namegen(&mut self, kind: ai::namegen::Kind, constraints: &str) {
        let constraints = constraints.trim().to_string();
        let model = self.ai.model.clone();
        self.spawn_outcome("\u{27f3} Generating names\u{2026}", move |backend| {
            let req = ai::namegen::build_request(kind, &constraints, &model);
            Outcome::NameGen(
                backend
                    .complete(&req)
                    .and_then(|r| ai::namegen::parse(&r.text)),
            )
        });
    }

    /// Show the name-generator candidates as a pickable list (#610): each row is a
    /// candidate name with its note/tags; Enter places the chosen name at the
    /// cursor (mirroring the snippet picker). The author picks and Quill places —
    /// no prose is written, only the one name the author chose. Empty result → a
    /// status telling the author to loosen the constraints.
    pub(crate) fn apply_namegen(&mut self, result: Result<Vec<ai::namegen::Name>, ai::AiError>) {
        let Some(names) = self.ok_or_report(result) else {
            return;
        };
        let names: Vec<ai::namegen::Name> = names
            .into_iter()
            .filter(|n| !n.text.trim().is_empty())
            .collect();
        if names.is_empty() {
            self.message = Some("No names — try loosening the constraints.".into());
            return;
        }
        let items = names
            .iter()
            .map(|n| {
                PaletteItem::new(
                    n.text.clone(),
                    namegen_desc(n),
                    Command::InsertName(n.text.clone()),
                )
            })
            .collect();
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Pick a name to place",
            items,
            selected: 0,
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// Place the chosen candidate name at the cursor (#610), through the normal
    /// edit path (so it works in both views) as one undo group. Verbatim: smart
    /// typography is bypassed, the name lands exactly as chosen.
    pub(crate) fn insert_name(&mut self, name: &str) {
        self.edit_buffer_mut().seal_undo_group();
        for c in name.chars() {
            self.edit_insert_raw(c);
        }
        self.edit_buffer_mut().seal_undo_group();
        self.message = Some(format!("Placed \u{201c}{name}\u{201d}."));
    }
}
