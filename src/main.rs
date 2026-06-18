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
    event::{self, DisableMouseCapture, EnableMouseCapture, Event},
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
    },
};
use ratatui::{backend::CrosstermBackend, Terminal};

use crate::app::App;
use crate::buffer::Buffer;
use crate::project::Project;

type Tui = Terminal<CrosstermBackend<Stdout>>;

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

    let mut terminal = setup_terminal()?;
    let result = run(&mut terminal, &mut app);
    restore_terminal(&mut terminal)?;
    result
}

fn setup_terminal() -> io::Result<Tui> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    Terminal::new(CrosstermBackend::new(stdout))
}

fn restore_terminal(terminal: &mut Tui) -> io::Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()
}

fn run(terminal: &mut Tui, app: &mut App) -> io::Result<()> {
    while !app.should_quit {
        terminal.draw(|frame| ui::draw(frame, app))?;

        // Poll so we can stay responsive without busy-spinning.
        if event::poll(Duration::from_millis(100))? {
            handle_event(app, event::read()?);
            // Drain the rest of the input burst before doing any heavier work,
            // so a fast typist (or a key-repeat) is handled as one batch.
            while event::poll(Duration::ZERO)? {
                handle_event(app, event::read()?);
            }
            // Input has gone idle: reparse once to refresh highlighting. Kept
            // off the per-keystroke path so typing stays instant.
            app.buffer.refresh_syntax();
        }
    }
    Ok(())
}

fn handle_event(app: &mut App, ev: Event) {
    if let Event::Key(key) = ev {
        if key.kind == event::KeyEventKind::Press {
            app.on_key(key);
        }
    }
}
