//! The AI opponent: the right paddle's brain.
//!
//! Pure game logic, no I/O. [`Game::tick`] feeds the controller the ball
//! state and the right paddle's position every tick; the controller
//! answers with the direction the paddle should keep moving in — the
//! same steering a human's key presses produce. The AI therefore
//! inherits the full paddle speed model (tap tier, ramp, instant stop):
//! an AI paddle moves like a human one, never teleports.
//!
//! Difficulty is four knobs (see ARCHITECTURE.md):
//!
//! * **reaction delay** — the controller perceives the ball state from
//!   `delay_ticks` ago, through a small ring buffer: it literally sees
//!   the past;
//! * **aim noise** — a random offset added to the target, resampled
//!   once per approach (the error is committed when the ball turns
//!   toward the AI, and held for the whole rally leg);
//! * **prediction** — how the intercept point is computed: tracking the
//!   ball's current height, a linear extrapolation, or the full
//!   trajectory with wall reflections folded in;
//! * **speed ceiling** — the easy AI's paddle simply is slower.
//!
//! When the ball moves away, the AI drifts back to the field center;
//! inside a small dead zone around its target it stops, so a locked-on
//! AI doesn't jitter.

use std::collections::VecDeque;

use crate::game::{BALL_SIZE, PADDLE_INSET, PADDLE_SPEED, Xorshift};
use crate::protocol::{Difficulty, Direction, FIELD_HEIGHT, FIELD_WIDTH};

/// Paddle speed ceiling of the easy AI, in field units per second.
const EASY_MAX_SPEED: f32 = 48.0;

/// The AI stops steering when its paddle is within this distance of the
/// target — smaller would make it jitter, larger would make it sloppy.
const DEAD_ZONE: f32 = 1.0;

/// How the intercept target is computed from the perceived ball state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Prediction {
    /// Aim at the ball's current height: no look-ahead at all.
    TrackOnly,
    /// Linear extrapolation to the paddle's plane, clamped into the
    /// field — wall reflections are *not* folded in.
    Linear,
    /// Full trajectory: the extrapolated point is reflected off the top
    /// and bottom walls until it lies inside the field.
    Trajectory,
}

#[derive(Debug, Clone, Copy)]
struct Params {
    delay_ticks: usize,
    noise_sigma: f32,
    prediction: Prediction,
    max_speed: f32,
}

/// The right paddle's AI driver. Owned by [`Game`](crate::Game).
#[derive(Debug)]
pub struct AiController {
    params: Params,
    /// Ring buffer of recent ball states; the front is what the AI
    /// "sees" (the `delay_ticks`-old sample).
    history: VecDeque<[f32; 4]>,
    /// Aim offset of the current approach leg (units), sampled when the
    /// ball last turned toward the AI.
    noise: f32,
    /// Whether the (perceived) ball was moving toward the AI last tick.
    ball_toward: bool,
}

impl AiController {
    pub fn new(difficulty: Difficulty) -> Self {
        let params = match difficulty {
            Difficulty::Easy => Params {
                delay_ticks: 12,
                noise_sigma: 6.0,
                prediction: Prediction::TrackOnly,
                max_speed: EASY_MAX_SPEED,
            },
            Difficulty::Normal => Params {
                delay_ticks: 6,
                noise_sigma: 3.0,
                prediction: Prediction::Linear,
                max_speed: PADDLE_SPEED,
            },
            Difficulty::Hard => Params {
                delay_ticks: 2,
                noise_sigma: 0.0,
                prediction: Prediction::Trajectory,
                max_speed: PADDLE_SPEED,
            },
        };
        Self {
            params,
            history: VecDeque::new(),
            noise: 0.0,
            ball_toward: false,
        }
    }

    /// The paddle speed ceiling this AI plays with.
    pub fn max_speed(&self) -> f32 {
        self.params.max_speed
    }

