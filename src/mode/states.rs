//! Transient mode-state structs (#404): the overlay, search, and review state
//! that each [`Mode`](super::Mode) variant carries. Split out of `mode.rs` to keep
//! the core enums under the ~1,000-line convention. Pure move.

use std::path::PathBuf;

use unicode_segmentation::UnicodeSegmentation;

use crate::buffer::{Buffer, CharClass};
use crate::text::MatchOpts;

use super::{AiAction, MetaField, Pending};

/// State for an open [`Mode::Findings`]: the findings list and the selected row.
pub struct Findings {
    pub(crate) items: Vec<Finding>,
    pub(crate) selected: usize,
}

/// State for the navigable commit-history view (#587): the commits (newest first)
/// and the selected row. From the selected commit the author diffs the current
/// chapter against it or restores that chapter to it. The list is captured when
/// the view opens, so it's a stable target for the arrow keys.
pub struct History {
    pub(crate) commits: Vec<crate::vcs::CommitMeta>,
    pub(crate) selected: usize,
}

/// One triageable finding (a continuity contradiction, style note, …). UI-shaped
/// — the AI layer maps its typed findings into this for display.
#[derive(Clone, Debug)]
pub struct Finding {
    /// One-line summary shown in the list (e.g. "object: the locket …").
    pub(crate) summary: String,
    /// Chapters involved; the first is the jump target.
    pub(crate) chapters: Vec<String>,
    /// The book the finding belongs to in a multi-book series (#341):
    /// `Some(index)` when a series-bible refresh produced it, so triage can
    /// attribute and jump across books (chapter titles can repeat between
    /// books). `None` targets the active book — every standalone-novel and
    /// active-book pass, unchanged.
    pub(crate) book: Option<usize>,
    /// The pass that produced this finding, so the author can re-check just this
    /// one after a fix by re-running its originating pass scoped to `chapters`
    /// (#608). `None` for findings not tied to a re-runnable pass — Q&A citations
    /// and history rows — and for findings restored from the durable store, which
    /// records content, not the pass (a fresh pass re-attributes them).
    pub(crate) action: Option<AiAction>,
}

/// Identity is the finding's *content* — summary, chapters, and book — not the
/// originating pass (#608). This mirrors the durable store's content key
/// ([`crate::app`]): a re-produced finding is the same finding, and a finding
/// restored from disk (which carries no `action`) still matches the one a fresh
/// pass produces, so triage dedup and dismiss-matching stay intact.
impl PartialEq for Finding {
    fn eq(&self, other: &Self) -> bool {
        self.summary == other.summary && self.chapters == other.chapters && self.book == other.book
    }
}

/// State for an open [`Mode::BibleReview`]: the queue of conflicting bible files
/// (front of the queue is the one shown), and a scratch buffer while the author
/// edits the proposed version before saving.
pub struct BibleReview {
    pub(crate) conflicts: Vec<BibleConflict>,
    /// `Some` while editing the proposed version of the front conflict.
    pub(crate) editing: Option<Buffer>,
}

/// One bible file whose on-disk generated block differs from what a fresh pass
/// would write.
pub struct BibleConflict {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) proposed: String,
    /// Pre-rendered unified diff (current → proposed), each line prefixed with
    /// `+`/`-`/` `. Computed once when the conflict is staged so the overlay
    /// isn't re-diffing every frame.
    pub(crate) diff: Vec<String>,
}

/// State for an open [`Mode::EditMeta`]. `label` is public for the editor's
/// banner; the rest is the handler's business. The scratch `buffer` is reached
/// through `App::edit_buffer`.
pub struct MetaEdit {
    /// Which field is being edited (a quotation field).
    pub(crate) target: MetaField,
    /// Scratch buffer holding the field's text.
    pub(crate) buffer: Buffer,
    /// Banner label, e.g. "chapter epigraph".
    pub label: String,
    /// Chapter to reselect when editing finishes.
    pub(crate) nav_return: Option<usize>,
}

