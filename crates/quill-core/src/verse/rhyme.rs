//! Checking verse against a target rhyme scheme (#1325).
//!
//! The scheme is letters (`ABAB CDCD EFEF GG`, `X` for an unrhymed line) or a
//! named form, stored in front matter as `rhyme:`. Each line's end word is heard
//! through the pronouncing dictionary: two endings rhyme when their sound from
//! the last stressed vowel on matches. The report labels each pairing (perfect,
//! slant, eye, identical), gives each line the letter its sound earns, and flags
//! the lines where that disagrees with the scheme. Slant rhymes count only when
//! the author says so (`slant ABAB`). Quill only reports; it never offers a word
//! that would rhyme.

use std::fmt;
use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use super::lines::verse_lines;
use super::pronounce::pronounce;

/// The shape of a scheme.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Pattern {
    /// Letters, in groups. One group repeats down each stanza (`ABCB` for every
    /// ballad quatrain, `AA` for every couplet); several groups run in order
    /// across the poem's lines, whatever its stanza breaks (a sonnet).
    Letters(Vec<Vec<Option<u32>>>),
    /// Terza rima's interlocking tercets: ABA BCB CDC …
    TerzaRima,
}

/// Named schemes an author may give instead of letters.
const NAMED: &[(&str, &str)] = &[
    ("shakespearean sonnet", "ABAB CDCD EFEF GG"),
    ("petrarchan sonnet", "ABBAABBA CDECDE"),
    ("spenserian sonnet", "ABAB BCBC CDCD EE"),
    ("couplets", "AA"),
    ("ballad stanza", "ABCB"),
    ("limerick", "AABBA"),
    ("rhyme royal", "ABABBCC"),
    ("ottava rima", "ABABABCC"),
    ("villanelle", "ABA ABA ABA ABA ABA ABAA"),
];

/// Other spellings of the named schemes.
const ALIASES: &[(&str, &str)] = &[
    ("english sonnet", "shakespearean sonnet"),
    ("italian sonnet", "petrarchan sonnet"),
    ("heroic couplets", "couplets"),
    ("rhyming couplets", "couplets"),
    ("ballad", "ballad stanza"),
    ("limericks", "limerick"),
];

/// A target rhyme scheme.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RhymeScheme {
    pattern: Pattern,
    /// Whether a slant rhyme counts as a match.
    pub slant: bool,
    /// The named form the author gave, when they gave one.
    pub named: Option<&'static str>,
}

impl RhymeScheme {
    /// Read a scheme from the author's words: letters with spaces between
    /// groups (`ABAB CDCD EFEF GG`, case doesn't matter, `X` for an unrhymed
    /// line), a named form (`shakespearean sonnet`, `terza rima`, `limerick`), and
    /// an optional leading `slant` to count slant rhymes as matches.
    pub fn parse(text: &str) -> Option<Self> {
        let lower = text.trim().to_lowercase();
        let mut words: Vec<&str> = lower.split_whitespace().collect();
        let slant = words.first() == Some(&"slant");
        if slant {
            words.remove(0);
        }
        let rest = words.join(" ");
        if rest.is_empty() {
            return None;
        }
        let name = ALIASES
            .iter()
            .find(|(alias, _)| *alias == rest)
            .map_or(rest.as_str(), |(_, name)| name);
        if name == "terza rima" {
            return Some(Self {
                pattern: Pattern::TerzaRima,
                slant,
                named: Some("terza rima"),
            });
        }
        if let Some(&(named, letters)) = NAMED.iter().find(|(n, _)| *n == name) {
            return Some(Self {
                pattern: Pattern::Letters(letter_groups(letters)?),
                slant,
                named: Some(named),
            });
        }
        Some(Self {
            pattern: Pattern::Letters(letter_groups(&rest)?),
            slant,
            named: None,
        })
    }

    /// The canonical words for this scheme, as written to front matter.
    pub fn key(&self) -> String {
        let body = match (&self.pattern, self.named) {
            (_, Some(name)) => name.to_string(),
            (Pattern::Letters(groups), None) => groups
                .iter()
                .map(|g| g.iter().map(|l| letter_name(*l)).collect::<String>())
                .collect::<Vec<_>>()
                .join(" "),
            (Pattern::TerzaRima, None) => "terza rima".into(),
        };
        if self.slant {
            format!("slant {body}")
        } else {
            body
        }
    }

