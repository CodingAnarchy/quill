//! The corkboard (#589 Phase 1 / #636 Phase 2): an index-card overview of the
//! active chapter's scenes, built from the `##`/`###` Markdown headings Quill
//! already parses (the same headings the Ctrl-O outline lists). Each card is a
//! scene heading plus a synopsis, and the view is navigable; Enter jumps the
//! editor cursor to that scene.
//!
//! Phase 2 turns it into an editable planning surface. Each card carries an
//! author-written one-line synopsis and a drafting status (stub / draft / done),
//! stored positionally in the chapter's `scenes:` YAML front matter so the prose
//! stays clean; the author edits the synopsis, cycles the status, and reorders
//! scenes (the heading and its prose move together) from the board. Every edit
//! rewrites the chapter through the buffer/save machinery and rebuilds the cards,
//! so the byte offsets stay a valid jump target and the metadata stays aligned
//! with the headings. It's off the keystroke path — the board is a separate mode,
//! and reorder does one whole-body rewrite per gesture, never per key.

use crossterm::event::{KeyCode, KeyEvent};

use super::App;
use crate::frontmatter::SceneMeta;
use crate::mode::{Card, Corkboard, Mode, Pending};

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
        self.mode = Mode::Corkboard(Box::new(Corkboard { cards, selected: 0 }));
    }

    /// Build the corkboard cards for the active buffer (#589 / #636): one per scene
    /// heading, in document order, each with the heading's byte offset for the jump
    /// and its scene metadata (synopsis + status) from the chapter's `scenes:`
    /// front matter, reconciled by position — a chapter with no metadata (or fewer
    /// entries than headings) still lists every heading as a card, padded with the
    /// default (blank synopsis, stub status). Reads the already-parsed headings and
    /// a single slice per scene — no re-parse.
    pub(crate) fn corkboard_cards(&self) -> Vec<Card> {
        let buffer = self.edit_buffer();
        let text = buffer.text();
        let headings = buffer.syntax().headings();
        let scenes = buffer
            .front_matter()
            .map(|fm| fm.scenes())
            .unwrap_or_default();
        headings
            .iter()
            .enumerate()
            .map(|(i, h)| {
                // The scene body runs from the end of this heading's line up to
                // the next heading (or the end of the chapter).
                let body_start = line_end(&text, h.start);
                let body_end = headings.get(i + 1).map(|n| n.start).unwrap_or(text.len());
                let prose_line =
                    first_prose_line(&text[body_start.min(text.len())..body_end.min(text.len())]);
                // Reconcile the metadata by position; a missing entry pads to the
                // default rather than dropping the card.
                let meta = scenes.get(i).cloned().unwrap_or_default();
                Card {
                    title: h.title.clone(),
                    level: h.level,
                    prose_line,
                    synopsis: meta.synopsis,
                    status: meta.status,
                    start: h.start,
                }
            })
            .collect()
    }

    /// Key handling for the corkboard (#589 / #636): move (arrows / j-k), jump the
    /// editor cursor to the selected scene (Enter), reorder the selected scene up
    /// (`K` / `[`) or down (`J` / `]`), edit its synopsis (`e`), cycle its status
    /// (`s` / Space), close (Esc).
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
            KeyCode::Enter => {
                let start = board.cards.get(selected).map(|c| c.start);
                self.mode = self.editing_mode();
                if let Some(start) = start {
                    let buf = self.edit_buffer_mut();
                    let at = start.min(buf.len_bytes());
                    buf.cursor = buf.cursor_at_byte(at);
                }
            }
            KeyCode::Char('K') | KeyCode::Char('[') => self.reorder_scene(selected, -1),
            KeyCode::Char('J') | KeyCode::Char(']') => self.reorder_scene(selected, 1),
            KeyCode::Char('e') => self.prompt_scene_synopsis(selected),
            KeyCode::Char('s') | KeyCode::Char(' ') => self.cycle_scene_status(selected),
            KeyCode::Esc => self.mode = self.editing_mode(),
            _ => {}
        }
    }

    /// Move the scene at card `index` one slot in `dir` (−1 up, +1 down): its
    /// heading and prose (the body block from its heading to the next) swap with
    /// the neighbouring block, and its `scenes:` metadata entry moves with it so
    /// the two stay aligned (#636). A no-op at the ends. Rewrites the whole body
    /// once (off the keystroke path), persists, and rebuilds the cards with the
    /// moved scene still selected.
    fn reorder_scene(&mut self, index: usize, dir: isize) {
        let Some(other) = index.checked_add_signed(dir) else {
            return;
        };
        let buffer = self.edit_buffer();
        let text = buffer.text();
        let headings = buffer.syntax().headings();
        if index >= headings.len() || other >= headings.len() {
            return; // at an end — nothing to swap with
        }
        // The two adjacent scene blocks, in body order (lower index first).
        let (a, b) = (index.min(other), index.max(other));
        let blocks = scene_blocks(&text, headings);
        let mut body = String::with_capacity(text.len());
        body.push_str(&text[..blocks[a].0]); // everything before the pair
        body.push_str(&text[blocks[b].0..blocks[b].1]); // the later block, first
        body.push_str(&text[blocks[a].0..blocks[a].1]); // then the earlier one
        body.push_str(&text[blocks[b].1..]); // everything after the pair

        // Move the metadata entry with its scene, padding to heading count so the
        // swap is positional even when the block is shorter (or absent).
        let mut scenes = self.chapter_scenes(headings.len());
        scenes.swap(a, b);

        self.rewrite_chapter(&body, &scenes);
        self.reopen_board(other);
    }

    /// Cycle the selected scene's status (stub → draft → done → stub) and persist
    /// it to the chapter's `scenes:` front matter (#636), then rebuild the board.
    fn cycle_scene_status(&mut self, index: usize) {
        let count = self.edit_buffer().syntax().headings().len();
        if index >= count {
            return;
        }
        let mut scenes = self.chapter_scenes(count);
        scenes[index].status = scenes[index].status.next();
        self.set_chapter_scenes(&scenes);
        self.reopen_board(index);
    }

    /// Open a text prompt to edit the selected scene's one-line synopsis (#636),
    /// prefilled with the current value. Enter persists it; Esc leaves it.
    fn prompt_scene_synopsis(&mut self, index: usize) {
        let prefill = self
            .edit_buffer()
            .front_matter()
            .map(|fm| fm.scenes())
            .unwrap_or_default()
            .get(index)
            .map(|s| s.synopsis.clone())
            .unwrap_or_default();
        self.open_prompt(
            Pending::SetSceneSynopsis { index },
            "Scene synopsis: ",
            &prefill,
            None,
        );
    }

    /// Persist a scene's edited synopsis (#636), from the
    /// [`Pending::SetSceneSynopsis`] prompt. A blank entry clears it. Rebuilds the
    /// board with the scene selected.
    pub(crate) fn apply_scene_synopsis(&mut self, index: usize, text: &str) {
        let count = self.edit_buffer().syntax().headings().len();
        if index < count {
            let mut scenes = self.chapter_scenes(count);
            scenes[index].synopsis = text.trim().to_string();
            self.set_chapter_scenes(&scenes);
        }
        self.reopen_board(index);
    }

    /// The chapter's scene metadata, reconciled to exactly `count` entries so it
    /// aligns positionally with the body headings (#636): a shorter list pads with
    /// the default (blank synopsis, stub status), a longer one truncates. The one
    /// place that reads-then-normalizes before an edit.
    fn chapter_scenes(&self, count: usize) -> Vec<SceneMeta> {
        let mut scenes = self
            .edit_buffer()
            .front_matter()
            .map(|fm| fm.scenes())
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

    /// Rebuild the corkboard after an edit and re-enter it with `selected`
    /// highlighted, clamped to the card count. The cards are re-derived from the
    /// current headings + metadata, so reorder / status / synopsis changes show at
    /// once. Returns to the editor if the chapter no longer has any scenes (it
    /// always should after an in-place edit).
    fn reopen_board(&mut self, selected: usize) {
        let cards = self.corkboard_cards();
        if cards.is_empty() {
            self.mode = self.editing_mode();
            return;
        }
        let selected = selected.min(cards.len() - 1);
        self.mode = Mode::Corkboard(Box::new(Corkboard { cards, selected }));
    }
}

/// The body byte ranges `[start, end)` of every scene block, in document order:
/// each heading's line through just before the next heading (the last runs to the
/// end of the chapter). The card list and the heading list share this order, so a
/// block's index is its card's index (#636).
fn scene_blocks(text: &str, headings: &[crate::syntax::Heading]) -> Vec<(usize, usize)> {
    headings
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let start = h.start;
            let end = headings.get(i + 1).map(|n| n.start).unwrap_or(text.len());
            (start, end)
        })
        .collect()
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
