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
    ///
    /// `held` reports whether the frontend has confirmed the key is being
    /// held (its auto-repeat stream has arrived): a confirmed hold moves
    /// at [`PADDLE_SPEED`](crate::PADDLE_SPEED), anything else at the
    /// reduced [`PADDLE_TAP_SPEED`](crate::PADDLE_TAP_SPEED) — an
    /// unconfirmed tap cannot send the paddle halfway across the field
    /// while release detection is still pending. `held` is meaningless
    /// when `direction` is `None`.
    SetPaddleDirection {
        side: Side,
        direction: Option<Direction>,
        held: bool,
    },
    /// Restart the match after a game over.
    Restart,
    /// Shut the backend down.
    Shutdown,
}

/// High-level state of the match.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GamePhase {
    /// Waiting for the next serve: the ball waits at the center and will
    /// fly toward `toward` when `ticks_left` reaches zero. Paddles can
    /// still move while serving.
    Serving {
        /// Side the ball will fly toward (the player who just lost).
        toward: Side,
        /// Ticks until the serve, at [`TICKS_PER_SEC`](crate::TICKS_PER_SEC)
        /// ticks per second.
        ticks_left: u16,
    },
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

/// A discrete thing that happened during a tick, reported to the
/// frontend for presentation-layer feedback (e.g. sound).
///
/// The backend never performs I/O: it only *announces* events in the
/// snapshot stream; playing them is the frontend's business.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GameEvent {
    /// The ball bounced off a paddle.
    PaddleHit,
    /// A point was scored and the round ended (the serving pause
    /// follows). Not emitted for the match-deciding point — [`GameOver`]
    /// replaces it so the two tones never sound together.
    PointScored,
    /// The match ended: someone reached the winning score.
    GameOver,
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
///
/// `events` lists the [`GameEvent`]s of the tick this snapshot closes;
/// taking a snapshot consumes them, so each event appears exactly once
/// in the stream.
#[derive(Debug, Clone, PartialEq)]
pub struct GameSnapshot {
    pub phase: GamePhase,
    pub score: Score,
    pub left_paddle_y: f32,
    pub right_paddle_y: f32,
    pub ball_x: f32,
    pub ball_y: f32,
    pub events: Vec<GameEvent>,
}

/// Width of the play field in core units; x ranges over `0.0 .. FIELD_WIDTH`.
///
/// Elongated on purpose (7:3) so that crossing the field takes long enough
/// to react to the ball.
pub const FIELD_WIDTH: f32 = 140.0;

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
