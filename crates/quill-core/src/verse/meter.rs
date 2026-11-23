//! A verse target meter (#1324): the foot, how many feet a line holds, and
//! whether the usual licences count against the line. Stored in a chapter's
//! front matter as plain words (`meter: iambic pentameter`), which [`Meter::parse`]
//! reads and [`Meter::key`] writes back in a canonical spelling.

use std::fmt;

use super::lines::stanza_slot;

/// A metrical foot: its pattern of stressed and unstressed syllables.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Foot {
    /// ˘ /
    Iamb,
    /// / ˘
    Trochee,
    /// ˘ ˘ /
    Anapest,
    /// / ˘ ˘
    Dactyl,
    /// ˘ / ˘
    Amphibrach,
    /// / /
    Spondee,
}

impl Foot {
    const ALL: [Self; 6] = [
        Self::Iamb,
        Self::Trochee,
        Self::Anapest,
        Self::Dactyl,
        Self::Amphibrach,
        Self::Spondee,
    ];

    /// The foot's syllables, `true` for a stressed one.
    pub const fn pattern(self) -> &'static [bool] {
        match self {
            Self::Iamb => &[false, true],
            Self::Trochee => &[true, false],
            Self::Anapest => &[false, false, true],
            Self::Dactyl => &[true, false, false],
            Self::Amphibrach => &[false, true, false],
            Self::Spondee => &[true, true],
        }
    }

    /// The adjective a meter is named with: *iambic* pentameter.
    pub const fn adjective(self) -> &'static str {
        match self {
            Self::Iamb => "iambic",
            Self::Trochee => "trochaic",
            Self::Anapest => "anapestic",
            Self::Dactyl => "dactylic",
            Self::Amphibrach => "amphibrachic",
            Self::Spondee => "spondaic",
        }
    }

    /// The foot's own name, for a report line: an inverted *iamb*.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Iamb => "iamb",
            Self::Trochee => "trochee",
            Self::Anapest => "anapest",
            Self::Dactyl => "dactyl",
            Self::Amphibrach => "amphibrach",
            Self::Spondee => "spondee",
        }
    }

    fn from_adjective(word: &str) -> Option<Self> {
        if word == "anapaestic" {
            return Some(Self::Anapest);
        }
        Self::ALL.into_iter().find(|f| f.adjective() == word)
    }
}

/// The names of line lengths, one foot to eight.
const LENGTHS: [&str; 8] = [
    "monometer",
    "dimeter",
    "trimeter",
    "tetrameter",
    "pentameter",
    "hexameter",
    "heptameter",
    "octameter",
];

/// Named forms an author may give instead of a foot and a length: the foot,
/// the lengths, and which length (if any) repeats to fill a stanza's middle.
const NAMED: &[(&str, Foot, &[u8], Option<usize>)] = &[
    ("blank verse", Foot::Iamb, &[5], None),
    ("heroic couplets", Foot::Iamb, &[5], None),
    ("alexandrines", Foot::Iamb, &[6], None),
    ("common meter", Foot::Iamb, &[4, 3], None),
    ("long meter", Foot::Iamb, &[4], None),
    ("short meter", Foot::Iamb, &[3, 3, 4, 3], None),
    // The ballad stanza stretched by tetrameter lines before its closing
    // trimeter: 4 3 4 3, 4 3 4 4 3, 4 3 4 4 4 3, … (#1348). Also a rhyme scheme
    // (ABC+B), so the one name sets both.
    ("extended ballad stanza", Foot::Iamb, &[4, 3, 4, 3], Some(2)),
];

/// Spellings that mean one of the [`NAMED`] forms.
const ALIASES: &[(&str, &str)] = &[
    ("heroic couplet", "heroic couplets"),
    ("alexandrine", "alexandrines"),
    ("common measure", "common meter"),
    ("ballad meter", "common meter"),
    ("ballad measure", "common meter"),
    ("long measure", "long meter"),
    ("short measure", "short meter"),
    ("extended ballad", "extended ballad stanza"),
    ("extended ballad meter", "extended ballad stanza"),
];

