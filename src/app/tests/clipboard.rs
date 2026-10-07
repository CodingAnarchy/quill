//! System-clipboard tests (#811). Every app under test runs the in-memory
//! [`FakeClipboard`](crate::clipboard::FakeClipboard) (installed by `open_app`),
//! so a copy places text on the fake and a paste reads it back — the whole
//! copy / paste seam is exercised without touching the real OS clipboard or
//! emitting a terminal escape (#531).

use super::*;
use crate::mode::{Finding, Findings, Mode};
use crossterm::event::Event;

/// A one-chapter app with Vim mode on, resting in Normal, so the yank / delete /
/// Visual copy paths are reachable. The fake clipboard is already installed.
fn vim_app(tag: &str, body: &str) -> (App, TempProject) {
    let dir = make_project(tag, "Book", body);
    let mut app = open_app(&dir);
    app.flags.vim = true;
    app.mode = Mode::Normal;
    (app, TempProject::new(dir))
}

/// Drive a bracketed-paste event with `text` — the terminal's OS-clipboard paste.
fn paste_event(app: &mut App, text: &str) {
    app.handle_event(Event::Paste(text.to_string()));
}

#[test]
fn a_vim_yank_copies_to_the_clipboard() {
    // `yy` yanks the line; the whole line (with its newline) lands on the clipboard.
    let (mut app, _dir) = vim_app("clip-yank", "hello world\n");
    app.on_key(key(KeyCode::Char('y')));
    app.on_key(key(KeyCode::Char('y')));
    assert_eq!(
        app.clipboard_contents().as_deref(),
        Some("hello world\n"),
        "a yank mirrors onto the system clipboard"
    );
}

#[test]
fn a_vim_delete_copies_to_the_clipboard() {
    // `dw` deletes a word; the deleted text goes to the clipboard, like the
    // internal register.
    let (mut app, _dir) = vim_app("clip-delete", "hello world\n");
    app.on_key(key(KeyCode::Char('d')));
    app.on_key(key(KeyCode::Char('w')));
    assert_eq!(
        app.clipboard_contents().as_deref(),
        Some("hello "),
        "a delete mirrors onto the system clipboard"
    );
}

#[test]
fn a_visual_selection_copies_to_the_clipboard() {
    // `v` + motions select, `y` copies the selection to the clipboard.
    let (mut app, _dir) = vim_app("clip-visual", "hello world\n");
    app.on_key(key(KeyCode::Char('v')));
    for _ in 0..4 {
        app.on_key(key(KeyCode::Char('l'))); // extend right over "hello"
    }
    app.on_key(key(KeyCode::Char('y')));
    assert_eq!(
        app.clipboard_contents().as_deref(),
        Some("hello"),
        "a Visual copy places the selection on the clipboard"
    );
}

#[test]
fn the_internal_registers_still_work_after_a_clipboard_copy() {
    // Copying to the clipboard doesn't disturb the internal register: `yy` then
    // `p` still puts the yanked line from the unnamed register.
    let (mut app, _dir) = vim_app("clip-registers", "one\ntwo\n");
    app.on_key(key(KeyCode::Char('y')));
    app.on_key(key(KeyCode::Char('y'))); // yank "one\n"
    app.on_key(key(KeyCode::Char('p'))); // put it below
    assert!(
        app.buffer.text().starts_with("one\none\n"),
        "internal register put still works: {:?}",
        app.buffer.text()
    );
}

#[test]
fn a_bracketed_paste_inserts_at_the_cursor() {
    // A terminal paste (the OS clipboard) inserts at the cursor in Insert mode.
    let dir = make_project("clip-bracketed", "Book", "start\n");
    let mut app = open_app(&dir);
    let _keep = TempProject::new(dir);
    // Cursor at the start of the modeless buffer.
    paste_event(&mut app, "PASTED ");
    assert!(
        app.buffer.text().starts_with("PASTED start"),
        "the pasted text landed at the cursor: {:?}",
        app.buffer.text()
    );
}

#[test]
fn a_paste_is_a_single_undo_group() {
    // A multi-line paste undoes in one step, not char by char.
    let dir = make_project("clip-undo", "Book", "abc\n");
    let mut app = open_app(&dir);
    let _keep = TempProject::new(dir);
    let before = app.buffer.text();
    paste_event(&mut app, "X\nY\nZ");
    assert_ne!(app.buffer.text(), before, "the paste changed the buffer");
    app.buffer.seal_undo_group();
    app.on_key(ctrl('z')); // undo
    assert_eq!(
        app.buffer.text(),
        before,
        "one undo reverts the whole paste"
    );
}

#[test]
fn clipboard_register_put_pastes_the_clipboard() {
    // `"+p` pastes the system clipboard (not an internal register).
    let (mut app, _dir) = vim_app("clip-plus-put", "hello\n");
    app.set_clipboard("PASTED");
    app.on_key(key(KeyCode::Char('"')));
    app.on_key(key(KeyCode::Char('+')));
    app.on_key(key(KeyCode::Char('p')));
    assert!(
        app.buffer.text().contains("PASTED"),
        "\"+p pasted the clipboard: {:?}",
        app.buffer.text()
    );
}

#[test]
fn a_paste_replaces_a_visual_selection() {
    // In Visual mode a bracketed paste replaces the selection, like `p` there.
    let (mut app, _dir) = vim_app("clip-visual-paste", "hello world\n");
    app.on_key(key(KeyCode::Char('v')));
    for _ in 0..4 {
        app.on_key(key(KeyCode::Char('l'))); // select "hello"
    }
    paste_event(&mut app, "HI");
    assert!(
        app.buffer.text().starts_with("HI world"),
        "the paste replaced the selection: {:?}",
        app.buffer.text()
    );
    assert!(
        !matches!(app.mode, Mode::Visual { .. }),
        "the paste left Visual mode"
    );
}

#[test]
fn a_paste_in_a_non_editing_mode_is_ignored() {
    // Bracketed paste only inserts in an editing mode; a modal list owns its input,
    // so a stray paste there doesn't corrupt the prose buffer.
    let dir = make_project("clip-nonediting", "Book", "text\n");
    let mut app = open_app(&dir);
    let _keep = TempProject::new(dir);
    let before = app.buffer.text();
    app.mode = Mode::Findings(Box::new(Findings {
        items: vec![Finding {
            summary: "a".into(),
            kind: crate::mode::FindingKind::Other,
            chapters: Vec::new(),
            book: None,
            action: None,
            corroborators: Vec::new(),
        }],
        selected: 0,
        filter: String::new(),
        typing: false,
    }));
    paste_event(&mut app, "STRAY");
    assert_eq!(
        app.buffer.text(),
        before,
        "a paste over a modal list leaves the prose buffer untouched"
    );
}