    /// Decides the right paddle's steering for this tick.
    ///
    /// `ball` is `(x, y, vx, vy)` in core field units, velocities being
    /// per-tick displacements.
    pub(crate) fn decide(
        &mut self,
        ball: (f32, f32, f32, f32),
        paddle_y: f32,
        rng: &mut Xorshift,
    ) -> Option<Direction> {
        // A new approach leg begins when the ball turns toward the AI:
        // commit to one noise draw for the whole leg.
        let toward = ball.2 > 0.0;
        if toward && !self.ball_toward {
            self.noise = sample_noise(rng, self.params.noise_sigma);
        }
        self.ball_toward = toward;

        // Perceive the world as it was `delay_ticks` ago.
        self.history.push_back([ball.0, ball.1, ball.2, ball.3]);
        while self.history.len() > self.params.delay_ticks + 1 {
            self.history.pop_front();
        }
        let [x, y, vx, vy] = *self.history.front().expect("history is never empty");

        let target = if vx > 0.0 {
            let raw = match self.params.prediction {
                Prediction::TrackOnly => y,
                Prediction::Linear | Prediction::Trajectory => {
                    // Ticks until the ball reaches the paddle's plane.
                    let t = (FIELD_WIDTH - PADDLE_INSET - x) / vx;
                    let y_pred = y + vy * t;
                    match self.params.prediction {
                        Prediction::Trajectory => fold_into_field(y_pred),
                        _ => y_pred.clamp(BALL_SIZE / 2.0, FIELD_HEIGHT - BALL_SIZE / 2.0),
                    }
                }
            };
            raw + self.noise
        } else {
            // Ball moving away: drift back to the center.
            FIELD_HEIGHT / 2.0
        };

        if (target - paddle_y).abs() <= DEAD_ZONE {
            return None;
        }
        if target < paddle_y {
            Some(Direction::Up)
        } else {
            Some(Direction::Down)
        }
    }
}

/// Reflects `y` off the top/bottom walls until it lies inside the field
/// (the ball bounces at `BALL_SIZE/2` from each wall).
fn fold_into_field(y: f32) -> f32 {
    let half = BALL_SIZE / 2.0;
    let span = FIELD_HEIGHT - BALL_SIZE;
    let mut rel = (y - half).rem_euclid(2.0 * span);
    if rel > span {
        rel = 2.0 * span - rel;
    }
    rel + half
}

