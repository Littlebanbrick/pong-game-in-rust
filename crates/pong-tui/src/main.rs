//! Terminal frontend for the Pong game.
//!
//! Rendering and keyboard input only; all game rules and physics live in
//! `pong-core`. See ARCHITECTURE.md for the message protocol between the
//! two crates.

mod input;
mod render;
mod terminal;

use std::io;
use std::time::{Duration, Instant};

use crossterm::event::{self, Event};
use pong_core::Backend;

use input::{Action, InputState};

/// How long the score flash effect lasts.
const SCORE_FLASH_MS: u128 = 600;

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
    let mut input_state = InputState::new();
    let mut score_flash_started: Option<Instant> = None;
    let mut last_score_total = snapshot.score.left + snapshot.score.right;

    loop {
        // Wait for at most one frame worth of time for the first event,
        // then drain everything queued behind it.
        if event::poll(Duration::from_millis(16))? {
            loop {
                // The chain stops applying only when an action says Quit.
                if let Event::Key(key) = event::read()?
                    && let Some(action) = input_state.handle_key(key, Instant::now())
                    && !apply_action(action, backend)
                {
                    return Ok(());
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }

        // Held keys expire on silence, not on events: check every frame.
        for action in input_state.sweep(Instant::now()) {
            if !apply_action(action, backend) {
                return Ok(());
            }
        }

        // Swap in the freshest state the backend has produced.
        if let Some(newer) = backend.latest_snapshot() {
            let total = newer.score.left + newer.score.right;
            if total > last_score_total {
                score_flash_started = Some(Instant::now());
            }
            last_score_total = total;
            snapshot = newer;
        }

        let score_flash = score_flash_started.is_some_and(|started| {
            let elapsed = started.elapsed().as_millis();
            elapsed < SCORE_FLASH_MS && (elapsed / 120) % 2 == 0
        });

        guard
            .terminal_mut()
            .draw(|frame| render::draw(frame, &snapshot, score_flash))?;
    }
}

/// Applies one input action; returns false when the loop should stop.
fn apply_action(action: Action, backend: &Backend) -> bool {
    match action {
        Action::Send(event) => {
            backend.send(event);
            true
        }
        Action::Quit => false,
    }
}
