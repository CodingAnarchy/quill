//! A short story's scenes (#1240). Short stories usually separate scenes with a
//! scene break (a Markdown thematic break such as `* * *`) rather than titled
//! headings, so a story's scene is a run of prose bounded by scene breaks *or* by
//! `##` headings, whichever the author uses. A story with no boundaries is one
//! scene. A heading deeper than the scene it sits in (a `###` under a `##`, or
//! under an untitled break-opened scene) is a sub-beat that folds into it.
//!
//! This is the story counterpart of the corkboard's heading-only partition: the
//! corkboard, the outline pane, and quick switch read it for a short story so an
//! untitled story still shows its structure. Pure text in, scenes out — no buffer,
//! no UI — so the reorder that rewrites the body is exhaustively testable.

use crate::frontmatter::SceneMeta;
use crate::syntax::Heading;

/// The longest a scene label taken from prose runs before it's cut with `…`.
const LABEL_CHARS: usize = 48;

/// The break written between scenes when a reorder needs one and the story has
/// none to copy (every scene was headed).
const DEFAULT_BREAK: &str = "* * *";

/// One scene of a story: `[start, end)` is its byte range — the heading line (when
/// it has one) through its prose, up to the next boundary. A break-opened scene
/// starts on the line after its break, so the break itself belongs to no scene.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StoryScene {
    pub start: usize,
    pub end: usize,
    /// The scene's heading title, when a `##`-or-deeper heading opens it.
    pub title: Option<String>,
    /// The opening heading's level, or 2 for an untitled scene, so deeper
    /// headings fold in the same way under both.
    pub level: u8,
    /// Whether a scene break sits before this scene. Kept through a reorder so a
    /// headed scene the author separated with a break keeps it.
    pub after_break: bool,
}

/// The scenes of story `text`, in document order. `headings` are the parsed
/// headings and `break_lines` the lines scene breaks sit on (both from the
/// buffer's [`Syntax`](crate::syntax::Syntax)), so fenced code never counts.
/// Scenes with no content (a leading break, two breaks in a row, a trailing break)
/// are dropped, and a leading `#` title line is never a scene of its own.
pub fn story_scenes(text: &str, headings: &[Heading], break_lines: &[usize]) -> Vec<StoryScene> {
    let breaks = break_ranges(text, break_lines);
    // Merge breaks and headings into one start-ordered boundary list.
    let mut boundaries: Vec<Boundary> = breaks.iter().map(|&r| Boundary::Break(r)).collect();
    boundaries.extend(headings.iter().map(Boundary::Heading));
    boundaries.sort_by_key(Boundary::start);

    let mut scenes = Vec::new();
    let mut open = Open {
        start: 0,
        title: None,
        level: 2,
        after_break: false,
    };
    for boundary in boundaries {
        match boundary {
            Boundary::Break((from, to)) => {
                open.close(text, from, &mut scenes);
                open = Open {
                    start: to,
                    title: None,
                    level: 2,
                    after_break: true,
                };
            }
            // The `#` title is never scene content: a scene that hasn't begun its
            // prose yet starts after it, so a reorder keeps the title on top.
            Boundary::Heading(h) if h.level < 2 => {
                if open.title.is_none() && !has_prose(&text[open.start.min(h.start)..h.start]) {
                    open.start = line_end(text, h.start);
                }
            }
            // A heading deeper than the open scene is one of its sub-beats.
            Boundary::Heading(h) if h.level > open.level => {}
            Boundary::Heading(h) => {
                // A heading directly under a break titles that break's scene
                // rather than leaving an empty scene behind it.
                let after_break = open.after_break && is_blank(&text[open.start..h.start]);
                open.close(text, h.start, &mut scenes);
                open = Open {
                    start: h.start,
                    title: Some(h.title.clone()),
                    level: h.level,
                    after_break,
                };
            }
        }
    }
    open.close(text, text.len(), &mut scenes);
    scenes
}

/// A scene's label for a list: its heading title, else its first line of prose
/// cut to [`LABEL_CHARS`], else "(empty scene)".
pub fn scene_label(text: &str, scene: &StoryScene) -> String {
    if let Some(title) = &scene.title {
        return title.clone();
    }
    let body = &text[scene.start.min(text.len())..scene.end.min(text.len())];
    let line = body
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .unwrap_or("");
    if line.is_empty() {
        return "(empty scene)".into();
    }
    if line.chars().count() <= LABEL_CHARS {
        return line.to_string();
    }
    let cut: String = line.chars().take(LABEL_CHARS - 1).collect();
    format!("{}…", cut.trim_end())
}