/// State for an open [`Mode::Prompt`]. `label`/`input` are public for the status
/// bar to render; the rest is the handler's business.
pub struct PromptState {
    /// What confirming the prompt does.
    pub(crate) pending: Pending,
    /// The prompt label shown in the status bar.
    pub label: String,
    /// Text typed so far (empty for y/N confirmations).
    pub input: String,
    /// Caret position as a byte offset into `input` (#706). Insertions and
    /// deletions happen here; the arrow keys, Home/End, and word-delete move it.
    /// Always kept on a `char` boundary of `input`.
    pub caret: usize,
    /// Where to return when the prompt closes: `Some(selected)` back to the
    /// chapter browser, `None` to insert mode (e.g. the quit prompt).
    pub(crate) nav_return: Option<usize>,
}

impl PromptState {
    /// A prompt prefilled with `input`, caret at its end (the natural spot to
    /// keep typing or edit from).
    pub(crate) fn new(pending: Pending, label: String, input: String) -> Self {
        let caret = input.len();
        Self {
            pending,
            label,
            input,
            caret,
            nav_return: None,
        }
    }

    /// Clear the input and reset the caret to the start — used by tests to
    /// replace a prefilled default before typing a fresh value, keeping the
    /// caret invariant (in production the author backspaces through it).
    #[cfg(test)]
    pub(crate) fn clear(&mut self) {
        self.input.clear();
        self.caret = 0;
    }

    /// Insert a typed character at the caret and advance past it (#706).
    pub(crate) fn insert(&mut self, c: char) {
        self.input.insert(self.caret, c);
        self.caret += c.len_utf8();
    }

    /// Delete the character before the caret (Backspace); a no-op at the start.
    pub(crate) fn backspace(&mut self) {
        if let Some(prev) = self.prev_char_boundary(self.caret) {
            self.input.replace_range(prev..self.caret, "");
            self.caret = prev;
        }
    }

    /// Delete the character at the caret (forward Delete); a no-op at the end.
    pub(crate) fn delete_forward(&mut self) {
        if let Some(next) = self.next_char_boundary(self.caret) {
            self.input.replace_range(self.caret..next, "");
        }
    }

    /// Delete the word before the caret (Ctrl/Alt-Backspace), reusing the
    /// buffer's word classification (#706): skip any whitespace immediately
    /// before the caret, then the run of one character class before that.
    pub(crate) fn delete_word_back(&mut self) {
        let target = self.word_start_before(self.caret);
        if target < self.caret {
            self.input.replace_range(target..self.caret, "");
            self.caret = target;
        }
    }

    /// Move the caret one character left; a no-op at the start.
    pub(crate) fn move_left(&mut self) {
        if let Some(prev) = self.prev_char_boundary(self.caret) {
            self.caret = prev;
        }
    }

    /// Move the caret one character right; a no-op at the end.
    pub(crate) fn move_right(&mut self) {
        if let Some(next) = self.next_char_boundary(self.caret) {
            self.caret = next;
        }
    }

    /// Move the caret to the start of the input.
    pub(crate) fn move_home(&mut self) {
        self.caret = 0;
    }

    /// Move the caret to the end of the input.
    pub(crate) fn move_end(&mut self) {
        self.caret = self.input.len();
    }

    /// The byte offset of the character boundary before `at`, or `None` at the
    /// start of the string.
    fn prev_char_boundary(&self, at: usize) -> Option<usize> {
        self.input[..at].char_indices().next_back().map(|(i, _)| i)
    }

    /// The byte offset of the character boundary after `at`, or `None` at the
    /// end of the string.
    fn next_char_boundary(&self, at: usize) -> Option<usize> {
        self.input[at..].chars().next().map(|c| at + c.len_utf8())
    }