    /// The scheme as letters, for a named form too (`ABAB CDCD EFEF GG`).
    pub fn letters(&self) -> String {
        match &self.pattern {
            Pattern::Letters(groups) => groups
                .iter()
                .map(|g| g.iter().map(|l| letter_name(*l)).collect::<String>())
                .collect::<Vec<_>>()
                .join(" "),
            Pattern::TerzaRima => "ABA BCB CDC \u{2026}".into(),
        }
    }

    /// What the scheme expects of each line: the block its letters belong to
    /// (letters only pair up within a block) and its letter, `None` for an
    /// unrhymed line.
    fn expected(&self, lines: &[(usize, usize)]) -> Vec<(usize, Option<u32>)> {
        match &self.pattern {
            Pattern::TerzaRima => (0..lines.len())
                .map(|i| {
                    let tercet = u32::try_from(i / 3).unwrap_or(u32::MAX - 1);
                    let letter = if i % 3 == 1 { tercet + 1 } else { tercet };
                    (0, Some(letter))
                })
                .collect(),
            Pattern::Letters(groups) if groups.len() == 1 => {
                let group = &groups[0];
                lines
                    .iter()
                    .map(|&(stanza, position)| {
                        let block = (stanza, position / group.len());
                        (block_id(block), group[position % group.len()])
                    })
                    .collect()
            }
            Pattern::Letters(groups) => {
                let flat: Vec<Option<u32>> = groups.iter().flatten().copied().collect();
                (0..lines.len())
                    .map(|i| (i / flat.len(), flat[i % flat.len()]))
                    .collect()
            }
        }
    }
}

impl fmt::Display for RhymeScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.key())
    }
}

/// Examples to show an author who typed a scheme Quill doesn't know.
pub const RHYME_EXAMPLES: &str = "e.g. ABAB CDCD EFEF GG, ABCB, AABBA, shakespearean sonnet, terza rima, couplets; X for an unrhymed line; prefix slant to accept slant rhymes";

/// A stanza and a group number as one block id, so lines in different stanzas
/// never share a letter.
const fn block_id((stanza, group): (usize, usize)) -> usize {
    stanza.wrapping_mul(1 << 16).wrapping_add(group)
}

/// Letter groups from text like `abab cdcd`: each letter numbered in order
/// with X skipped (A is 0, W is 22, Y is 23), and X for an unrhymed line.
fn letter_groups(text: &str) -> Option<Vec<Vec<Option<u32>>>> {
    let groups: Option<Vec<Vec<Option<u32>>>> = text
        .split_whitespace()
        .map(|g| {
            g.chars()
                .map(|c| match c.to_ascii_uppercase() {
                    'X' => Some(None),
                    c @ 'A'..='W' => Some(Some(u32::from(c) - u32::from('A'))),
                    // Past X, which means unrhymed, the letters count on from W.
                    'Y' => Some(Some(23)),
                    'Z' => Some(Some(24)),
                    _ => None,
                })
                .collect()
        })
        .collect();
    groups.filter(|g| !g.is_empty())
}

/// A letter's name from its number (see [`letter_groups`]): A..W, Y, Z, and
/// past Z a numbered A (`A2`), which only a long terza rima reaches.
pub fn letter_name(letter: Option<u32>) -> String {
    match letter {
        None => "X".into(),
        Some(n) if n < 23 => {
            char::from_u32(u32::from('A') + n).map_or_else(|| "?".into(), String::from)
        }
        // Skip X, which means unrhymed.
        Some(n) if n < 25 => {
            char::from_u32(u32::from('A') + n + 1).map_or_else(|| "?".into(), String::from)
        }
        Some(n) => format!("A{}", n / 25 + 1),
    }
}

/// How two line endings sound against each other, best first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RhymeKind {
    /// The same word, or the same sound throughout (*rite*, *right*).
    Identical,
    /// The same sound from the last stressed vowel on (*day*, *May*).
    Perfect,
    /// Spelled alike but said differently (*love*, *move*).
    Eye,
    /// The last stressed vowel or the closing consonants match, not both
    /// (*shape*, *keep*; *eyes*, *light*).
    Slant,
    /// No rhyme.
    None,
}

impl fmt::Display for RhymeKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Identical => "identical rhyme",
            Self::Perfect => "perfect rhyme",
            Self::Eye => "eye rhyme",
            Self::Slant => "slant rhyme",
            Self::None => "no rhyme",
        })
    }
}

