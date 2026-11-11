//! The author's own pronunciations (#1328): how a name, an invented word, or an
//! archaic stress sounds, set once for the project and used by every meter and
//! rhyme check ahead of the dictionary.
//!
//! They live in `pronunciations.txt` at the project root, one word per line, so
//! the author can read and edit them and they travel in git with the manuscript:
//!
//! ```text
//! # word = stress, one digit a syllable (1 stressed, 2 secondary, 0 unstressed)
//! Hermione = 0100 rhymes with Tony
//! sometime = 01
//! ```
//!
//! The optional `rhymes with` names a word whose ending sounds the same, which
//! the rhyme check hears in its place.

use std::collections::HashMap;
use std::fmt::Write;

use super::pronounce::{normalize, Stress};

/// The project file the pronunciations live in.
pub const PRONUNCIATIONS_NAME: &str = "pronunciations.txt";

/// The comment a new pronunciations file opens with.
const HEADER: &str = "# How words sound in this project, for the verse report (one per line):\n# word = stress, one digit a syllable (1 stressed, 2 secondary, 0 unstressed),\n# optionally followed by \"rhymes with\" and a word that ends with the same sound.\n";

/// One word's pronunciation as the author gave it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// The word as the author wrote it.
    pub word: String,
    pub stresses: Vec<Stress>,
    /// A word whose ending sounds like this one's, for rhyme.
    pub rhymes_with: Option<String>,
}

impl Entry {
    /// Read a pronunciation from the value side of a line (`0100 rhymes with
    /// Tony`). The stress is one digit a syllable; spaces and hyphens between the
    /// digits are allowed. An error says what's wrong.
    pub fn parse(word: &str, value: &str) -> Result<Self, String> {
        let word = word.trim();
        if word.is_empty() || word.chars().any(char::is_whitespace) {
            return Err("give one word before the =".into());
        }
        let lower = value.to_lowercase();
        let (stress, rhyme) = match lower.split_once("rhymes with") {
            Some((s, r)) => (s, Some(r.trim())),
            None => (lower.as_str(), None),
        };
        let stresses = stress
            .chars()
            .filter(|c| !matches!(c, ' ' | '-' | '\t'))
            .map(|c| match c {
                '0' => Ok(Stress::Unstressed),
                '1' => Ok(Stress::Primary),
                '2' => Ok(Stress::Secondary),
                other => Err(format!(
                    "'{other}' isn't a stress digit (1 stressed, 2 secondary, 0 unstressed)"
                )),
            })
            .collect::<Result<Vec<_>, _>>()?;
        if stresses.is_empty() {
            return Err("give the stress, one digit a syllable, like 0100".into());
        }
        let rhymes_with = match rhyme {
            Some("") => return Err("name a word after \"rhymes with\"".into()),
            Some(r) if r.chars().any(char::is_whitespace) => {
                return Err("\"rhymes with\" takes one word".into())
            }
            Some(r) => Some(r.to_string()),
            None => None,
        };
        Ok(Self {
            word: word.to_string(),
            stresses,
            rhymes_with,
        })
    }

    /// The value side of the entry's line, as [`Entry::parse`] reads it.
    pub fn value(&self) -> String {
        let digits: String = self
            .stresses
            .iter()
            .map(|s| match s {
                Stress::Unstressed => '0',
                Stress::Primary => '1',
                Stress::Secondary => '2',
            })
            .collect();
        match &self.rhymes_with {
            Some(word) => format!("{digits} rhymes with {word}"),
            None => digits,
        }
    }
}

/// The project's pronunciations, by normalized word.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Lexicon {
    entries: HashMap<String, Entry>,
}

impl Lexicon {
    /// Read a pronunciations file. Blank lines and `#` comments are skipped; a
    /// line that doesn't parse is skipped too, and reported with its number.
    pub fn parse(text: &str) -> (Self, Vec<String>) {
        let mut lexicon = Self::default();
        let mut errors = Vec::new();
        for (n, line) in text.lines().enumerate() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let parsed = line
                .split_once('=')
                .ok_or_else(|| "write it as word = stress".to_string())
                .and_then(|(word, value)| Entry::parse(word, value));
            match parsed {
                Ok(entry) => {
                    lexicon.entries.insert(normalize(&entry.word), entry);
                }
                Err(e) => errors.push(format!("line {}: {e}", n + 1)),
            }
        }
        (lexicon, errors)
    }

    /// The author's pronunciation of `word`, if they gave one.
    pub fn get(&self, word: &str) -> Option<&Entry> {
        if self.entries.is_empty() {
            return None;
        }
        self.entries.get(&normalize(word))
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The pronunciations file's text with `word`'s line set to `entry`, or removed
/// when `entry` is `None`. Every other line, comments included, is kept as it
/// was; a new word goes at the end, and a new file opens with a short header.
pub fn set_line(text: &str, word: &str, entry: Option<&Entry>) -> String {
    let key = normalize(word);
    let mut out = String::new();
    let mut placed = false;
    for line in text.lines() {
        let ours = !line.trim_start().starts_with('#')
            && line
                .split_once('=')
                .is_some_and(|(w, _)| normalize(w.trim()) == key);
        if ours {
            if let Some(e) = entry.filter(|_| !placed) {
                let _ = writeln!(out, "{} = {}", e.word, e.value());
                placed = true;
            }
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if let Some(e) = entry.filter(|_| !placed) {
        if out.trim().is_empty() {
            out = HEADER.to_string();
        }
        let _ = writeln!(out, "{} = {}", e.word, e.value());
    }
    out
}
