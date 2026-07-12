//! In-app series management (#648): the book switcher, converting a standalone
//! novel into a series, and the manage-books palette (add / remove / reorder).
//! Split out of `project_ops` so the create/open/switch lifecycle and the
//! project browser stay in their own files; child module of `project_ops`, so it
//! keeps access to `App`'s private fields.

use crate::buffer::Buffer;
use crate::mode::{Command, Mode, Palette, PaletteItem, Pending};

use super::super::App;

impl App {
    /// Open the book switcher for a multi-book series (#329): list the books with
    /// the active one marked, selecting one makes it active. A no-op (with a
    /// status message) for a standalone novel.
    pub(crate) fn open_book_switcher(&mut self) {
        let count = self.project.book_count();
        if count <= 1 {
            self.message = Some("This project has only one book".into());
            return;
        }
        let active = self.project.active_book();
        let items = (0..count)
            .map(|i| {
                let title = self
                    .project
                    .book_title(i)
                    .unwrap_or("(untitled)")
                    .to_string();
                let label = if i == active {
                    format!("{title}  ·  current")
                } else {
                    title
                };
                PaletteItem::new(
                    label,
                    format!("book {} of {count}", i + 1),
                    Command::SwitchBook(i),
                )
            })
            .collect();
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Switch book",
            items,
            selected: active,
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// Make book `index` the active one (#329): flush the outgoing book's edits,
    /// swap the project to the target book, reload the editor on its first
    /// chapter, and re-baseline the daily tally for the new book's structure.
    pub(crate) fn switch_book(&mut self, index: usize) {
        if index == self.project.active_book() || index >= self.project.book_count() {
            return;
        }
        // Refuse the switch while an AI pass is in flight (#870). A bible pass
        // records its digests under whatever book is active *when it lands*
        // (`record_pass_state` via `bible_source` / the merge set), so switching
        // books mid-pass would file Book 1's result under Book 2 and mis-scope the
        // next incremental pass's changed/removed set (the #645 hazard, re-opened).
        // The switch is refused rather than queued: the author re-switches once the
        // pass ends.
        if self.ai_task.is_busy() {
            self.message = Some(
                "An AI pass is running — wait for it to finish before switching books.".into(),
            );
            return;
        }
        // The continuous view windows one book; drop to single-chapter, saving it.
        self.leave_continuous();
        // Persist the chapter we're leaving before swapping books — never drop work.
        if self.flush_active().is_err() {
            return;
        }
        self.project.set_active_book(index);
        let title = self.project.book_title(index).unwrap_or("").to_string();
        if self.reload_active_book() {
            self.message = Some(format!("Book: {title}"));
        }
    }

    /// Reload the editor onto the active book's first chapter after the project's
    /// active book changed (#329/#648): open the buffer and re-baseline the daily
    /// tally for the new book's structure. Returns whether the reload landed (a
    /// buffer-open failure sets a message and returns false). Shared by switching
    /// books and the series mutators (add / remove a book).
    fn reload_active_book(&mut self) -> bool {
        match Buffer::from_path(self.project.active_path()) {
            Ok(buffer) => {
                self.buffer = buffer;
                // The new book has its own parts and chapters: re-baseline so its
                // existing words aren't counted as today's writing and the per-part
                // counters match its structure (#329).
                self.reset_tally_for_active_book();
                true
            }
            Err(e) => {
                self.message = Some(format!("Open failed: {e}"));
                false
            }
        }
    }

    /// Prompt for a series title to convert this standalone novel into a series
    /// (#648). The current book becomes book one; a blank title uses the book's own
    /// title as the series name. A no-op (with a message) if it's already a series.
    pub(crate) fn start_convert_to_series(&mut self) {
        if self.project.book_count() > 1 {
            self.message = Some("This project is already a series.".into());
            return;
        }
        let prefill = self.project.manifest.series.clone().unwrap_or_default();
        self.open_prompt(
            Pending::ConvertToSeries,
            "Series title (blank uses the book title): ",
            &prefill,
            None,
        );
    }

    /// Convert the standalone novel into a series titled `series_title` (#648),
    /// then offer to add a second book straight away — turning one book into a
    /// series is only useful alongside another. A blank title falls back to the
    /// current book's title.
    pub(crate) fn convert_to_series(&mut self, series_title: &str) {
        let title = series_title.trim();
        let title = if title.is_empty() {
            self.project.manifest.title.clone()
        } else {
            title.to_string()
        };
        // Series design (`[style]`/`[print]`) is intentionally managed per-standalone
        // (#1105), so the series path doesn't persist it. If the standalone had either
        // set, warn the author it won't ride into the series rather than silently
        // dropping it — captured before the convert, surfaced after (#1141).
        let dropped_design =
            self.project.manifest.style.is_some() || self.project.manifest.print.is_some();
        match self.project.convert_to_series(&title) {
            Ok(()) => {
                if dropped_design {
                    self.message = Some(format!(
                        "Started series \u{201c}{title}\u{201d}. Note: series-level style/print \
                         design isn\u{2019}t carried over \u{2014} it\u{2019}s managed per book."
                    ));
                } else {
                    self.message = Some(format!("Started series \u{201c}{title}\u{201d}."));
                }
                // A one-book series is only useful with a second book; chain to it.
                self.prompt_add_book();
            }
            Err(e) => {
                self.message = Some(format!("Couldn't start series: {e}"));
                self.mode = self.editing_mode();
            }
        }
    }

    /// Prompt for the title of a new book to add to the series (#648). A standalone
    /// novel isn't a series yet, so it routes to the convert-to-series flow first
    /// (which chains back here once the series exists). Note a freshly-converted
    /// one-book series *is* a series, so this gates on series membership, not the
    /// book count.
    pub(crate) fn prompt_add_book(&mut self) {
        if self.project.series_title().is_none() {
            self.start_convert_to_series();
            return;
        }
        let n = self.project.book_count() + 1;
        self.open_prompt(
            Pending::AddBook,
            "New book title: ",
            &format!("Book {n}"),
            None,
        );
    }

    /// Add a new book titled `title` to the series and switch to it (#648): flush
    /// the outgoing chapter, create the book with a starter chapter, and reload the
    /// editor on it. A blank title is rejected (re-prompts).
    pub(crate) fn add_book(&mut self, title: &str) {
        let title = title.trim();
        if title.is_empty() {
            self.message = Some("Enter a book title.".into());
            self.prompt_add_book();
            return;
        }
        // The continuous view windows one book; drop to single-chapter, saving it,
        // and persist the chapter we're leaving before swapping books.
        self.leave_continuous();
        if self.flush_active().is_err() {
            return;
        }
        match self.project.add_book(title) {
            Ok(_) => {
                if self.reload_active_book() {
                    self.message = Some(format!("Added book \u{201c}{title}\u{201d}."));
                }
                self.mode = self.editing_mode();
            }
            Err(e) => {
                self.message = Some(format!("Couldn't add book: {e}"));
                self.mode = self.editing_mode();
            }
        }
    }

    /// Open the book-management palette (#648): each book with a remove and a
    /// move-up / move-down entry, plus "Add a book…". The single place an author
    /// reorders or removes books. A no-op (with a message) for a standalone novel —
    /// there's nothing to manage until it's a series (use "Add a book…" to start).
    pub(crate) fn open_manage_books(&mut self) {
        if self.project.book_count() <= 1 {
            // Nothing to reorder/remove yet; offer to start a series instead.
            self.prompt_add_book();
            return;
        }
        let count = self.project.book_count();
        let active = self.project.active_book();
        let mut items = Vec::new();
        for i in 0..count {
            let title = self
                .project
                .book_title(i)
                .unwrap_or("(untitled)")
                .to_string();
            let marker = if i == active { "  ·  current" } else { "" };
            items.push(PaletteItem::new(
                format!("{title}{marker}"),
                format!("switch to book {}", i + 1),
                Command::SwitchBook(i),
            ));
            if i > 0 {
                items.push(PaletteItem::new(
                    format!("  ↑ move \u{201c}{title}\u{201d} up"),
                    "reorder earlier in the series".to_string(),
                    Command::MoveBook { index: i, up: true },
                ));
            }
            if i + 1 < count {
                items.push(PaletteItem::new(
                    format!("  ↓ move \u{201c}{title}\u{201d} down"),
                    "reorder later in the series".to_string(),
                    Command::MoveBook {
                        index: i,
                        up: false,
                    },
                ));
            }
            items.push(PaletteItem::new(
                format!("  ✕ remove \u{201c}{title}\u{201d}"),
                "retire its chapters to unused/ (confirmed)".to_string(),
                Command::RemoveBook(i),
            ));
        }
        items.push(PaletteItem::new(
            "Add a book…".to_string(),
            "append a new book to the series".to_string(),
            Command::AddBook,
        ));
        self.mode = Mode::Palette(Box::new(Palette {
            title: "Manage books",
            items,
            selected: 0,
            collapsed: std::collections::HashSet::default(),
        }));
    }

    /// Confirm removing book `index` before touching any files (#648): its chapters
    /// are retired to `unused/`, not deleted, but it's still a structural change, so
    /// it takes an explicit y/N. Guarded — a series always keeps at least one book.
    pub(crate) fn confirm_remove_book(&mut self, index: usize) {
        if self.project.book_count() <= 1 {
            self.message = Some("A series must keep at least one book.".into());
            return;
        }
        let title = self.project.book_title(index).unwrap_or("this book");
        self.open_prompt(
            Pending::ConfirmRemoveBook { index },
            format!("Remove book \u{201c}{title}\u{201d}? Its chapters move to unused/ (y/N)"),
            "",
            None,
        );
    }

    /// Remove book `index` from the series (#648): retire its chapter files, and —
    /// if the active book changed — reload the editor onto the new active book.
    /// Flushes the outgoing chapter first so no work is lost.
    pub(crate) fn remove_book(&mut self, index: usize) {
        self.leave_continuous();
        if self.flush_active().is_err() {
            return;
        }
        match self.project.remove_book(index) {
            Ok(active_changed) => {
                if !active_changed || self.reload_active_book() {
                    self.message = Some("Book removed — its chapters are in unused/.".into());
                }
            }
            Err(e) => self.message = Some(format!("Couldn't remove book: {e}")),
        }
        self.mode = self.editing_mode();
    }

    /// Move book `index` up or down in the series order (#648), then re-open the
    /// manage-books palette so the new order is visible and more moves are one
    /// keystroke away. Reordering never moves files or changes the active book.
    pub(crate) fn move_book(&mut self, index: usize, up: bool) {
        match self.project.move_book(index, up) {
            Ok(true) => self.message = Some("Reordered books.".into()),
            Ok(false) => {}
            Err(e) => self.message = Some(format!("Couldn't reorder books: {e}")),
        }
        self.open_manage_books();
    }
}
