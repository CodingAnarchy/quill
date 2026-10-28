//! The corkboard (#589 Phase 1 / #636 Phase 2 / #698): an index-card overview of
//! the active chapter's scenes, built from the Markdown headings Quill already
//! parses (the same headings the Ctrl-O outline lists). Each card is a scene
//! heading plus a synopsis, and the view is navigable; Enter jumps the editor
//! cursor to that scene.
//!
//! A *scene* is a heading (level 2 or deeper — the `#` chapter title is not a
//! reorderable scene) together with everything up to the next heading of the same
//! or a shallower level (#698). So a `##` scene owns its `###` sub-beats: they
//! fold into its body rather than each becoming its own card, and reordering the
//! `##` carries them with it. This keeps the card list a clean, non-overlapping
//! partition of the body, so the whole-body rewrite reorder does never splits a
//! scene's prose.
//!
//! Phase 2 turns it into an editable planning surface. Each card carries an
//! author-written one-line synopsis and a drafting status (stub / draft / done),
//! stored positionally in the chapter's `scenes:` YAML front matter so the prose
//! stays clean; the author edits the synopsis, cycles the status, and reorders
//! scenes (the heading and its prose move together) from the board. Every edit
//! rewrites the chapter through the buffer/save machinery and rebuilds the cards,
//! so the byte offsets stay a valid jump target and the metadata stays aligned
//! with the scenes. It's off the keystroke path — the board is a separate mode,
//! and reorder does one whole-body rewrite per gesture, never per key.

use crossterm::event::{KeyCode, KeyEvent};

use super::App;
use crate::frontmatter::{self, FrontMatter, SceneMeta};
use crate::mode::{Card, Corkboard, Mode, Pending};
use crate::scenes::scene_label;
use crate::syntax::{Heading, Syntax};

/// One undoable cross-chapter scene move (#974): the source and destination
/// chapters' full contents *before* the move, so the board's `u` can restore the
/// exact prior arrangement with the same atomic two-file write. A cross-chapter
/// move rewrites two files outside the buffer's rope-level undo, so its undo is
/// this captured snapshot rather than a rope step. `selected` is the scene's
/// original (chapter, within-chapter index), so the restored board reopens on it.
pub(super) struct CrossMoveUndo {
    src: usize,
    src_content: String,
    dst: usize,
    dst_content: String,
    selected: (usize, usize),
}

impl App {
    /// Open the corkboard for the active chapter (#589 / #636), or report when the
    /// chapter has no scene headings to card up. Refused in the continuous view,
    /// whose buffer spans several chapters and has no single front matter — the
    /// author leaves it (Ctrl-l) first, like the other chapter-metadata features.
    pub(crate) fn open_corkboard(&mut self) {
        if self.manuscript.is_some() {
            self.message = Some("Leave the continuous view (Ctrl-l) to use the corkboard.".into());
            return;
        }
        // The per-chapter board reads and edits scenes through the focused editor
        // buffer, so it must be the active chapter. With a reference pane focused (a
        // bible entry / note, #165) `edit_buffer()` is that file — carding up its
        // `##` sections as the chapter's scenes, and an edit would write them back
        // into the reference file. Refuse until a chapter is focused, like the
        // outline (#165), whose buffer this board also edits into.
        if !self.focus_shows_active_chapter() {
            self.message = Some("Focus a chapter to use the corkboard.".into());
            return;
        }
        let cards = self.corkboard_cards();
        if cards.is_empty() {
            self.message = Some(if self.editing_story() {
                "No scenes yet — write some prose to build the corkboard.".into()
            } else {
                "No scenes here — add a ## or ### heading to build the corkboard.".into()
            });
            return;
        }
        self.mode = Mode::Corkboard(Box::new(Corkboard {
            cards,
            selected: 0,
            manuscript: false,
        }));
    }

    /// Open the manuscript-wide corkboard (#701): scene cards across *every*
    /// chapter, grouped by chapter, navigable, with jump-to-scene. Unlike the
    /// per-chapter board this is reachable from the continuous view — it leaves it
    /// first (like the other project-wide features) so a jump lands in a normal
    /// single-chapter editing buffer. Cards are sourced from each chapter's body +
    /// `scenes:` front matter off the keystroke path (the active chapter from its
    /// live buffer, the rest read on demand, one file at a time), so this scales
    /// with the manuscript, never the per-frame budget.
    pub(crate) fn open_manuscript_corkboard(&mut self) {
        self.leave_continuous();
        let _ = self.flush_active();
        let cards = self.manuscript_corkboard_cards();
        if cards.is_empty() {
            self.message =
                Some("No scenes anywhere — add a ## or ### heading to build the corkboard.".into());
            return;
        }
        self.mode = Mode::Corkboard(Box::new(Corkboard {
            cards,
            selected: 0,
            manuscript: true,
        }));
    }

