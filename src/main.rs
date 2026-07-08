//! Quill — a terminal editor optimized for writing novels.
//!
//! Run `quill [PATH]` to open a project (or create one in an empty directory).
//! A bare `quill` resolves a project via the startup chooser; `quill --help`
//! and `quill --version` print usage / the version and exit.

mod ai;
mod analysis;
mod annotation;
mod app;
mod archive;
mod atomic;
mod buffer;
mod clipboard;
mod config;
#[cfg(test)]
mod datapath_tests;
mod date;
mod export;
mod fonts;
mod frontmatter;
mod guide;
mod history;
mod history_view;
mod lastpos;
mod manuscript;
mod manuscript_import;
mod mode;
mod onboarding;
mod project;
mod recent;
mod recovery;
mod report;
mod spell;
mod startup;
mod syntax;
#[cfg(test)]
mod testutil;
mod text;
mod theme;
mod typography;
mod ui;
mod vcs;
mod workflow;
mod wrap;

use std::io::{self, Stdout, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossterm::{
    cursor::Show,
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        KeyboardEnhancementFlags, PopKeyboardEnhancementFlags, PushKeyboardEnhancementFlags,
    },
    execute,
    terminal::{
        disable_raw_mode, enable_raw_mode, supports_keyboard_enhancement, EnterAlternateScreen,
        LeaveAlternateScreen,
    },
};
use ratatui::{backend::CrosstermBackend, Terminal};

use crate::app::App;
use crate::buffer::Buffer;
use crate::project::Project;

/// Whether the enhanced keyboard protocol was pushed at startup (#348), so
/// teardown pops it only when we pushed it. Shared by the `Tui` drop guard and
/// the panic hook (which has no per-instance state) via this process-global.
static KEYBOARD_ENHANCED: AtomicBool = AtomicBool::new(false);

/// The `quill <version>` banner printed for `--version`/`-V` (#516). The version
/// is the crate version from `Cargo.toml`, baked in at compile time.
const VERSION_LINE: &str = concat!("quill ", env!("CARGO_PKG_VERSION"));

/// What the first command-line argument asks Quill to do. Decided by [`parse_arg`]
/// before any filesystem work so a flag-style argument (`--help`, `--version`) is
/// never mistaken for a project directory and scaffolded into a junk folder
/// (#517/#516).
#[derive(Debug, PartialEq, Eq)]
enum Invocation {
    /// `quill --help` / `quill -h`: print usage and exit (#517).
    Help,
    /// `quill --version` / `quill -V`: print the version and exit (#516).
    Version,
    /// `quill <path>`: open or create a project at this directory.
    Path(PathBuf),
    /// A bare `quill`: resolve a project via the startup chooser.
    NoArg,
}

/// Classify the first command-line argument into an [`Invocation`]. Pure (no
/// filesystem or environment access) so the flag-vs-path decision is unit-testable
/// and so a help/version query does zero filesystem work (#517/#516): the flag
/// checks come first, then anything else is taken as a project path.
fn parse_arg(arg: Option<&str>) -> Invocation {
    match arg {
        Some("--help" | "-h") => Invocation::Help,
        Some("--version" | "-V") => Invocation::Version,
        Some(path) => Invocation::Path(PathBuf::from(path)),
        None => Invocation::NoArg,
    }
}

/// The usage summary printed for `quill --help`/`-h` (#517). Kept short and
/// accurate to current behavior: a bare launch resolves to a project in the
/// projects home (#423) via the startup chooser (#526), and an explicit path opens
/// or creates a project there. A pure function so the text is unit-testable.
fn usage_text() -> String {
    format!(
        "{VERSION_LINE}\n\
         A terminal editor for writing novels.\n\
         \n\
         Usage: quill [OPTIONS] [PATH]\n\
         \n\
         Arguments:\n  \
         [PATH]  Open or create a project at this directory\n\
         \n\
         Options:\n  \
         -h, --help     Print this help and exit\n  \
         -V, --version  Print version and exit\n\
         \n\
         With no PATH, Quill opens the current directory if it is a project,\n\
         otherwise it offers to open a recent project or create a new one in\n\
         your projects home."
    )
}

