//! Structure templates for a new project (#611): a small, pure-data table that
//! scaffolds a starter skeleton at create time. "Empty" reproduces the historic
//! behavior (one blank chapter); the others write a `[[parts]]` grouping plus
//! titled empty chapter stubs, so an author starting a structured novel doesn't
//! hand-build the skeleton or hand-edit `book.toml`.
//!
//! Templates are curated static data — no AI, offline — in the spirit of the
//! curated font list. Each non-empty template is a list of named parts, and each
//! part carries the titles of its (empty) chapter stubs; [`super::Project::create_with_template`]
//! walks them through the existing `add_chapter`/manifest path.

/// One structure template offered when creating a project. `parts` is empty for
/// the "Empty" template (the historic single-blank-chapter default) and
/// otherwise names each part alongside the titles of its empty chapter stubs.
pub struct ProjectTemplate {
    /// The name shown in the picker (e.g. "Empty", "Three-act").
    pub name: &'static str,
    /// A one-line description of what the template scaffolds.
    pub desc: &'static str,
    /// The named parts to scaffold, each with its chapter-stub titles. Empty for
    /// the "Empty" template, which falls back to a single blank starter chapter.
    pub parts: &'static [TemplatePart],
}

/// A named part in a template, plus the titles of the empty chapter stubs it
/// groups. The chapter range in the manifest's `[[parts]]` is derived from the
/// stub counts as the chapters are scaffolded in order.
pub struct TemplatePart {
    /// The part name written to the manifest (e.g. "Act I — Setup").
    pub name: &'static str,
    /// Titles of the empty chapter stubs under this part, in order.
    pub chapters: &'static [&'static str],
}

/// The curated templates, in the order the picker shows them. The first entry is
/// always "Empty" (the default), so a create with no explicit choice reproduces
/// the historic behavior.
pub const TEMPLATES: &[ProjectTemplate] = &[
    ProjectTemplate {
        name: "Empty",
        desc: "One blank chapter — start from scratch",
        parts: &[],
    },
    ProjectTemplate {
        name: "Three-act",
        desc: "Three acts with titled chapter stubs",
        parts: &[
            TemplatePart {
                name: "Act I \u{2014} Setup",
                chapters: &["Opening", "Inciting Incident", "First Turning Point"],
            },
            TemplatePart {
                name: "Act II \u{2014} Confrontation",
                chapters: &["Rising Action", "Midpoint", "Complications", "Crisis"],
            },
            TemplatePart {
                name: "Act III \u{2014} Resolution",
                chapters: &["Climax", "Falling Action", "Resolution"],
            },
        ],
    },
];

impl ProjectTemplate {
    /// The template at `index` in [`TEMPLATES`], or the default "Empty" template
    /// when the index is out of range (so a stale picker selection can't panic).
    pub fn at(index: usize) -> &'static Self {
        TEMPLATES.get(index).unwrap_or(&TEMPLATES[0])
    }

    /// Whether this template scaffolds no structure — the historic "Empty"
    /// default (a single blank chapter, no parts).
    pub const fn is_empty(&self) -> bool {
        self.parts.is_empty()
    }
}
