//! The fuzzy chapter/scene quick-switcher (#976): a `Ctrl-P`-style overlay that
//! lists every chapter *and* every scene heading and narrows them live by a typed
//! substring, jumping to the chosen target on Enter. It reuses the existing go-to
//! machinery — the same `activate_chapter` + jumplist path the corkboard and
//! outline jumps take — so a jump lands in a normal single-chapter buffer and
//! `` `` `` returns.
//!
//! Speed is the whole point on large / series projects, so the target index is
//! built **once** when the switcher opens (from the manifest's cached titles plus
//! a one-time read of each non-active chapter's scene headings — the same bounded,
//! off-keystroke work the manuscript-wide corkboard does). Filtering as the author
//! types then runs purely over that in-memory index: no per-keystroke file reads,
//! and independent of project size. The active chapter's headings come from its
//! live buffer so pending edits are reflected without a re-read.

use crossterm::event::{KeyCode, KeyEvent};

use crate::mode::{Mode, QuickSwitch, QuickTarget};
use crate::syntax::{Heading, Syntax};

use super::story::scene_entries;
use super::App;

/// The scene headings of a chapter body, extracted **without** a full tree-sitter
/// parse when the body is plain ATX (`#`-prefixed) markdown (#976 perf): the
/// switcher opens O(project) chapters, so a synchronous `Syntax::new` per chapter
/// (which builds the whole block *and* inline emphasis trees) stalls the keystroke
/// on a series. A body whose headings are all ATX — the overwhelmingly common case
/// for scene headings — is scanned line-by-line here instead, matching tree-sitter's
/// ATX rules (0–3 leading spaces, `#{1,6}` then a space or line end, trailing `#`
/// stripped, none inside a fenced code block) and its start byte (the line start).
///
/// Setext headings (`Title` underlined with `===` / `---`) are context-sensitive —
/// tree-sitter folds preceding paragraph lines into the heading — so any body that
/// carries a candidate underline (or a `---` scene break) falls back to the
/// authoritative `Syntax::new` parse. The result is byte-for-byte identical to the
/// parse for every body; only the all-ATX fast path skips it.
pub fn scene_headings(body: &str) -> Vec<Heading> {
    scan_atx_headings(body).unwrap_or_else(|| Syntax::new(body).headings().to_vec())
}

/// Scan `body` for ATX headings, matching tree-sitter's offsets, or `None` if the
/// body carries a setext-candidate underline (a line of only `=` or only `-`) that
/// needs the authoritative parse. Fenced code blocks (``` ``` ``` / `~~~`) are
/// skipped so a `#` inside them isn't misread as a heading.
fn scan_atx_headings(body: &str) -> Option<Vec<Heading>> {
    let mut headings = Vec::new();
    let mut offset = 0usize;
    let mut fence: Option<char> = None;
    for line in body.split_inclusive('\n') {
        let start = offset;
        offset += line.len();
        let content = line.strip_suffix('\n').unwrap_or(line);
        let trimmed = content.trim_start();
        let indent = content.len() - trimmed.len();

        // A fenced code block opens/closes on a ``` / ~~~ delimiter line. Track it so
        // headings inside a block are ignored, exactly as the parse does.
        if let Some(f) = fence {
            if is_fence_delimiter(trimmed, f) {
                fence = None;
            }
            continue;
        }
        if let Some(f) = fence_open(trimmed) {
            fence = Some(f);
            continue;
        }

        // A setext-candidate underline is context-sensitive; bail to the full parse.
        if is_setext_underline(trimmed) {
            return None;
        }

        // ATX heading: 0–3 leading spaces, then 1–6 `#`, then a space or the line end.
        if indent > 3 {
            continue;
        }
        let hashes = trimmed.bytes().take_while(|&b| b == b'#').count();
        if hashes == 0 || hashes > 6 {
            continue;
        }
        let after = &trimmed[hashes..];
        if !after.is_empty() && !after.starts_with(' ') {
            continue;
        }
        let level = hashes as u8;
        let title = after.trim().trim_end_matches('#').trim().to_string();
        headings.push(Heading {
            level,
            title,
            start,
        });
    }
    Some(headings)
}

/// Whether `trimmed` (a leading-trimmed line) opens a fenced code block: at least
/// three back-ticks or three tildes, all the same, as the line's leading run.
fn fence_open(trimmed: &str) -> Option<char> {
    for f in ['`', '~'] {
        let run = trimmed.bytes().take_while(|&b| b == f as u8).count();
        if run >= 3 {
            return Some(f);
        }
    }
    None
}

/// Whether `trimmed` closes a fence opened with `f`: a run of ≥3 of the same char,
/// nothing else on the line.
fn is_fence_delimiter(trimmed: &str, f: char) -> bool {
    let run = trimmed.bytes().take_while(|&b| b == f as u8).count();
    run >= 3 && trimmed[run..].trim().is_empty()
}

/// Whether `trimmed` is a setext-candidate underline — a non-empty line of only `=`
/// or only `-` (which could underline a heading, or be a `---` scene break). Either
/// way the parse decides, so its presence sends the body down the full-parse path.
fn is_setext_underline(trimmed: &str) -> bool {
    (!trimmed.is_empty() && trimmed.bytes().all(|b| b == b'='))
        || (!trimmed.is_empty() && trimmed.bytes().all(|b| b == b'-'))
}

