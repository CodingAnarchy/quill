//! Setting how a word sounds for verse analysis (#1328): the word at the cursor
//! from a prompt, or the whole `pronunciations.txt` in an edit pane. Either way
//! the verse report hears the word the author's way from then on. Only how
//! Quill hears the word changes; the text never does.

use super::*;

use crate::verse::pronounce::{pronounce_with, Stress};
use crate::verse::Entry;

impl App {
    /// Prompt for how the word at the cursor sounds, prefilled with how Quill
    /// hears it now: the author's entry, else the dictionary's first stress
    /// pattern, else a guess stressed on its first syllable.
    pub(crate) fn prompt_pronunciation(&mut self) {
        let Some((start, end)) = self.word_at_cursor() else {
            self.message = Some("Put the cursor on a word to set how it sounds.".into());
            return;
        };
        let word = self
            .edit_buffer()
            .byte_slice(start, end)
            .trim_matches(['\'', '\u{2019}'])
            .to_string();
        if word.is_empty() {
            self.message = Some("Put the cursor on a word to set how it sounds.".into());
            return;
        }
        let lexicon = self.project.lexicon();
        let prefill = lexicon.get(&word).map_or_else(
            || {
                let sound = pronounce_with(&word, lexicon);
                let stresses = sound
                    .prons
                    .first()
                    .map(|p| p.stresses.clone())
                    .unwrap_or_default();
                if sound.guessed {
                    (0..stresses.len())
                        .map(|i| if i == 0 { '1' } else { '0' })
                        .collect()
                } else {
                    stresses.iter().map(|s| digit(*s)).collect()
                }
            },
            Entry::value,
        );
        let label = format!(
            "How '{word}' sounds (a digit a syllable: 1 stressed, 2 secondary, 0 unstressed; then optionally \"rhymes with WORD\"; blank = dictionary): "
        );
        self.pronounce_word = Some(word);
        self.open_prompt(Pending::SetPronunciation, label, &prefill, None);
    }

    /// Store (or, on blank input, remove) the prompted word's pronunciation.
    pub(crate) fn apply_pronunciation(&mut self, text: &str) {
        let Some(word) = self.pronounce_word.take() else {
            return;
        };
        let entry = if text.trim().is_empty() {
            None
        } else {
            match Entry::parse(&word, text) {
                Ok(e) => Some(e),
                Err(e) => {
                    self.message = Some(format!("Pronunciation not set: {e}."));
                    return;
                }
            }
        };
        self.message = Some(
            match self.project.set_pronunciation(&word, entry.as_ref()) {
                Ok(()) => entry.map_or_else(
                    || format!("'{word}' is back to the dictionary's pronunciation."),
                    |e| format!("'{word}' now sounds {} for the verse report.", e.value()),
                ),
                Err(e) => format!("Couldn't write the pronunciations: {e}"),
            },
        );
    }

    /// Open `pronunciations.txt` in a pane beside the manuscript to list, change,
    /// and remove the project's pronunciations. Closing the pane reloads them.
    pub(crate) fn edit_pronunciations(&mut self) {
        let path = self.project.pronunciations_path();
        self.open_file_in_pane(path);
        if self.split.is_some() {
            self.message = Some(
                "Editing pronunciations: word = stress (e.g. Hermione = 0100 rhymes with Tony); Ctrl-w c closes and reloads them."
                    .into(),
            );
        }
    }

    /// Re-read the pronunciations after their pane closed, saying which lines
    /// didn't parse.
    pub(super) fn reload_pronunciations(&mut self) {
        let errors = self.project.reload_pronunciations();
        if let Some(first) = errors.first() {
            self.message = Some(format!(
                "pronunciations.txt {first}{}",
                if errors.len() > 1 {
                    format!(" (and {} more lines)", errors.len() - 1)
                } else {
                    String::new()
                }
            ));
        }
    }
}

/// A stress as its digit.
const fn digit(stress: Stress) -> char {
    match stress {
        Stress::Unstressed => '0',
        Stress::Primary => '1',
        Stress::Secondary => '2',
    }
}