/// Where a line breaks the scheme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RhymeFault {
    /// It should rhyme with an earlier line (its number, from 0) and doesn't;
    /// how the two do sound together.
    Missing { with: usize, kind: RhymeKind },
    /// It should bring a new sound but rhymes with an earlier line.
    Unwanted { with: usize },
}

/// One line's ending, checked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RhymeLine {
    /// The line's number in the text, from 0.
    pub line: usize,
    pub stanza: usize,
    pub text: String,
    /// The end word, and where it sits in the line.
    pub word: String,
    pub range: Range<usize>,
    /// The letter its sound earns, counted within its block like the scheme's.
    pub heard: Option<u32>,
    /// The letter the scheme expects (`None` for an unrhymed line).
    pub expected: Option<u32>,
    /// The earlier line it pairs with, and how they rhyme.
    pub partner: Option<(usize, RhymeKind)>,
    pub fault: Option<RhymeFault>,
    /// Whether the end word's sound was guessed from its spelling.
    pub guessed: bool,
}

impl RhymeLine {
    pub const fn fits(&self) -> bool {
        self.fault.is_none()
    }
}

/// A line's end word, heard.
struct Ending {
    word: String,
    range: Range<usize>,
    /// Each pronunciation's phones, stress digits kept.
    phones: Vec<String>,
    guessed: bool,
}

fn ending(line: &str) -> Option<Ending> {
    let (start, word) = line
        .unicode_word_indices()
        .rfind(|(_, w)| w.chars().any(char::is_alphabetic))?;
    let sound = pronounce(word);
    Some(Ending {
        word: word.to_lowercase().replace('\u{2019}', "'"),
        range: start..start + word.len(),
        phones: sound
            .prons
            .into_iter()
            .map(|p| p.phones)
            .filter(|p| !p.is_empty())
            .collect(),
        guessed: sound.guessed,
    })
}

/// A pronunciation's rhyming part: the last stressed vowel (or the last vowel,
/// when none is stressed) and the phones after it, stress digits dropped.
fn rhyme_part(phones: &str) -> (String, Vec<String>) {
    let all: Vec<&str> = phones.split_whitespace().collect();
    let vowel = |p: &&str| p.ends_with(['0', '1', '2']);
    let stressed = |p: &&str| p.ends_with(['1', '2']);
    let at = all
        .iter()
        .rposition(stressed)
        .or_else(|| all.iter().rposition(vowel))
        .unwrap_or(0);
    let bare = |p: &str| p.trim_end_matches(['0', '1', '2']).to_string();
    let nucleus = all.get(at).map_or_else(String::new, |p| bare(p));
    let coda = all.iter().skip(at + 1).map(|p| bare(p)).collect();
    (nucleus, coda)
}

/// The spelled ending: from the last vowel group to the end, with a silent
/// final `e` looked past (`love` and `move` both end `ove`).
fn spelled_tail(word: &str) -> String {
    let letters: Vec<char> = word.chars().filter(|c| c.is_alphabetic()).collect();
    let is_vowel = |c: char| matches!(c, 'a' | 'e' | 'i' | 'o' | 'u' | 'y');
    let mut core = letters.len();
    if core > 2 && letters[core - 1] == 'e' && !is_vowel(letters[core - 2]) {
        core -= 1;
    }
    let Some(last) = letters[..core].iter().rposition(|&c| is_vowel(c)) else {
        return letters.iter().collect();
    };
    let start = letters[..=last]
        .iter()
        .rposition(|&c| !is_vowel(c))
        .map_or(0, |i| i + 1);
    letters[start..].iter().collect()
}

/// How two endings sound against each other: the best over their
/// pronunciations. Without a pronunciation for one of them, the spelling
/// decides.
fn relation(a: &Ending, b: &Ending) -> RhymeKind {
    if a.word == b.word {
        return RhymeKind::Identical;
    }
    if a.phones.is_empty() || b.phones.is_empty() {
        return if spelled_tail(&a.word) == spelled_tail(&b.word) {
            RhymeKind::Perfect
        } else {
            RhymeKind::None
        };
    }
    let mut best = RhymeKind::None;
    for pa in &a.phones {
        for pb in &b.phones {
            let kind = if strip(pa) == strip(pb) {
                RhymeKind::Identical
            } else {
                let (va, ca) = rhyme_part(pa);
                let (vb, cb) = rhyme_part(pb);
                if va == vb && ca == cb {
                    RhymeKind::Perfect
                } else if spelled_tail(&a.word) == spelled_tail(&b.word) {
                    RhymeKind::Eye
                } else if va == vb || (!ca.is_empty() && ca == cb) {
                    RhymeKind::Slant
                } else {
                    RhymeKind::None
                }
            };
            best = best.min(kind);
        }
    }
    best
}

