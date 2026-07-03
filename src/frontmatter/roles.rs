//! Section roles, ordering bands, and revision-status enums for chapter
//! front matter (#179, #180, #435, #636). Each enum is declared from a single
//! table macro so a variant cannot be added without its key, label, and band —
//! the accessors and the `from_key` parse all derive from the one row.
//!
//! Re-exported from [`super`], so callers still use `frontmatter::{Status,
//! Role, Band, SceneStatus}` unchanged.

/// Declare the [`Status`] enum and its accessors from one table (#435): each
/// variant's `status:` key, status-bar label, and pane badge live on a single
/// line, so adding a status can't leave an accessor — or the `from_key` parse —
/// silently behind. [`Status::next`] cycles in the declaration order below.
macro_rules! status_table {
    ($($variant:ident => $key:literal, $label:literal, $badge:literal);+ $(;)?) => {
        /// A chapter's revision status, cycled by the author and shown in the pane.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Status { $($variant),+ }

        impl Status {
            /// The `status:` value as written in front matter.
            pub const fn key(self) -> &'static str {
                match self { $(Status::$variant => $key),+ }
            }

            /// Parse a front-matter `status:` value (case-insensitive).
            pub fn from_key(s: &str) -> Option<Status> {
                match s.trim().to_ascii_lowercase().as_str() {
                    $($key => Some(Status::$variant),)+
                    _ => None,
                }
            }

            /// Human label for the status bar.
            pub const fn label(self) -> &'static str {
                match self { $(Status::$variant => $label),+ }
            }

            /// Single-character badge for the chapter pane.
            pub const fn badge(self) -> char {
                match self { $(Status::$variant => $badge),+ }
            }

            /// The next status in the cycle (the declaration order below,
            /// wrapping back to the first): draft → revised → final → draft.
            pub fn next(self) -> Status {
                const ALL: &[Status] = &[$(Status::$variant),+];
                let i = ALL.iter().position(|&s| s == self).unwrap_or(0);
                ALL[(i + 1) % ALL.len()]
            }
        }
    };
}

status_table! {
    Draft => "draft", "Draft", 'D';
    Revised => "revised", "Revised", 'R';
    Final => "final", "Final", 'F';
}

/// Declare the [`SceneStatus`] enum and its accessors from one table, mirroring
/// [`status_table`] (#636): each variant's `status:` value, its corkboard label,
/// and a single-character badge on one line. [`SceneStatus::next`] cycles in the
/// declaration order (stub → draft → done → stub), and [`SceneStatus::from_key`]
/// parses the value case-insensitively.
macro_rules! scene_status_table {
    ($($variant:ident => $key:literal, $label:literal, $badge:literal);+ $(;)?) => {
        /// A single scene's drafting status (#636), edited from the corkboard and
        /// stored per scene in the chapter's `scenes:` front matter. Distinct from
        /// the whole-chapter [`Status`]: a chapter holds many scenes, each at its
        /// own stage.
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
        pub enum SceneStatus {
            #[default]
            $($variant),+
        }

        impl SceneStatus {
            /// The value as written in the `scenes:` front matter.
            pub const fn key(self) -> &'static str {
                match self { $(SceneStatus::$variant => $key),+ }
            }

            /// Parse a scene `status:` value (case-insensitive); `None` when unknown.
            pub fn from_key(s: &str) -> Option<SceneStatus> {
                match s.trim().to_ascii_lowercase().as_str() {
                    $($key => Some(SceneStatus::$variant),)+
                    _ => None,
                }
            }

            /// Human label for the corkboard card.
            pub const fn label(self) -> &'static str {
                match self { $(SceneStatus::$variant => $label),+ }
            }

            /// Single-character badge for the card.
            pub const fn badge(self) -> char {
                match self { $(SceneStatus::$variant => $badge),+ }
            }

            /// The next status in the cycle (declaration order, wrapping).
            pub fn next(self) -> SceneStatus {
                const ALL: &[SceneStatus] = &[$(SceneStatus::$variant),+];
                let i = ALL.iter().position(|&s| s == self).unwrap_or(0);
                ALL[(i + 1) % ALL.len()]
            }
        }
    };
}

scene_status_table! {
    Stub => "stub", "Stub", '·';
    Draft => "draft", "Draft", '◐';
    Done => "done", "Done", '●';
}