    /// Build the corkboard cards for the active buffer (#589 / #636 / #698): one
    /// per scene, in document order, each with the scene heading's byte offset for
    /// the jump and its metadata (synopsis + status) from the chapter's `scenes:`
    /// front matter, reconciled by position — a chapter with no metadata (or fewer
    /// entries than scenes) still lists every scene as a card, padded with the
    /// default (blank synopsis, stub status). A scene owns its deeper sub-beats and
    /// the `#` chapter title is not a card (#698). Reads the already-parsed headings
    /// and a single slice per scene — no re-parse.
    pub(crate) fn corkboard_cards(&self) -> Vec<Card> {
        // A short story's cards are its scenes, split by breaks too (#1240).
        if self.editing_story() {
            return self.story_cards();
        }
        let buffer = self.edit_buffer();
        let text = buffer.text();
        let headings = buffer.syntax().headings();
        let meta = buffer
            .front_matter()
            .map(super::super::frontmatter::FrontMatter::scenes)
            .unwrap_or_default();
        // Per-chapter board: cards belong to the active buffer, so they carry no
        // chapter tag or group header.
        cards_for(&text, headings, &meta, None, "")
    }

    /// Build the manuscript-wide corkboard cards (#701): every chapter's scenes in
    /// chapter then document order, each tagged with its chapter index and title so
    /// the board groups them and a jump/edit targets the right file. The active
    /// chapter's cards come from its live buffer (so pending edits show); other
    /// chapters are read from disk on demand — one body + one front-matter read
    /// each, off the keystroke path.
    fn manuscript_corkboard_cards(&self) -> Vec<Card> {
        let active = self.project.active_index();
        (0..self.chapter_count())
            .flat_map(|ch| {
                let title = self.project.chapter_title(ch).unwrap_or("").to_string();
                // The active chapter is already parsed in the buffer; reuse it
                // rather than re-reading the file — but only when the focused buffer
                // actually shows it. With a reference pane focused (a bible entry /
                // note, #165) `edit_buffer()` is that file, so its `##` sections
                // would be carded up as this chapter's scenes; fall back to the
                // on-disk read like the other chapters (focusing away flushed the
                // chapter, so disk is current), matching `chapter_text_with_active`.
                if ch == active && self.focus_shows_active_chapter() {
                    let buffer = self.edit_buffer();
                    let text = buffer.text();
                    let meta = buffer
                        .front_matter()
                        .map(super::super::frontmatter::FrontMatter::scenes)
                        .unwrap_or_default();
                    cards_for(&text, buffer.syntax().headings(), &meta, Some(ch), &title)
                } else {
                    let body = self.project.chapter_body(ch);
                    let headings = Syntax::new(&body).headings().to_vec();
                    let meta = self
                        .project
                        .chapter_front_matter(ch)
                        .map(|fm| fm.scenes())
                        .unwrap_or_default();
                    cards_for(&body, &headings, &meta, Some(ch), &title)
                }
            })
            .collect()
    }

    /// Key handling for the corkboard (#589 / #636 / #701 / #974): move (arrows /
    /// j-k), jump the editor cursor to the selected scene (Enter), reorder the
    /// selected scene up (`K` / `[`) or down (`J` / `]`) — which on the
    /// manuscript-wide board carries a scene across a chapter boundary at the ends
    /// (#974) — edit its synopsis (`e`), cycle its status (`s` / Space), undo the
    /// last cross-chapter move (`u`), close (Esc). On the manuscript-wide board every
    /// edit first targets the selected card's chapter, then rebuilds the board.
    pub(crate) fn on_key_corkboard(&mut self, key: KeyEvent) {
        let Mode::Corkboard(board) = &mut self.mode else {
            return;
        };
        if let Some(s) = super::assist::list_move(board.selected, board.cards.len(), key.code) {
            board.selected = s;
            return;
        }
        let selected = board.selected;
        match key.code {
            KeyCode::Enter => self.jump_to_card(selected),
            KeyCode::Char('K' | '[') => self.reorder_scene(selected, -1),
            KeyCode::Char('J' | ']') => self.reorder_scene(selected, 1),
            KeyCode::Char('e') => self.prompt_scene_synopsis(selected),
            KeyCode::Char('s' | ' ') => self.cycle_scene_status(selected),
            KeyCode::Char('u') => self.undo_cross_move(),
            KeyCode::Esc => self.mode = self.editing_mode(),
            _ => {}
        }
    }

