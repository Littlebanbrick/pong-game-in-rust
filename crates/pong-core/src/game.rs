//! Game simulation: state, physics, and rules.
//!
//! [`Game`] is a pure state machine: advance it with [`Game::tick`], feed it
//! player intent with [`Game::handle_input`], and read a renderable copy of
//! its state with [`Game::snapshot`]. It performs no I/O and measures time
//! only in ticks, which keeps the simulation deterministic and testable.
//!
//! Phase-1 physics is deliberately minimal: the ball always travels along a
//! 45° diagonal (both velocity components have equal magnitude) and every
//! bounce simply flips one component's sign. Position changes by hits on the
//! paddle, hit-offset-dependent angles, and acceleration are left for later
//! phases.

use std::time::{SystemTime, UNIX_EPOCH};

use crate::protocol::{
    Direction, FIELD_HEIGHT, FIELD_WIDTH, GamePhase, GameSnapshot, InputEvent, Score, Side,
};

/// Simulation rate of the backend, in ticks per second.
pub const TICKS_PER_SEC: u32 = 60;

/// Duration of a single tick in seconds.
pub const DT: f32 = 1.0 / TICKS_PER_SEC as f32;

/// Points a player needs to win the match.
pub const WIN_SCORE: u32 = 11;

/// Paddle dimensions in field units.
pub const PADDLE_WIDTH: f32 = 2.0;
pub const PADDLE_HEIGHT: f32 = 10.0;

/// Horizontal distance from each side wall to the center of that wall's paddle.
pub const PADDLE_INSET: f32 = 5.0;

/// Ball diameter in field units.
pub const BALL_SIZE: f32 = 1.0;

/// Paddle speed in field units per second.
pub const PADDLE_SPEED: f32 = 50.0;

/// Ball speed (path length) in field units per second.
pub const BALL_SPEED: f32 = 80.0;

const PADDLE_HALF_W: f32 = PADDLE_WIDTH / 2.0;
const PADDLE_HALF_H: f32 = PADDLE_HEIGHT / 2.0;
const BALL_HALF: f32 = BALL_SIZE / 2.0;

/// Absolute value of each ball velocity component, per tick. The ball always
/// moves at exactly 45°, so this is the per-tick displacement on both axes.
const BALL_STEP: f32 = BALL_SPEED * std::f32::consts::FRAC_1_SQRT_2 * DT;

/// Ball position and velocity (center coordinates, field units).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Ball {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
}

/// Full game state. See the [module docs](self) for the intended usage.
#[derive(Debug)]
pub struct Game {
    phase: GamePhase,
    score: Score,
    left_paddle_y: f32,
    right_paddle_y: f32,
    left_direction: Option<Direction>,
    right_direction: Option<Direction>,
    ball: Ball,
    rng: Xorshift,
}

impl Game {
    /// Creates a game in the initial state with a randomly directed serve.
    pub fn new() -> Self {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock before UNIX epoch")
            .as_nanos() as u64;
        Self::initial(nanos)
    }

    /// Creates a game with a deterministic seed (used by `new` and tests).
    fn initial(seed: u64) -> Self {
        let mut game = Game {
            phase: GamePhase::Playing,
            score: Score::default(),
            left_paddle_y: FIELD_HEIGHT / 2.0,
            right_paddle_y: FIELD_HEIGHT / 2.0,
            left_direction: None,
            right_direction: None,
            ball: Ball {
                x: 0.0,
                y: 0.0,
                vx: 0.0,
                vy: 0.0,
            },
            rng: Xorshift::new(seed),
        };
        game.serve();
        game
    }

    /// Applies one input event from the frontend.
    pub fn handle_input(&mut self, event: InputEvent) {
        match event {
            InputEvent::SetPaddleDirection { side, direction } => match side {
                Side::Left => self.left_direction = direction,
                Side::Right => self.right_direction = direction,
            },
            InputEvent::Restart => {
                if self.phase != GamePhase::Playing {
                    self.reset();
                }
            }
            // Shutdown is handled by the runtime, not by the simulation.
            InputEvent::Shutdown => {}
        }
    }

