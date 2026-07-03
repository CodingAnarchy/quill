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
use crate::frontmatter::SceneMeta;
use crate::mode::{Card, Corkboard, Mode, Pending};
use crate::syntax::{Heading, Syntax};

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
        let cards = self.corkboard_cards();
        if cards.is_empty() {
            self.message =
                Some("No scenes here — add a ## or ### heading to build the corkboard.".into());
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
                // rather than re-reading the file.
                if ch == active {
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

    /// Key handling for the corkboard (#589 / #636 / #701): move (arrows / j-k),
    /// jump the editor cursor to the selected scene (Enter), reorder the selected
    /// scene up (`K` / `[`) or down (`J` / `]`), edit its synopsis (`e`), cycle its
    /// status (`s` / Space), close (Esc). On the manuscript-wide board every edit
    /// first targets the selected card's chapter, then rebuilds the board.
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
        // On the manuscript-wide board (#701), reorder stays inside the card's own
        // chapter: a move past a chapter boundary would touch the manifest and
        // rewrite two files, which is deferred. Focus the chapter, then work in its
        // within-chapter scene indices.
        let Some((chapter, scene, manuscript)) = self.card_edit_target(index) else {
            return;
        };
        self.focus_chapter(chapter);
        let Some(other) = scene.checked_add_signed(dir) else {
            return;
        };
        let buffer = self.edit_buffer();
        let text = buffer.text();
        let scenes = scenes(&text, buffer.syntax().headings());
        if scene >= scenes.len() || other >= scenes.len() {
            // At a chapter boundary — the neighbour is in another chapter (or off
            // the end). Cross-chapter reorder is deferred (#701).
            if manuscript && !scenes.is_empty() {
                self.message = Some(
                    "Cross-chapter reorder isn't supported yet — reorder within a chapter.".into(),
                );
            }
            self.reopen_board_for(manuscript, chapter, scene);
            return;
        }
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

    /// Cycle the selected scene's status (stub → draft → done → stub) and persist
    /// it to the chapter's `scenes:` front matter (#636), then rebuild the board.
    fn cycle_scene_status(&mut self, index: usize) {
        let Some((chapter, scene, manuscript)) = self.card_edit_target(index) else {
            return;
        };
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

    /// Open a text prompt to edit the selected scene's one-line synopsis (#636),
    /// prefilled with the current value. Enter persists it; Esc leaves it. The
    /// target (chapter + within-chapter scene index) is resolved now and carried on
    /// the prompt, so the write lands on the right file after the mode leaves the
    /// board for the prompt (#701).
    fn prompt_scene_synopsis(&mut self, index: usize) {
        let Some((chapter, scene, manuscript)) = self.card_edit_target(index) else {
            return;
        };
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
        let buffer = self.edit_buffer();
        scenes(&buffer.text(), buffer.syntax().headings()).len()
    }

    /// The chapter's scene metadata, reconciled to exactly `count` entries so it
    /// aligns positionally with the scenes (#636 / #698): a shorter list pads with
    /// the default (blank synopsis, stub status), a longer one truncates. The one
    /// place that reads-then-normalizes before an edit.
    fn chapter_scenes(&self, count: usize) -> Vec<SceneMeta> {
        let mut scenes = self
            .edit_buffer()
            .front_matter()
            .map(super::super::frontmatter::FrontMatter::scenes)
            .unwrap_or_default();
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
    fn rewrite_chapter(&mut self, body: &str, scenes: &[SceneMeta]) {
        self.buffer.replace_text(body);
        self.buffer.refresh_syntax(); // headings must be current before rebuilding cards
        self.buffer.front_matter_mut().set_scenes(scenes);
        self.persist_active_chapter();
    }

    /// Save the active chapter to disk and refresh its cached metadata + word
    /// count, so a corkboard edit survives reopening the project and the chapter
    /// pane reflects it. Reports a save failure in the status message.
    fn persist_active_chapter(&mut self) {
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
        let selected = match chapter {
            Some(ch) => cards
                .iter()
                .enumerate()
                .filter(|(_, c)| c.chapter == Some(ch))
                .nth(scene)
                .map_or(cards.len() - 1, |(i, _)| i),
            None => scene.min(cards.len() - 1),
        };
        self.mode = Mode::Corkboard(Box::new(Corkboard {
            cards,
            selected,
            manuscript,
        }));
    }
}

/// Build one corkboard card per scene of a chapter body (#589 / #636 / #698 /
/// #701): scenes in document order, each with the scene heading's byte offset for
/// the jump and its metadata (synopsis + status) reconciled by position — a chapter
/// with no metadata (or fewer entries than scenes) still lists every scene, padded
/// with the default (blank synopsis, stub status). A scene owns its deeper sub-beats
/// and the `#` chapter title is not a card (#698). `chapter` / `chapter_title` tag
/// the cards for the manuscript-wide board (#701) and are `None` / empty on the
/// per-chapter board. Shared by both boards so scene identity is defined once.
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
    match text[at..].find('\n') {
        Some(nl) => at + nl + 1,
        None => text.len(),
    }
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
