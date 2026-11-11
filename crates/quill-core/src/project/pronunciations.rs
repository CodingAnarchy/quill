//! The project's own pronunciations for verse analysis (#1328): loaded from
//! `pronunciations.txt` at open, changed one word at a time from the editor,
//! and reloaded when the author edits the file by hand.

use std::fs;
use std::path::{Path, PathBuf};

use super::io::read_existing;
use super::Project;
use crate::atomic;
use crate::verse::lexicon::set_line;
use crate::verse::{Entry, Lexicon, PRONUNCIATIONS_NAME};

/// Read the project's pronunciations, best effort: a missing or unreadable
/// file, or a line that doesn't parse, leaves those words to the dictionary.
pub(super) fn load(root: &Path) -> Lexicon {
    let text = fs::read_to_string(root.join(PRONUNCIATIONS_NAME)).unwrap_or_default();
    Lexicon::parse(&text).0
}

impl Project {
    /// The project's own pronunciations.
    pub const fn lexicon(&self) -> &Lexicon {
        &self.lexicon
    }

    /// Path to `pronunciations.txt` at the project root.
    pub fn pronunciations_path(&self) -> PathBuf {
        self.root.join(PRONUNCIATIONS_NAME)
    }

    /// Re-read `pronunciations.txt` after the author edited it, returning a note
    /// for each line that didn't parse (those words fall back to the dictionary).
    pub fn reload_pronunciations(&mut self) -> Vec<String> {
        let text = fs::read_to_string(self.pronunciations_path()).unwrap_or_default();
        let (lexicon, errors) = Lexicon::parse(&text);
        self.lexicon = lexicon;
        errors
    }

    /// Set `word`'s pronunciation, or remove it with `None`: rewrite its line in
    /// `pronunciations.txt` (keeping every other line and comment) and reload.
    /// The file is read first, so a failed read never truncates it; a read-only
    /// project (another instance holds it) refuses.
    pub fn set_pronunciation(&mut self, word: &str, entry: Option<&Entry>) -> std::io::Result<()> {
        if self.read_only {
            return Err(Self::read_only_error());
        }
        let path = self.pronunciations_path();
        let text = read_existing(&path)?;
        let updated = set_line(&text, word, entry);
        if updated != text {
            atomic::write(&path, updated.as_bytes())?;
        }
        self.lexicon = Lexicon::parse(&updated).0;
        Ok(())
    }
}
