//! Backend game logic for the terminal Pong game.
//!
//! Simulation (state, physics, rules) and its runtime (the authoritative
//! tick thread and its channels) live here. Neither performs I/O or knows
//! anything about terminals. The frontend crate (`pong-tui`) communicates
//! with the backend exclusively through the message protocol described in
//! ARCHITECTURE.md.

pub mod ai;
pub mod game;
pub mod protocol;
pub mod runtime;

pub use ai::AiController;
pub use game::{
    BALL_SIZE, BALL_SPEED, BALL_SPEED_SLOW, DT, Game, MUTABLE_GROWTH, PADDLE_ACCEL, PADDLE_HEIGHT,
    PADDLE_INSET, PADDLE_SPEED, PADDLE_TAP_SPEED, PADDLE_WIDTH, TICKS_PER_SEC, WIN_SCORE,
};
pub use protocol::{
    BallSpeedMode, Difficulty, Direction, FIELD_HEIGHT, FIELD_WIDTH, GameEvent, GameOptions,
    GamePhase, GameSnapshot, InputEvent, Opponent, Score, Side,
};
pub use runtime::{Backend, BackendClosed};
