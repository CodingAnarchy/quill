//! YAML front matter at the top of a chapter file: a `---`-delimited block that
//! carries chapter metadata (status, summary, …) above the prose.
//!
//! For now Quill preserves the block **verbatim** — it splits it off so the
//! editor shows only the body, and re-attaches it byte-for-byte on save, so
//! unknown keys and an author's hand formatting survive untouched. Typed access
//! to individual fields lands with the metadata tickets that build on this.
//!
//! The concern is split across submodules, all re-exported here so callers use
//! `frontmatter::…` unchanged:
//! - [`block`] — the `---`-fence [`split`]/[`join`] round-trip (#872).
//! - [`yaml`] — the surgical key/scalar/`|`-block editing under [`FrontMatter`].
//! - [`meta`] — the typed [`Epigraph`]/[`SceneMeta`] types and field accessors.
//! - [`roles`] — the [`Status`]/[`Role`]/[`Band`]/[`SceneStatus`] enums.

mod block;
mod meta;
mod roles;
mod yaml;

pub use block::{join, split};
pub use meta::{Epigraph, SceneMeta};
pub use roles::{series_list_lines, Band, Role, SceneStatus, Status};
// The generated series-list heading (#820) is part of the public front-matter API
// (the export writers key their pages off `series_list_lines`, which embeds it) and
// is asserted on directly by the writer tests; the binary itself references it only
// through `series_list_lines`, so silence the unused re-export lint in that build.
#[cfg_attr(not(test), allow(unused_imports))]
pub use roles::SERIES_LIST_HEADING;

/// A chapter's front matter, held as the verbatim block between the `---`
/// fences. Reads and writes are **surgical**: a write replaces (or appends) only
/// the one key's entry, so unknown keys and an author's hand formatting are left
/// exactly as they were — preserving the round-trip guarantee from the
/// foundation while still giving typed access to the fields Quill manages.
///
/// Scope: top-level keys whose value is a scalar or a single-level literal
/// (`|`) block (the metadata Quill writes — e.g. a multi-line epigraph). Nested
/// mappings and sequences are preserved verbatim but not interpreted; if a
/// future field needs those, this is where a real parse-edit-reserialize layer
/// would go.
///
/// The raw-block editing lives in [`yaml`]; the typed field accessors in
/// [`meta`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FrontMatter {
    raw: String,
}

impl FrontMatter {
    /// Wrap the verbatim block text (between the fences) returned by [`split`].
    pub fn from_raw(inner: &str) -> Self {
        Self {
            raw: inner.to_string(),
        }
    }

    /// The verbatim block, for [`join`].
    pub fn as_str(&self) -> &str {
        &self.raw
    }

