//! How a word sounds, for verse analysis (#1324): its syllables, the stress on
//! each, and its phonemes (the rhyme analysis reads the sound from the last
//! stressed vowel on).
//!
//! The source is the CMU Pronouncing Dictionary (`assets/pronouncing/`, BSD
//! licence), compiled into the binary and parsed into a map the first time a
//! word is looked up. A word the dictionary lacks falls back to the shared
//! syllable estimator with every syllable's stress left open, and is marked as a
//! guess so the report can say which scansions are uncertain.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::analysis::syllables;

/// The CMU Pronouncing Dictionary: one `word PH1 PH2 …` line per pronunciation,
/// alternates as `word(2)`, and an optional `# comment` at the end of a line.
const CMUDICT: &str = include_str!("../../../../assets/pronouncing/cmudict.dict");

/// How strongly a syllable is stressed in the word said on its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stress {
    Unstressed,
    Primary,
    Secondary,
}

/// One way to say a word.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pronunciation {
    /// Each syllable's stress, in order.
    pub stresses: Vec<Stress>,
    /// The ARPAbet phonemes with their stress digits (`R EH1 K ER0 D`), or empty
    /// for a guess, which has no sound to go on.
    pub phones: String,
}

impl Pronunciation {
    /// A pronunciation from a dictionary phone string: every phone ending in a
    /// stress digit is a vowel, so a syllable.
    fn from_phones(phones: &str) -> Self {
        let stresses = phones
            .split_whitespace()
            .filter_map(|p| match p.chars().last() {
                Some('0') => Some(Stress::Unstressed),
                Some('1') => Some(Stress::Primary),
                Some('2') => Some(Stress::Secondary),
                _ => None,
            })
            .collect();
        Self {
            stresses,
            phones: phones.to_string(),
        }
    }
}

/// A word's possible pronunciations, most common first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Sound {
    pub prons: Vec<Pronunciation>,
    /// Whether the word wasn't in the dictionary, so its syllables are estimated
    /// from the spelling and its stress is unknown.
    pub guessed: bool,
}

/// The parsed dictionary: each word (lowercase) to its phone strings.
fn dictionary() -> &'static HashMap<&'static str, Vec<&'static str>> {
    static DICT: OnceLock<HashMap<&'static str, Vec<&'static str>>> = OnceLock::new();
    DICT.get_or_init(|| {
        let mut map: HashMap<&'static str, Vec<&'static str>> = HashMap::with_capacity(130_000);
        for line in CMUDICT.lines() {
            let Some((word, phones)) = line.split_once(' ') else {
                continue;
            };
            // `record(2)` is the second way to say `record`.
            let word = word.split_once('(').map_or(word, |(w, _)| w);
            let phones = phones.split_once(" #").map_or(phones, |(p, _)| p).trim();
            map.entry(word).or_default().push(phones);
        }
        map
    })
}

/// Parse the dictionary now, off whatever path will first need it (the first
/// lookup otherwise pays the parse, a few tens of milliseconds).
pub fn warm() {
    dictionary();
}

/// How `word` sounds. Matching is case-insensitive and treats a curly
/// apostrophe as a straight one. A possessive the dictionary lacks is said as
/// its base word plus the `s` (a syllable after a hiss: *Rose's*). Anything else
/// missing is estimated from the spelling and marked as a guess.
pub fn pronounce(word: &str) -> Sound {
    let key = normalize(word);
    if let Some(prons) = lookup(&key) {
        return Sound {
            prons,
            guessed: false,
        };
    }
    if let Some(base) = key.strip_suffix("'s").or_else(|| key.strip_suffix("s'")) {
        if let Some(prons) = lookup(base) {
            let prons = prons.into_iter().map(possessive).collect();
            return Sound {
                prons,
                guessed: false,
            };
        }
    }
    guess(&key)
}

/// The dictionary entry for an already-normalized word.
fn lookup(key: &str) -> Option<Vec<Pronunciation>> {
    dictionary()
        .get(key)
        .map(|all| all.iter().map(|p| Pronunciation::from_phones(p)).collect())
}

/// Lowercase, with a curly apostrophe straightened.
fn normalize(word: &str) -> String {
    word.chars()
        .map(|c| if c == '\u{2019}' { '\'' } else { c })
        .flat_map(char::to_lowercase)
        .collect()
}

/// A base word's pronunciation with a possessive `s` added: `IH0 Z`, a new
/// syllable, after a sibilant (*Rose's*, *Church's*), else a bare `Z`.
fn possessive(mut pron: Pronunciation) -> Pronunciation {
    let last = pron.phones.split_whitespace().last().unwrap_or("");
    if matches!(last, "S" | "Z" | "SH" | "ZH" | "CH" | "JH") {
        pron.phones.push_str(" IH0 Z");
        pron.stresses.push(Stress::Unstressed);
    } else {
        pron.phones.push_str(" Z");
    }
    pron
}

/// An estimate for a word the dictionary lacks: the spelling's syllable count,
/// with a one-syllable word stressed and a longer word's stress left open
/// (secondary, which scansion may read either way).
fn guess(word: &str) -> Sound {
    let count = syllables(word).max(1);
    let stresses = if count == 1 {
        vec![Stress::Primary]
    } else {
        vec![Stress::Secondary; count]
    };
    Sound {
        prons: vec![Pronunciation {
            stresses,
            phones: String::new(),
        }],
        guessed: true,
    }
}

/// Words that carry little weight in a line: articles, prepositions,
/// conjunctions, pronouns, and auxiliaries. Said alone they may be stressed, but
/// in verse they usually fall on the offbeat, and a line reads them either way.
pub fn is_function_word(word: &str) -> bool {
    const WORDS: &[&str] = &[
        "a", "about", "above", "after", "against", "am", "among", "an", "and", "are", "as", "at",
        "be", "been", "before", "being", "below", "beneath", "but", "by", "can", "could", "did",
        "do", "does", "for", "from", "had", "has", "have", "he", "her", "hers", "him", "his",
        "how", "i", "if", "in", "into", "is", "it", "its", "may", "me", "might", "must", "my",
        "nor", "of", "on", "onto", "or", "our", "ours", "shall", "she", "should", "so", "than",
        "that", "the", "thee", "their", "them", "then", "there", "these", "they", "thine", "this",
        "those", "thou", "thy", "to", "unto", "upon", "us", "was", "we", "were", "what", "when",
        "where", "which", "while", "who", "whom", "whose", "why", "will", "with", "would", "ye",
        "yet", "you", "your", "yours", "o'er", "e'er", "ne'er", "th'", "'tis", "'twas",
    ];
    let key = normalize(word);
    WORDS.contains(&key.as_str())
}
