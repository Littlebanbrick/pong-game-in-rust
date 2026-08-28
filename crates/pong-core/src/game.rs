//! Game simulation: state, physics, and rules.
//!
//! [`Game`] is a pure state machine: advance it with [`Game::tick`], feed it
//! player intent with [`Game::handle_input`], and read a renderable copy of
//! its state with [`Game::snapshot`]. It performs no I/O and measures time
//! only in ticks, which keeps the simulation deterministic and testable.
//!
//! Phase-2 physics: the ball always travels at exactly [`BALL_SPEED`] (only
//! its direction changes) and the bounce angle off a paddle depends on where
//! the ball hits it — the paddle edge returns steep angles, the center
//! returns a flat one. Between points the game pauses in
//! [`GamePhase::Serving`] before serving toward the player who just
//! conceded.

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

/// Paddle speed in field units per second while the player's key is
/// confirmed held (its auto-repeat stream has arrived). Matches the ball
/// speed: a full-field-height traverse takes 0.75 s.
pub const PADDLE_SPEED: f32 = 80.0;

/// Paddle speed before the hold is confirmed — a bare tap. Kept at 30
/// (not a fraction of [`PADDLE_SPEED`]): it is the overshoot guard for
/// the tap-detection window, whose budget is independent of the max
/// speed. The window moves the paddle at most ~0.3 of the field height.
pub const PADDLE_TAP_SPEED: f32 = 30.0;

/// Paddle acceleration while ramping from tap to full speed after the
/// hold is confirmed, in field units per second squared. Chosen so the
/// ramp takes ~250 ms.
pub const PADDLE_ACCEL: f32 = (PADDLE_SPEED - PADDLE_TAP_SPEED) / 0.25;

/// Ball speed (path length) in field units per second. The resultant speed
/// magnitude never changes; only its direction does.
pub const BALL_SPEED: f32 = 80.0;

/// Pause between a point and the next serve, in ticks.
pub const SERVE_PAUSE_TICKS: u16 = TICKS_PER_SEC as u16;

/// Steepest bounce angle off a paddle (hit at the paddle edge), in radians.
pub const MAX_BOUNCE_ANGLE: f32 = std::f32::consts::PI / 3.0;

/// Serve angle options, in degrees: flat, ±15°, ±30°, ±45°. None steeper
/// than 45°, and the flat serve is twice as likely as any angle — the
/// same table shape as the author's FPGA Pong (`Pong-dld`, whose lookup
/// lists the horizontal entry twice).
const SERVE_ANGLE_TABLE: [f32; 7] = [0.0, 15.0, -15.0, 30.0, -30.0, 45.0, -45.0];

/// Maps a random roll (0..8) to a serve angle in degrees, doubling the
/// flat serve (roll 7 maps to the same entry as roll 0).
fn serve_angle_degrees(roll: u64) -> f32 {
    let index = match (roll % 8) as usize {
        7 => 0,
        other => other,
    };
    SERVE_ANGLE_TABLE[index]
}

const PADDLE_HALF_W: f32 = PADDLE_WIDTH / 2.0;
const PADDLE_HALF_H: f32 = PADDLE_HEIGHT / 2.0;
const BALL_HALF: f32 = BALL_SIZE / 2.0;

/// Ball displacement per tick at [`BALL_SPEED`].
const BALL_STEP: f32 = BALL_SPEED * DT;

/// Ball position and velocity (center coordinates, field units).
#[derive(Debug, Clone, Copy, PartialEq)]
struct Ball {
    x: f32,
    y: f32,
    vx: f32,
    vy: f32,
}

impl Ball {
    /// Resultant speed magnitude per tick.
    fn speed(&self) -> f32 {
        (self.vx * self.vx + self.vy * self.vy).sqrt()
    }
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
    left_held: bool,
    right_held: bool,
    left_speed: f32,
    right_speed: f32,
    ball: Ball,
    rng: Xorshift,
}