impl App {
    /// Open the quick-switcher (#976): build the chapter + scene-heading index and
    /// enter [`Mode::QuickSwitch`]. Reached from an editing mode; leaves the
    /// continuous view first so a jump lands in a single-chapter buffer, and flushes
    /// the active chapter so the index reads its latest saved headings for the
    /// off-buffer chapters. There is always at least one chapter, so the index is
    /// never empty.
    pub(crate) fn open_quick_switch(&mut self) {
        self.leave_continuous();
        let _ = self.flush_active();
        let targets = self.quick_switch_targets();
        self.mode = Mode::QuickSwitch(Box::new(QuickSwitch::new(targets)));
    }

    /// Build the quick-switcher's jump targets (#976): for every chapter, a target
    /// for the chapter itself (jumps to its top) followed by one per scene heading
    /// (level ≥ 2 — the `#` chapter title isn't a separate scene), each carrying the
    /// heading's byte offset. Built once when the switcher opens: the active
    /// chapter's headings come from its live buffer; the rest are read from disk one
    /// file at a time, off the keystroke path. The match `haystack` is pre-lowercased
    /// so the live filter never re-lowercases or allocates.
    fn quick_switch_targets(&self) -> Vec<QuickTarget> {
        let active = self.project.active_index();
        let mut targets = Vec::new();
        for ch in 0..self.chapter_count() {
            let title = self.project.chapter_title(ch).unwrap_or("").to_string();
            // The chapter itself — jumps to its top (byte 0).
            targets.push(QuickTarget {
                chapter: ch,
                byte: 0,
                haystack: title.to_lowercase(),
                title: title.clone(),
                chapter_title: String::new(),
                is_scene: false,
            });
            // Its scene headings. The active chapter is already parsed in the live
            // buffer; other chapters read + parse their body once here. But only take
            // the live buffer when it actually shows the active chapter — with a
            // reference pane focused (a story-bible entry / note opened beside the
            // prose, #165) `edit_buffer()` is that pane, not the chapter, so fall back
            // to the on-disk read for the active chapter too. Focusing away from a
            // chapter flushes it, so disk is current.
            // A short story's targets are its scenes, split by breaks too (#1240).
            let headings = if self.project.manifest.story_chapter() == Some(ch) {
                if ch == active && self.focus_shows_active_chapter() {
                    self.outline_entries().into_owned()
                } else {
                    let body = self.project.chapter_body(ch);
                    scene_entries(&body, &Syntax::new(&body))
                }
            } else if ch == active && self.focus_shows_active_chapter() {
                self.edit_buffer().syntax().headings().to_vec()
            } else {
                // Extract the off-buffer chapter's headings without the full parse on
                // the common all-ATX body (#976 perf) — see [`scene_headings`].
                scene_headings(&self.project.chapter_body(ch))
            };
            for h in headings.iter().filter(|h| h.level >= 2) {
                let haystack = format!("{} {}", h.title, title).to_lowercase();
                targets.push(QuickTarget {
                    chapter: ch,
                    byte: h.start,
                    haystack,
                    title: h.title.clone(),
                    chapter_title: title.clone(),
                    is_scene: true,
                });
            }
        }
        targets
    }

    /// Key handling for the quick-switcher (#976): printable characters build the
    /// filter query (live), Backspace edits it, the arrows / Ctrl-N,P move within
    /// the narrowed list, Enter jumps to the highlighted target, and Esc closes
    /// without moving. Modeled on the chapter browser's filter sub-mode, but always
    /// in type-to-filter mode — the switcher is a single fuzzy field.
    pub(crate) fn on_key_quick_switch(&mut self, key: KeyEvent) {
        let Mode::QuickSwitch(state) = &mut self.mode else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.mode = self.editing_mode(),
            KeyCode::Enter => self.quick_switch_jump(),
            KeyCode::Up => state.step(-1),
            KeyCode::Down => state.step(1),
            KeyCode::Backspace => state.pop(),
            KeyCode::Char(c) => state.push(c),
            _ => {}
        }
    }

    /// Jump to the quick-switcher's selected target (#976) and close it: record the
    /// jump-back position, open the target chapter (a no-op when it's already
    /// active), then move the cursor to the target byte. Reuses the corkboard's
    /// jump shape so `` `` `` returns here. A no-op close when the filter left
    /// nothing selected.
    fn quick_switch_jump(&mut self) {
        let Mode::QuickSwitch(state) = &self.mode else {
            return;
        };
        let Some(target) = state.current().cloned() else {
            self.mode = self.editing_mode();
            return;
        };
        // Record where we jumped from before the cursor moves (possibly to another
        // chapter), so the jump-back list can return here.
        self.push_jump();
        self.mode = self.editing_mode();
        self.activate_chapter(target.chapter);
        let buf = self.edit_buffer_mut();
        let at = target.byte.min(buf.len_bytes());
        buf.cursor = buf.cursor_at_byte(at);
        // A same-chapter jump doesn't clear folds (`activate_chapter` is a no-op when
        // the target is already active), so the landing byte can sit inside a
        // collapsed section. Reveal the fold so the cursor isn't hidden on a zero-row
        // line (#975); cross-chapter switches already cleared folds, so this no-ops.
        self.reveal_fold_at_cursor();
    }
}
