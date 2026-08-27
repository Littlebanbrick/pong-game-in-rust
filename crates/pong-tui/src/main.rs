//! Terminal frontend for the Pong game.
//!
//! Rendering and keyboard input only; all game rules and physics live in
//! `pong-core`. See ARCHITECTURE.md for the message protocol between the
//! two crates.

mod input;
mod terminal;

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event};
use pong_core::Backend;
use ratatui::style::Style;
use ratatui::widgets::Paragraph;

use input::Action;

fn main() -> io::Result<()> {
    let mut guard = terminal::TerminalGuard::new()?;
    let backend = Backend::spawn();

    let result = run(&mut guard, &backend);

    // Restore the shell before anything else, then stop the backend.
    drop(guard);
    backend.join();
    result
}

fn run(guard: &mut terminal::TerminalGuard, backend: &Backend) -> io::Result<()> {
    loop {
        // Poll keys at ~30 Hz; a timeout keeps the loop responsive even
        // without input.
        if event::poll(Duration::from_millis(33))?
            && let Event::Key(key) = event::read()?
        {
            match input::map_key(key) {
                Action::Send(event) => backend.send(event),
                Action::Quit => return Ok(()),
                Action::Ignore => {}
            }
        }

        // Drain queued snapshots; real rendering arrives in the next step.
        let _ = backend.latest_snapshot();

        guard.terminal_mut().draw(|frame| {
            frame.render_widget(
                Paragraph::new(
                    "pong — rendering arrives in the next step\n\n\
                     left paddle: w/s    right paddle: Up/Down\n\n\
                     press q to quit",
                )
                .style(Style::new().cyan()),
                frame.area(),
            );
        })?;
    }
}