fn main() -> io::Result<()> {
    // Classify the first argument before any filesystem work (#517/#516), so a
    // flag-style argument prints and exits rather than being treated as a project
    // directory and scaffolding a junk folder named after the flag.
    let first_arg = std::env::args().nth(1);
    let dir = match parse_arg(first_arg.as_deref()) {
        Invocation::Help => {
            println!("{}", usage_text());
            return Ok(());
        }
        Invocation::Version => {
            println!("{VERSION_LINE}");
            return Ok(());
        }
        // An explicit path opens (or creates) a project there.
        Invocation::Path(path) => path,
        // A bare launch resolves to an existing project, one chosen from the
        // library, or a freshly named one in the projects home — never a stray
        // project in the current folder (#423/#526).
        Invocation::NoArg => match startup_dir() {
            Ok(Some(dir)) => dir,
            // The author quit at the startup chooser without picking anything (#526).
            Ok(None) => return Ok(()),
            Err(e) => {
                eprintln!("quill: {e}");
                std::process::exit(1);
            }
        },
    };

    // A foreseeable data-path failure at startup — a malformed manifest, an
    // unreadable chapter, a directory we can't write — exits with a clear,
    // single-line message instead of a panic or Rust's `Error: Custom {…}`
    // debug dump (#264). The terminal isn't in raw mode yet, so stderr is clean.
    let (project, buffer) = match load_project(&dir) {
        Ok(loaded) => loaded,
        Err(e) => {
            eprintln!("quill: cannot open project at {}: {e}", dir.display());
            std::process::exit(1);
        }
    };
    // Remember this project so the in-app project browser (#270) can offer it.
    let (root, title) = (project.root.clone(), project.manifest.title.clone());
    let mut app = App::new(buffer, project);
    // Resume today's word-delta counters from the history log.
    app.load_history();
    // Resume where the last clean session left off — active chapter + cursor
    // (#588). Before offering recovery, so a crash sidecar still targets its own
    // chapter and isn't overridden by the resume.
    app.restore_position();
    // If a previous session left unsaved work behind (a crash), offer it back.
    app.offer_recovery();
    // On the very first launch, show the dismissible quick-start card (#837).
    // After recovery, so a crash prompt takes precedence and the card waits for a
    // clean slate (it's a no-op when a prompt/overlay is already up).
    app.maybe_offer_onboarding();
    recent::record(&root, &title);

    // Drop restores the terminal on a normal or `?`-error exit, but the release
    // `panic = "abort"` profile never unwinds — so install a panic hook too.
    install_panic_hook();

    // `Tui` restores the terminal when dropped, so any return from `run` —
    // including a `?` error — leaves the terminal usable.
    let mut tui = Tui::new()?;
    let result = run(&mut tui, &mut app);
    // Record today's momentum so it survives the next reopen.
    app.persist_snapshot();
    // Record this session's output to the per-session log (#835) — the calendar
    // heatmap's per-day breakdown reads it.
    app.record_session();
    // Remember where the author left off, to resume here next open (#588).
    app.persist_position();
    result
}

/// Open the project at `dir` — opening an existing one (a directory with a
/// manifest) or initializing a new one in an empty directory — and load its
/// active chapter. Every foreseeable failure (a malformed manifest, an
/// unreadable chapter, a directory that can't be created or written) comes back
/// as an `Err` carrying a clear message, never a panic (#264).
fn load_project(dir: &Path) -> io::Result<(Project, Buffer)> {
    let project = if Project::manifest_path(dir).exists() {
        Project::open(dir)?
    } else {
        Project::create(dir)?
    };
    let buffer = Buffer::from_path(project.active_path())?;
    Ok((project, buffer))
}

