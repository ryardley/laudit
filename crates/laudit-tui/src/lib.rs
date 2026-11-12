//! Interactive terminal tour for `laudit`.
//!
//! The TUI is a thin frontend over `laudit-core`: it loads the same tour
//! artifact as the Neovim plugin, renders the anchored code range, and streams
//! narration into an explanation pane. No RPC server is involved (D23).

pub mod app;
pub mod narration;
pub mod terminal;
pub mod ui;

use std::fmt;
use std::io::{self, IsTerminal};
use std::path::PathBuf;
use std::time::Duration;

use crossterm::event::{self, Event, KeyEventKind};

use crate::app::{load_session, App};
use crate::narration::NarrationManager;
use crate::terminal::{install_panic_hook, TerminalGuard, Tui};

/// Errors produced while running the terminal tour.
#[derive(Debug)]
pub enum TuiError {
    /// A human-readable error.
    Message(String),
    /// Terminal I/O failed.
    Io(io::Error),
}

impl TuiError {
    /// Build an error from anything that can be displayed.
    pub fn msg(error: impl fmt::Display) -> Self {
        Self::Message(error.to_string())
    }
}

impl fmt::Display for TuiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Message(message) => write!(f, "{message}"),
            Self::Io(error) => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for TuiError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Message(_) => None,
            Self::Io(error) => Some(error),
        }
    }
}

impl From<io::Error> for TuiError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Run the interactive tour against `repo_root`.
///
/// The terminal is restored on normal exit and on panic. Interactive mode
/// requires a TTY; callers that are not attached to one should use
/// `laudit tour --print` instead.
pub fn run(repo_root: PathBuf, offline: bool) -> Result<(), TuiError> {
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Err(TuiError::Message(
            "interactive mode requires a terminal; use `laudit tour --print`".to_owned(),
        ));
    }

    install_panic_hook();
    if !offline {
        match laudit_core::prompts::ensure_installed() {
            Ok(report) => {
                for warning in report.warnings {
                    eprintln!("laudit: warning: {warning}");
                }
            }
            Err(error) => eprintln!("laudit: warning: could not install prompts: {error}"),
        }
    }
    let session = load_session(repo_root, offline)?;
    let mut app = App::new(session);
    let mut narration = NarrationManager::new()?;
    app.request_current_narration(&mut narration);

    let mut guard = TerminalGuard::new()?;
    let result = event_loop(guard.terminal_mut(), &mut app, &mut narration);
    drop(guard);
    result
}

fn event_loop(
    terminal: &mut Tui,
    app: &mut App,
    narration: &mut NarrationManager,
) -> Result<(), TuiError> {
    loop {
        for event in narration.poll() {
            app.handle_narration_event(event);
        }
        if app.quit {
            break;
        }

        terminal.draw(|frame| ui::draw(frame, app))?;

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) if key.kind == KeyEventKind::Press => {
                    app.handle_key(key, narration);
                }
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
    Ok(())
}