    /// The start of the word ending at `at`: skip trailing whitespace, then the
    /// run of the class the preceding character belongs to. Mirrors the editor's
    /// `delete_word_back` (#641) using the shared [`CharClass`].
    fn word_start_before(&self, at: usize) -> usize {
        let mut boundaries = self.input[..at].grapheme_indices(true).rev().peekable();
        // Skip a run of trailing whitespace first.
        let mut cut = at;
        while let Some(&(i, g)) = boundaries.peek() {
            if CharClass::of(g) == CharClass::Space {
                cut = i;
                boundaries.next();
            } else {
                break;
            }
        }
        // Then delete the contiguous run of the next class we hit.
        let Some(&(_, g)) = boundaries.peek() else {
            return cut;
        };
        let class = CharClass::of(g);
        for (i, g) in boundaries {
            if CharClass::of(g) == class {
                cut = i;
            } else {
                break;
            }
        }
        cut
    }
}

/// State for an open [`Mode::Search`] — find or find-and-replace (#144), scoped
/// to the single active chapter. `query`/`replacement` are public for the status
/// bar; the match list and indices are the handler's business.
pub struct Search {
    /// What the user is doing right now.
    pub(crate) stage: SearchStage,
    /// The text being searched for.
    pub query: String,
    /// The replacement text (replace flow only).
    pub replacement: String,
    /// Byte ranges of the current matches in the active buffer, in document
    /// order. Recomputed whenever the query or the document changes.
    pub(crate) matches: Vec<(usize, usize)>,
    /// Index into `matches` of the current (strongly highlighted) match.
    pub(crate) current: usize,
    /// Where the search started, so incremental typing jumps to the match
    /// nearest the original cursor rather than chasing each partial query.
    pub(crate) origin_byte: usize,
    /// How many replacements have been made, reported when the flow ends.
    pub(crate) replaced: usize,
    /// Vim `?` search — match nearest *before* the origin, and `n`/`N` invert
    /// (#206). `false` for forward (`/`) and the Ctrl-F find.
    pub(crate) backward: bool,
    /// Opened by Vim's `/` or `?` (#206): Enter finalizes back to Normal and
    /// records the search for `n`/`N`, rather than entering the find/replace
    /// navigation. Esc restores the cursor to the origin.
    pub(crate) from_vim: bool,
    /// Case-sensitive / whole-word toggles (#570). Default (both off) is the
    /// historic case-insensitive substring rule, byte-for-byte unchanged.
    pub(crate) opts: MatchOpts,
}

impl Search {
    /// A fresh search at the query stage, anchored at `origin_byte`. `backward` /
    /// `from_vim` distinguish the Vim `/`/`?` search (#206) from the Ctrl-F find.
    pub(crate) fn new(origin_byte: usize, backward: bool, from_vim: bool) -> Self {
        Self {
            stage: SearchStage::Query,
            query: String::new(),
            replacement: String::new(),
            matches: Vec::new(),
            current: 0,
            origin_byte,
            replaced: 0,
            backward,
            from_vim,
            opts: MatchOpts::default(),
        }
    }

    /// The current match range, if any.
    pub(crate) fn current_match(&self) -> Option<(usize, usize)> {
        self.matches.get(self.current).copied()
    }
}

/// State for an open [`Mode::ProjectSearch`] — manuscript-wide find & replace
/// (#295). `query`/`replacement` are public for the overlay; the match list and
/// indices are the handler's business. Matches are byte ranges into each
/// chapter's prose **body** (front matter excluded), collected on demand without
/// holding every chapter's text at once.
pub struct ProjectSearch {
    pub(crate) stage: ProjectSearchStage,
    /// The text being searched for, across every chapter.
    pub query: String,
    /// The replacement text (replace flow only).
    pub replacement: String,
    /// Every match across the manuscript, in chapter then document order.
    pub(crate) matches: Vec<ProjectMatch>,
    /// Index into `matches` of the current (highlighted) row.
    pub(crate) selected: usize,
    /// How many occurrences have been replaced, reported when the flow ends.
    pub(crate) replaced: usize,
    /// Case-sensitive / whole-word toggles (#570), same default as [`Search`].
    pub(crate) opts: MatchOpts,
}