/// The project directory for a no-path launch (#423/#526), or `None` if the
/// author quit the startup chooser without picking anything.
///
/// If the current directory is already a Quill project, open it in place (the
/// long-standing behavior). Otherwise, when there are projects to open — recents
/// or a configured library — offer a chooser: create a new book or open an
/// existing one (#526). With nothing to open, the chooser would be a dead end, so
/// it degrades straight to the new-project prompt. Creating a new book asks the
/// author what to call it and places it in their projects home, never scattering a
/// project into whatever folder the shell happens to be in.
///
/// The freshly created project skips the in-app setup wizard (#282), matching
/// `quill <path>`; the wizard is offered only for projects created from within the
/// running editor (Ctrl-B → new).
fn startup_dir() -> io::Result<Option<PathBuf>> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let in_project = Project::manifest_path(&cwd).exists();
    let cfg = config::ProjectsConfig::load();
    let projects = recent::openable(cfg.home_dir().as_deref());
    // The reopen-last shortcut (#613) targets the most-recent project specifically —
    // the head of the recents list, not a library-only project — so it never
    // "reopens" a book the author has yet to open.
    let most_recent = recent::list().into_iter().next().map(|p| p.path);
    match startup::decide(
        &cwd,
        in_project,
        projects.len(),
        cfg.open_last_enabled(),
        most_recent.as_deref(),
    ) {
        startup::StartupAction::OpenInPlace(dir) => Ok(Some(dir)),
        startup::StartupAction::NewProject => Ok(Some(new_project_dir()?)),
        startup::StartupAction::Choose => choose_startup_project(&projects),
    }
}

/// Run the no-path chooser (#526) and turn the author's choice into a project
/// directory: an existing project's path, a freshly named new project, or `None`
/// when they quit. The chooser is a brief alternate-screen session (its own
/// [`Tui`], restored on drop) so it matches the in-editor browser; "create new"
/// then falls through to the plain-terminal name prompt, like before.
fn choose_startup_project(projects: &[recent::RecentProject]) -> io::Result<Option<PathBuf>> {
    let choice = {
        let mut tui = Tui::new()?;
        startup::choose(&mut tui.terminal, projects)?
        // `tui` drops here, restoring the plain terminal before any stdin prompt.
    };
    match choice {
        startup::Choice::Open(path) => Ok(Some(path)),
        startup::Choice::New => Ok(Some(new_project_dir()?)),
        startup::Choice::Quit => Ok(None),
    }
}

/// The directory for a brand-new project on a no-path launch (#423): the projects
/// home (with the `~/Quill` fallback, flagged so it isn't silent) joined with the
/// name the author types at the plain-terminal prompt. Shared by the
/// degrade-to-new path and the chooser's "create new".
fn new_project_dir() -> io::Result<PathBuf> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let (base, fallback) = new_project_base(
        config::ProjectsConfig::load().home_dir(),
        std::env::var_os("HOME").map(PathBuf::from),
        &cwd,
    );
    // Don't fall back to the current directory silently (#423): when there's no
    // projects home, say where the book is going and how to choose.
    if fallback {
        eprintln!(
            "quill: no projects home configured — set [projects] home in your config \
             to choose where new books live. Creating in {} for now.",
            base.display()
        );
    }
    let name = prompt_project_name()?;
    Ok(base.join(name))
}

/// Where a new project goes for a no-path launch (#423): the configured projects
/// home if set, else a `Quill` folder under the OS home, else the current
/// directory. The second return is whether this is the unconfigured fallback, so
/// the caller can surface the choice instead of placing the book silently.
fn new_project_base(
    projects_home: Option<PathBuf>,
    os_home: Option<PathBuf>,
    cwd: &Path,
) -> (PathBuf, bool) {
    projects_home.map_or_else(
        || {
            (
                os_home.map_or_else(|| cwd.to_path_buf(), |h| h.join("Quill")),
                true,
            )
        },
        |home| (home, false),
    )
}