    /// Jump the editor to the scene of the card at `index` and close the board: on
    /// the manuscript-wide board (#701) open the card's chapter first (a normal
    /// single-chapter buffer), then move the cursor to the scene heading. On the
    /// per-chapter board the card is already in the active buffer.
    fn jump_to_card(&mut self, index: usize) {
        let Some((chapter, start)) = self.card_target(index) else {
            self.mode = self.editing_mode();
            return;
        };
        // Record the jump-back target (#825) before the board sends the cursor to
        // the scene — possibly in another chapter — so `` `` `` returns here.
        self.push_jump();
        self.mode = self.editing_mode();
        if let Some(chapter) = chapter {
            self.activate_chapter(chapter);
        }
        let buf = self.edit_buffer_mut();
        let at = start.min(buf.len_bytes());
        buf.cursor = buf.cursor_at_byte(at);
    }

    /// The chapter and scene-heading byte offset the card at `index` targets: the
    /// `chapter` is `Some` only on the manuscript-wide board (#701), where an edit
    /// or jump must open that file first; `None` means the active buffer.
    fn card_target(&self, index: usize) -> Option<(Option<usize>, usize)> {
        let Mode::Corkboard(board) = &self.mode else {
            return None;
        };
        board.cards.get(index).map(|c| (c.chapter, c.start))
    }

    /// Resolve what the card at board `index` edits (#701): its chapter (`Some` only
    /// on the manuscript-wide board), its within-chapter scene index (the position
    /// the per-chapter reorder / status / synopsis machinery indexes by), and
    /// whether this is the manuscript-wide board (so the right board re-opens). The
    /// within-chapter index counts the cards before it that share its chapter, so on
    /// the per-chapter board it is simply the card index.
    fn card_edit_target(&self, index: usize) -> Option<(Option<usize>, usize, bool)> {
        let Mode::Corkboard(board) = &self.mode else {
            return None;
        };
        let card = board.cards.get(index)?;
        let within = board.cards[..index]
            .iter()
            .filter(|c| c.chapter == card.chapter)
            .count();
        Some((card.chapter, within, board.manuscript))
    }

    /// Move the scene at card `index` one slot in `dir` (−1 up, +1 down): its
    /// heading and prose (the whole block, including any folded-in `###` sub-beats)
    /// swap with the neighbouring scene block, and its `scenes:` metadata entry
    /// moves with it so the two stay aligned (#636 / #698). A no-op at the ends.
    /// Rewrites the whole body once (off the keystroke path), persists, and
    /// rebuilds the cards with the moved scene still selected.
    fn reorder_scene(&mut self, index: usize, dir: isize) {
        // Resolve the card's target, then focus its chapter so the per-chapter
        // machinery below works in that file's within-chapter scene indices.
        let Some((chapter, scene, manuscript)) = self.card_edit_target(index) else {
            return;
        };
        if self.edit_hits_a_reference_pane(chapter) {
            self.message = Some("Focus a chapter to edit its scenes.".into());
            self.reopen_board_for(manuscript, chapter, scene);
            return;
        }
        self.focus_chapter(chapter);
        // A short story reorders by its own scene model (#1240), keeping exactly
        // one break between scenes; it has no neighbouring chapter to cross into.
        if !manuscript && self.editing_story() {
            let to = self.move_story_scene(scene, dir).unwrap_or(scene);
            self.reopen_board_for(false, chapter, to);
            return;
        }
        let buffer = self.edit_buffer();
        let text = buffer.text();
        let scenes = scenes(&text, buffer.syntax().headings());
        // Whether the neighbour is off this chapter's ends: moving up from the first
        // scene, or down from the last. On the manuscript-wide board (#701) that
        // reaches into the adjacent chapter — a cross-chapter move (#974). On the
        // per-chapter board it's simply a no-op at the ends.
        let other = scene.checked_add_signed(dir);
        let crosses = scene >= scenes.len() || other.is_none_or(|o| o >= scenes.len());
        if crosses {
            if manuscript && !scenes.is_empty() {
                self.cross_chapter_move(chapter, scene, dir);
            } else {
                self.reopen_board_for(manuscript, chapter, scene);
            }
            return;
        }
        let Some(other) = other else {
            self.reopen_board_for(manuscript, chapter, scene);
            return;
        };
        // The two adjacent scene blocks, in body order (lower index first).
        let (a, b) = (scene.min(other), scene.max(other));
        let mut body = String::with_capacity(text.len());
        body.push_str(&text[..scenes[a].start]); // everything before the pair
        body.push_str(&text[scenes[b].start..scenes[b].end]); // the later block, first
        body.push_str(&text[scenes[a].start..scenes[a].end]); // then the earlier one
        body.push_str(&text[scenes[b].end..]); // everything after the pair

        // Move the metadata entry with its scene, padding to scene count so the
        // swap is positional even when the block is shorter (or absent).
        let mut meta = self.chapter_scenes(scenes.len());
        meta.swap(a, b);

        self.rewrite_chapter(&body, &meta);
        self.reopen_board_for(manuscript, chapter, other);
    }