/// The story body with scenes `a` and `b` swapped, every scene keeping its own
/// text (heading included) and the break before it, if any. The result puts
/// exactly one break line between scenes that had one, none before the first
/// scene, and none after the last; whatever came before the first scene (a `#`
/// title) is kept. An index out of range returns the text unchanged.
pub fn swap_scenes(text: &str, scenes: &[StoryScene], a: usize, b: usize) -> String {
    if a >= scenes.len() || b >= scenes.len() || a == b {
        return text.to_string();
    }
    let mut order: Vec<&StoryScene> = scenes.iter().collect();
    order.swap(a, b);
    let rule = first_break(text).unwrap_or(DEFAULT_BREAK);
    let prefix = text[..scenes[0].start].trim_end_matches(|c: char| c.is_whitespace());
    // Text before the first scene that is itself only a break would leave a
    // leading break once that scene moves; keep the prefix only when it's content.
    let prefix = if is_break_only(prefix) { "" } else { prefix };

    let mut out = String::with_capacity(text.len() + rule.len() * 2);
    if !prefix.is_empty() {
        out.push_str(prefix);
        out.push_str("\n\n");
    }
    for (i, scene) in order.iter().enumerate() {
        if i > 0 {
            // An untitled scene always needs a break to stand apart; a headed one
            // keeps the break it had.
            if scene.title.is_none() || scene.after_break {
                out.push_str(rule);
                out.push_str("\n\n");
            }
        }
        out.push_str(text[scene.start..scene.end].trim_matches('\n').trim_end());
        out.push_str("\n\n");
    }
    // One trailing newline, not a blank line, closes the story.
    let trimmed = out.trim_end().len();
    out.truncate(trimmed);
    out.push('\n');
    out
}

/// A story's stored scene metadata matched to its current scenes (#1240), one
/// entry per label in `labels`, each stamped with that label as its anchor. An
/// entry goes to the scene whose label equals its anchor; a scene with no anchor
/// match takes the entry at its own position when no other scene claims that
/// entry by anchor (an edited first line keeps its notes); anything else starts
/// blank. So typing a break mid-story adds a blank scene instead of shifting every
/// later synopsis onto the wrong scene.
pub fn reconcile_scene_meta(stored: &[SceneMeta], labels: &[String]) -> Vec<SceneMeta> {
    let mut used = vec![false; stored.len()];
    let mut slots: Vec<Option<usize>> = labels
        .iter()
        .map(|label| {
            let i = stored
                .iter()
                .enumerate()
                .position(|(i, m)| !used[i] && !m.anchor.is_empty() && m.anchor == *label)?;
            used[i] = true;
            Some(i)
        })
        .collect();
    // Positional fallback for the scenes no anchor matched.
    for (pos, slot) in slots.iter_mut().enumerate() {
        if slot.is_none() && pos < stored.len() && !used[pos] {
            let claimed = !stored[pos].anchor.is_empty() && labels.contains(&stored[pos].anchor);
            if !claimed {
                used[pos] = true;
                *slot = Some(pos);
            }
        }
    }
    slots
        .into_iter()
        .zip(labels)
        .map(|(slot, label)| {
            let mut meta = slot.map(|i| stored[i].clone()).unwrap_or_default();
            meta.anchor.clone_from(label);
            meta
        })
        .collect()
}

/// A boundary between scenes: a scene break's byte range, or a heading.
enum Boundary<'a> {
    Break((usize, usize)),
    Heading(&'a Heading),
}

impl Boundary<'_> {
    const fn start(&self) -> usize {
        match self {
            Self::Break((from, _)) => *from,
            Self::Heading(h) => h.start,
        }
    }
}

/// The scene being accumulated while walking the boundaries.
struct Open {
    start: usize,
    title: Option<String>,
    level: u8,
    after_break: bool,
}

impl Open {
    /// End this scene at `end` and keep it if it has content — a title, or any
    /// non-blank text that isn't just a leading `#` title line.
    fn close(&mut self, text: &str, end: usize, scenes: &mut Vec<StoryScene>) {
        let mut start = self.start.min(end);
        // An untitled scene starts at its first line of prose, so a jump lands
        // on the words rather than the blank line under a break.
        if self.title.is_none() {
            for line in text[start..end].split_inclusive('\n') {
                if !line.trim().is_empty() {
                    break;
                }
                start += line.len();
            }
        }
        let body = &text[start..end];
        if self.title.is_some() || has_prose(body) {
            scenes.push(StoryScene {
                start,
                end,
                title: self.title.take(),
                level: self.level,
                after_break: self.after_break,
            });
        }
    }
}