/// One noise draw: the sum of three uniform variables in `[-sigma,
/// sigma]` — mean 0, standard deviation `sigma` (Irwin–Hall), no
/// trigonometry needed.
fn sample_noise(rng: &mut Xorshift, sigma: f32) -> f32 {
    if sigma == 0.0 {
        return 0.0;
    }
    let mut sum = 0.0;
    for _ in 0..3 {
        // 53 random bits → uniform in [0, 1), shifted to [-1, 1).
        let u = (rng.next_u64() >> 11) as f32 / (1u64 << 53) as f32;
        sum += u * 2.0 - 1.0;
    }
    sum * sigma
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_rng() -> Xorshift {
        Xorshift::new(42)
    }

    fn hard() -> AiController {
        AiController::new(Difficulty::Hard)
    }

    /// A ball state: parked at the center, flying toward the right
    /// paddle at one unit of x per tick.
    fn ball_flying_right(y: f32, vy: f32) -> (f32, f32, f32, f32) {
        (70.0, y, 1.0, vy)
    }

    #[test]
    fn chases_a_ball_above_the_paddle() {
        let mut ai = hard();
        let mut rng = test_rng();
        assert_eq!(
            ai.decide(ball_flying_right(10.0, 0.0), 30.0, &mut rng),
            Some(Direction::Up)
        );
    }

    #[test]
    fn chases_a_ball_below_the_paddle() {
        let mut ai = hard();
        let mut rng = test_rng();
        assert_eq!(
            ai.decide(ball_flying_right(50.0, 0.0), 30.0, &mut rng),
            Some(Direction::Down)
        );
    }

    #[test]
    fn stops_inside_the_dead_zone() {
        let mut ai = hard();
        let mut rng = test_rng();
        // Dead-on target (hard has no noise): straight ball at the
        // paddle's height.
        assert_eq!(
            ai.decide(ball_flying_right(30.0, 0.0), 30.0, &mut rng),
            None
        );
        // One unit off is still inside the ±1 dead zone.
        assert_eq!(
            ai.decide(ball_flying_right(31.0, 0.0), 30.0, &mut rng),
            None
        );
        assert_eq!(
            ai.decide(ball_flying_right(29.0, 0.0), 30.0, &mut rng),
            None
        );
    }

    #[test]
    fn drifts_to_center_when_the_ball_moves_away() {
        let mut ai = hard();
        let mut rng = test_rng();
        let away = (70.0, 30.0, -1.0, 0.0);
        // Paddle above the center → move down toward it.
        assert_eq!(ai.decide(away, 10.0, &mut rng), Some(Direction::Down));
        // Paddle below the center → move up toward it.
        assert_eq!(ai.decide(away, 50.0, &mut rng), Some(Direction::Up));
    }

    /// The reaction delay is real: a fresh change of the ball's height
    /// must not affect the steering until `delay_ticks` ticks have
    /// passed. Hard has delay 2: one intervening tick keeps the old
    /// target, the third tick finally sees the new height.
    #[test]
    fn reaction_delay_hides_recent_ball_movement() {
        let mut ai = hard();
        let mut rng = test_rng();
        // Tick 1: ball at height 10 → paddle at 30 steers up.
        ai.decide(ball_flying_right(10.0, 0.0), 30.0, &mut rng);
        // Tick 2: the ball teleports to height 50 (an extreme change).
        // The AI still perceives tick 1's height (10) → still up.
        assert_eq!(
            ai.decide(ball_flying_right(50.0, 0.0), 30.0, &mut rng),
            Some(Direction::Up)
        );
        // Tick 3: the sample is now 2 ticks old (exactly the delay) —
        // still tick 1's height.
        assert_eq!(
            ai.decide(ball_flying_right(50.0, 0.0), 30.0, &mut rng),
            Some(Direction::Up)
        );
        // Tick 4: the delayed sample finally reaches the height-50
        // one → down.
        assert_eq!(
            ai.decide(ball_flying_right(50.0, 0.0), 30.0, &mut rng),
            Some(Direction::Down)
        );
    }

    /// Trajectory prediction folds wall reflections: a ball that would
    /// linearly overshoot far above the field is aimed back inside.
    #[test]
    fn trajectory_prediction_folds_wall_reflections() {
        let mut ai = hard();
        let mut rng = test_rng();
        // Ball at the center moving steeply up: it will bounce off the
        // top wall before reaching the right paddle. The linear
        // prediction would be y = 30 - 0.8 * 70 = -26 (way out); the
        // folded one lands back in the field, above the center.
        let steering = ai.decide((70.0, 30.0, 1.0, -0.8), 55.0, &mut rng);
        assert_eq!(steering, Some(Direction::Up));
        // Where exactly? The intercept time is (135 - 70) / 1 = 65
        // ticks, so the linear point is 30 - 0.8 * 65 = -22; folding
        // it back off the walls lands at 23. Re-running with the
        // paddle at that target must report None (dead zone).
        let folded = fold_into_field(30.0 - 0.8 * (FIELD_WIDTH - PADDLE_INSET - 70.0));
        assert!(
            (5.0..FIELD_HEIGHT / 2.0).contains(&folded),
            "folded target {folded} should be in the upper half"
        );
        let mut ai = hard();
        let mut rng = test_rng();
        assert_eq!(
            ai.decide((70.0, 30.0, 1.0, -0.8), folded, &mut rng),
            None,
            "hard AI parks exactly on the folded trajectory target"
        );
    }

    /// Linear prediction truncates instead of folding: a steep ball's
    /// raw extrapolation lands far outside the field, and the clamp
    /// pins the target at the wall. Each assertion uses a fresh
    /// controller so the 6-tick reaction delay holds no stale samples.
    #[test]
    fn linear_prediction_clamps_to_the_field() {
        // Steep climb: clamped to the top wall; even the worst noise
        // draw (σ = 3, so ±9 units) keeps the target well above any
        // paddle in the lower half.
        let steep_up = (70.0, 30.0, 1.0, -0.9);
        for paddle_y in [30.0, 45.0] {
            let mut ai = AiController::new(Difficulty::Normal);
            let mut rng = test_rng();
            assert_eq!(ai.decide(steep_up, paddle_y, &mut rng), Some(Direction::Up));
        }
        // Steep dive: mirrored at the bottom wall.
        let steep_down = (70.0, 30.0, 1.0, 0.9);
        for paddle_y in [15.0, 30.0] {
            let mut ai = AiController::new(Difficulty::Normal);
            let mut rng = test_rng();
            assert_eq!(
                ai.decide(steep_down, paddle_y, &mut rng),
                Some(Direction::Down)
            );
        }
    }

    /// Noise is committed per approach leg: while the ball keeps coming
    /// at the AI, the draw is held; a new leg resamples.
    #[test]
    fn noise_is_resampled_only_on_new_approaches() {
        let mut ai = AiController::new(Difficulty::Easy);
        let mut rng = test_rng();
        ai.decide(ball_flying_right(30.0, 0.0), 30.0, &mut rng);
        let committed = ai.noise;
        // Same approach leg continues: the draw is held.
        ai.decide(ball_flying_right(30.0, 0.0), 30.0, &mut rng);
        assert_eq!(ai.noise, committed);
        // The ball goes away and comes back: a new leg, a fresh draw.
        // (Deterministic: the rng stream has advanced, so the redraw
        // differs — verified by this very run.)
        ai.decide((70.0, 30.0, -1.0, 0.0), 30.0, &mut rng);
        ai.decide(ball_flying_right(30.0, 0.0), 30.0, &mut rng);
        assert_ne!(ai.noise, committed);
    }

    #[test]
    fn easy_ai_is_slower_than_the_others() {
        assert!(AiController::new(Difficulty::Easy).max_speed() < PADDLE_SPEED);
        assert_eq!(
            AiController::new(Difficulty::Normal).max_speed(),
            PADDLE_SPEED
        );
        assert_eq!(
            AiController::new(Difficulty::Hard).max_speed(),
            PADDLE_SPEED
        );
    }

    /// Sanity of the noise distribution: mean ≈ 0, spread ≈ sigma.
    #[test]
    fn noise_has_the_right_spread() {
        let mut rng = test_rng();
        let mut sum = 0.0;
        let mut sq = 0.0;
        let n = 6000;
        for _ in 0..n {
            let v = sample_noise(&mut rng, 6.0);
            sum += v;
            sq += v * v;
        }
        let mean = sum / n as f32;
        let var = sq / n as f32;
        assert!(mean.abs() < 0.5, "mean {mean}");
        assert!((var - 36.0).abs() < 6.0, "variance {var} (sigma^2 = 36)");
    }

    #[test]
    fn fold_is_identity_inside_the_field() {
        assert_eq!(fold_into_field(0.5), 0.5);
        assert_eq!(fold_into_field(30.0), 30.0);
        assert_eq!(fold_into_field(59.5), 59.5);
        // One reflection above: 0.5 - 1.0 → 1.5.
        assert!((fold_into_field(-0.5) - 1.5).abs() < 1e-6);
        // Two reflections: 0.5 - 2*59 → mirrored twice.
        assert!((fold_into_field(0.5 - 2.0 * 59.0) - 0.5).abs() < 1e-6);
    }
}