impl ProjectSearch {
    pub(crate) fn new() -> Self {
        Self {
            stage: ProjectSearchStage::Query,
            query: String::new(),
            replacement: String::new(),
            matches: Vec::new(),
            selected: 0,
            replaced: 0,
            opts: MatchOpts::default(),
        }
    }

    /// The number of distinct chapters the current matches span.
    pub(crate) fn chapter_span(&self) -> usize {
        let mut chapters: Vec<usize> = self.matches.iter().map(|m| m.chapter).collect();
        chapters.dedup();
        chapters.len()
    }
}

/// One manuscript-wide match (#295): which chapter, the byte range within that
/// chapter's prose body, and a short context snippet for the results list.
pub(crate) struct ProjectMatch {
    /// Chapter index (the jump target).
    pub(crate) chapter: usize,
    /// Byte range `[start, end)` within the chapter's body (front matter excluded).
    pub(crate) start: usize,
    pub(crate) end: usize,
    /// The chapter's title, for grouping/labeling the row.
    pub(crate) title: String,
    /// The match's line, trimmed and clipped, shown as context.
    pub(crate) context: String,
}

/// Which step of the manuscript-wide find / replace interaction is active (#295).
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProjectSearchStage {
    /// Typing the query; Enter runs the search across every chapter.
    Query,
    /// Results listed; navigate, jump to a match, or start a replace.
    Results,
    /// Typing the replacement text.
    Replacement,
    /// Stepping through matches, choosing replace / skip; `a` triggers the
    /// confirmed replace-all.
    Step,
    /// y/N confirm before the irreversible manuscript-wide replace-all.
    Confirm,
}

/// State for the manuscript-import preview (#702): the split the parser produced,
/// shown to the author before anything is written so a mis-split (scene breaks
/// read as chapters, or one giant chapter) is caught up front rather than after
/// the project exists. Enter confirms and proceeds to the write+open; Esc cancels
/// and writes nothing.
///
/// Everything needed to write is captured here when the preview opens, so the
/// confirm never re-reads the source. For a Markdown / plain-text source the split
/// heading level can be toggled (`levels` lists the levels the manuscript uses)
/// and the preview re-parsed at the chosen level; other formats (`.docx`,
/// `.scriv`, a folder of chapters) don't split on a Markdown level, so `levels` is
/// empty and only confirm/cancel apply.
pub struct ImportPreview {
    /// The source manuscript path (read-only; never moved or altered).
    pub(crate) source: PathBuf,
    /// Where the new project will be created on confirm.
    pub(crate) dest: PathBuf,
    /// The Markdown source text, kept so a level toggle can re-split without
    /// re-reading; `None` for formats that don't re-split (`.docx`/`.scriv`/folder).
    pub(crate) markdown: Option<String>,
    /// The book title mined from the source, if any.
    pub(crate) title: Option<String>,
    /// The book author mined from the source, if any.
    pub(crate) author: Option<String>,
    /// The chapters the current split produced (title + body), in reading order.
    pub(crate) chapters: Vec<(String, String)>,
    /// The heading level the current Markdown split broke on (`None` for the
    /// non-Markdown formats, which don't split on a level).
    pub(crate) level: Option<usize>,
    /// The heading levels the Markdown source actually uses, offered for re-split
    /// (empty for the non-Markdown formats).
    pub(crate) levels: Vec<usize>,
    /// The dropped-element / unreadable-body warning suffix (#511/#646), already
    /// formatted (empty when the parse was clean).
    pub(crate) warning: String,
    /// The first visible chapter row, so a long chapter list scrolls.
    pub(crate) scroll: usize,
}

/// Which step of the find / replace interaction is active.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SearchStage {
    /// Typing the search query (find or replace). Matches update as you type.
    Query,
    /// Find: query entered, stepping through matches with n / N.
    Navigate,
    /// Replace: typing the replacement text.
    Replacement,
    /// Replace: stepping through matches, choosing replace / skip / all.
    Step,
}