    /// Move the scene at within-chapter index `scene` of `chapter` across a chapter
    /// boundary in `dir` (−1 up into the previous chapter, +1 down into the next)
    /// (#974): the whole scene block (heading + prose + any folded-in `###`
    /// sub-beats) leaves the source chapter and lands in the destination, carrying
    /// its `scenes:` metadata entry (synopsis + status). Moving down inserts the
    /// scene *before* the destination's first scene; moving up inserts it *after*
    /// the destination's last, so it stays adjacent across the seam. Both files are
    /// rewritten atomically — a failure leaves neither changed (#262) — and the
    /// prior arrangement is captured so the board's `u` restores it. The board
    /// reopens on the moved scene, now in its new chapter.
    fn cross_chapter_move(&mut self, chapter: Option<usize>, scene: usize, dir: isize) {
        let Some(src) = chapter else { return }; // manuscript board only
        let dst = if dir < 0 {
            src.checked_sub(1)
        } else {
            (src + 1 < self.chapter_count()).then_some(src + 1)
        };
        let Some(dst) = dst else {
            // No chapter beyond this end (the manuscript's first or last scene).
            self.reopen_board_for(true, chapter, scene);
            return;
        };
        // Flush the active buffer so both source and destination read their current
        // bytes from disk — either may be the active chapter.
        if self.flush_active().is_err() {
            self.reopen_board_for(true, chapter, scene);
            return;
        }
        // Read both chapters as (front matter, body), off the keystroke path. A
        // *hard* read error on either file must abort the move: composing the
        // destination's new content onto an empty body would durably wipe its real
        // prose while reporting success, and the undo would capture that empty
        // state (#565/#871). Both reads are guarded before anything is written.
        let (src_front, src_body) = match self.split_chapter_file(src) {
            Ok(pair) => pair,
            Err(e) => {
                self.message = Some(format!("Move failed: could not read chapter: {e}"));
                self.reopen_board_for(true, chapter, scene);
                return;
            }
        };
        let (dst_front, dst_body) = match self.split_chapter_file(dst) {
            Ok(pair) => pair,
            Err(e) => {
                self.message = Some(format!("Move failed: could not read chapter: {e}"));
                self.reopen_board_for(true, chapter, scene);
                return;
            }
        };
        let src_blocks = scenes(&src_body, Syntax::new(&src_body).headings());
        let Some(block) = src_blocks.get(scene) else {
            self.reopen_board_for(true, chapter, scene);
            return;
        };
        // The scene block leaves the source body; its metadata entry leaves the
        // source `scenes:`, both reconciled to the source's scene count first.
        let moved_prose = src_body[block.start..block.end].to_string();
        let mut src_meta = src_front.scenes();
        src_meta.resize(src_blocks.len(), SceneMeta::default());
        let moved_meta = src_meta.remove(scene);
        let mut new_src_body = String::with_capacity(src_body.len());
        new_src_body.push_str(&src_body[..block.start]);
        new_src_body.push_str(&src_body[block.end..]);

        // Insert into the destination: moving down lands before its first scene,
        // moving up lands after its last, so the scene stays adjacent across the
        // seam. `at` is the byte offset in the destination body and `pos` the
        // metadata index; both fall back to the body's own bounds when the
        // destination has no scenes of its own yet.
        let dst_blocks = scenes(&dst_body, Syntax::new(&dst_body).headings());
        let (at, pos) = if dir < 0 {
            (
                dst_blocks.last().map_or(dst_body.len(), |b| b.end),
                dst_blocks.len(),
            )
        } else {
            (dst_blocks.first().map_or(dst_body.len(), |b| b.start), 0)
        };
        let insert = normalized_scene_block(&moved_prose);
        let mut new_dst_body = String::with_capacity(dst_body.len() + insert.len());
        new_dst_body.push_str(&dst_body[..at]);
        new_dst_body.push_str(&insert);
        new_dst_body.push_str(&dst_body[at..]);
        let mut dst_meta = dst_front.scenes();
        dst_meta.resize(dst_blocks.len(), SceneMeta::default());
        dst_meta.insert(pos.min(dst_meta.len()), moved_meta);

        // Capture both chapters' prior contents before the write, so the board's
        // `u` can restore the exact prior arrangement even though this edit lands
        // outside the buffer's own rope-level undo.
        let undo = CrossMoveUndo {
            src,
            src_content: join_chapter(&src_front, &src_body),
            dst,
            dst_content: join_chapter(&dst_front, &dst_body),
            selected: (src, scene),
        };

        // Rewrite both files atomically: build each chapter's full new contents
        // (front matter with the moved `scenes:` entry, then body) and write the
        // pair in one all-or-nothing step.
        let new_src = compose_chapter(&src_front, &src_meta, &new_src_body);
        let new_dst = compose_chapter(&dst_front, &dst_meta, &new_dst_body);
        if let Err(e) = self
            .project
            .write_chapters_atomic(&[(src, new_src), (dst, new_dst)])
        {
            self.message = Some(format!("Move failed: {e}"));
            self.reopen_board_for(true, chapter, scene);
            return;
        }
        self.cork_undo = Some(undo);
        // If the active chapter was one of the two, its on-disk content changed
        // under the live buffer — reload it so the buffer and the board agree.
        self.reload_active_after_disk_edit();
        self.cache_focus_words();
        // The board reopens on the moved scene, now the `pos`-th scene of `dst`.
        self.reopen_board_for(true, Some(dst), pos);
    }