    /// Advances the simulation by one tick.
    ///
    /// While the match is over, the whole simulation freezes until a
    /// [`InputEvent::Restart`] arrives.
    pub fn tick(&mut self) {
        if self.phase != GamePhase::Playing {
            return;
        }
        self.move_paddle(Side::Left);
        self.move_paddle(Side::Right);
        self.ball.x += self.ball.vx;
        self.ball.y += self.ball.vy;
        self.bounce_off_walls();
        self.bounce_off_paddle(Side::Left);
        self.bounce_off_paddle(Side::Right);
        self.score_if_ball_exited();
    }

    /// Returns a complete renderable copy of the current state.
    pub fn snapshot(&self) -> GameSnapshot {
        GameSnapshot {
            phase: self.phase,
            score: self.score,
            left_paddle_y: self.left_paddle_y,
            right_paddle_y: self.right_paddle_y,
            ball_x: self.ball.x,
            ball_y: self.ball.y,
        }
    }

    /// Resets to a fresh match and serves.
    fn reset(&mut self) {
        self.phase = GamePhase::Playing;
        self.score = Score::default();
        self.left_paddle_y = FIELD_HEIGHT / 2.0;
        self.right_paddle_y = FIELD_HEIGHT / 2.0;
        self.left_direction = None;
        self.right_direction = None;
        self.serve();
    }

    /// Places the ball at the center with a random 45° direction.
    fn serve(&mut self) {
        self.ball = Ball {
            x: FIELD_WIDTH / 2.0,
            y: FIELD_HEIGHT / 2.0,
            vx: if self.rng.next_bool() {
                BALL_STEP
            } else {
                -BALL_STEP
            },
            vy: if self.rng.next_bool() {
                BALL_STEP
            } else {
                -BALL_STEP
            },
        };
    }

    fn move_paddle(&mut self, side: Side) {
        let (y, direction) = match side {
            Side::Left => (&mut self.left_paddle_y, self.left_direction),
            Side::Right => (&mut self.right_paddle_y, self.right_direction),
        };
        let step = match direction {
            Some(Direction::Up) => -PADDLE_SPEED * DT,
            Some(Direction::Down) => PADDLE_SPEED * DT,
            None => return,
        };
        *y = (*y + step).clamp(PADDLE_HALF_H, FIELD_HEIGHT - PADDLE_HALF_H);
    }

    fn bounce_off_walls(&mut self) {
        if self.ball.y < BALL_HALF {
            self.ball.y = BALL_HALF;
            self.ball.vy = self.ball.vy.abs();
        } else if self.ball.y > FIELD_HEIGHT - BALL_HALF {
            self.ball.y = FIELD_HEIGHT - BALL_HALF;
            self.ball.vy = -self.ball.vy.abs();
        }
    }

    fn bounce_off_paddle(&mut self, side: Side) {
        let (paddle_x, paddle_y, moving_toward) = match side {
            Side::Left => (PADDLE_INSET, self.left_paddle_y, self.ball.vx < 0.0),
            Side::Right => (
                FIELD_WIDTH - PADDLE_INSET,
                self.right_paddle_y,
                self.ball.vx > 0.0,
            ),
        };
        if !moving_toward {
            return;
        }
        let overlaps_x = self.ball.x - BALL_HALF < paddle_x + PADDLE_HALF_W
            && self.ball.x + BALL_HALF > paddle_x - PADDLE_HALF_W;
        let overlaps_y = self.ball.y + BALL_HALF > paddle_y - PADDLE_HALF_H
            && self.ball.y - BALL_HALF < paddle_y + PADDLE_HALF_H;
        if !(overlaps_x && overlaps_y) {
            return;
        }
        // Send the ball back the way it came and push it out of the paddle,
        // so the overlap test cannot fire again on the next tick.
        self.ball.vx = -self.ball.vx;
        self.ball.x = match side {
            Side::Left => paddle_x + PADDLE_HALF_W + BALL_HALF,
            Side::Right => paddle_x - PADDLE_HALF_W - BALL_HALF,
        };
    }

    fn score_if_ball_exited(&mut self) {
        let scorer = if self.ball.x + BALL_HALF < 0.0 {
            Some(Side::Right)
        } else if self.ball.x - BALL_HALF > FIELD_WIDTH {
            Some(Side::Left)
        } else {
            None
        };
        if let Some(scorer) = scorer {
            self.score.record(scorer);
            if self.score.left >= WIN_SCORE || self.score.right >= WIN_SCORE {
                self.phase = GamePhase::GameOver { winner: scorer };
            } else {
                self.serve();
            }
        }
    }
}

