//! The style-flag sweep (#1001): the author picks a flag category from the
//! analysis report's axes (adverbs, filter words, weak / passive verbs, long
//! sentences, over-used words) and walks each occurrence in the manuscript the way
//! they walk spelling mistakes — `]r` / `[r` (or the palette) step the cursor onto
//! the next / previous hit, and the active category's hits underline in place until
//! the sweep is toggled off.
//!
//! Quill never edits the flagged prose — this only navigates to it and marks it, so
//! the author makes every change (the AI/analysis boundary). The flag set is
//! computed on the idle tick and cached ([`refresh_style_flags`]), never per
//! keystroke or per draw; the renderer reads the cache. Stepping wraps within the
//! active buffer and, in the single-chapter view, crosses into the next / previous
//! chapter when the current one is exhausted, reporting clearly at the ends of the
//! manuscript.

use std::sync::Arc;

use crate::analysis::{flag_ranges, FlagCategory, WordLists};
use crate::mode::{Command, Mode, Palette, PaletteItem};

use super::App;

/// What a style-flag scan depends on (#1001): the edit buffer's content revision
/// (process-unique, so it also names which buffer), the category, the run-on
/// threshold, and the word lists (by identity — the cache rebuilds them as a new
/// allocation when the `[analysis]` section changes, and holding the `Arc` here
/// keeps that address from being reused).
pub(super) struct StyleKey {
    rev: u64,
    category: FlagCategory,
    threshold: usize,
    lists: Arc<WordLists>,
}

impl StyleKey {
    fn matches(&self, other: &Self) -> bool {
        self.rev == other.rev
            && self.category == other.category
            && self.threshold == other.threshold
            && Arc::ptr_eq(&self.lists, &other.lists)
    }
}

impl App {
    /// Bring the active category's flag ranges for the edit buffer up to date
    /// (#1001), off the keystroke path. A no-op (and empty) when no sweep is active,
    /// so it costs nothing until the author turns one on — and even then it's
    /// window-scoped, like the spell pass. Runs on the idle tick and on demand
    /// before a step; skips the rescan when none of its inputs ([`StyleKey`]) moved
    /// since the last one, so a quiet pass after a cursor move costs nothing.
    pub(crate) fn refresh_style_flags(&mut self) {
        let Some(category) = self.style_flag_category else {
            self.style_flags.clear();
            self.style_flags_key = None;
            return;
        };
        let key = StyleKey {
            rev: self.edit_buffer().revision(),
            category,
            threshold: self.project.manifest.run_on_threshold(),
            lists: self.analysis_word_lists(),
        };
        if self
            .style_flags_key
            .as_ref()
            .is_some_and(|k| k.matches(&key))
        {
            return;
        }
        let text = self.edit_buffer().text();
        self.style_flags = flag_ranges(&text, category, &key.lists, Some(key.threshold));
        self.style_flags_key = Some(key);
    }