/// The byte range of each scene-break line: from the line's start through its
/// newline. `break_lines` are line indices in document order, so one pass over
/// the text finds every line start.
fn break_ranges(text: &str, break_lines: &[usize]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::with_capacity(break_lines.len());
    let mut wanted = break_lines.iter().copied().peekable();
    let mut offset = 0;
    for (line, piece) in text.split_inclusive('\n').enumerate() {
        let Some(&next) = wanted.peek() else { break };
        if line == next {
            ranges.push((offset, offset + piece.len()));
            wanted.next();
        }
        offset += piece.len();
    }
    ranges
}

/// The first scene-break line's text, to copy when a reorder writes a new break,
/// so the story keeps the author's own glyph.
fn first_break(text: &str) -> Option<&str> {
    text.lines().map(str::trim).find(|l| is_break_line(l))
}

/// Whether `line` is a CommonMark thematic break: three or more `*`, `-`, or `_`
/// (one character throughout), optionally separated by spaces.
fn is_break_line(line: &str) -> bool {
    let marks: Vec<char> = line.chars().filter(|c| !c.is_whitespace()).collect();
    marks.len() >= 3 && matches!(marks[0], '*' | '-' | '_') && marks.iter().all(|&c| c == marks[0])
}

/// Whether `text` is nothing but scene-break lines and blank space.
fn is_break_only(text: &str) -> bool {
    text.lines()
        .map(str::trim)
        .all(|l| l.is_empty() || is_break_line(l))
}

/// The byte just past the newline ending the line that holds `at`.
fn line_end(text: &str, at: usize) -> usize {
    let at = at.min(text.len());
    text[at..].find('\n').map_or(text.len(), |nl| at + nl + 1)
}

fn is_blank(text: &str) -> bool {
    text.trim().is_empty()
}

/// Whether `body` holds prose: any non-blank line that isn't a `#` heading.
fn has_prose(body: &str) -> bool {
    body.lines()
        .map(str::trim)
        .any(|l| !l.is_empty() && !l.starts_with('#'))
}

#[cfg(test)]
mod tests {
    use super::{reconcile_scene_meta, scene_label, story_scenes, swap_scenes, StoryScene};
    use crate::frontmatter::{SceneMeta, SceneStatus};
    use crate::syntax::Syntax;

    fn scenes(text: &str) -> Vec<StoryScene> {
        let syntax = Syntax::new(text);
        story_scenes(text, syntax.headings(), syntax.scene_break_lines())
    }

    fn labels(text: &str) -> Vec<String> {
        scenes(text).iter().map(|s| scene_label(text, s)).collect()
    }

    #[test]
    fn a_story_with_no_boundaries_is_one_scene() {
        let text = "The bell rang.\n\nNobody came.\n";
        assert_eq!(labels(text), vec!["The bell rang."]);
        assert_eq!(scenes(text)[0].end, text.len());
    }

    #[test]
    fn scene_breaks_split_untitled_scenes() {
        let text = "The bell rang.\n\n* * *\n\nNobody came.\n\n***\n\nThe tide.\n";
        assert_eq!(
            labels(text),
            vec!["The bell rang.", "Nobody came.", "The tide."]
        );
        let s = scenes(text);
        assert!(!s[0].after_break && s[1].after_break && s[2].after_break);
        assert!(
            !text[s[0].start..s[0].end].contains('*'),
            "the break is no scene's"
        );
    }

    #[test]
    fn headings_also_split_scenes_and_a_heading_under_a_break_titles_it() {
        let text = "Opening.\n\n## Shore\n\nWaves.\n\n* * *\n\n## Bell\n\nIt rang.\n";
        assert_eq!(labels(text), vec!["Opening.", "Shore", "Bell"]);
        let s = scenes(text);
        assert!(!s[1].after_break, "Shore follows a heading only");
        assert!(s[2].after_break, "Bell's break is kept with it");
    }

    #[test]
    fn deeper_headings_fold_into_their_scene() {
        let text = "## Shore\n\nWaves.\n\n### Later\n\nMore waves.\n\n* * *\n\nNight.\n\n### Dark\n\nStill.\n";
        assert_eq!(labels(text), vec!["Shore", "Night."]);
    }