    /// Undo the last cross-chapter scene move (#974): restore both chapters to the
    /// exact contents captured before the move, atomically, and reopen the board on
    /// the scene in its original chapter. A no-op (with a message) when there's
    /// nothing to undo — the one level a cross-chapter move records, cleared once
    /// used. The within-chapter reorder / status / synopsis edits go through the
    /// buffer + front-matter machinery and aren't part of this one-level structural
    /// undo.
    fn undo_cross_move(&mut self) {
        let Some(undo) = self.cork_undo.take() else {
            self.message = Some("Nothing to undo on the board.".into());
            return;
        };
        // Flush the active buffer first so the restore lands cleanly even if the
        // active chapter is one of the two being reverted.
        if self.flush_active().is_err() {
            self.cork_undo = Some(undo); // keep it — the undo didn't happen
            return;
        }
        if let Err(e) = self.project.write_chapters_atomic(&[
            (undo.src, undo.src_content.clone()),
            (undo.dst, undo.dst_content.clone()),
        ]) {
            self.message = Some(format!("Undo failed: {e}"));
            self.cork_undo = Some(undo); // keep it — the undo didn't happen
            return;
        }
        self.reload_active_after_disk_edit();
        self.cache_focus_words();
        let (chapter, scene) = undo.selected;
        self.message = Some("Reverted the cross-chapter move.".into());
        self.reopen_board_for(true, Some(chapter), scene);
    }

    /// Read chapter `index` from disk and split it into its [`FrontMatter`] (default
    /// when the file has none) and prose body (#974). Off the keystroke path; used by
    /// the cross-chapter move to work on both the source and destination files.
    ///
    /// A *hard* read failure (permissions, an I/O fault, the path being a
    /// directory) is propagated as `Err` rather than collapsing to default front
    /// matter + an empty body, so the caller aborts the move instead of composing
    /// the destination's new content onto an empty base — which would durably wipe
    /// its real prose while reporting success (#565/#871). A missing file is the
    /// legitimate "empty chapter" case and yields the default front matter + `""`.
    fn split_chapter_file(&self, index: usize) -> std::io::Result<(FrontMatter, String)> {
        let (front, body) = self.project.try_split_chapter(index)?;
        Ok((front.unwrap_or_default(), body))
    }

    /// Reload the active chapter's buffer from disk after an off-buffer edit changed
    /// its file (#974): the cross-chapter move (and its undo) rewrites two chapter
    /// files directly, and if one of them is the active chapter the live buffer is
    /// now stale. Reloads it from disk so the buffer, the pane, and the board agree.
    /// A no-op-safe reload — a read failure is surfaced but the board still reopens.
    fn reload_active_after_disk_edit(&mut self) {
        match crate::buffer::Buffer::from_path(self.project.active_path()) {
            Ok(buffer) => self.buffer = buffer,
            Err(e) => self.message = Some(format!("Open failed: {e}")),
        }
    }

    /// Cycle the selected scene's status (stub → draft → done → stub) and persist
    /// it to the chapter's `scenes:` front matter (#636), then rebuild the board.
    fn cycle_scene_status(&mut self, index: usize) {
        let Some((chapter, scene, manuscript)) = self.card_edit_target(index) else {
            return;
        };
        if self.edit_hits_a_reference_pane(chapter) {
            self.message = Some("Focus a chapter to edit its scenes.".into());
            self.reopen_board_for(manuscript, chapter, scene);
            return;
        }
        self.focus_chapter(chapter);
        let count = self.scene_count();
        if scene >= count {
            return;
        }
        let mut scenes = self.chapter_scenes(count);
        scenes[scene].status = scenes[scene].status.next();
        self.set_chapter_scenes(&scenes);
        self.reopen_board_for(manuscript, chapter, scene);
    }