/// Declare the [`Role`] enum and its accessors from one table (#435): each
/// variant's `role:` key (plus any accepted alias spellings), label, and
/// ordering band live on a single line, so adding a section role can't parse
/// without a label or sort into the wrong band by a missed `match` arm. A body
/// chapter has no role (`None`), so existing projects — which have no `role:`
/// key — read as plain chapters (#179).
macro_rules! role_table {
    ($($variant:ident => $key:literal $(| $alias:literal)*, $label:literal, $band:ident, $epub_type:literal);+ $(;)?) => {
        /// A project file's **section role** (#179): a front- or back-matter
        /// section type for the non-chapter parts of a book.
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Role { $($variant),+ }

        impl Role {
            /// Which ordering band this section sits in (#180).
            pub const fn band(self) -> Band {
                match self { $(Role::$variant => Band::$band),+ }
            }

            /// The EPUB 3 structural `epub:type` for this section (#695), so the
            /// EPUB writer can tag the section's `<body>` and its landmark with the
            /// precise semantic (`foreword`, `dedication`, `glossary`, …) rather
            /// than the generic front/back-matter band. `None` for a role with no
            /// distinct term in the EPUB Structural Semantics Vocabulary (the
            /// writer falls back to the band's generic `frontmatter`/`backmatter`);
            /// every `Some` value is a term epubcheck/ACE recognize.
            pub const fn epub_type(self) -> Option<&'static str> {
                match self { $(Role::$variant => none_if_empty($epub_type)),+ }
            }

            /// The `role:` value as written in front matter.
            pub const fn key(self) -> &'static str {
                match self { $(Role::$variant => $key),+ }
            }

            /// Parse a front-matter `role:` value (case-insensitive). Accepts the
            /// British "acknowledgements" spelling too. `None` for an unknown
            /// value (read as a plain body chapter).
            pub fn from_key(s: &str) -> Option<Role> {
                match s.trim().to_ascii_lowercase().as_str() {
                    $($key $(| $alias)* => Some(Role::$variant),)+
                    _ => None,
                }
            }

            /// Capitalized label for the chapter pane and messages.
            pub const fn label(self) -> &'static str {
                match self { $(Role::$variant => $label),+ }
            }
        }
    };
}

role_table! {
    // Front matter (sorts before the body), in conventional reading order. The
    // final column is the EPUB Structural Semantics term (#695); an empty string
    // means "no distinct term — use the band's generic frontmatter/backmatter".
    Dedication => "dedication", "Dedication", Front, "dedication";
    Foreword => "foreword", "Foreword", Front, "foreword";
    Preface => "preface", "Preface", Front, "preface";
    Introduction => "introduction", "Introduction", Front, "introduction";
    Prologue => "prologue", "Prologue", Front, "prologue";
    // Back matter (sorts after the body), in conventional reading order.
    Epilogue => "epilogue", "Epilogue", Back, "epilogue";
    Afterword => "afterword", "Afterword", Back, "afterword";
    Acknowledgments => "acknowledgments" | "acknowledgements", "Acknowledgments", Back, "acknowledgments";
    Appendix => "appendix", "Appendix", Back, "appendix";
    Glossary => "glossary", "Glossary", Back, "glossary";
    // "also by" and "about the author" have no distinct EPUB term — they keep the
    // generic backmatter semantic.
    AlsoBy => "also-by" | "also by the author", "Also by the Author", Back, "";
    AboutTheAuthor => "about-the-author" | "about the author", "About the Author", Back, "";
    Colophon => "colophon", "Colophon", Back, "colophon";
}

/// `None` for an empty string, else `Some(s)` — lets a [`role_table!`] row leave
/// the EPUB-type column blank to mean "no distinct term, use the band default".
const fn none_if_empty(s: &'static str) -> Option<&'static str> {
    if s.is_empty() {
        None
    } else {
        Some(s)
    }
}

/// The three ordering bands of a manuscript (#180): front matter sorts before
/// the body, which sorts before back matter. Declared in that order so the
/// derived `Ord` sorts chapters into reading order.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Band {
    Front,
    Body,
    Back,
}

impl Band {
    /// The band a file belongs to: a role's band, or `Body` when it has no role.
    pub const fn of(role: Option<Role>) -> Self {
        match role {
            Some(r) => r.band(),
            None => Self::Body,
        }
    }
}
