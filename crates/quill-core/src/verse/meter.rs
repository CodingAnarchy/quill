//! A verse target meter (#1324): the foot, how many feet a line holds, and
//! whether the usual licences count against the line. Stored in a chapter's
//! front matter as plain words (`meter: iambic pentameter`), which [`Meter::parse`]
//! reads and [`Meter::key`] writes back in a canonical spelling.

use std::fmt;

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

/// Named forms an author may give instead of a foot and a length.
const NAMED: &[(&str, Foot, &[u8])] = &[
    ("blank verse", Foot::Iamb, &[5]),
    ("heroic couplets", Foot::Iamb, &[5]),
    ("alexandrines", Foot::Iamb, &[6]),
    ("common meter", Foot::Iamb, &[4, 3]),
    ("long meter", Foot::Iamb, &[4]),
    ("short meter", Foot::Iamb, &[3, 3, 4, 3]),
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
];

/// A target meter for a chapter or story.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Meter {
    pub foot: Foot,
    /// Feet per line, cycling through a stanza's lines: `[5]` for pentameter,
    /// `[4, 3]` for common meter's alternating tetrameter and trimeter.
    pub lengths: Vec<u8>,
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
    /// (`blank verse`, `common meter`). A leading `strict` counts the licences as
    /// deviations. Case and extra spaces don't matter; anything else is `None`.
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
        if let Some(&(named, foot, lengths)) = NAMED.iter().find(|(n, ..)| *n == name) {
            return Some(Self {
                foot,
                lengths: lengths.to_vec(),
                strict,
                named: Some(named),
            });
        }
        let [adjective, lengths] = words.as_slice() else {
            return None;
        };
        let foot = Foot::from_adjective(adjective)?;
        let lengths = lengths
            .split('/')
            .map(|l| {
                LENGTHS
                    .iter()
                    .position(|n| *n == l)
                    .and_then(|i| u8::try_from(i + 1).ok())
            })
            .collect::<Option<Vec<u8>>>()?;
        Some(Self {
            foot,
            lengths,
            strict,
            named: None,
        })
    }

    /// The canonical words for this meter, as written to front matter and shown in
    /// the report: `iambic pentameter`, `strict common meter`.
    pub fn key(&self) -> String {
        let body = self.named.map_or_else(
            || {
                let lengths: Vec<&str> = self
                    .lengths
                    .iter()
                    .map(|&n| {
                        LENGTHS
                            .get(usize::from(n).saturating_sub(1))
                            .copied()
                            .unwrap_or("?")
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

    /// How many feet the `n`th line of a stanza (from 0) should hold.
    pub fn feet_for_line(&self, n: usize) -> usize {
        match self.lengths.as_slice() {
            [] => 0,
            lengths => usize::from(lengths[n % lengths.len()]),
        }
    }
}

impl fmt::Display for Meter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.key())
    }
}

/// Examples to show an author who typed a meter Quill doesn't know.
pub const METER_EXAMPLES: &str =
    "e.g. iambic pentameter, trochaic tetrameter, iambic tetrameter/trimeter, blank verse, common meter; prefix strict to count licences";
