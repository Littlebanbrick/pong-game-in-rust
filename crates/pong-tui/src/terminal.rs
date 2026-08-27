//! Terminal lifecycle: enter raw mode + the alternate screen, restore on exit.
//!
//! A panic hook guarantees the terminal is restored even if the frontend
//! panics while in raw mode — otherwise a crashed app would leave the
//! user's shell unusable.

use std::io::{self, Stdout};

use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal as RatatuiTerminal;
use ratatui::backend::CrosstermBackend;

pub type Terminal = RatatuiTerminal<CrosstermBackend<Stdout>>;

/// Holds the terminal in raw mode on the alternate screen while alive.
pub struct TerminalGuard {
    terminal: Terminal,
}

impl TerminalGuard {
    /// Sets up the terminal and installs the panic-restoring hook.
    pub fn new() -> io::Result<Self> {
        install_panic_hook();
        enable_raw_mode()?;
        let mut stdout = io::stdout();
        execute!(stdout, EnterAlternateScreen)?;
        let terminal = RatatuiTerminal::new(CrosstermBackend::new(stdout))?;
        Ok(Self { terminal })
    }

    /// Access to the ratatui terminal for drawing.
    pub fn terminal_mut(&mut self) -> &mut Terminal {
        &mut self.terminal
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = restore();
    }
}

/// Best-effort restoration of the shell's terminal state.
fn restore() -> io::Result<()> {
    disable_raw_mode()?;
    execute!(io::stdout(), crossterm::cursor::Show, LeaveAlternateScreen)?;
    Ok(())
}

fn install_panic_hook() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = restore();
        previous(info);
    }));
}