    #[test]
    fn a_title_line_and_stray_breaks_make_no_empty_scenes() {
        let text = "# The Drowned Bell\n\n* * *\n\nOne.\n\n* * *\n\n* * *\n\nTwo.\n\n* * *\n";
        assert_eq!(labels(text), vec!["One.", "Two."]);
    }

    #[test]
    fn breaks_inside_fenced_code_do_not_count() {
        let text = "One.\n\n```\n***\n```\n\nStill one.\n";
        assert_eq!(scenes(text).len(), 1);
    }

    #[test]
    fn long_first_lines_are_cut_into_a_label() {
        let text = format!("{}\n", "word ".repeat(30));
        let label = &labels(&text)[0];
        assert!(
            label.ends_with('…') && label.chars().count() <= 48,
            "{label}"
        );
    }

    #[test]
    fn swapping_untitled_scenes_keeps_one_break_between_each() {
        let text = "One.\n\n* * *\n\nTwo.\n\n* * *\n\nThree.\n";
        let s = scenes(text);
        assert_eq!(
            swap_scenes(text, &s, 0, 1),
            "Two.\n\n* * *\n\nOne.\n\n* * *\n\nThree.\n"
        );
        assert_eq!(
            swap_scenes(text, &s, 1, 2),
            "One.\n\n* * *\n\nThree.\n\n* * *\n\nTwo.\n"
        );
    }

    #[test]
    fn swapping_keeps_the_authors_break_glyph_and_title_line() {
        let text = "# Tide\n\nOne.\n\n#\n\nTwo.\n";
        // `#` alone is an empty heading, not a break; use a real thematic break.
        let text = text.replace("\n#\n", "\n---\n");
        let s = scenes(&text);
        assert_eq!(s.len(), 2);
        assert_eq!(
            swap_scenes(&text, &s, 0, 1),
            "# Tide\n\nTwo.\n\n---\n\nOne.\n"
        );
    }

    #[test]
    fn swapping_headed_scenes_adds_no_break_they_never_had() {
        let text = "## A\n\nOne.\n\n## B\n\nTwo.\n";
        let s = scenes(text);
        assert_eq!(
            swap_scenes(text, &s, 0, 1),
            "## B\n\nTwo.\n\n## A\n\nOne.\n"
        );
    }

    fn meta(synopsis: &str, anchor: &str) -> SceneMeta {
        SceneMeta {
            synopsis: synopsis.into(),
            status: SceneStatus::Draft,
            anchor: anchor.into(),
        }
    }

    fn synopses(meta: &[SceneMeta]) -> Vec<&str> {
        meta.iter().map(|m| m.synopsis.as_str()).collect()
    }

    fn owned(labels: &[&str]) -> Vec<String> {
        labels.iter().map(|l| (*l).to_string()).collect()
    }

    #[test]
    fn a_new_scene_typed_mid_story_starts_blank_and_shifts_nothing() {
        let stored = vec![meta("arrival", "One."), meta("bell", "Two.")];
        let got = reconcile_scene_meta(&stored, &owned(&["One.", "New.", "Two."]));
        assert_eq!(synopses(&got), vec!["arrival", "", "bell"]);
        assert_eq!(got[1].anchor, "New.");
    }

    #[test]
    fn a_deleted_scene_takes_its_notes_with_it() {
        let stored = vec![meta("a", "One."), meta("b", "Two."), meta("c", "Three.")];
        let got = reconcile_scene_meta(&stored, &owned(&["One.", "Three."]));
        assert_eq!(synopses(&got), vec!["a", "c"]);
    }

    #[test]
    fn an_edited_first_line_keeps_its_notes_by_position() {
        let stored = vec![meta("a", "One."), meta("b", "Two.")];
        let got = reconcile_scene_meta(&stored, &owned(&["One.", "Two, edited."]));
        assert_eq!(synopses(&got), vec!["a", "b"]);
        assert_eq!(got[1].anchor, "Two, edited.", "the anchor follows the edit");
    }

    #[test]
    fn unanchored_metadata_aligns_by_position() {
        let stored = vec![meta("a", ""), meta("b", "")];
        let got = reconcile_scene_meta(&stored, &owned(&["One.", "Two.", "Three."]));
        assert_eq!(synopses(&got), vec!["a", "b", ""]);
    }

    #[test]
    fn swapping_round_trips() {
        let text = "One.\n\n* * *\n\n## Two\n\nTwo.\n\n* * *\n\nThree.\n";
        let s = scenes(text);
        let once = swap_scenes(text, &s, 1, 2);
        let s2 = scenes(&once);
        assert_eq!(swap_scenes(&once, &s2, 1, 2), text);
    }
}