impl Game {
    /// Creates a game paused before the first serve (random direction).
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
            left_held: false,
            right_held: false,
            left_speed: 0.0,
            right_speed: 0.0,
            ball: Ball {
                x: 0.0,
                y: 0.0,
                vx: 0.0,
                vy: 0.0,
            },
            rng: Xorshift::new(seed),
        };
        // The opening serve goes to a random side.
        let toward = if game.rng.next_bool() {
            Side::Left
        } else {
            Side::Right
        };
        game.begin_serve(toward);
        game
    }

    /// Applies one input event from the frontend.
    pub fn handle_input(&mut self, event: InputEvent) {
        match event {
            InputEvent::SetPaddleDirection {
                side,
                direction,
                held,
            } => match side {
                Side::Left => {
                    self.left_direction = direction;
                    self.left_held = held;
                }
                Side::Right => {
                    self.right_direction = direction;
                    self.right_held = held;
                }
            },
            InputEvent::Restart => {
                if matches!(self.phase, GamePhase::GameOver { .. }) {
                    self.reset();
                }
            }
            // Shutdown is handled by the runtime, not by the simulation.
            InputEvent::Shutdown => {}
        }
    }

    /// Advances the simulation by one tick.
    ///
    /// * [`GamePhase::Serving`] — paddles keep moving, the ball waits at
    ///   the center until the countdown runs out.
    /// * [`GamePhase::Playing`] — full simulation.
    /// * [`GamePhase::GameOver`] — everything freezes until a restart.
    pub fn tick(&mut self) {
        match self.phase {
            GamePhase::Serving { toward, ticks_left } => {
                self.move_paddle(Side::Left);
                self.move_paddle(Side::Right);
                if ticks_left <= 1 {
                    self.launch_serve(toward);
                } else {
                    self.phase = GamePhase::Serving {
                        toward,
                        ticks_left: ticks_left - 1,
                    };
                }
            }
            GamePhase::Playing => {
                self.move_paddle(Side::Left);
                self.move_paddle(Side::Right);
                self.ball.x += self.ball.vx;
                self.ball.y += self.ball.vy;
                self.bounce_off_walls();
                self.bounce_off_paddle(Side::Left);
                self.bounce_off_paddle(Side::Right);
                self.score_if_ball_exited();
            }
            GamePhase::GameOver { .. } => {}
        }
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

    /// Resets to a fresh match with an opening serve toward a random side.
    fn reset(&mut self) {
        self.score = Score::default();
        self.left_paddle_y = FIELD_HEIGHT / 2.0;
        self.right_paddle_y = FIELD_HEIGHT / 2.0;
        self.left_direction = None;
        self.right_direction = None;
        self.left_held = false;
        self.right_held = false;
        self.left_speed = 0.0;
        self.right_speed = 0.0;
        let toward = if self.rng.next_bool() {
            Side::Left
        } else {
            Side::Right
        };
        self.begin_serve(toward);
    }

    /// Enters the serving pause: ball parked at the center, velocity zero.
    fn begin_serve(&mut self, toward: Side) {
        self.phase = GamePhase::Serving {
            toward,
            ticks_left: SERVE_PAUSE_TICKS,
        };
        self.ball = Ball {
            x: FIELD_WIDTH / 2.0,
            y: FIELD_HEIGHT / 2.0,
            vx: 0.0,
            vy: 0.0,
        };
    }

    /// Launches the ball toward `toward` at a random angle from
    /// [`SERVE_ANGLE_TABLE`], keeping the speed magnitude at [`BALL_SPEED`].
    fn launch_serve(&mut self, toward: Side) {
        let angle = serve_angle_degrees(self.rng.next_u64()).to_radians();
        let vx = match toward {
            Side::Left => -BALL_STEP * angle.cos(),
            Side::Right => BALL_STEP * angle.cos(),
        };
        self.ball.vx = vx;
        self.ball.vy = BALL_STEP * angle.sin();
        self.phase = GamePhase::Playing;
    }

    fn move_paddle(&mut self, side: Side) {
        let (y, direction, held, speed) = match side {
            Side::Left => (
                &mut self.left_paddle_y,
                self.left_direction,
                self.left_held,
                &mut self.left_speed,
            ),
            Side::Right => (
                &mut self.right_paddle_y,
                self.right_direction,
                self.right_held,
                &mut self.right_speed,
            ),
        };
        // Speed model: an unconfirmed tap moves at a constant crawl; a
        // confirmed hold ramps linearly up to the full speed; releasing
        // stops instantly. Pressing from standstill starts at the crawl
        // speed — first-tick responsiveness is the whole point of the
        // tap tier, so only the tap→full transition is smoothed.
        *speed = match direction {
            None => 0.0,
            Some(_) if !held => PADDLE_TAP_SPEED,
            Some(_) => (*speed + PADDLE_ACCEL * DT).clamp(PADDLE_TAP_SPEED, PADDLE_SPEED),
        };
        let Some(direction) = direction else { return };
        let step = match direction {
            Direction::Up => -*speed * DT,
            Direction::Down => *speed * DT,
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

        // Where the ball hit the paddle, from -1 (below the paddle center)
        // to +1 (above it). Steep returns come from the edges, flat ones
        // from the center. Only the direction changes; the resultant speed
        // magnitude is kept at BALL_SPEED.
        let offset = ((self.ball.y - paddle_y) / PADDLE_HALF_H).clamp(-1.0, 1.0);
        let angle = offset * MAX_BOUNCE_ANGLE;
        let (dir_x, dir_y) = match side {
            Side::Left => (1.0, 1.0),
            Side::Right => (-1.0, 1.0),
        };
        self.ball.vx = dir_x * BALL_STEP * angle.cos();
        self.ball.vy = dir_y * BALL_STEP * angle.sin();

        // The invariant the whole physics rests on: bounces change only
        // the direction, never the speed magnitude.
        debug_assert!((self.ball.speed() - BALL_STEP).abs() < 1e-5);

        // Push the ball out of the paddle so the overlap test cannot fire
        // again on the next tick.
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
                // Serve toward the player who just conceded.
                self.begin_serve(scorer.opposite());
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

    /// A playing game whose ball is replaced with a deterministic one.
    fn game_with_ball(x: f32, y: f32, vx: f32, vy: f32) -> Game {
        let mut game = Game::initial(42);
        game.phase = GamePhase::Playing;
        game.ball = Ball { x, y, vx, vy };
        game
    }

    /// A playing game with the left paddle at a chosen height.
    fn game_with_left_paddle(y: f32) -> Game {
        let mut game = Game::initial(42);
        game.phase = GamePhase::Playing;
        game.left_paddle_y = y;
        game
    }

    /// Advances the game past the serving pause, into play.
    fn play_after_serve(game: &mut Game) {
        while matches!(game.phase, GamePhase::Serving { .. }) {
            game.tick();
        }
    }

    /// Steers one paddle; `held` reports a confirmed key hold.
    fn steer(game: &mut Game, side: Side, direction: Option<Direction>, held: bool) {
        game.handle_input(InputEvent::SetPaddleDirection {
            side,
            direction,
            held,
        });
    }

    #[test]
    fn unconfirmed_tap_moves_at_tap_speed() {
        let mut game = Game::initial(42);
        steer(&mut game, Side::Left, Some(Direction::Up), false);
        let before = game.left_paddle_y;
        game.tick();
        let expected = before - PADDLE_TAP_SPEED * DT;
        assert!((game.left_paddle_y - expected).abs() < 1e-6);
    }

    #[test]
    fn held_flag_without_direction_moves_nothing() {
        let mut game = Game::initial(42);
        steer(&mut game, Side::Left, None, true);
        let before = game.left_paddle_y;
        game.tick();
        assert_eq!(game.left_paddle_y, before);
    }

    #[test]
    fn confirming_the_same_direction_speeds_the_paddle_up() {
        let mut game = Game::initial(42);
        steer(&mut game, Side::Left, Some(Direction::Up), false);
        let start = game.left_paddle_y;
        game.tick();
        let tap_step = start - game.left_paddle_y; // upward: positive
        steer(&mut game, Side::Left, Some(Direction::Up), true);
        game.tick();
        let ramp_step = (start - game.left_paddle_y) - tap_step;
        assert!(
            ramp_step > tap_step,
            "ramp step {ramp_step} should exceed tap step {tap_step}"
        );
    }

    /// The confirmation ramp: starts at the tap speed, accelerates
    /// monotonically, and settles at exactly the full speed.
    #[test]
    fn confirmed_hold_ramps_to_full_speed() {
        let mut game = Game::initial(42);
        steer(&mut game, Side::Left, Some(Direction::Up), true);
        let start = game.left_paddle_y;
        game.tick();
        let first_step = start - game.left_paddle_y;
        assert!(
            (first_step - PADDLE_TAP_SPEED * DT).abs() < 1e-6,
            "ramp starts at the tap speed"
        );
        // ~20 ticks: ramp completes long before the wall is near.
        let mut last_step = first_step;
        for _ in 0..20 {
            let before = game.left_paddle_y;
            game.tick();
            let step = before - game.left_paddle_y;
            assert!(
                step >= last_step - 1e-6,
                "speed must not decrease while held"
            );
            last_step = step;
        }
        assert!(
            (last_step - PADDLE_SPEED * DT).abs() < 1e-6,
            "reaches the full speed"
        );
    }

    /// Reversing direction mid-ramp keeps the current speed: reversing
    /// is instant, no re-acceleration from the tap speed.
    #[test]
    fn reversing_direction_keeps_the_current_speed() {
        let mut game = Game::initial(42);
        steer(&mut game, Side::Left, Some(Direction::Down), true);
        for _ in 0..20 {
            game.tick();
        }
        let before = game.left_paddle_y;
        steer(&mut game, Side::Left, Some(Direction::Up), true);
        game.tick();
        let step = game.left_paddle_y - before; // upward: negative
        // 1e-3: the position accumulates 20 ticks of f32 rounding before
        // this single-step measurement; far below the step size itself.
        assert!(
            (step + PADDLE_SPEED * DT).abs() < 1e-3,
            "reversal at full speed stays at full speed"
        );
    }

    /// After a stop, a new unconfirmed press snaps back to the tap
    /// speed — leftover ramp momentum must not leak into fresh taps.
    #[test]
    fn speed_snaps_back_to_tap_after_a_stop() {
        let mut game = Game::initial(42);
        steer(&mut game, Side::Left, Some(Direction::Down), true);
        for _ in 0..20 {
            game.tick();
        }
        steer(&mut game, Side::Left, None, false);
        game.tick(); // comes to rest
        steer(&mut game, Side::Left, Some(Direction::Down), false);
        let before = game.left_paddle_y;
        game.tick();
        let expected = before + PADDLE_TAP_SPEED * DT;
        assert!((game.left_paddle_y - expected).abs() < 1e-6);
    }

    #[test]
    fn new_game_starts_in_serving_pause() {
        let game = Game::initial(42);
        assert!(matches!(game.phase, GamePhase::Serving { .. }));
        assert_eq!(game.ball.x, FIELD_WIDTH / 2.0);
        assert_eq!(game.ball.y, FIELD_HEIGHT / 2.0);
        assert_eq!(game.ball.speed(), 0.0);
    }

    #[test]
    fn serve_launches_after_the_pause_with_constant_speed() {
        for seed in 0..64 {
            let mut game = Game::initial(seed);
            let GamePhase::Serving { toward, .. } = game.phase else {
                panic!("expected serving phase");
            };
            play_after_serve(&mut game);
            assert_eq!(game.phase, GamePhase::Playing);
            // Moving toward the announced side.
            let vx_sign = match toward {
                Side::Left => -1.0,
                Side::Right => 1.0,
            };
            assert_eq!(game.ball.vx.signum(), vx_sign);
            // Speed magnitude is BALL_SPEED, the angle comes from the table.
            assert!((game.ball.speed() - BALL_STEP).abs() < 1e-6);
            let angle_deg = (game.ball.vy / game.ball.vx.abs()).atan().to_degrees();
            assert!(
                SERVE_ANGLE_TABLE
                    .iter()
                    .any(|table_deg| (angle_deg - table_deg).abs() < 1e-4),
                "serve angle {angle_deg}° not in the table"
            );
        }
    }

    /// Every table entry shows up across seeds: no angle is unreachable.
    #[test]
    fn serve_angles_cover_the_whole_table() {
        let mut seen = [false; SERVE_ANGLE_TABLE.len()];
        for seed in 0..400 {
            let mut game = Game::initial(seed);
            play_after_serve(&mut game);
            let angle_deg = (game.ball.vy / game.ball.vx.abs()).atan().to_degrees();
            for (i, table_deg) in SERVE_ANGLE_TABLE.iter().enumerate() {
                if (angle_deg - table_deg).abs() < 1e-4 {
                    seen[i] = true;
                }
            }
        }
        assert!(
            seen.iter().all(|entry| *entry),
            "unreachable angles: {seen:?}"
        );
    }

    /// The flat serve is twice as likely as any single angle: with the
    /// doubled table entry it must appear noticeably more often than
    /// each angled option.
    #[test]
    fn flat_serve_is_doubly_weighted() {
        let mut flat = 0;
        let mut one_angle = 0;
        for seed in 0..800 {
            let mut game = Game::initial(seed);
            play_after_serve(&mut game);
            if game.ball.vy.abs() < 1e-9 {
                flat += 1;
            } else if (game.ball.vy / game.ball.vx.abs()).atan().to_degrees() > 37.0 {
                // +45° is one specific angled entry. The cut sits midway
                // between 30° and 45° so f32 tan/atan round-trip noise on
                // the 30° serves cannot leak into this bucket.
                one_angle += 1;
            }
        }
        // Expected: flat ≈ 2/8, one angled entry ≈ 1/8 of 800.
        assert!(
            flat > one_angle * 3 / 2,
            "flat {flat} should clearly outweigh one angle {one_angle}"
        );
    }

    #[test]
    fn serve_after_a_point_goes_to_the_loser() {
        let mut game = game_with_ball(-BALL_HALF - 0.1, CENTER_Y, -BALL_STEP, 0.5);
        game.tick();
        assert_eq!(game.score.right, 1);
        // Ball exited past the LEFT wall: left conceded, next serve goes left.
        assert_eq!(
            game.phase,
            GamePhase::Serving {
                toward: Side::Left,
                ticks_left: SERVE_PAUSE_TICKS
            }
        );
        play_after_serve(&mut game);
        assert!(game.ball.vx < 0.0, "ball should fly toward the left player");
    }

    #[test]
    fn paddles_move_while_serving() {
        let mut game = Game::initial(42);
        game.handle_input(InputEvent::SetPaddleDirection {
            side: Side::Left,
            direction: Some(Direction::Up),
            held: true,
        });
        let before = game.left_paddle_y;
        game.tick();
        // First tick after the press runs at the tap speed even when the
        // hold is already confirmed: the ramp starts there.
        let expected = before - PADDLE_TAP_SPEED * DT;
        assert!((game.left_paddle_y - expected).abs() < 1e-6);
    }

    #[test]
    fn serving_countdown_decrements_per_tick() {
        let mut game = Game::initial(42);
        let GamePhase::Serving {
            ticks_left: first, ..
        } = game.phase
        else {
            panic!("expected serving phase");
        };
        game.tick();
        let GamePhase::Serving {
            ticks_left: second, ..
        } = game.phase
        else {
            panic!("expected serving phase");
        };
        assert_eq!(first - 1, second);
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
            held: true,
        });
        let before = game.left_paddle_y;
        game.tick();
        // Ramping starts at the tap speed (see the speed model).
        let expected = before - PADDLE_TAP_SPEED * DT;
        assert!((game.left_paddle_y - expected).abs() < 1e-6);
    }

    #[test]
    fn paddle_stops_when_direction_cleared() {
        let mut game = Game::initial(42);
        game.handle_input(InputEvent::SetPaddleDirection {
            side: Side::Right,
            direction: Some(Direction::Down),
            held: true,
        });
        game.tick();
        game.handle_input(InputEvent::SetPaddleDirection {
            side: Side::Right,
            direction: None,
            held: true,
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
            held: true,
        });
        for _ in 0..TICKS_PER_SEC {
            game.tick();
        }
        assert_eq!(game.left_paddle_y, PADDLE_HALF_H);

        game.handle_input(InputEvent::SetPaddleDirection {
            side: Side::Left,
            direction: Some(Direction::Down),
            held: true,
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
    fn wall_bounce_keeps_the_speed_magnitude() {
        // Constructed so that the resultant per-tick speed is BALL_STEP.
        let mut game = game_with_ball(
            FIELD_WIDTH / 2.0,
            BALL_HALF + 0.1,
            0.8 * BALL_STEP,
            -0.6 * BALL_STEP,
        );
        game.tick();
        assert!((game.ball.speed() - BALL_STEP).abs() < 1e-6);
    }

    #[test]
    fn center_hit_returns_flat() {
        // Ball meets the paddle dead center: flat return (no vertical part).
        let mut game = game_with_left_paddle(CENTER_Y);
        game.ball = Ball {
            x: LEFT_EDGE + 0.5,
            y: CENTER_Y,
            vx: -BALL_STEP,
            vy: 0.0,
        };
        game.tick();
        assert_eq!(game.ball.vy, 0.0);
        assert!((game.ball.vx - BALL_STEP).abs() < 1e-6);
        assert_eq!(game.ball.x, LEFT_EDGE);
    }

    #[test]
    fn edge_hit_returns_steepest_angle() {
        // Ball meets the top edge of the paddle: steepest return upward.
        let mut game = game_with_left_paddle(CENTER_Y);
        let hit_y = CENTER_Y - PADDLE_HALF_H + 0.1;
        game.ball = Ball {
            x: LEFT_EDGE + 0.5,
            y: hit_y,
            vx: -BALL_STEP,
            vy: 0.0,
        };
        game.tick();
        assert!(game.ball.vy < 0.0, "edge hit should return upward");
        let angle = (game.ball.vy / game.ball.vx).abs().atan();
        assert!((angle - MAX_BOUNCE_ANGLE).abs() < 0.05, "angle {angle}");
        assert!((game.ball.speed() - BALL_STEP).abs() < 1e-6);
    }

    #[test]
    fn bounce_keeps_the_speed_magnitude_for_all_hit_positions() {
        for step in 0..=10 {
            let offset = -1.0 + 2.0 * step as f32 / 10.0;
            let paddle_y = CENTER_Y;
            let hit_y = paddle_y + offset * (PADDLE_HALF_H - BALL_HALF);
            let mut game = game_with_left_paddle(paddle_y);
            game.ball = Ball {
                x: LEFT_EDGE + 0.5,
                y: hit_y,
                vx: -BALL_STEP,
                vy: 0.0,
            };
            game.tick();
            assert!(
                (game.ball.speed() - BALL_STEP).abs() < 1e-6,
                "speed changed at offset {offset}"
            );
        }
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
    fn missed_ball_scores_and_parks_at_center() {
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
        assert!(matches!(game.phase, GamePhase::Serving { .. }));
        assert_eq!(game.score, Score::default());
        assert_eq!(game.left_paddle_y, CENTER_Y);
        assert_eq!(game.right_paddle_y, CENTER_Y);
    }

    #[test]
    fn restart_is_ignored_while_playing() {
        let mut game = Game::initial(42);
        game.phase = GamePhase::Playing;
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
            held: true,
        });
        game.tick();
        assert_eq!(game.left_paddle_y, paddle_y);
        assert_eq!(game.ball.x, ball_x);
    }

    #[test]
    fn snapshot_mirrors_the_internal_state() {
        let mut game = Game::initial(42);
        game.phase = GamePhase::Playing;
        game.ball = Ball {
            x: 10.0,
            y: 20.0,
            vx: BALL_STEP,
            vy: -BALL_STEP,
        };
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
        assert!(matches!(game.phase, GamePhase::Serving { .. }));
        assert_eq!(game.score, Score::default());
    }
}