    /// Focus `chapter` as the active buffer if it is `Some` and not already active
    /// (manuscript-wide board, #701), so the per-chapter edit helpers act on the
    /// right file. A no-op for `None` (the per-chapter board, already the active
    /// buffer).
    fn focus_chapter(&mut self, chapter: Option<usize>) {
        if let Some(chapter) = chapter {
            self.activate_chapter(chapter);
        }
    }

    /// Whether a manuscript-wide board edit of the card at `chapter` would land on
    /// the active chapter while a reference pane holds focus (#165): the target *is*
    /// the active chapter, but the focused editor buffer is a bible entry / note, not
    /// that chapter. `focus_chapter` can't switch to it — its index already is the
    /// active one, so `activate_chapter` is a no-op — so the edit helpers below would
    /// flow through the reference buffer and rewrite that file as if it were the
    /// chapter. Such an edit is refused instead (the caller reports it and reopens the
    /// board), mirroring the per-chapter board, which refuses to open at all when a
    /// reference pane is focused.
    fn edit_hits_a_reference_pane(&self, chapter: Option<usize>) -> bool {
        chapter == Some(self.project.active_index()) && !self.focus_shows_active_chapter()
    }

    /// Open a text prompt to edit the selected scene's one-line synopsis (#636),
    /// prefilled with the current value. Enter persists it; Esc leaves it. The
    /// target (chapter + within-chapter scene index) is resolved now and carried on
    /// the prompt, so the write lands on the right file after the mode leaves the
    /// board for the prompt (#701).
    fn prompt_scene_synopsis(&mut self, index: usize) {
        let Some((chapter, scene, manuscript)) = self.card_edit_target(index) else {
            return;
        };
        if self.edit_hits_a_reference_pane(chapter) {
            self.message = Some("Focus a chapter to edit its scenes.".into());
            self.reopen_board_for(manuscript, chapter, scene);
            return;
        }
        let Mode::Corkboard(board) = &self.mode else {
            return;
        };
        let prefill = board
            .cards
            .get(index)
            .map(|c| c.synopsis.clone())
            .unwrap_or_default();
        self.open_prompt(
            Pending::SetSceneSynopsis {
                chapter,
                scene,
                manuscript,
            },
            "Scene synopsis: ",
            &prefill,
            None,
        );
    }

    /// Persist a scene's edited synopsis (#636 / #701), from the
    /// [`Pending::SetSceneSynopsis`] prompt. A blank entry clears it. Focuses the
    /// resolved `chapter` first (manuscript-wide board), writes to its `scenes:`
    /// front matter, and rebuilds the board with the scene selected.
    pub(crate) fn apply_scene_synopsis(
        &mut self,
        chapter: Option<usize>,
        scene: usize,
        manuscript: bool,
        text: &str,
    ) {
        self.focus_chapter(chapter);
        let count = self.scene_count();
        if scene < count {
            let mut scenes = self.chapter_scenes(count);
            scenes[scene].synopsis = text.trim().to_string();
            self.set_chapter_scenes(&scenes);
        }
        self.reopen_board_for(manuscript, chapter, scene);
    }

    /// How many scenes the active chapter has (#698): the card count, one per scene
    /// heading with its sub-beats folded in, excluding the `#` chapter title. The
    /// count the metadata list is reconciled against before an edit.
    fn scene_count(&self) -> usize {
        if self.editing_story() {
            return self.story_scenes_now().1.len();
        }
        let buffer = self.edit_buffer();
        scenes(&buffer.text(), buffer.syntax().headings()).len()
    }

    /// The chapter's scene metadata, reconciled to exactly `count` entries so it
    /// aligns positionally with the scenes (#636 / #698): a shorter list pads with
    /// the default (blank synopsis, stub status), a longer one truncates. The one
    /// place that reads-then-normalizes before an edit.
    pub(super) fn chapter_scenes(&self, count: usize) -> Vec<SceneMeta> {
        let mut scenes = self
            .edit_buffer()
            .front_matter()
            .map(super::super::frontmatter::FrontMatter::scenes)
            .unwrap_or_default();
        // A story's scenes come and go as breaks are typed (#1240): match its
        // metadata by anchor rather than position.
        if self.editing_story() {
            return self.reconciled_story_meta(&scenes);
        }
        scenes.resize(count, SceneMeta::default());
        scenes
    }

