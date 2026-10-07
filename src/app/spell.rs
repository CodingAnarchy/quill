//! Idle spell-checking and the author-facing correction actions (#357–#647,
//! #935): recomputing the misspelled-word flags off the keystroke path,
//! navigating between them, accepting a word into the project dictionary,
//! viewing the dictionary, and offering / applying offline suggestions.
//!
//! Extracted from [`super`] as an `impl App` block on the shared `App` type;
//! the fields these touch stay defined in `mod.rs`.

use crate::annotation::Remapper;
use crate::mode::{Command, Mode, Palette, PaletteItem};

use super::App;

/// The inputs a full spell judgment depends on (#357): the edit buffer's content
/// [`revision`](crate::buffer::Buffer::revision) (process-unique, so it also names
/// which buffer), the project dictionary's generation, and the TODO marker. The
/// bundled word list is fixed, so when these match, a re-judge would produce the
/// same flags.
#[derive(Debug, PartialEq, Eq)]
pub(super) struct SpellKey {
    rev: u64,
    dictionary: u64,
    marker: String,
}

impl App {
    /// Bring the misspelled-word flags for the active buffer up to date (#357), off
    /// the keystroke path. The configured TODO marker is never flagged.
    ///
    /// Skipped when nothing the judgment depends on has changed since the last full
    /// pass ([`SpellKey`]) — an idle pass after a cursor move or a mouse hover costs
    /// a few compares, not a whole-window copy and scan. Flags that were only
    /// remapped through an edit burst don't count as judged: the edit moved the
    /// revision, so the next call re-judges.
    pub(crate) fn refresh_spell_flags(&mut self) {
        let key = self.spell_key();
        if self.spell_judged.as_ref() == Some(&key) {
            return;
        }
        let marker = key.marker.to_ascii_lowercase();
        // Refill the persistent scratch in place rather than allocating a fresh
        // whole-document `String` every idle tick (#780). Take it out so the
        // buffer read and the write target don't alias `self`, then put it back.
        let mut scratch = std::mem::take(&mut self.spell_scratch);
        self.edit_buffer().text_into(&mut scratch);
        // Accept the TODO marker and any word in the project's custom dictionary
        // (#358), so invented names aren't flagged.
        self.spell_flags = crate::spell::misspellings(&scratch, |w| {
            w.eq_ignore_ascii_case(&marker) || self.project.is_accepted(w)
        });
        self.spell_scratch = scratch;
        self.spell_rev = key.rev;
        self.spell_judged = Some(key);
    }

    /// The current [`SpellKey`]: what a spell judgment run now would depend on.
    fn spell_key(&self) -> SpellKey {
        SpellKey {
            rev: self.edit_buffer().revision(),
            dictionary: self.project.dictionary_generation(),
            marker: self.project.manifest.todo_marker().to_string(),
        }
    }

    /// Carry the spell flags through a burst of edits (the deltas `mapper`
    /// composes, drained from the buffer at revision `base`), so the underlines
    /// stay on their words while typing without a full spell pass per keystroke.
    /// Only valid when the flags were computed (or last remapped) at `base`;
    /// otherwise they're left for [`on_input_settled`](Self::on_input_settled) to
    /// recompute. Each endpoint maps through the same edits as the text, so a
    /// remapped range stays on char boundaries and inside the buffer; a word an
    /// edit consumed is dropped. A word being typed is re-judged on the idle pass.
    pub(crate) fn remap_spell_flags(&mut self, mapper: &Remapper, base: u64) {
        if base == 0 || self.spell_rev != base {
            return;
        }
        self.spell_flags
            .retain_mut(|f| mapper.remap_range(f.0, f.1).map(|r| *f = r).is_some());
        self.spell_rev = self.edit_buffer().revision();
    }

    /// Move the cursor to the next (`dir >= 0`) or previous misspelled word in the
    /// active buffer (#359), wrapping around the ends. Refreshes the flag set
    /// first, so a word that's since been fixed or accepted isn't a target; when
    /// none remain it says so rather than moving silently.
    pub(crate) fn navigate_spell_flag(&mut self, dir: i32) {
        self.refresh_spell_flags();
        if self.spell_flags.is_empty() {
            self.message = Some("No spelling flags.".into());
            return;
        }
        let cur = self.edit_buffer().cursor_byte();
        let target = if dir >= 0 {
            self.spell_flags
                .iter()
                .find(|&&(s, _)| s > cur)
                .map_or(self.spell_flags[0].0, |&(s, _)| s)
        } else {
            self.spell_flags
                .iter()
                .rev()
                .find(|&&(s, _)| s < cur)
                .map_or(self.spell_flags[self.spell_flags.len() - 1].0, |&(s, _)| s)
        };
        let buffer = self.edit_buffer_mut();
        buffer.cursor = buffer.cursor_at_byte(target);
        // The flag can sit inside a collapsed section (#975); reveal it so the
        // cursor lands on a visible line (mirrors the search / style-flag jumps).
        self.reveal_fold_at_cursor();
    }