    /// Open the category picker (#1001): the report's flag axes as a pick-list, plus
    /// an "(off)" entry to clear an active sweep. Reachable from the run-action menu
    /// and the analysis report; choosing a category turns the sweep on and jumps to
    /// its first hit.
    pub(crate) fn open_style_flag_picker(&mut self) {
        let mut items = vec![PaletteItem::new(
            "(off) — clear flags",
            "Turn the style-flag sweep off and remove the underlines",
            Command::ClearStyleFlags,
        )];
        items.extend(FlagCategory::ALL.iter().map(|&cat| {
            PaletteItem::new(
                cat.label(),
                format!(
                    "Underline and step through {} in the manuscript",
                    cat.label()
                ),
                Command::PickStyleFlag(cat),
            )
        }));
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Style flags \u{2014} pick a category to sweep",
            items,
            selected: self.style_flag_category.map_or(0, |c| {
                // Land the selection on the active category, past the "(off)" row.
                FlagCategory::ALL
                    .iter()
                    .position(|&x| x == c)
                    .map_or(0, |i| i + 1)
            }),
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// Turn on the sweep for `category` (#1001): mark it active, compute its flags,
    /// underline them, and jump the cursor to the first one (or say the chapter has
    /// none). The chosen category persists, so `]r` / `[r` keep stepping it.
    pub(crate) fn start_style_flag_sweep(&mut self, category: FlagCategory) {
        self.style_flag_category = Some(category);
        self.refresh_style_flags();
        if self.style_flags.is_empty() {
            self.message = Some(format!(
                "{}: none in this chapter (underlining on; ]r/[r to step).",
                category.label()
            ));
            return;
        }
        // Land on the first hit at or after the cursor, so turning the sweep on also
        // walks the author to the nearest occurrence.
        let cur = self.edit_buffer().cursor_byte();
        let target = self
            .style_flags
            .iter()
            .find(|&&(s, _)| s >= cur)
            .map_or(self.style_flags[0].0, |&(s, _)| s);
        // The first hit can sit inside a collapsed section (#975); reveal it so
        // turning the sweep on doesn't strand the cursor on a hidden line.
        self.land_cursor_revealing(target);
        self.message = Some(format!(
            "{} sweep on ({} in this chapter) \u{2014} ]r/[r to step, off to clear.",
            category.label(),
            self.style_flags.len()
        ));
    }

    /// Turn the sweep off (#1001): clear the active category and its cached flags, so
    /// the underlines vanish on the next frame.
    pub(crate) fn clear_style_flags(&mut self) {
        if self.style_flag_category.is_none() {
            self.message = Some("No style-flag sweep is active.".into());
            return;
        }
        self.style_flag_category = None;
        self.style_flags.clear();
        self.style_flags_key = None;
        self.message = Some("Style-flag sweep off.".into());
    }

    /// Step to the next (`dir >= 0`) or previous flag of the active category
    /// (#1001) — Vim's `]r` / `[r`. Within the current chapter it moves to the
    /// nearest hit past the cursor; when none remain in that direction it crosses
    /// into the next / previous chapter (single-chapter view), landing on that
    /// chapter's first / last hit and skipping chapters with none. At the manuscript
    /// end it says so rather than moving silently. With no sweep active it prompts
    /// the author to pick a category first.
    pub(crate) fn navigate_style_flag(&mut self, dir: i32) {
        let Some(category) = self.style_flag_category else {
            self.message =
                Some("No style-flag sweep active \u{2014} pick a category first.".into());
            return;
        };
        self.refresh_style_flags();
        // Within the current buffer, is there a hit past the cursor in `dir`?
        let cur = self.edit_buffer().cursor_byte();
        let within = if dir >= 0 {
            self.style_flags
                .iter()
                .find(|&&(s, _)| s > cur)
                .map(|&(s, _)| s)
        } else {
            self.style_flags
                .iter()
                .rev()
                .find(|&&(s, _)| s < cur)
                .map(|&(s, _)| s)
        };
        if let Some(target) = within {
            // The hit can sit inside a collapsed section (#975); land on it and
            // reveal the fold so the cursor isn't hidden away (mirrors search.rs).
            self.land_cursor_revealing(target);
            return;
        }
        // Nothing left in this chapter's buffer that direction: cross chapters. The
        // continuous view already spans a window, so its wrap-in-place is the sane
        // fallback there; the single-chapter view walks the manuscript file by file.
        if self.manuscript.is_some() {
            self.wrap_within_buffer(dir, category);
        } else {
            self.step_across_chapters(dir, category);
        }
    }

    /// Move the edit cursor to byte `target` and reveal any collapsed section it
    /// lands inside (#975), so a same-buffer sweep landing never strands the cursor
    /// on a folded (zero-row) line. The single landing site the within-chapter
    /// step, the continuous-view wrap, and the sweep's first-hit jump funnel
    /// through, mirroring search.rs's reveal-after-jump.
    fn land_cursor_revealing(&mut self, target: usize) {
        let buffer = self.edit_buffer_mut();
        buffer.cursor = buffer.cursor_at_byte(target);
        self.reveal_fold_at_cursor();
    }

    /// No more hits ahead in the (continuous-view) buffer: wrap to the first / last
    /// hit in it, or report the sweep found none.
    fn wrap_within_buffer(&mut self, dir: i32, category: FlagCategory) {
        let Some(&(first, _)) = self.style_flags.first() else {
            self.message = Some(format!("{}: none in view.", category.label()));
            return;
        };
        let target = if dir >= 0 {
            first
        } else {
            self.style_flags.last().map_or(first, |&(s, _)| s)
        };
        // The wrap target can sit inside a collapsed section (#975); reveal it.
        self.land_cursor_revealing(target);
        self.message = Some(format!(
            "{}: wrapped to the {} hit.",
            category.label(),
            if dir >= 0 { "first" } else { "last" }
        ));
    }

    /// Single-chapter view: walk to the next / previous chapter that has a hit of
    /// `category`, open it, and land on its first (forward) / last (backward) hit
    /// (#1001). Reports the end of the manuscript when there is no further chapter
    /// with a hit, and leaves the cursor put.
    fn step_across_chapters(&mut self, dir: i32, category: FlagCategory) {
        let threshold = Some(self.project.manifest.run_on_threshold());
        let lists = self.analysis_word_lists();
        let active = self.project.active_index();
        let count = self.chapter_count();
        // Walk outward from the active chapter, one at a time, until a chapter with a
        // hit is found — scanning each off disk (the report's own read path), so this
        // never loads the whole manuscript at once.
        let mut idx = active;
        loop {
            idx = if dir >= 0 {
                if idx + 1 >= count {
                    self.message = Some(format!("{}: end of the manuscript.", category.label()));
                    return;
                }
                idx + 1
            } else {
                if idx == 0 {
                    self.message = Some(format!("{}: start of the manuscript.", category.label()));
                    return;
                }
                idx - 1
            };
            let text = self.chapter_text_with_active(idx, active);
            let ranges = flag_ranges(&text, category, &lists, threshold);
            let landing = if dir >= 0 {
                ranges.first().map(|&(s, _)| s)
            } else {
                ranges.last().map(|&(s, _)| s)
            };
            if let Some(target) = landing {
                // `activate_chapter` refreshes the flag cache for the newly opened
                // chapter (#1001), so the hit spans are already correct here. Place
                // the cursor on the found hit (clamped, in case the live buffer
                // differs from the disk read) and reveal any fold hiding it.
                self.activate_chapter(idx);
                let at = target.min(self.buffer.len_bytes());
                self.buffer.cursor = self.buffer.cursor_at_byte(at);
                self.reveal_fold_at_cursor();
                if let Some(title) = self.project.active_title() {
                    self.message = Some(format!("{} \u{2014} {}", category.label(), title));
                }
                return;
            }
        }
    }
}