    /// Write the scene metadata to the active chapter's front matter and persist
    /// (#636), refreshing the pane cache. Front-matter-only change (no body
    /// rewrite), used by the status cycle and synopsis edit.
    fn set_chapter_scenes(&mut self, scenes: &[SceneMeta]) {
        self.buffer.front_matter_mut().set_scenes(scenes);
        self.buffer.dirty = true;
        self.persist_active_chapter();
    }

    /// Replace the active chapter's prose body and scene metadata together (#636),
    /// then persist. Used by reorder, which moves a body block and its metadata
    /// entry in one rewrite. Goes through [`crate::buffer::Buffer::replace_text`]
    /// (a structural rewrite, off the keystroke path), then writes the metadata.
    pub(super) fn rewrite_chapter(&mut self, body: &str, scenes: &[SceneMeta]) {
        self.buffer.replace_text(body);
        self.buffer.refresh_syntax(); // headings must be current before rebuilding cards
        self.buffer.front_matter_mut().set_scenes(scenes);
        self.persist_active_chapter();
    }

    /// Save the active chapter to disk and refresh its cached metadata + word
    /// count, so a corkboard edit survives reopening the project and the chapter
    /// pane reflects it. Reports a save failure in the status message.
    fn persist_active_chapter(&mut self) {
        // Never race a background save of the same file.
        if !self.settle_saves() {
            return;
        }
        if let Err(e) = self.buffer.save() {
            self.message = Some(format!("Save failed: {e}"));
            return;
        }
        let index = self.project.active_index();
        self.project
            .cache_chapter_meta(index, self.buffer.front_matter());
        self.cache_focus_words();
    }

    /// Rebuild the corkboard after an edit and re-enter it with the scene at
    /// (`chapter`, `scene`) highlighted (#701). `chapter` is `Some` on the
    /// manuscript-wide board, where the board is re-derived across every chapter
    /// and the selection is the card that is `scene` scenes into that chapter;
    /// `None` on the per-chapter board, where `scene` is simply the card index. The
    /// cards are re-derived from the current headings + metadata, so reorder /
    /// status / synopsis changes show at once. Returns to the editor if no scenes
    /// remain (it always should after an in-place edit).
    fn reopen_board_for(&mut self, manuscript: bool, chapter: Option<usize>, scene: usize) {
        let cards = if manuscript {
            self.manuscript_corkboard_cards()
        } else {
            self.corkboard_cards()
        };
        if cards.is_empty() {
            self.mode = self.editing_mode();
            return;
        }
        // Re-find the moved scene by its (chapter, within-chapter index) so the
        // highlight follows it even though the flat board index shifted.
        let selected = chapter.map_or_else(
            || scene.min(cards.len() - 1),
            |ch| {
                cards
                    .iter()
                    .enumerate()
                    .filter(|(_, c)| c.chapter == Some(ch))
                    .nth(scene)
                    .map_or(cards.len() - 1, |(i, _)| i)
            },
        );
        self.mode = Mode::Corkboard(Box::new(Corkboard {
            cards,
            selected,
            manuscript,
        }));
    }
}

impl App {
    /// A short story's corkboard cards (#1240): one per scene, split by scene
    /// breaks or `##` headings, labelled by heading or first line, with the
    /// positional `scenes:` metadata like a chapter's.
    fn story_cards(&self) -> Vec<Card> {
        let (text, scenes) = self.story_scenes_now();
        let meta = self.chapter_scenes(scenes.len());
        scenes
            .iter()
            .zip(meta)
            .map(|(s, meta)| {
                let prose_line =
                    first_prose_line(&text[line_end(&text, s.start).min(s.end)..s.end]);
                Card {
                    title: scene_label(&text, s),
                    level: s.level,
                    prose_line: if s.title.is_some() {
                        prose_line
                    } else {
                        // An untitled card's title already is its first line.
                        first_prose_line(&text[s.start..s.end])
                    },
                    synopsis: meta.synopsis,
                    status: meta.status,
                    start: s.start,
                    chapter: None,
                    chapter_title: String::new(),
                }
            })
            .collect()
    }
}

/// Re-attach `front`'s block verbatim to `body` (#974): a chapter's on-disk bytes
/// from the [`FrontMatter`] it was split into and its prose. An empty front matter
/// yields the bare body, matching how the loader round-trips a file with none.
fn join_chapter(front: &FrontMatter, body: &str) -> String {
    let front = (!front.is_empty()).then(|| front.as_str());
    frontmatter::join(front, body).into_owned()
}

/// A chapter's full new contents for the cross-chapter move (#974): a clone of
/// `front` with its `scenes:` sequence set to `meta` (every other front-matter key
/// preserved), re-attached to `body`. The one place the moved scene's metadata is
/// written into a chapter that isn't the active buffer.
fn compose_chapter(front: &FrontMatter, meta: &[SceneMeta], body: &str) -> String {
    let mut front = front.clone();
    front.set_scenes(meta);
    join_chapter(&front, body)
}

