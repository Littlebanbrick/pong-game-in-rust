//! Message protocol between the backend and the frontend.
//!
//! The backend owns all game state; the frontend only renders it. The two
//! sides communicate exclusively through the types in this module:
//!
//! * [`InputEvent`] travels frontend -> backend over the command channel.
//! * [`GameSnapshot`] travels backend -> frontend over the state channel.

/// Identifies one of the two players / halves of the field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Side {
    Left,
    Right,
}

impl Side {
    /// Returns the side on the opposite end of the field.
    pub fn opposite(self) -> Self {
        match self {
            Side::Left => Side::Right,
            Side::Right => Side::Left,
        }
    }
}

/// Vertical movement direction of a paddle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Up,
    Down,
}

/// An input command sent from the frontend to the backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    /// Set the direction a paddle should keep moving in (`None` stops it).
    SetPaddleDirection {
        side: Side,
        direction: Option<Direction>,
    },
    /// Restart the match after a game over.
    Restart,
    /// Shut the backend down.
    Shutdown,
}

/// High-level state of the match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GamePhase {
    /// The ball is in play.
    Playing,
    /// Someone reached the winning score.
    GameOver { winner: Side },
}

/// Current score of both players.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Score {
    pub left: u32,
    pub right: u32,
}

impl Score {
    /// Records one point for `side`.
    pub fn record(&mut self, side: Side) {
        match side {
            Side::Left => self.left += 1,
            Side::Right => self.right += 1,
        }
    }
}

/// Complete renderable state of the game, sent once per backend tick.
///
/// All coordinates are in core field units (see [`FIELD_WIDTH`] and
/// [`FIELD_HEIGHT`]) and denote the **center** of the object: paddle y is
/// the center of the paddle, ball x/y the center of the ball.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GameSnapshot {
    pub phase: GamePhase,
    pub score: Score,
    pub left_paddle_y: f32,
    pub right_paddle_y: f32,
    pub ball_x: f32,
    pub ball_y: f32,
}

/// Width of the play field in core units; x ranges over `0.0 .. FIELD_WIDTH`.
pub const FIELD_WIDTH: f32 = 100.0;

/// Height of the play field in core units; y ranges over `0.0 .. FIELD_HEIGHT`.
pub const FIELD_HEIGHT: f32 = 60.0;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opposite_side_round_trips() {
        assert_eq!(Side::Left.opposite(), Side::Right);
        assert_eq!(Side::Right.opposite().opposite(), Side::Right);
    }

    #[test]
    fn score_records_for_the_correct_side() {
        let mut score = Score::default();
        score.record(Side::Left);
        score.record(Side::Left);
        score.record(Side::Right);
        assert_eq!(score, Score { left: 2, right: 1 });
    }
}
