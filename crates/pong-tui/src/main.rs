//! Terminal frontend for the Pong game.
//!
//! Rendering and keyboard input only; all game rules and physics live in
//! `pong-core`. See ARCHITECTURE.md for the message protocol between the
//! two crates.

mod input;
mod render;
mod terminal;

use std::io;
use std::time::Duration;

use crossterm::event::{self, Event};
use pong_core::Backend;

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
    // Block for the very first snapshot so the first frame is a real one.
    let mut snapshot = backend.next_snapshot().map_err(io::Error::other)?;

    loop {
        // Wait for at most one frame worth of time for the first event,
        // then drain everything queued behind it.
        if event::poll(Duration::from_millis(16))? {
            loop {
                if let Event::Key(key) = event::read()? {
                    match input::map_key(key) {
                        Action::Send(event) => backend.send(event),
                        Action::Quit => return Ok(()),
                        Action::Ignore => {}
                    }
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }

        // Swap in the freshest state the backend has produced.
        if let Some(newer) = backend.latest_snapshot() {
            snapshot = newer;
        }

        guard
            .terminal_mut()
            .draw(|frame| render::draw(frame, &snapshot))?;
    }
}