/// Normalize a scene block for insertion into another chapter's body (#974): the
/// extracted block ends where the next heading began (or the body ended), so it may
/// carry a run of trailing blank lines — trim it to a single trailing newline so the
/// inserted scene sits cleanly against the destination's next scene rather than
/// accreting blank lines across repeated moves. A block with no trailing newline
/// gains one so the following heading still starts its own line.
fn normalized_scene_block(block: &str) -> String {
    let trimmed = block.trim_end_matches(['\n', ' ', '\t']);
    format!("{trimmed}\n\n")
}

/// Build one corkboard card per scene of a chapter body (#589 / #636 / #698 /
/// #701 / #974): scenes in document order, each with the scene heading's byte
/// offset for the jump and its metadata (synopsis + status) reconciled by position.
fn cards_for(
    text: &str,
    headings: &[Heading],
    meta: &[SceneMeta],
    chapter: Option<usize>,
    chapter_title: &str,
) -> Vec<Card> {
    scenes(text, headings)
        .iter()
        .enumerate()
        .map(|(i, s)| {
            // The scene body runs from the end of its heading's line up to the end
            // of its block (past any folded-in sub-beats).
            let body_start = line_end(text, s.start).min(text.len());
            let prose_line = first_prose_line(&text[body_start..s.end.min(text.len())]);
            // Reconcile the metadata by position; a missing entry pads to the
            // default rather than dropping the card.
            let meta = meta.get(i).cloned().unwrap_or_default();
            Card {
                title: headings[s.heading].title.clone(),
                level: headings[s.heading].level,
                prose_line,
                synopsis: meta.synopsis,
                status: meta.status,
                start: s.start,
                chapter,
                chapter_title: chapter_title.to_string(),
            }
        })
        .collect()
}

/// One scene of the corkboard (#698): a scene heading plus the block of body it
/// owns. `heading` indexes into the buffer's heading list (for the title/level),
/// and `[start, end)` is the byte range of the whole block — the heading line
/// through everything up to the next heading of the same-or-shallower level, so a
/// `##` scene's `###` sub-beats fold into its range. The card list and the scene
/// list share this order, so a scene's index is its card's index.
struct Scene {
    heading: usize,
    start: usize,
    end: usize,
}

/// The scenes of a chapter body, in document order (#698). A scene begins at a
/// heading of level 2 or deeper that is *not* nested inside a shallower open scene
/// — a heading deeper than the current scene folds into it as a sub-beat rather
/// than starting a card, and a same-or-shallower heading (including a `#` chapter
/// title) closes the current scene and, if it is level 2+, opens the next. The
/// level-1 `#` title is never a card; it only bounds the scene before it. The
/// result is a non-overlapping partition of the body's scene regions, so reorder
/// can swap two blocks without splitting a scene's prose.
fn scenes(text: &str, headings: &[Heading]) -> Vec<Scene> {
    let mut scenes: Vec<Scene> = Vec::new();
    // Index of the scene still accepting sub-beats, and its heading level; `None`
    // once a same-or-shallower heading (or the start) closes it.
    let mut open: Option<(usize, u8)> = None;
    for (i, h) in headings.iter().enumerate() {
        // A heading deeper than the open scene is one of its sub-beats; it extends
        // that scene's block rather than starting a new card.
        if let Some((_, level)) = open {
            if h.level > level {
                continue;
            }
        }
        // Same-or-shallower: this heading closes the open scene, ending its block
        // just before this heading (so a bounding `#` title is not folded in).
        if let Some((idx, _)) = open.take() {
            scenes[idx].end = h.start;
        }
        // The `#` chapter title is not a reorderable scene — it only bounds the
        // block before it.
        if h.level >= 2 {
            open = Some((scenes.len(), h.level));
            scenes.push(Scene {
                heading: i,
                start: h.start,
                end: text.len(),
            });
        }
    }
    scenes
}

/// The byte offset just past the newline that ends the line containing `at` — the
/// start of the scene body under a heading. The heading node's range already
/// covers its own line; this finds where the following prose begins.
fn line_end(text: &str, at: usize) -> usize {
    let at = at.min(text.len());
    text[at..].find('\n').map_or(text.len(), |nl| at + nl + 1)
}

/// The first non-empty, non-heading line of `body`, trimmed — the card's prose
/// fallback. Skips blank lines and any further `#` heading lines (a card's body
/// ends before the next scene, but a leading blank run is common). Empty when the
/// scene has no prose yet.
fn first_prose_line(body: &str) -> String {
    body.lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .unwrap_or("")
        .to_string()
}
