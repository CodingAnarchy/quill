//! Quill — a terminal editor optimized for writing novels.
//!
//! Run `quill [DIR]` to open a project (or create one in an empty directory).
//! Defaults to the current directory.

mod app;
mod buffer;
mod project;
mod syntax;
mod ui;

use std::io::{self, Stdout};
use std::path::PathBuf;
use std::time::Duration;

use crossterm::{
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

    // `Tui` restores the terminal when dropped, so any return from `run` —
    // including a `?` error — leaves the terminal usable. (A panic under the
    // release `panic = "abort"` profile still won't run Drop; that's issue #35.)
    let mut tui = Tui::new()?;
    run(&mut tui, &mut app)
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

    fn draw(&mut self, app: &App) -> io::Result<()> {
        self.terminal.draw(|frame| ui::draw(frame, app))?;
        Ok(())
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            self.terminal.backend_mut(),
            LeaveAlternateScreen,
            DisableMouseCapture
        );
        let _ = self.terminal.show_cursor();
    }
}

fn run(tui: &mut Tui, app: &mut App) -> io::Result<()> {
    while !app.should_quit {
        tui.draw(app)?;

        // Poll so we can stay responsive without busy-spinning.
        if event::poll(Duration::from_millis(100))? {
            app.handle_event(event::read()?);
            // Drain the rest of the input burst before doing any heavier work,
            // so a fast typist (or a key-repeat) is handled as one batch.
            while event::poll(Duration::ZERO)? {
                app.handle_event(event::read()?);
            }
            // Input has gone idle: reparse once to refresh highlighting. Kept
            // off the per-keystroke path so typing stays instant.
            app.buffer.refresh_syntax();
        }
    }
    Ok(())
}