impl Default for Game {
    fn default() -> Self {
        Self::new()
    }
}

/// Tiny xorshift generator — just enough randomness to pick serve
/// directions without adding a dependency.
#[derive(Debug)]
struct Xorshift(u64);

impl Xorshift {
    fn new(seed: u64) -> Self {
        // `| 1` guarantees a non-zero state (a zero state would never change).
        Xorshift((seed ^ 0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }

    fn next_bool(&mut self) -> bool {
        self.next_u64() & 1 == 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CENTER_Y: f32 = FIELD_HEIGHT / 2.0;
    const LEFT_PADDLE_X: f32 = PADDLE_INSET;
    const RIGHT_PADDLE_X: f32 = FIELD_WIDTH - PADDLE_INSET;
    const LEFT_EDGE: f32 = LEFT_PADDLE_X + PADDLE_HALF_W + BALL_HALF;
    const RIGHT_EDGE: f32 = RIGHT_PADDLE_X - PADDLE_HALF_W - BALL_HALF;

    /// A game whose ball is replaced with a deterministic one.
    fn game_with_ball(x: f32, y: f32, vx: f32, vy: f32) -> Game {
        let mut game = Game::initial(42);
        game.ball = Ball { x, y, vx, vy };
        game
    }

    #[test]
    fn idle_paddle_does_not_move() {
        let mut game = Game::initial(42);
        let before = game.left_paddle_y;
        game.tick();
        assert_eq!(game.left_paddle_y, before);
    }

    #[test]
    fn paddle_moves_one_step_per_tick() {
        let mut game = Game::initial(42);
        game.handle_input(InputEvent::SetPaddleDirection {
            side: Side::Left,
            direction: Some(Direction::Up),
        });
        let before = game.left_paddle_y;
        game.tick();
        let expected = before - PADDLE_SPEED * DT;
        assert!((game.left_paddle_y - expected).abs() < 1e-6);
    }

    #[test]
    fn paddle_stops_when_direction_cleared() {
        let mut game = Game::initial(42);
        game.handle_input(InputEvent::SetPaddleDirection {
            side: Side::Right,
            direction: Some(Direction::Down),
        });
        game.tick();
        game.handle_input(InputEvent::SetPaddleDirection {
            side: Side::Right,
            direction: None,
        });
        let before = game.right_paddle_y;
        game.tick();
        assert_eq!(game.right_paddle_y, before);
    }

    #[test]
    fn paddle_clamps_at_both_walls() {
        let mut game = Game::initial(42);
        game.handle_input(InputEvent::SetPaddleDirection {
            side: Side::Left,
            direction: Some(Direction::Up),
        });
        for _ in 0..TICKS_PER_SEC {
            game.tick();
        }
        assert_eq!(game.left_paddle_y, PADDLE_HALF_H);

        game.handle_input(InputEvent::SetPaddleDirection {
            side: Side::Left,
            direction: Some(Direction::Down),
        });
        for _ in 0..2 * TICKS_PER_SEC {
            game.tick();
        }
        assert_eq!(game.left_paddle_y, FIELD_HEIGHT - PADDLE_HALF_H);
    }

    #[test]
    fn ball_bounces_off_top_wall() {
        let mut game = game_with_ball(FIELD_WIDTH / 2.0, BALL_HALF + 0.1, BALL_STEP, -BALL_STEP);
        game.tick();
        assert_eq!(game.ball.y, BALL_HALF);
        assert!(game.ball.vy > 0.0);
    }

    #[test]
    fn ball_bounces_off_bottom_wall() {
        let mut game = game_with_ball(
            FIELD_WIDTH / 2.0,
            FIELD_HEIGHT - BALL_HALF - 0.1,
            BALL_STEP,
            BALL_STEP,
        );
        game.tick();
        assert_eq!(game.ball.y, FIELD_HEIGHT - BALL_HALF);
        assert!(game.ball.vy < 0.0);
    }

    #[test]
    fn ball_bounces_off_left_paddle() {
        let mut game = game_with_ball(LEFT_EDGE + 0.5, CENTER_Y, -BALL_STEP, BALL_STEP);
        game.tick();
        assert!(game.ball.vx > 0.0);
        assert_eq!(game.ball.x, LEFT_EDGE);
    }

    #[test]
    fn ball_bounces_off_right_paddle() {
        let mut game = game_with_ball(RIGHT_EDGE - 0.5, CENTER_Y, BALL_STEP, -BALL_STEP);
        game.tick();
        assert!(game.ball.vx < 0.0);
        assert_eq!(game.ball.x, RIGHT_EDGE);
    }

    #[test]
    fn missed_ball_scores_and_reserves_at_center() {
        let mut game = game_with_ball(-BALL_HALF - 0.1, CENTER_Y, -BALL_STEP, BALL_STEP);
        game.tick();
        assert_eq!(game.score.right, 1);
        assert_eq!(game.score.left, 0);
        assert_eq!(game.ball.x, FIELD_WIDTH / 2.0);
        assert_eq!(game.ball.y, FIELD_HEIGHT / 2.0);
    }

    #[test]
    fn match_ends_when_win_score_is_reached() {
        let mut game = game_with_ball(-BALL_HALF - 0.1, CENTER_Y, -BALL_STEP, BALL_STEP);
        game.score = Score {
            left: 0,
            right: WIN_SCORE - 1,
        };
        game.tick();
        assert_eq!(
            game.phase,
            GamePhase::GameOver {
                winner: Side::Right
            }
        );
    }

    #[test]
    fn restart_resets_the_match_after_game_over() {
        let mut game = game_with_ball(-BALL_HALF - 0.1, CENTER_Y, -BALL_STEP, BALL_STEP);
        game.score = Score {
            left: 0,
            right: WIN_SCORE - 1,
        };
        game.tick();
        assert_ne!(game.phase, GamePhase::Playing); // setup guard

        game.handle_input(InputEvent::Restart);
        assert_eq!(game.phase, GamePhase::Playing);
        assert_eq!(game.score, Score::default());
        assert_eq!(game.left_paddle_y, CENTER_Y);
        assert_eq!(game.right_paddle_y, CENTER_Y);
    }

    #[test]
    fn restart_is_ignored_while_playing() {
        let mut game = Game::initial(42);
        game.score = Score { left: 3, right: 5 };
        game.handle_input(InputEvent::Restart);
        assert_eq!(game.score, Score { left: 3, right: 5 });
    }

    #[test]
    fn simulation_freezes_while_game_over() {
        let mut game = Game::initial(42);
        game.phase = GamePhase::GameOver { winner: Side::Left };
        let (paddle_y, ball_x) = (game.left_paddle_y, game.ball.x);
        game.handle_input(InputEvent::SetPaddleDirection {
            side: Side::Left,
            direction: Some(Direction::Down),
        });
        game.tick();
        assert_eq!(game.left_paddle_y, paddle_y);
        assert_eq!(game.ball.x, ball_x);
    }

    #[test]
    fn serve_is_always_a_45_degree_diagonal_from_center() {
        for seed in 0..64 {
            let game = Game::initial(seed);
            assert_eq!(game.ball.x, FIELD_WIDTH / 2.0);
            assert_eq!(game.ball.y, FIELD_HEIGHT / 2.0);
            assert!((game.ball.vx.abs() - BALL_STEP).abs() < 1e-6);
            assert!((game.ball.vy.abs() - BALL_STEP).abs() < 1e-6);
        }
    }

    #[test]
    fn snapshot_mirrors_the_internal_state() {
        let game = game_with_ball(10.0, 20.0, BALL_STEP, -BALL_STEP);
        let snapshot = game.snapshot();
        assert_eq!(snapshot.phase, GamePhase::Playing);
        assert_eq!(snapshot.score, Score::default());
        assert_eq!(snapshot.ball_x, 10.0);
        assert_eq!(snapshot.ball_y, 20.0);
        assert_eq!(snapshot.left_paddle_y, CENTER_Y);
        assert_eq!(snapshot.right_paddle_y, CENTER_Y);
    }

    #[test]
    fn default_matches_new_in_shape() {
        let game = Game::default();
        assert_eq!(game.phase, GamePhase::Playing);
        assert_eq!(game.score, Score::default());
    }
}