/// Ask the author to name the new book (#423), on the plain terminal before the
/// alternate screen opens (like the startup error path). The folder is derived
/// from the name; `Project::create` takes the book title from that folder.
fn prompt_project_name() -> io::Result<String> {
    print!("New project name: ");
    io::stdout().flush()?;
    let mut input = String::new();
    io::stdin().read_line(&mut input)?;
    let name = crate::project::sanitize_dir_name(&input);
    if name.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "no project name entered",
        ));
    }
    Ok(name)
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
    // Pop the enhanced keyboard protocol first, but only if we pushed it (#348),
    // so a terminal that never had it enabled gets no stray escape sequence. The
    // `swap` makes this idempotent: whichever teardown path runs first (the drop
    // guard or the panic hook) pops, the other is a no-op.
    if KEYBOARD_ENHANCED.swap(false, Ordering::SeqCst) {
        let _ = execute!(w, PopKeyboardEnhancementFlags);
    }
    let _ = disable_raw_mode();
    let _ = execute!(
        w,
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableBracketedPaste,
        Show
    );
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
        // Bracketed paste (#811) so a terminal paste (Ctrl-Shift-V / Cmd-V /
        // middle-click) arrives as one `Event::Paste` with the OS clipboard's
        // text, inserted as a single undoable edit rather than replayed key by key.
        execute!(
            stdout,
            EnterAlternateScreen,
            EnableMouseCapture,
            EnableBracketedPaste
        )?;
        // Turn on the terminal's enhanced ("Kitty") keyboard protocol when it's
        // supported (#348), so combinations the legacy encoding can't tell apart
        // — Tab vs Ctrl-I, Enter vs Ctrl-M, and various Ctrl/Shift combos — arrive
        // as distinct key events. `DISAMBIGUATE_ESCAPE_CODES` is the high-value,
        // low-risk flag: it doesn't add key-release/repeat events, so the existing
        // press-only handling is unchanged. Terminals without support are left
        // exactly as before; the query and push are best-effort.
        if supports_keyboard_enhancement().unwrap_or(false)
            && execute!(
                stdout,
                PushKeyboardEnhancementFlags(KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES)
            )
            .is_ok()
        {
            KEYBOARD_ENHANCED.store(true, Ordering::SeqCst);
        }
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
        app.tick_sprint(); // end a writing sprint when its time is up (#299)
        app.tick_autosave(); // idle-timer autosave, when enabled (#409)
        app.tick_backup(); // automatic off-site backup on its cadence, when enabled (#887)
        app.poll_assistant(); // advance the assistant pane's tool-use loop (#307)
        app.poll_backup(); // surface a finished automatic backup (worker thread) (#887)
        app.poll_vc(); // surface a finished push/pull (worker thread) before drawing
        app.poll_model_download(); // surface a finished local-model download (#382)
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

    fn tmpdir(name: &str) -> PathBuf {
        crate::testutil::scratch_dir("main", name)
    }

    #[test]
    fn load_project_reports_a_malformed_manifest_instead_of_panicking() {
        let dir = tmpdir("bad-manifest");
        // A manifest that exists but is broken TOML: opened, not re-created.
        std::fs::write(Project::manifest_path(&dir), "title = \"X\"\n[[chapters]\n").unwrap();
        let err = load_project(&dir)
            .err()
            .expect("a malformed manifest should be an error");
        assert!(
            err.to_string().contains(crate::project::MANIFEST_NAME),
            "the message should name the manifest: {err}"
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_arg_classifies_flags_paths_and_no_arg() {
        // The help and version flags are recognized in both spellings (#517/#516)
        // and never treated as a path, so neither scaffolds a junk folder.
        assert_eq!(parse_arg(Some("--help")), Invocation::Help);
        assert_eq!(parse_arg(Some("-h")), Invocation::Help);
        assert_eq!(parse_arg(Some("--version")), Invocation::Version);
        assert_eq!(parse_arg(Some("-V")), Invocation::Version);
        // Anything else is a project path; a bare launch is the no-arg case.
        assert_eq!(
            parse_arg(Some("/tmp/book")),
            Invocation::Path(PathBuf::from("/tmp/book"))
        );
        assert_eq!(parse_arg(None), Invocation::NoArg);
        // Near-misses are paths, not flags — `-v` and a bare `help` aren't the flag.
        assert_eq!(parse_arg(Some("-v")), Invocation::Path(PathBuf::from("-v")));
        assert_eq!(
            parse_arg(Some("help")),
            Invocation::Path(PathBuf::from("help"))
        );
    }

    #[test]
    fn usage_text_describes_the_real_invocations() {
        // The help text leads with the version banner and covers the actual
        // invocations: a bare launch, an explicit path, and both flags (#517).
        let usage = usage_text();
        assert!(
            usage.starts_with(VERSION_LINE),
            "leads with the banner: {usage}"
        );
        assert!(usage.contains("Usage: quill [OPTIONS] [PATH]"));
        assert!(usage.contains("[PATH]"));
        assert!(usage.contains("--help"));
        assert!(usage.contains("--version"));
        assert!(
            usage.contains("projects home"),
            "explains the no-path behavior: {usage}"
        );
    }

    #[test]
    fn version_line_matches_the_crate_version() {
        // The banner is `quill <Cargo.toml version>`, baked in at compile time.
        assert_eq!(VERSION_LINE, format!("quill {}", env!("CARGO_PKG_VERSION")));
    }

    #[test]
    fn new_project_base_prefers_the_configured_projects_home() {
        // A configured projects home is used as-is, no fallback note (#423).
        let home = PathBuf::from("/books");
        let (base, fallback) = new_project_base(
            Some(home.clone()),
            Some(PathBuf::from("/home/me")),
            Path::new("/tmp/cwd"),
        );
        assert_eq!(base, home);
        assert!(!fallback);
    }

    #[test]
    fn new_project_base_falls_back_to_a_quill_folder_in_home() {
        // With no projects home, a `Quill` folder under the OS home — never the
        // current directory — and the fallback is flagged so it isn't silent.
        let (base, fallback) =
            new_project_base(None, Some(PathBuf::from("/home/me")), Path::new("/tmp/cwd"));
        assert_eq!(base, PathBuf::from("/home/me/Quill"));
        assert!(fallback);
    }

    #[test]
    fn new_project_base_last_resorts_to_the_current_directory() {
        // Only when there's no home at all does it land beside the shell.
        let (base, fallback) = new_project_base(None, None, Path::new("/tmp/cwd"));
        assert_eq!(base, PathBuf::from("/tmp/cwd"));
        assert!(fallback);
    }

    #[test]
    fn load_project_initializes_an_empty_directory() {
        let dir = tmpdir("fresh");
        let (project, _buffer) = load_project(&dir).expect("an empty dir becomes a new project");
        assert!(Project::manifest_path(&project.root).exists());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn restore_terminal_teardown_sequences() {
        // Both phases share the process-global `KEYBOARD_ENHANCED`, so they're
        // one sequential test (no cross-test race on the flag).

        // restore_terminal writes the teardown escapes to the given sink;
        // disable_raw_mode touches the (absent) tty and is harmlessly ignored.
        KEYBOARD_ENHANCED.store(false, Ordering::SeqCst); // not pushed this session
        let mut out: Vec<u8> = Vec::new();
        restore_terminal(&mut out);
        let seq = String::from_utf8(out).unwrap();
        // The user-visible halves of teardown: back to the main screen, cursor
        // shown. If these regress, a crash leaves the shell unusable.
        assert!(
            seq.contains("\x1b[?1049l"),
            "should leave alt screen: {seq:?}"
        );
        assert!(seq.contains("\x1b[?25h"), "should show the cursor: {seq:?}");
        // With the enhanced keyboard protocol never pushed, teardown must not emit
        // its pop sequence — no stray escape on a terminal that didn't enable it.
        assert!(
            !seq.contains("\x1b[<1u"),
            "no keyboard-enhancement pop when unpushed: {seq:?}"
        );

        // A session that enabled the enhanced protocol (#348) pops it on teardown.
        KEYBOARD_ENHANCED.store(true, Ordering::SeqCst);
        let mut out: Vec<u8> = Vec::new();
        restore_terminal(&mut out);
        assert!(
            String::from_utf8(out).unwrap().contains("\x1b[<1u"),
            "should pop the enhancement"
        );
        // The flag is cleared, so the second teardown path (panic hook after the
        // drop guard) is a no-op — no double-pop.
        assert!(
            !KEYBOARD_ENHANCED.load(Ordering::SeqCst),
            "flag cleared after pop"
        );
        let mut again: Vec<u8> = Vec::new();
        restore_terminal(&mut again);
        assert!(
            !String::from_utf8(again).unwrap().contains("\x1b[<1u"),
            "no double-pop"
        );
    }
}