/// Phones without stress digits.
fn strip(phones: &str) -> String {
    phones.replace(['0', '1', '2'], "")
}

impl RhymeKind {
    /// Whether this pairing counts as the two lines rhyming.
    const fn counts(self, slant: bool) -> bool {
        match self {
            Self::Identical | Self::Perfect => true,
            Self::Slant => slant,
            Self::Eye | Self::None => false,
        }
    }
}

/// Check every verse line's ending in `text` against `scheme`.
pub fn check_rhyme(text: &str, scheme: &RhymeScheme) -> Vec<RhymeLine> {
    let lines: Vec<_> = verse_lines(text)
        .into_iter()
        .filter_map(|l| ending(l.text).map(|e| (l, e)))
        .collect();
    let places: Vec<(usize, usize)> = lines.iter().map(|(l, _)| (l.stanza, l.position)).collect();
    let expected = scheme.expected(&places);
    let mut out: Vec<RhymeLine> = Vec::with_capacity(lines.len());
    for (i, ((line, end), &(block, want))) in lines.iter().zip(&expected).enumerate() {
        // The earlier lines of the same block.
        let earlier: Vec<usize> = (0..i).filter(|&j| expected[j].0 == block).collect();
        let heard_with = earlier
            .iter()
            .copied()
            .find(|&j| relation(end, &lines[j].1).counts(scheme.slant));
        let heard = heard_with.map_or_else(|| Some(next_letter(&earlier, &out)), |j| out[j].heard);
        let mates: Vec<usize> = earlier
            .iter()
            .copied()
            .filter(|&j| want.is_some() && expected[j].1 == want)
            .collect();
        let mut partner = None;
        let mut fault = None;
        if let Some(&first) = mates.first() {
            let rhyming = mates
                .iter()
                .map(|&j| (j, relation(end, &lines[j].1)))
                .find(|(_, k)| k.counts(scheme.slant));
            if let Some((j, kind)) = rhyming {
                partner = Some((lines[j].0.line, kind));
            } else {
                let kind = relation(end, &lines[first].1);
                partner = Some((lines[first].0.line, kind));
                fault = Some(RhymeFault::Missing {
                    with: lines[first].0.line,
                    kind,
                });
            }
        } else if want.is_some() {
            // A new sound is wanted; a perfect rhyme with an earlier line of
            // another letter breaks the scheme.
            let unwanted = earlier.iter().copied().find(|&j| {
                expected[j].1.is_some()
                    && matches!(
                        relation(end, &lines[j].1),
                        RhymeKind::Perfect | RhymeKind::Identical
                    )
            });
            if let Some(j) = unwanted {
                fault = Some(RhymeFault::Unwanted {
                    with: lines[j].0.line,
                });
            }
        }
        out.push(RhymeLine {
            line: line.line,
            stanza: line.stanza,
            text: line.text.to_string(),
            word: end.word.clone(),
            range: end.range.clone(),
            heard,
            expected: want,
            partner,
            fault,
            guessed: end.guessed,
        });
    }
    out
}

/// The next unused letter in a block: one past the distinct letters its earlier
/// lines have earned.
fn next_letter(earlier: &[usize], out: &[RhymeLine]) -> u32 {
    let mut seen: Vec<Option<u32>> = Vec::new();
    for &j in earlier {
        if !seen.contains(&out[j].heard) {
            seen.push(out[j].heard);
        }
    }
    u32::try_from(seen.len()).unwrap_or(u32::MAX)
}

/// Totals over a rhyme check.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RhymeSummary {
    pub lines: usize,
    pub fitting: usize,
    /// Each stanza's fitting endings and total endings, in order.
    pub stanzas: Vec<(usize, usize)>,
}

pub fn summarize_rhyme(lines: &[RhymeLine]) -> RhymeSummary {
    let mut summary = RhymeSummary {
        lines: lines.len(),
        ..RhymeSummary::default()
    };
    for line in lines {
        if summary.stanzas.len() <= line.stanza {
            summary.stanzas.resize(line.stanza + 1, (0, 0));
        }
        summary.stanzas[line.stanza].1 += 1;
        if line.fits() {
            summary.stanzas[line.stanza].0 += 1;
            summary.fitting += 1;
        }
    }
    summary.stanzas.retain(|&(_, total)| total > 0);
    summary
}
