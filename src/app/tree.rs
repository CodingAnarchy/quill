//! The series tree navigator (#330): the whole project as a series → book →
//! part → chapter tree, navigable with the keyboard, that jumps the editor to
//! any chapter (switching books as needed). Reached via the run-action menu
//! ("Project tree", Ctrl-K).
//!
//! The visible rows are derived from the project's cached metadata on demand —
//! never from chapter text — so the pane stays off the filesystem (the render
//! perf guard). A standalone novel is just a one-book tree.

use std::collections::HashSet;

use crossterm::event::{KeyCode, KeyEvent};

use crate::frontmatter::Band;
use crate::mode::{Mode, Tree};
use crate::project::{front_chapter_count, part_at_body_position, ChapterEntry};
use crate::report::thousands;

use super::App;

/// What pressing Enter on a row acts on.
pub(crate) enum TreeNode {
    /// The series root — no action.
    Series,
    /// A book: Enter toggles its expansion.
    Book(usize),
    /// A chapter (or a part header, which points at its first chapter): Enter
    /// jumps there. Carried by file identity so it survives a band-reorder when
    /// the book is activated.
    Chapter { book: usize, file: String },
}

/// One visible row of the tree, in display order.
pub(crate) struct TreeRow {
    /// Indentation depth (in tree levels).
    pub(crate) indent: u16,
    pub(crate) label: String,
    /// The active book / active chapter, accented in the pane.
    pub(crate) active: bool,
    pub(crate) node: TreeNode,
}

/// Book-chapter indices in band order (front → body → back), a stable sort so
/// already-ordered books (the active one) keep their order.
fn band_order(chapters: &[ChapterEntry]) -> Vec<usize> {
    let mut idx: Vec<usize> = (0..chapters.len()).collect();
    idx.sort_by_key(|&i| Band::of(chapters[i].role));
    idx
}

impl App {
    /// The visible tree rows for the current expansion state (#330). Cheap —
    /// reads cached per-chapter metadata, never files.
    pub(crate) fn tree_rows(&self, expanded: &HashSet<usize>) -> Vec<TreeRow> {
        let multi = self.project.book_count() > 1;
        let mut rows = Vec::new();
        if multi {
            rows.push(TreeRow {
                indent: 0,
                label: format!("▾ {}  ·  {} w", self.project.series_title().unwrap_or("Series"), thousands(self.series_word_count() as i64)),
                active: false,
                node: TreeNode::Series,
            });
        }
        let book_indent = u16::from(multi);
        for b in 0..self.project.book_count() {
            let is_expanded = expanded.contains(&b);
            let active_book = b == self.project.active_book();
            rows.push(TreeRow {
                indent: book_indent,
                label: format!(
                    "{} {}  ·  {} w",
                    if is_expanded { "▾" } else { "▸" },
                    self.project.book_title(b).unwrap_or("(untitled)"),
                    thousands(self.book_word_count(b) as i64),
                ),
                active: active_book,
                node: TreeNode::Book(b),
            });
            if !is_expanded {
                continue;
            }
            self.push_book_chapters(&mut rows, b, book_indent, active_book);
        }
        rows
    }

    /// Append book `b`'s chapters (in band order, with part headers) to `rows`.
    fn push_book_chapters(&self, rows: &mut Vec<TreeRow>, b: usize, book_indent: u16, active_book: bool) {
        let chapters = self.project.book_chapters(b);
        let parts = self.project.book_parts(b);
        let order = band_order(chapters);
        let front = front_chapter_count(chapters);
        // Word count for the chapter at index `ci`: the active book is counted
        // live (its active chapter from the buffer), the others from the cache.
        let words_at = |ci: usize| {
            if active_book {
                self.chapter_word_count(ci)
            } else {
                chapters[ci].words
            }
        };
        // Per-part totals for this book's part headers.
        let part_words: Vec<usize> = parts.map_or_else(Vec::new, |ps| {
            let mut totals = vec![0usize; ps.len()];
            for (pos, &ci) in order.iter().enumerate() {
                if Band::of(chapters[ci].role) == Band::Body {
                    if let Some(pi) = part_at_body_position(ps, pos.saturating_sub(front) + 1) {
                        totals[pi] += words_at(ci);
                    }
                }
            }
            totals
        });
        let mut last_part: Option<usize> = None;
        for (pos, &ci) in order.iter().enumerate() {
            let ch = &chapters[ci];
            let body = Band::of(ch.role) == Band::Body;
            // Emit a part header when the body chapter's part changes.
            let part = parts
                .filter(|_| body)
                .and_then(|ps| part_at_body_position(ps, pos.saturating_sub(front) + 1));
            if body && part != last_part {
                last_part = part;
                if let Some(pi) = part {
                    // `part` is `Some` only when `parts` is `Some` (from `and_then`),
                    // and `pi` is an in-bounds index from `part_at_body_position`.
                    let name = parts.expect("a part index implies a parts table")[pi]
                        .name
                        .clone()
                        .unwrap_or_else(|| format!("{} {}", self.project.book_part_label(b), pi + 1));
                    rows.push(TreeRow {
                        indent: book_indent + 1,
                        label: format!("{name}  ·  {} w", thousands(part_words[pi] as i64)),
                        active: false,
                        node: TreeNode::Chapter { book: b, file: ch.file.clone() },
                    });
                }
            } else if !body {
                last_part = None;
            }
            let chap_indent = book_indent + 1 + u16::from(part.is_some());
            rows.push(TreeRow {
                indent: chap_indent,
                label: format!("{}  ·  {} w", ch.title, thousands(words_at(ci) as i64)),
                active: active_book && self.project.active_file() == ch.file,
                node: TreeNode::Chapter { book: b, file: ch.file.clone() },
            });
        }
    }