    /// Whether the block has no content (no keys to write).
    pub fn is_empty(&self) -> bool {
        self.raw.trim().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::yaml::split_lines;
    use super::*;

    #[test]
    fn no_front_matter_leaves_text_as_the_body() {
        let text = "# Chapter\n\nProse here.\n";
        let (front, body) = split(text);
        assert_eq!(front, None);
        assert_eq!(body, text);
    }

    #[test]
    fn splits_a_fenced_block_from_the_body() {
        let text = "---\nstatus: draft\npov: Mara\n---\n# Chapter\n\nProse.\n";
        let (front, body) = split(text);
        assert_eq!(front, Some("status: draft\npov: Mara\n"));
        assert_eq!(body, "# Chapter\n\nProse.\n");
    }

    #[test]
    fn split_then_join_round_trips() {
        let text = "---\nstatus: draft\n---\nThe body.\n";
        let (front, body) = split(text);
        assert_eq!(&*join(front, body), text);
    }

    #[test]
    fn splits_a_crlf_fenced_block_and_round_trips() {
        // A chapter imported or edited on Windows: CRLF fences and CRLF inside
        // the block. It must split (YAML off, prose in the body) and re-join
        // byte-for-byte, never leaking the YAML into the prose.
        let text = "---\r\nstatus: draft\r\npov: Mara\r\n---\r\n# Chapter\r\n\r\nProse.\r\n";
        let (front, body) = split(text);
        assert_eq!(front, Some("status: draft\r\npov: Mara\r\n"));
        assert_eq!(body, "# Chapter\r\n\r\nProse.\r\n");
        // The YAML block does not leak into the body.
        assert!(!body.contains("status:"));
        // Save reproduces the original chapter exactly, CRLF fences and all.
        assert_eq!(&*join(front, body), text);
    }

    #[test]
    fn an_empty_crlf_block_round_trips() {
        // With no inner lines the fence newline is read from the body.
        let text = "---\r\n---\r\nBody only.\r\n";
        let (front, body) = split(text);
        assert_eq!(front, Some(""));
        assert_eq!(body, "Body only.\r\n");
        assert_eq!(&*join(front, body), text);
    }

    #[test]
    fn an_empty_block_round_trips() {
        let text = "---\n---\nBody only.\n";
        let (front, body) = split(text);
        assert_eq!(front, Some(""));
        assert_eq!(body, "Body only.\n");
        assert_eq!(&*join(front, body), text);
    }

    #[test]
    fn a_leading_rule_without_a_closing_fence_is_body() {
        // A Markdown thematic break at the top is not front matter.
        let text = "---\njust a horizontal rule, no closing fence\n";
        let (front, body) = split(text);
        assert_eq!(front, None);
        assert_eq!(body, text);
    }

    #[test]
    fn join_none_is_the_body_unchanged() {
        assert_eq!(&*join(None, "Body.\n"), "Body.\n");
    }

    #[test]
    fn set_replaces_a_key_in_place_and_leaves_others_untouched() {
        let mut fm = FrontMatter::from_raw("status: draft\npov: Mara\ncustom: keep me\n");
        fm.set_status(Status::Final);
        // The status line changed; the other keys (incl. an unknown one) are intact.
        assert_eq!(fm.status(), Some(Status::Final));
        assert_eq!(fm.pov().as_deref(), Some("Mara"));
        assert!(fm.as_str().contains("custom: keep me\n"));
        assert_eq!(fm.as_str(), "status: final\npov: Mara\ncustom: keep me\n");
    }

    #[test]
    fn set_appends_a_missing_key() {
        let mut fm = FrontMatter::default();
        fm.set_status(Status::Draft);
        fm.set_pov("Jonah");
        assert_eq!(fm.as_str(), "status: draft\npov: Jonah\n");
    }

    #[test]
    fn summary_round_trips_through_quoting() {
        // A summary with a colon must be quoted to stay valid, and read back whole.
        let mut fm = FrontMatter::default();
        fm.set_summary("The reckoning: part one");
        assert!(fm
            .as_str()
            .contains(r#"summary: "The reckoning: part one""#));
        assert_eq!(fm.summary().as_deref(), Some("The reckoning: part one"));
    }

    #[test]
    fn blank_text_removes_the_key() {
        let mut fm = FrontMatter::from_raw("summary: old\nstatus: draft\n");
        fm.set_summary("   ");
        assert_eq!(fm.summary(), None);
        assert!(!fm.as_str().contains("summary"));
        // Other keys survive the removal.
        assert_eq!(fm.status(), Some(Status::Draft));
    }

    #[test]
    fn role_round_trips_and_clears() {
        let mut fm = FrontMatter::from_raw("status: draft\n");
        assert_eq!(fm.role(), None); // absent → body chapter
        fm.set_role(Some(Role::Prologue));
        assert!(fm.as_str().contains("role: prologue\n"));
        assert_eq!(fm.role(), Some(Role::Prologue));
        assert_eq!(fm.status(), Some(Status::Draft)); // sibling untouched
                                                      // Clearing drops the key (back to a body chapter).
        fm.set_role(None);
        assert_eq!(fm.role(), None);
        assert!(!fm.as_str().contains("role"));
    }

    #[test]
    fn role_keys_parse_case_insensitively_with_the_british_alias() {
        assert_eq!(Role::from_key("Prologue"), Some(Role::Prologue));
        assert_eq!(Role::from_key("  EPILOGUE "), Some(Role::Epilogue));
        assert_eq!(
            Role::from_key("acknowledgements"),
            Some(Role::Acknowledgments)
        );
        assert_eq!(
            Role::from_key("acknowledgments"),
            Some(Role::Acknowledgments)
        );
        assert_eq!(Role::from_key("chapter"), None); // unknown → body
        assert_eq!(Role::Appendix.key(), "appendix");
        assert_eq!(Role::Appendix.label(), "Appendix");
    }

    #[test]
    fn roles_map_to_their_epub_structural_terms() {
        // Each role reaching the EPUB writer (#695) carries its precise EPUB
        // Structural Semantics term; the two roles with no distinct term fall back
        // to `None` so the writer keeps the generic front/back-matter band.
        assert_eq!(Role::Dedication.epub_type(), Some("dedication"));
        assert_eq!(Role::Foreword.epub_type(), Some("foreword"));
        assert_eq!(Role::Glossary.epub_type(), Some("glossary"));
        assert_eq!(Role::Epilogue.epub_type(), Some("epilogue"));
        assert_eq!(Role::Colophon.epub_type(), Some("colophon"));
        // No distinct EPUB term for these → the band default is used.
        assert_eq!(Role::AlsoBy.epub_type(), None);
        assert_eq!(Role::AboutTheAuthor.epub_type(), None);
    }

    #[test]
    fn front_and_back_matter_roles_parse_into_the_right_band() {
        // Dedication is a front-matter page; the "also by" / "about the author" /
        // colophon pages are back matter (#605). Each parses (with its longhand
        // aliases and case-insensitively) and reports the conventional band.
        assert_eq!(Role::from_key("Dedication"), Some(Role::Dedication));
        assert_eq!(Role::Dedication.band(), Band::Front);

        assert_eq!(Role::from_key("also-by"), Some(Role::AlsoBy));
        assert_eq!(Role::from_key("Also By The Author"), Some(Role::AlsoBy));
        assert_eq!(Role::AlsoBy.band(), Band::Back);

        assert_eq!(
            Role::from_key("about-the-author"),
            Some(Role::AboutTheAuthor)
        );
        assert_eq!(
            Role::from_key(" About the Author "),
            Some(Role::AboutTheAuthor)
        );
        assert_eq!(Role::AboutTheAuthor.band(), Band::Back);

        assert_eq!(Role::from_key("COLOPHON"), Some(Role::Colophon));
        assert_eq!(Role::Colophon.band(), Band::Back);

        // Keys and labels round-trip for the manifest and the pane.
        assert_eq!(Role::Dedication.key(), "dedication");
        assert_eq!(Role::AboutTheAuthor.label(), "About the Author");
    }

    #[test]
    fn new_roles_sort_into_reading_order_across_bands() {
        // The band a file lands in is what orders it in the compiled book: front
        // matter (dedication) before the body, back matter (also-by, about-the-
        // author, colophon) after it. A stable sort by band preserves the
        // author's within-band order, so the section-assembly picks these up.
        let roles = [
            Some(Role::AboutTheAuthor),
            Some(Role::Dedication),
            None, // a plain body chapter
            Some(Role::Colophon),
            Some(Role::AlsoBy),
        ];
        let mut order: Vec<usize> = (0..roles.len()).collect();
        order.sort_by_key(|&i| Band::of(roles[i]));
        // Dedication (front) first, body next, then the back matter in the
        // author's declared order.
        assert_eq!(order, vec![1, 2, 0, 3, 4]);
    }

    #[test]
    fn status_cycles() {
        assert_eq!(Status::Draft.next(), Status::Revised);
        assert_eq!(Status::Revised.next(), Status::Final);
        assert_eq!(Status::Final.next(), Status::Draft);
    }

    #[test]
    fn multiline_value_round_trips_as_a_literal_block() {
        let mut fm = FrontMatter::from_raw("status: draft\n");
        fm.set_epigraph_text("Line one\nLine two\nLine three");
        // Written as a `|` literal block, indented, other keys intact.
        assert_eq!(
            fm.as_str(),
            "status: draft\nepigraph: |\n  Line one\n  Line two\n  Line three\n"
        );
        assert_eq!(
            fm.epigraph().unwrap().text,
            "Line one\nLine two\nLine three"
        );
        assert_eq!(fm.status(), Some(Status::Draft));
    }

    #[test]
    fn a_block_preserves_interior_blank_lines() {
        let mut fm = FrontMatter::default();
        fm.set_epigraph_text("Roses are red\n\nViolets are blue");
        fm.set_epigraph_by("Anon"); // a key after the block stays separate
        let ep = fm.epigraph().unwrap();
        assert_eq!(ep.text, "Roses are red\n\nViolets are blue");
        assert_eq!(ep.attribution.as_deref(), Some("Anon"));
    }

    #[test]
    fn replacing_a_block_with_a_scalar_drops_the_old_continuation() {
        let mut fm = FrontMatter::default();
        fm.set_epigraph_text("Line one\nLine two");
        fm.set_status(Status::Final); // a key written after the block
                                      // Shrink the block back to a single line.
        fm.set_epigraph_text("Just one line");
        assert_eq!(fm.epigraph().unwrap().text, "Just one line");
        assert!(fm.as_str().contains("epigraph: Just one line\n"));
        assert!(!fm.as_str().contains("Line two"));
        assert_eq!(fm.status(), Some(Status::Final));
    }

    #[test]
    fn removing_a_block_drops_its_whole_span() {
        let mut fm = FrontMatter::from_raw("status: draft\n");
        fm.set_epigraph_text("A\nB\nC");
        fm.set_epigraph_text(""); // clear
        assert!(fm.epigraph().is_none());
        assert!(!fm.as_str().contains("epigraph"));
        assert_eq!(fm.status(), Some(Status::Draft)); // sibling untouched
    }

    #[test]
    fn epigraph_round_trips_quotation_and_attribution() {
        let mut fm = FrontMatter::default();
        fm.set_epigraph_text("Call me Ishmael.");
        fm.set_epigraph_by("Melville");
        let ep = fm.epigraph().unwrap();
        assert_eq!(ep.text, "Call me Ishmael.");
        assert_eq!(ep.attribution.as_deref(), Some("Melville"));
    }

    #[test]
    fn scenes_round_trip_through_an_ordered_sequence() {
        // Per-scene synopsis + status write as an ordered block and read back in
        // order, aligned positionally with the body headings (#636).
        let mut fm = FrontMatter::from_raw("status: draft\n");
        fm.set_scenes(&[
            SceneMeta {
                synopsis: "The arrival: rain".into(),
                status: SceneStatus::Done,
            },
            SceneMeta {
                synopsis: "The meeting".into(),
                status: SceneStatus::Stub,
            },
        ]);
        let read = fm.scenes();
        assert_eq!(read.len(), 2);
        assert_eq!(read[0].synopsis, "The arrival: rain");
        assert_eq!(read[0].status, SceneStatus::Done);
        assert_eq!(read[1].synopsis, "The meeting");
        assert_eq!(read[1].status, SceneStatus::Stub);
        // The sibling chapter key survives the sequence write.
        assert_eq!(fm.status(), Some(Status::Draft));
    }

    #[test]
    fn a_chapter_with_no_scenes_key_reads_as_empty() {
        let fm = FrontMatter::from_raw("status: draft\npov: Mara\n");
        assert!(fm.scenes().is_empty());
    }

    #[test]
    fn set_scenes_replaces_the_block_and_trims_trailing_blanks() {
        let mut fm = FrontMatter::default();
        fm.set_scenes(&[
            SceneMeta {
                synopsis: "One".into(),
                status: SceneStatus::Draft,
            },
            SceneMeta {
                synopsis: "Two".into(),
                status: SceneStatus::Draft,
            },
        ]);
        fm.set_status(Status::Final); // a key written after the block
                                      // Rewrite with a trailing blank entry: it's dropped, the sibling stays.
        fm.set_scenes(&[
            SceneMeta {
                synopsis: "Only one now".into(),
                status: SceneStatus::Done,
            },
            SceneMeta::default(),
        ]);
        let read = fm.scenes();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].synopsis, "Only one now");
        assert!(!fm.as_str().contains("Two"));
        assert_eq!(fm.status(), Some(Status::Final));
    }

    #[test]
    fn clearing_scenes_removes_the_whole_block() {
        let mut fm = FrontMatter::from_raw("status: draft\n");
        fm.set_scenes(&[SceneMeta {
            synopsis: "A".into(),
            status: SceneStatus::Draft,
        }]);
        // All-blank entries clear the block back out.
        fm.set_scenes(&[SceneMeta::default(), SceneMeta::default()]);
        assert!(fm.scenes().is_empty());
        assert!(!fm.as_str().contains("scenes"));
        assert_eq!(fm.status(), Some(Status::Draft));
    }

    #[test]
    fn scene_status_cycles_and_parses() {
        assert_eq!(SceneStatus::Stub.next(), SceneStatus::Draft);
        assert_eq!(SceneStatus::Draft.next(), SceneStatus::Done);
        assert_eq!(SceneStatus::Done.next(), SceneStatus::Stub);
        assert_eq!(SceneStatus::from_key("DONE"), Some(SceneStatus::Done));
        assert_eq!(SceneStatus::from_key("nonsense"), None);
        assert_eq!(SceneStatus::default(), SceneStatus::Stub);
    }

    #[test]
    fn epigraph_without_a_quotation_is_none_and_clearing_removes_keys() {
        let mut fm = FrontMatter::default();
        // Attribution alone is not an epigraph.
        fm.set_epigraph_by("Nobody");
        assert!(fm.epigraph().is_none());

        fm.set_epigraph_text("A line.");
        assert!(fm.epigraph().is_some());
        // Clearing the quotation drops the epigraph; keys are removed.
        fm.set_epigraph_text("");
        assert!(fm.epigraph().is_none());
        assert!(!fm.as_str().contains("epigraph:"));
    }

    #[test]
    fn set_on_a_duplicated_key_collapses_to_one_canonical_entry() {
        // A hand-edited/pasted/imported block with the same key twice: `get`
        // reads the first, but a real YAML parser reads the last. A `set` must
        // collapse them so both agree — the first is replaced, the stale second
        // dropped, leaving exactly one entry.
        let mut fm = FrontMatter::from_raw("status: draft\npov: Mara\nstatus: final\n");
        fm.set_status(Status::Revised);
        assert_eq!(fm.as_str(), "status: revised\npov: Mara\n");
        assert_eq!(fm.status(), Some(Status::Revised));
        // Only one `status:` line survives, so no parser can read a stale value.
        assert_eq!(fm.as_str().matches("status:").count(), 1);
    }

    #[test]
    fn removing_a_duplicated_key_leaves_no_ghost_that_resurrects() {
        // Clearing a key that appears twice must drop *both* — otherwise the
        // second is a ghost that `get` reads on the next round.
        let mut fm = FrontMatter::from_raw("summary: first\nstatus: draft\nsummary: second\n");
        fm.set_summary("   "); // blank clears the key
        assert_eq!(fm.summary(), None);
        assert!(!fm.as_str().contains("summary"));
        assert_eq!(fm.status(), Some(Status::Draft)); // sibling untouched
    }

    #[test]
    fn set_collapses_a_duplicated_block_key_and_its_span() {
        // The stale duplicate is a `|` block: its whole indented span must be
        // dropped, not just its `key:` line, or continuation lines linger.
        let raw =
            "epigraph: |\n  Old A\n  Old B\nstatus: draft\nepigraph: |\n  Ghost one\n  Ghost two\n";
        let mut fm = FrontMatter::from_raw(raw);
        fm.set_epigraph_text("Fresh line");
        assert_eq!(fm.epigraph().unwrap().text, "Fresh line");
        assert_eq!(fm.as_str().matches("epigraph:").count(), 1);
        assert!(!fm.as_str().contains("Ghost"));
        assert!(!fm.as_str().contains("Old B"));
        assert_eq!(fm.status(), Some(Status::Draft));
    }

    #[test]
    fn set_scenes_collapses_a_duplicated_scenes_block() {
        // Two `scenes:` sequences: writing collapses to one so `scenes()` and an
        // external parser don't disagree on which the author meant.
        let raw = "scenes:\n  - synopsis: old\n    status: draft\nstatus: draft\nscenes:\n  - synopsis: ghost\n    status: done\n";
        let mut fm = FrontMatter::from_raw(raw);
        fm.set_scenes(&[SceneMeta {
            synopsis: "fresh".into(),
            status: SceneStatus::Done,
        }]);
        let read = fm.scenes();
        assert_eq!(read.len(), 1);
        assert_eq!(read[0].synopsis, "fresh");
        assert_eq!(fm.as_str().matches("scenes:").count(), 1);
        assert!(!fm.as_str().contains("ghost"));
    }

    #[test]
    fn a_value_with_a_bare_cr_round_trips_as_valid_yaml() {
        // An old-Mac paste with a lone `\r`: it must take the safe `|`-block
        // path, not a one-line quoted scalar with a literal carriage return.
        let mut fm = FrontMatter::from_raw("status: draft\n");
        fm.set_summary("first\rsecond");
        // Written as a `|` block, one line per visual line — no interior CR/LF.
        assert_eq!(
            fm.as_str(),
            "status: draft\nsummary: |\n  first\n  second\n"
        );
        assert!(!fm.as_str().contains('\r'));
        // The value reads back as newline-joined lines.
        assert_eq!(fm.summary().as_deref(), Some("first\nsecond"));
        assert_eq!(fm.status(), Some(Status::Draft));
    }

    #[test]
    fn a_value_with_crlf_round_trips_without_a_spurious_blank_line() {
        // A CRLF value: the CR+LF pair is one break, so the block has one line
        // per visual line — not a blank line between each.
        let mut fm = FrontMatter::default();
        fm.set_summary("alpha\r\nbeta\r\ngamma");
        assert_eq!(fm.as_str(), "summary: |\n  alpha\n  beta\n  gamma\n");
        assert!(!fm.as_str().contains('\r'));
        assert_eq!(fm.summary().as_deref(), Some("alpha\nbeta\ngamma"));
    }

    #[test]
    fn split_lines_treats_cr_lf_and_crlf_each_as_one_break() {
        assert_eq!(split_lines("a\nb"), vec!["a", "b"]);
        assert_eq!(split_lines("a\rb"), vec!["a", "b"]);
        assert_eq!(split_lines("a\r\nb"), vec!["a", "b"]);
        // Interior blank lines survive (verse stanza break).
        assert_eq!(split_lines("a\r\n\r\nb"), vec!["a", "", "b"]);
        assert_eq!(split_lines("solo"), vec!["solo"]);
    }
}
