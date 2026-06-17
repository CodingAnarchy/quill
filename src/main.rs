//! Quill — a terminal editor optimized for writing novels.
//!
//! Run `quill [FILE]` to open (or create) a manuscript file.

mod app;
mod buffer;
mod ui;

use std::io::{self, Stdout};
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

type Tui = Terminal<CrosstermBackend<Stdout>>;

fn main() -> io::Result<()> {
    let buffer = match std::env::args().nth(1) {
        Some(path) => Buffer::from_path(path)?,
        None => Buffer::empty(),
    };
    let mut app = App::new(buffer);

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
            if let Event::Key(key) = event::read()? {
                if key.kind == event::KeyEventKind::Press {
                    app.on_key(key);
                }
            }
        }
    }
    Ok(())
}
