//! Quill — a terminal editor optimized for writing novels.
//!
//! Run `quill [DIR]` to open a project (or create one in an empty directory).
//! Defaults to the current directory.

mod ai;
mod analysis;
mod app;
mod buffer;
mod date;
mod export;
mod frontmatter;
mod history;
mod manuscript;
mod mode;
mod project;
mod report;
mod syntax;
mod text;
mod ui;
mod vcs;
mod wrap;

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use crossterm::{
    cursor::Show,
    event::{self, DisableMouseCapture, EnableMouseCapture},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use ratatui::{backend::CrosstermBackend, Terminal};

use crate::app::App;
use crate::buffer::Buffer;
use crate::project::Project;

fn main() -> io::Result<()> {
    // A project lives in a directory (default: the current one). A directory
    // with a manifest is opened; one without is initialized as a new project.
    let dir = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    let project = if Project::manifest_path(&dir).exists() {
        Project::open(&dir)?
    } else {
        Project::create(&dir)?
    };
    let buffer = Buffer::from_path(project.active_path())?;
    let mut app = App::new(buffer, project);
    // Resume today's word-delta counters from the history log.
    app.load_history();

    // Drop restores the terminal on a normal or `?`-error exit, but the release
    // `panic = "abort"` profile never unwinds — so install a panic hook too.
    install_panic_hook();

    // `Tui` restores the terminal when dropped, so any return from `run` —
    // including a `?` error — leaves the terminal usable.
    let mut tui = Tui::new()?;
    let result = run(&mut tui, &mut app);
    // Record today's momentum so it survives the next reopen.
    app.persist_snapshot();
    result
}

/// Undo the terminal setup from [`Tui::new`]: leave raw mode and the alternate
/// screen, and re-show the cursor. Shared by the [`Tui`] drop guard and the
/// panic hook so the two teardown paths can't drift. Best-effort — errors are
/// ignored because there's nothing useful to do with them mid-teardown.
///
/// Hand-rolled rather than `ratatui::restore()`: that helper only disables raw
/// mode and leaves the alternate screen, so it would skip the mouse-capture and
/// cursor teardown that [`Tui::new`]'s `EnableMouseCapture` requires.
fn restore_terminal(w: &mut impl io::Write) {
    let _ = disable_raw_mode();
    let _ = execute!(w, LeaveAlternateScreen, DisableMouseCapture, Show);
}

/// Install a panic hook that restores the terminal before the default hook
/// prints the panic message, so a crash doesn't leave the shell in raw mode /
/// the alternate screen. Chains to the previous hook so the message still shows.
fn install_panic_hook() {
    let prev = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        restore_terminal(&mut io::stdout());
        prev(info);
    }));
}

/// Owns the terminal in raw / alternate-screen mode and restores it on drop.
struct Tui {
    terminal: Terminal<CrosstermBackend<Stdout>>,
}

impl Tui {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
        Ok(Self {
            terminal: Terminal::new(CrosstermBackend::new(stdout))?,
        })
    }

    fn draw(&mut self, app: &mut App) -> io::Result<()> {
        self.terminal.draw(|frame| {
            // Record the terminal height so the key handler can clamp overlay
            // scrolling to the content that actually exists.
            app.viewport_height = frame.area().height;
            ui::draw(frame, app);
        })?;
        Ok(())
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        restore_terminal(self.terminal.backend_mut());
    }
}

fn run(tui: &mut Tui, app: &mut App) -> io::Result<()> {
    while !app.should_quit {
        // Surface a finished AI request (runs on a worker thread) before drawing,
        // so its answer appears within the poll window even with no keypress.
        app.poll_ai();
        app.poll_vc(); // surface a finished push/pull (worker thread) before drawing
        tui.draw(app)?;

        // Poll so we can stay responsive without busy-spinning.
        if event::poll(Duration::from_millis(100))? {
            app.handle_event(event::read()?);
            // Drain the rest of the input burst before doing any heavier work,
            // so a fast typist (or a key-repeat) is handled as one batch.
            while event::poll(Duration::ZERO)? {
                app.handle_event(event::read()?);
            }
            // Input has gone idle: reparse for highlighting and tally the day's
            // word delta. Kept off the per-keystroke path so typing stays instant.
            app.on_idle();
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restore_terminal_leaves_alt_screen_and_shows_cursor() {
        // restore_terminal writes the teardown escapes to the given sink;
        // disable_raw_mode touches the (absent) tty and is harmlessly ignored.
        let mut out: Vec<u8> = Vec::new();
        restore_terminal(&mut out);
        let seq = String::from_utf8(out).unwrap();
        // The user-visible halves of teardown: back to the main screen, cursor
        // shown. If these regress, a crash leaves the shell unusable.
        assert!(seq.contains("\x1b[?1049l"), "should leave alt screen: {seq:?}");
        assert!(seq.contains("\x1b[?25h"), "should show the cursor: {seq:?}");
    }
}