    /// Accept the flagged word at the cursor into the project's custom dictionary
    /// (#358): add it (persisted to `dictionary.txt`), then clear the flag. The
    /// cursor must be on (or just past) a misspelled word; otherwise it says so.
    pub(crate) fn accept_word_at_cursor(&mut self) {
        // Judge the word as it stands now — between idle passes the flags are only
        // remapped, so a word mid-edit may not be (or no longer be) misspelled.
        self.refresh_spell_flags();
        let Some((s, e)) = self.flagged_word_at_cursor() else {
            self.message = Some("No flagged word at the cursor.".into());
            return;
        };
        let word = self.edit_buffer().byte_slice(s, e);
        match self.project.accept_word(&word) {
            Ok(true) => {
                self.refresh_spell_flags();
                self.message = Some(format!("Added '{word}' to the dictionary."));
            }
            Ok(false) => self.message = Some(format!("'{word}' is already in the dictionary.")),
            Err(err) => self.message = Some(format!("Couldn't write the dictionary: {err}")),
        }
    }

    /// Open the project's custom dictionary in a reference pane to view and edit it
    /// (#935): the accepted-word list (`dictionary.txt`, one word per line) as a
    /// plain editable buffer beside the manuscript. Editing and saving it there is
    /// how the author removes a word or fixes a typo'd entry; closing the pane
    /// reloads the checker's in-memory set (`close_split_if_any`), so the change
    /// takes effect without reopening the project. The reload stays off the idle
    /// spell pass — it fires only when this pane closes, never per tick.
    pub(crate) fn view_dictionary(&mut self) {
        let path = self.project.dictionary_path();
        self.open_file_in_pane(path);
        // `open_file_in_pane` set its own message; only override it when the pane
        // actually opened, so an error (e.g. a continuous view in the way) shows.
        if self.split.is_some() {
            self.message = Some(
                "Editing the project dictionary — one word per line; \
                 Ctrl-w c closes and reloads it."
                    .into(),
            );
        }
    }

    /// The flagged misspelling at (or nearest before) the cursor: the flag the
    /// cursor sits inside, or — right after typing the word — the one it rests just
    /// after. `None` when the cursor isn't on a flag. Shared by the accept (#358)
    /// and suggest (#647) paths.
    fn flagged_word_at_cursor(&self) -> Option<(usize, usize)> {
        let cursor = self.edit_buffer().cursor_byte();
        self.spell_flags
            .iter()
            .copied()
            .find(|&(s, e)| cursor >= s && cursor < e)
            .or_else(|| self.spell_flags.iter().copied().find(|&(_, e)| cursor == e))
    }

    /// Open a pick-list of offline spelling suggestions for the flagged word at the
    /// cursor (#647), and replace the word with the one the author picks. Within the
    /// assist boundary: the author singled out a flagged word and asked for
    /// corrections; Quill looks up candidates from the bundled dictionary and the
    /// author picks and places one — no prose is generated. Refreshes the flags
    /// first so a since-fixed word isn't offered; says so when the cursor isn't on a
    /// flag or the engine has no candidate.
    pub(crate) fn suggest_word_at_cursor(&mut self) {
        self.refresh_spell_flags();
        let Some((s, e)) = self.flagged_word_at_cursor() else {
            self.message = Some("No flagged word at the cursor.".into());
            return;
        };
        let word = self.edit_buffer().byte_slice(s, e);
        let suggestions = crate::spell::suggestions(&word);
        if suggestions.is_empty() {
            self.message = Some(format!("No suggestions for '{word}'."));
            return;
        }
        let items = suggestions
            .into_iter()
            .map(|sugg| {
                PaletteItem::new(
                    sugg.clone(),
                    format!("Replace '{word}' with '{sugg}'"),
                    Command::ReplaceMisspelling {
                        start: s,
                        end: e,
                        word: word.clone(),
                        replacement: sugg,
                    },
                )
            })
            .collect();
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Spelling suggestions \u{2014} pick a correction",
            items,
            selected: 0,
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// Replace the flagged word in byte range `[start, end)` with `replacement`
    /// (#647), the correction picked from the suggestions list. Goes through the
    /// normal edit path (so it works in both the single-chapter and continuous
    /// views) as one undo group: position the cursor at the word, delete it, then
    /// type the replacement verbatim (no typography substitution — the dictionary
    /// word lands exactly as chosen). Refreshes the flags so the corrected word
    /// clears at once. Guarded against a stale range (the buffer changed since the
    /// list opened): if the slice no longer matches `word`, it declines rather than
    /// corrupting other text.
    pub(crate) fn replace_misspelling(
        &mut self,
        start: usize,
        end: usize,
        word: &str,
        replacement: &str,
    ) {
        let buffer = self.edit_buffer();
        if end > buffer.len_bytes() || buffer.byte_slice(start, end) != word {
            self.message = Some("The text changed \u{2014} re-run suggestions.".into());
            return;
        }
        let buffer = self.edit_buffer_mut();
        buffer.seal_undo_group();
        buffer.cursor = buffer.cursor_at_byte(start);
        // Delete the flagged word, then type the correction — the same char-by-char
        // edit path a keystroke takes, so it's undoable and works in both views.
        for _ in word.chars() {
            self.edit_delete_forward();
        }
        for c in replacement.chars() {
            self.edit_insert_raw(c);
        }
        self.edit_buffer_mut().seal_undo_group();
        self.refresh_spell_flags();
        self.message = Some(format!("Replaced '{word}' with '{replacement}'."));
    }
}
