//! Disposable playtest harness for `pong-core`.
//!
//! Drives the real game simulation with ASCII rendering and lets a human
//! control the left paddle (w/s). The right paddle chases the ball
//! automatically. This exists to validate the backend logic before the
//! ratatui frontend is built; it is not part of the shipped game.

use std::io::{self, Write};
use std::thread;
use std::time::Duration;

use crossterm::cursor;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use pong_core::{
    Direction, FIELD_HEIGHT, FIELD_WIDTH, Game, GamePhase, InputEvent, PADDLE_HEIGHT, PADDLE_INSET,
    Side,
};

/// Terminal view size, chosen for how it looks on screen, not in core
/// units. Character cells are roughly twice as tall as they are wide, so a
/// view with `VIEW_COLS / VIEW_ROWS == 2 * FIELD_WIDTH / FIELD_HEIGHT`
/// makes the field appear as a horizontal rectangle AND keeps 45° ball
/// trajectories looking like 45°.
const VIEW_COLS: usize = 80;
const VIEW_ROWS: usize = 24;
const X_SCALE: f32 = VIEW_COLS as f32 / FIELD_WIDTH;
const Y_SCALE: f32 = VIEW_ROWS as f32 / FIELD_HEIGHT;
const RENDER_EVERY_TICKS: u32 = 3;
const TICK_SLEEP: Duration = Duration::from_millis(15);

/// Holds the terminal in raw mode on the alternate screen while alive.
struct TerminalGuard;

impl TerminalGuard {
    fn new() -> io::Result<Self> {
        enable_raw_mode()?;
        execute!(io::stdout(), EnterAlternateScreen, cursor::Hide)?;
        Ok(TerminalGuard)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = execute!(io::stdout(), cursor::Show, LeaveAlternateScreen);
        let _ = disable_raw_mode();
    }
}

fn main() -> io::Result<()> {
    let _guard = TerminalGuard::new()?;
    let mut game = Game::new();
    let mut quit = false;
    let mut tick_index: u32 = 0;

    while !quit {
        // Drain all pending key events without blocking.
        while event::poll(Duration::ZERO)? {
            if let Event::Key(key) = event::read()? {
                handle_key(&mut game, &mut quit, key);
            }
        }

        // The right paddle chases the ball with a small deadband.
        let snapshot = game.snapshot();
        let chase = if snapshot.ball_y + 1.5 < snapshot.right_paddle_y {
            Some(Direction::Up)
        } else if snapshot.ball_y - 1.5 > snapshot.right_paddle_y {
            Some(Direction::Down)
        } else {
            None
        };
        game.handle_input(InputEvent::SetPaddleDirection {
            side: Side::Right,
            direction: chase,
            held: true,
        });

        game.tick();

        if tick_index.is_multiple_of(RENDER_EVERY_TICKS) {
            render(&game.snapshot())?;
        }
        tick_index += 1;
        thread::sleep(TICK_SLEEP);
    }
    Ok(())
}

fn handle_key(game: &mut Game, quit: &mut bool, key: event::KeyEvent) {
    // Plain terminals report key presses only (no release events), so
    // movement is edge-triggered: the last key pressed keeps steering the
    // paddle until the opposite key is pressed.
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return;
    }
    match key.code {
        // The harness keeps inputs simple: full speed, no repeat modeling.
        KeyCode::Char('w') => game.handle_input(InputEvent::SetPaddleDirection {
            side: Side::Left,
            direction: Some(Direction::Up),
            held: true,
        }),
        KeyCode::Char('s') => game.handle_input(InputEvent::SetPaddleDirection {
            side: Side::Left,
            direction: Some(Direction::Down),
            held: true,
        }),
        KeyCode::Char('r') => game.handle_input(InputEvent::Restart),
        KeyCode::Char('q') | KeyCode::Esc => *quit = true,
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => *quit = true,
        _ => {}
    }
}

fn draw_paddle(buf: &mut [Vec<char>], col: usize, y_center: f32) {
    let top = ((y_center - PADDLE_HEIGHT / 2.0) * Y_SCALE)
        .round()
        .max(0.0) as usize;
    let bottom = (((y_center + PADDLE_HEIGHT / 2.0) * Y_SCALE).round() as usize).min(VIEW_ROWS);
    for line in &mut buf[top..bottom] {
        line[col] = '┃';
    }
}

fn render(snapshot: &pong_core::GameSnapshot) -> io::Result<()> {
    let mut buf = vec![vec![' '; VIEW_COLS]; VIEW_ROWS];

    // Dashed net down the middle.
    for (row, line) in buf.iter_mut().enumerate() {
        line[VIEW_COLS / 2] = if row.is_multiple_of(2) { '┊' } else { ' ' };
    }

    draw_paddle(
        &mut buf,
        (PADDLE_INSET * X_SCALE) as usize,
        snapshot.left_paddle_y,
    );
    draw_paddle(
        &mut buf,
        ((FIELD_WIDTH - PADDLE_INSET) * X_SCALE) as usize,
        snapshot.right_paddle_y,
    );

    let ball_col = (snapshot.ball_x * X_SCALE).round() as isize;
    let ball_row = (snapshot.ball_y * Y_SCALE).round() as isize;
    if ball_col >= 0
        && ball_col < VIEW_COLS as isize
        && ball_row >= 0
        && ball_row < VIEW_ROWS as isize
    {
        buf[ball_row as usize][ball_col as usize] = '█';
    }

    let mut out = String::new();
    out.push_str("\x1b[H");
    out.push_str(&format!(
        " score {:>2} : {:<2}   left: w/s   right: auto   r restart   q quit\r\n",
        snapshot.score.left, snapshot.score.right
    ));
    out.push('┌');
    out.extend(std::iter::repeat_n('─', VIEW_COLS));
    out.push_str("┐\r\n");
    for row in &buf {
        out.push('│');
        out.extend(row.iter());
        out.push_str("│\r\n");
    }
    out.push('└');
    out.extend(std::iter::repeat_n('─', VIEW_COLS));
    out.push_str("┘\r\n");
    if let GamePhase::GameOver { winner } = snapshot.phase {
        let who = match winner {
            Side::Left => "LEFT",
            Side::Right => "RIGHT",
        };
        out.push_str(&format!(
            " GAME OVER — {} wins!   r to restart, q to quit\r\n",
            who
        ));
    }
    io::stdout().write_all(out.as_bytes())?;
    io::stdout().flush()
}
