//! Terminal frontend for the Pong game.
//!
//! Rendering and keyboard input only; all game rules and physics live in
//! `pong-core`. See ARCHITECTURE.md for the message protocol between the
//! two crates.

mod input;
mod menu;
mod render;
mod sound;
mod terminal;

use std::io::{self, Write};
use std::time::{Duration, Instant};

use crossterm::event::{self, Event};
use pong_core::{Backend, GameEvent, GamePhase, InputEvent, Opponent};

use input::{Action, InputState};
use menu::{Menu, MenuAction};

/// How long the score flash effect lasts.
const SCORE_FLASH_MS: u128 = 600;

fn main() -> io::Result<()> {
    // Audio first: a warning about a missing device must land on the
    // normal screen, before the alternate screen hides it.
    let sound = sound::SoundPlayer::new();
    if sound.is_none() {
        eprintln!("no audio device found: falling back to the terminal bell");
    }

    let mut guard = terminal::TerminalGuard::new()?;
    let backend = Backend::spawn();

    let result = run(&mut guard, &backend, sound.as_ref());

    // Restore the shell before anything else, then stop the backend.
    drop(guard);
    backend.join();
    result
}

/// Which screen the frontend is showing.
enum Mode {
    /// The start menu: pick an opponent and a ball speed mode.
    Menu(Menu),
    /// A match is running (or over — the game-over overlay belongs here).
    Match,
}

fn run(
    guard: &mut terminal::TerminalGuard,
    backend: &Backend,
    sound: Option<&sound::SoundPlayer>,
) -> io::Result<()> {
    // Block for the very first snapshot so the first frame is a real one.
    // A fresh backend is in the waiting phase; the menu leads from there.
    let mut snapshot = backend.next_snapshot().map_err(io::Error::other)?;
    let mut mode = Mode::Menu(Menu::new());
    let mut input_state = InputState::new();
    let mut score_flash_started: Option<Instant> = None;
    // No flash on the fresh match the menu is about to start.
    let mut last_score_total = 0u32;
    let mut ai_opponent = false;

    loop {
        // Wait for at most one frame worth of time for the first event,
        // then drain everything queued behind it.
        if event::poll(Duration::from_millis(16))? {
            loop {
                if let Event::Key(key) = event::read()? {
                    if matches!(mode, Mode::Match) {
                        // The chain stops applying only when an action
                        // says Quit.
                        if let Some(action) = input_state.handle_key(key, Instant::now())
                            && !apply_action(
                                action,
                                backend,
                                &mut mode,
                                &mut input_state,
                                &snapshot,
                            )
                        {
                            return Ok(());
                        }
                    } else if let Mode::Menu(menu) = &mut mode
                        && let Some(action) = menu.handle_key(key)
                    {
                        match action {
                            MenuAction::Start(event) => {
                                // Remap the keys first so the footer is
                                // right even before the first new snapshot.
                                if let InputEvent::StartMatch(options) = event {
                                    input_state.start_match(options.opponent);
                                    ai_opponent = matches!(options.opponent, Opponent::Ai(_));
                                }
                                backend.send(event);
                                last_score_total = 0;
                                score_flash_started = None;
                                mode = Mode::Match;
                            }
                            MenuAction::Quit => return Ok(()),
                        }
                    }
                }
                if !event::poll(Duration::ZERO)? {
                    break;
                }
            }
        }

        // Held keys expire on silence, not on events: check every frame.
        // (In menu mode nothing is held, so this sweeps nothing.)
        if matches!(mode, Mode::Match) {
            for action in input_state.sweep(Instant::now()) {
                if !apply_action(action, backend, &mut mode, &mut input_state, &snapshot) {
                    return Ok(());
                }
            }
        }

        // Swap in the freshest state the backend has produced. Also
        // happens in menu mode, so the backend's queue never grows.
        if let Some(newer) = backend.latest_snapshot() {
            let total = newer.score.left + newer.score.right;
            if total > last_score_total {
                score_flash_started = Some(Instant::now());
            }
            last_score_total = total;
            for event in &newer.events {
                play_sound(sound, *event);
            }
            snapshot = newer;
        }

        let score_flash = score_flash_started.is_some_and(|started| {
            let elapsed = started.elapsed().as_millis();
            elapsed < SCORE_FLASH_MS && (elapsed / 120) % 2 == 0
        });

        guard.terminal_mut().draw(|frame| match &mode {
            Mode::Menu(menu) => menu.draw(frame),
            Mode::Match => render::draw(frame, &snapshot, score_flash, ai_opponent),
        })?;
    }
}

/// Applies one input action; returns false when the loop should stop.
fn apply_action(
    action: Action,
    backend: &Backend,
    mode: &mut Mode,
    input_state: &mut InputState,
    snapshot: &pong_core::GameSnapshot,
) -> bool {
    match action {
        Action::Send(event) => {
            backend.send(event);
            true
        }
        // Back to the menu — but only from a finished match; the docs
        // promise M as an end-of-match key, not a rage-quit.
        Action::Menu => {
            if let GamePhase::GameOver { .. } = snapshot.phase {
                *mode = Mode::Menu(Menu::new());
                // Drop held-key bookkeeping so it cannot leak stale
                // directions into the next match.
                input_state.start_match(Opponent::TwoPlayer);
            }
            true
        }
        Action::Quit => false,
    }
}

/// Plays one game event; without an audio device, falls back to the
/// terminal bell (one pitch for everything, but better than silence).
fn play_sound(sound: Option<&sound::SoundPlayer>, event: GameEvent) {
    match sound {
        Some(player) => player.play(event),
        None => {
            let mut out = io::stdout();
            let _ = out.write_all(b"\x07");
            let _ = out.flush();
        }
    }
}