    /// Open the series tree navigator (#330): the active book expanded, the
    /// selection on the active chapter.
    pub(crate) fn open_tree(&mut self) {
        let active_book = self.project.active_book();
        let expanded = HashSet::from([active_book]);
        let rows = self.tree_rows(&expanded);
        let active_file = self.project.active_file();
        let selected = rows
            .iter()
            .position(|r| matches!(&r.node, TreeNode::Chapter { book, file } if *book == active_book && file == active_file))
            .or_else(|| rows.iter().position(|r| matches!(r.node, TreeNode::Book(b) if b == active_book)))
            .unwrap_or(0);
        self.mode = Mode::Tree(Box::new(Tree { selected, expanded }));
    }

    pub(crate) fn on_key_tree(&mut self, key: KeyEvent) {
        let Mode::Tree(tree) = &self.mode else { return };
        let expanded = tree.expanded.clone();
        let rows = self.tree_rows(&expanded);
        let sel = tree.selected.min(rows.len().saturating_sub(1));
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.set_tree_selected(sel.saturating_sub(1)),
            KeyCode::Down | KeyCode::Char('j') => self.set_tree_selected((sel + 1).min(rows.len().saturating_sub(1))),
            // Expand a book (Right), or collapse the selected node's book (Left).
            KeyCode::Right | KeyCode::Char('l') => {
                if let Some(TreeRow { node: TreeNode::Book(b), .. }) = rows.get(sel) {
                    self.set_book_expanded(*b, true);
                }
            }
            KeyCode::Left | KeyCode::Char('h') => self.collapse_selected_book(&rows, sel),
            KeyCode::Enter => match rows.get(sel).map(|r| &r.node) {
                Some(TreeNode::Book(b)) => self.toggle_book(*b),
                Some(TreeNode::Chapter { book, file }) => {
                    let (book, file) = (*book, file.clone());
                    self.jump_tree(book, &file);
                }
                Some(TreeNode::Series) | None => {}
            },
            KeyCode::Esc => self.mode = self.editing_mode(),
            _ => {}
        }
    }

    fn set_tree_selected(&mut self, index: usize) {
        if let Mode::Tree(tree) = &mut self.mode {
            tree.selected = index;
        }
    }

    fn set_book_expanded(&mut self, book: usize, expanded: bool) {
        if let Mode::Tree(tree) = &mut self.mode {
            if expanded {
                tree.expanded.insert(book);
            } else {
                tree.expanded.remove(&book);
            }
        }
    }

    fn toggle_book(&mut self, book: usize) {
        if let Mode::Tree(tree) = &mut self.mode {
            if !tree.expanded.remove(&book) {
                tree.expanded.insert(book);
            }
        }
    }

    /// Collapse the book that owns the selected row and move the selection to
    /// that book's row.
    fn collapse_selected_book(&mut self, rows: &[TreeRow], sel: usize) {
        let book = match rows.get(sel).map(|r| &r.node) {
            Some(TreeNode::Book(b)) | Some(TreeNode::Chapter { book: b, .. }) => *b,
            _ => return,
        };
        self.set_book_expanded(book, false);
        // Re-find the book's row in the now-collapsed tree and select it.
        if let Mode::Tree(tree) = &self.mode {
            let expanded = tree.expanded.clone();
            if let Some(i) = self.tree_rows(&expanded).iter().position(|r| matches!(r.node, TreeNode::Book(b) if b == book)) {
                self.set_tree_selected(i);
            }
        }
    }

    /// Jump to a chapter from the tree: switch books first when it belongs to
    /// another, then open it (by file identity, so a band-reorder on activation
    /// can't misaddress it).
    fn jump_tree(&mut self, book: usize, file: &str) {
        if book != self.project.active_book() {
            self.switch_book(book);
        }
        if let Some(i) = self.project.index_of_file(file) {
            self.activate_chapter(i);
        }
        self.mode = self.editing_mode();
    }
}