/// A target meter for a chapter or story.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meter {
    pub foot: Foot,
    /// Feet per line, cycling through a stanza's lines: `[5]` for pentameter,
    /// `[4, 3]` for common meter's alternating tetrameter and trimeter.
    pub lengths: Vec<u8>,
    /// The length that repeats to fill a stanza's middle, as an index into
    /// `lengths` (#1348). With `[4, 3, 4, 3]` and `Some(2)`, a stanza's first two
    /// lines and its last are checked as 4, 3, and 3, and every line between as
    /// 4, however long the stanza runs. `None` cycles `lengths` instead.
    pub stretch: Option<usize>,
    /// Whether a licence (a feminine ending, an inverted first foot, …) counts as
    /// a deviation rather than an accepted variation.
    pub strict: bool,
    /// The named form the author gave, when they gave one (`blank verse`).
    pub named: Option<&'static str>,
}

impl Meter {
    /// Read a meter from the author's words: a foot adjective and a length
    /// (`iambic pentameter`, `trochaic tetrameter`), lengths separated by `/` to
    /// alternate line by line (`iambic tetrameter/trimeter`), or a named form
    /// (`blank verse`, `common meter`). One length may end in `+` to repeat
    /// through the middle of each stanza (`iambic
    /// tetrameter/trimeter/tetrameter+/trimeter`). A leading `strict` counts the
    /// licences as deviations. Case and extra spaces don't matter; anything else
    /// is `None`.
    pub fn parse(text: &str) -> Option<Self> {
        let lower = text.trim().to_lowercase();
        let mut words: Vec<&str> = lower.split_whitespace().collect();
        let strict = words.first() == Some(&"strict");
        if strict {
            words.remove(0);
        }
        let rest = words.join(" ");
        let name = ALIASES
            .iter()
            .find(|(alias, _)| *alias == rest)
            .map_or(rest.as_str(), |(_, name)| name);
        if let Some(&(named, foot, lengths, stretch)) = NAMED.iter().find(|(n, ..)| *n == name) {
            return Some(Self {
                foot,
                lengths: lengths.to_vec(),
                stretch,
                strict,
                named: Some(named),
            });
        }
        let [adjective, lengths] = words.as_slice() else {
            return None;
        };
        let foot = Foot::from_adjective(adjective)?;
        let mut stretch = None;
        let mut feet = Vec::new();
        for (i, length) in lengths.split('/').enumerate() {
            let name = match length.strip_suffix('+') {
                // Only one length can stretch.
                Some(_) if stretch.is_some() => return None,
                Some(name) => {
                    stretch = Some(i);
                    name
                }
                None => length,
            };
            let n = LENGTHS.iter().position(|n| *n == name)?;
            feet.push(u8::try_from(n + 1).ok()?);
        }
        Some(Self {
            foot,
            lengths: feet,
            stretch,
            strict,
            named: None,
        })
    }

    /// The canonical words for this meter, as written to front matter and shown in
    /// the report: `iambic pentameter`, `strict common meter`.
    pub fn key(&self) -> String {
        let body = self.named.map_or_else(
            || {
                let lengths: Vec<String> = self
                    .lengths
                    .iter()
                    .enumerate()
                    .map(|(i, &n)| {
                        let name = LENGTHS
                            .get(usize::from(n).saturating_sub(1))
                            .copied()
                            .unwrap_or("?");
                        if self.stretch == Some(i) {
                            format!("{name}+")
                        } else {
                            name.to_string()
                        }
                    })
                    .collect();
                format!("{} {}", self.foot.adjective(), lengths.join("/"))
            },
            str::to_string,
        );
        if self.strict {
            format!("strict {body}")
        } else {
            body
        }
    }

    /// How many feet the `n`th line (from 0) of a stanza of `stanza_len` lines
    /// should hold.
    pub fn feet_for_line(&self, n: usize, stanza_len: usize) -> usize {
        if self.lengths.is_empty() {
            return 0;
        }
        let slot = stanza_slot(n, stanza_len, self.lengths.len(), self.stretch);
        self.lengths.get(slot).map_or(0, |&l| usize::from(l))
    }
}

impl fmt::Display for Meter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.key())
    }
}

/// Examples to show an author who typed a meter Quill doesn't know.
pub const METER_EXAMPLES: &str =
    "e.g. iambic pentameter, trochaic tetrameter, iambic tetrameter/trimeter, blank verse, common meter, extended ballad stanza; end a length in + to repeat it through a stanza's middle; prefix strict to count licences";
