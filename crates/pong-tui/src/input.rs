//! Maps crossterm key events to backend input events.
//!
//! Most terminals (the whole VTE family — GNOME Terminal, Ptyxis — among
//! others) never report key releases; the kitty keyboard protocol fixes
//! that but is far from universal, so the frontend cannot depend on it.
//! Key state is instead **inferred from auto-repeat**: the OS re-sends a
//! held key roughly 30 times per second, so a steady stream of events
//! means "still held", and a silent gap means "released".
//!
//! Consequences (see the timeout constants):
//! * once the repeat stream is flowing, a release is detected within an
//!   **adaptive window**: the stream's measured interval (EWMA over
//!   confirmed-to-confirmed arrivals) times [`RELEASE_TIMEOUT_INTERVALS`],
//!   clamped to [`RELEASE_TIMEOUT_MIN`]..=[`RELEASE_TIMEOUT`]. A typical
//!   30 Hz stream yields a ~75 ms window; the fallback before any
//!   interval has been measured is [`RELEASE_TIMEOUT`];
//! * before the first repeat arrives (the OS repeat delay, ~500 ms with
//!   default settings), a gap is only conclusive after
//!   [`HOLD_START_TIMEOUT`], so a quick tap moves the paddle for up to
//!   that long;
//! * terminals that do encode releases (kitty protocol) get exact
//!   behavior for free: a Release event clears the key immediately.
//!
//! If both keys of one side are held, the most recently pressed one wins;
//! when a key expires or is released, the still-held one takes over.

use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use pong_core::{Direction, InputEvent, Opponent, Side};

/// Upper bound (and pre-measurement fallback) of the adaptive window
/// after which a repeating key that went silent is considered released.
pub const RELEASE_TIMEOUT: Duration = Duration::from_millis(150);

/// Lower bound of the adaptive release window. Protects a jittery stream
/// whose measured interval would otherwise shrink the window below the
/// stream's own noise.
pub const RELEASE_TIMEOUT_MIN: Duration = Duration::from_millis(60);

/// How many measured repeat intervals a key may stay silent before it is
/// considered released. Two-and-a-half intervals tolerates a couple of
/// lost events while still braking noticeably sooner than the ceiling.
const RELEASE_TIMEOUT_INTERVALS: f32 = 2.5;

/// How long to wait for the first auto-repeat after a press. Must exceed
/// the OS key-repeat delay (~500 ms with common default settings).
pub const HOLD_START_TIMEOUT: Duration = Duration::from_millis(600);

/// What the frontend should do with an input-model update.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Forward this event to the backend.
    Send(InputEvent),
    /// Return to the start menu. Meaningful only at a game over — the
    /// main loop checks the snapshot phase before honoring it.
    Menu,
    /// Stop the frontend loop (the backend shuts down via `Drop`).
    Quit,
}

/// Bookkeeping for one held movement key.
#[derive(Debug)]
struct KeyHold {
    last_seen: Instant,
    /// Whether the hold is **confirmed**: evidence beyond the initial
    /// press has arrived, i.e. an auto-repeat event (legacy terminals
    /// deliver repeats as fresh presses, so a second arrival is the
    /// signal) or an explicit `Repeat`/`Release` kind. Unconfirmed keys
    /// still move the paddle, but at the reduced tap speed.
    confirmed: bool,
    /// EWMA of the auto-repeat interval, sampled between confirmed
    /// arrivals only: the first gap after a press carries the OS repeat
    /// delay (~500 ms), not the stream's rhythm, and must not poison
    /// the estimate. `None` until the first rhythm sample.
    repeat_interval: Option<Duration>,
}

/// Folds one measured interval into the running estimate (weight 0.5 on
/// the new sample: converges within a few events).
fn fold_interval(prev: Option<Duration>, sample: Duration) -> Option<Duration> {
    match prev {
        None => Some(sample),
        Some(prev) => Some(Duration::from_secs_f32(
            0.5 * prev.as_secs_f32() + 0.5 * sample.as_secs_f32(),
        )),
    }
}

/// The adaptive silence window of a confirmed key: measured interval
/// times [`RELEASE_TIMEOUT_INTERVALS`], clamped; the conservative
/// [`RELEASE_TIMEOUT`] until a rhythm has been measured at all.
fn release_window(interval: Option<Duration>) -> Duration {
    match interval {
        Some(interval) => {
            Duration::from_secs_f32(interval.as_secs_f32() * RELEASE_TIMEOUT_INTERVALS)
                .clamp(RELEASE_TIMEOUT_MIN, RELEASE_TIMEOUT)
        }
        None => RELEASE_TIMEOUT,
    }
}

impl KeyHold {
    fn expired(&self, now: Instant) -> bool {
        let timeout = if self.confirmed {
            release_window(self.repeat_interval)
        } else {
            HOLD_START_TIMEOUT
        };
        now.duration_since(self.last_seen) > timeout
    }
}

/// Per-side held-key bookkeeping.
#[derive(Debug, Default)]
struct SideState {
    up: Option<KeyHold>,
    down: Option<KeyHold>,
    /// Most recently pressed direction among the held keys, used to break
    /// the "both keys held" tie.
    last_pressed: Option<Direction>,
    /// Direction last reported to the backend for this side.
    current: Option<Direction>,
    /// `held` flag of the last report; a confirmation arriving for the
    /// same direction must be forwarded so the paddle speeds up.
    current_held: bool,
}

impl SideState {
    fn effective(&self) -> Option<Direction> {
        match (self.up.is_some(), self.down.is_some()) {
            (true, false) => Some(Direction::Up),
            (false, true) => Some(Direction::Down),
            (true, true) => self.last_pressed,
            (false, false) => None,
        }
    }

    /// Whether the key behind `direction` (if any) is a confirmed hold.
    fn held_of(&self, direction: Option<Direction>) -> bool {
        match direction {
            Some(Direction::Up) => self.up.as_ref().is_some_and(|hold| hold.confirmed),
            Some(Direction::Down) => self.down.as_ref().is_some_and(|hold| hold.confirmed),
            None => false,
        }
    }

    /// Records the new effective direction and its confirmation, returning
    /// the event to send if either changed (including a change *to*
    /// stopped, or a tap becoming a confirmed full-speed hold).
    fn update_current(&mut self, side: Side) -> Option<InputEvent> {
        let new = self.effective();
        let new_held = self.held_of(new);
        if new == self.current && new_held == self.current_held {
            return None;
        }
        self.current = new;
        self.current_held = new_held;
        Some(InputEvent::SetPaddleDirection {
            side,
            direction: new,
            held: new_held,
        })
    }
}

/// Tracks which movement keys are currently held.
///
/// One instance lives in the frontend loop: every key event goes through
/// [`InputState::handle_key`], and every frame [`InputState::sweep`]
/// expires keys that went silent.
#[derive(Debug, Default)]
pub struct InputState {
    left: SideState,
    right: SideState,
    /// AI matches: the arrow keys steer the left paddle (the right one
    /// belongs to the backend).
    arrows_are_left: bool,
}

impl InputState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Reconfigures the model for a fresh match: drops all held-key
    /// bookkeeping and remaps the arrow keys per the opponent. The
    /// backend clears its own paddle state on `StartMatch`, so no stop
    /// events need to be sent here.
    pub fn start_match(&mut self, opponent: Opponent) {
        self.left = SideState::default();
        self.right = SideState::default();
        self.arrows_are_left = matches!(opponent, Opponent::Ai(_));
    }

    fn side_mut(&mut self, side: Side) -> &mut SideState {
        match side {
            Side::Left => &mut self.left,
            Side::Right => &mut self.right,
        }
    }

    /// Updates the model from one key event and returns the resulting
    /// action, if any. `None` means the event needs no backend traffic
    /// (repeats of an already-held key, unrelated keys).
    pub fn handle_key(&mut self, key: KeyEvent, now: Instant) -> Option<Action> {
        let pressed = !matches!(key.kind, KeyEventKind::Release);

        if key.modifiers.contains(KeyModifiers::CONTROL) {
            return match key.code {
                KeyCode::Char('c') if pressed => Some(Action::Quit),
                _ => None,
            };
        }

        let arrow_side = if self.arrows_are_left {
            Side::Left
        } else {
            Side::Right
        };
        let movement = match key.code {
            KeyCode::Up => Some((arrow_side, Direction::Up)),
            KeyCode::Down => Some((arrow_side, Direction::Down)),
            KeyCode::Char(c) => match c.to_ascii_lowercase() {
                'w' => Some((Side::Left, Direction::Up)),
                's' => Some((Side::Left, Direction::Down)),
                _ => None,
            },
            _ => None,
        };

        let Some((side, direction)) = movement else {
            if !pressed {
                return None;
            }
            return match key.code {
                KeyCode::Char('r') => Some(Action::Send(InputEvent::Restart)),
                KeyCode::Char('m') => Some(Action::Menu),
                KeyCode::Char('q') | KeyCode::Esc => Some(Action::Quit),
                _ => None,
            };
        };

        let side_state = self.side_mut(side);
        let slot = match direction {
            Direction::Up => &mut side_state.up,
            Direction::Down => &mut side_state.down,
        };
        if pressed {
            // A new event for a key already tracked means auto-repeat, and
            // so does an explicit `Repeat` kind: either way the hold is
            // confirmed.
            let was_held = slot.is_some();
            if let Some(hold) = slot.as_mut() {
                // Measure the rhythm only between confirmed arrivals: the
                // first gap after the press is the OS repeat delay.
                if hold.confirmed {
                    hold.repeat_interval =
                        fold_interval(hold.repeat_interval, now.duration_since(hold.last_seen));
                }
                hold.last_seen = now;
                hold.confirmed = true;
            } else {
                *slot = Some(KeyHold {
                    last_seen: now,
                    confirmed: matches!(key.kind, KeyEventKind::Repeat),
                    repeat_interval: None,
                });
            }
            // Only genuine presses update the tie-breaker: interleaved
            // repeat streams must not flip the winner back and forth.
            if !was_held {
                side_state.last_pressed = Some(direction);
            }
        } else {
            // A terminal that does report releases (kitty protocol).
            *slot = None;
            if side_state.last_pressed == Some(direction) {
                side_state.last_pressed = side_state.effective();
            }
        }
        side_state.update_current(side).map(Action::Send)
    }

    /// Expires keys whose auto-repeat stream went silent. Call once per
    /// frame; returns the direction updates to send.
    pub fn sweep(&mut self, now: Instant) -> Vec<Action> {
        let mut actions = Vec::new();
        for (side, side_state) in [(Side::Left, &mut self.left), (Side::Right, &mut self.right)] {
            let up_expired = side_state.up.as_ref().is_some_and(|hold| hold.expired(now));
            if up_expired {
                side_state.up = None;
            }
            let down_expired = side_state
                .down
                .as_ref()
                .is_some_and(|hold| hold.expired(now));
            if down_expired {
                side_state.down = None;
            }
            let last_pressed_expired = match side_state.last_pressed {
                Some(Direction::Up) => up_expired,
                Some(Direction::Down) => down_expired,
                None => false,
            };
            if last_pressed_expired {
                side_state.last_pressed = side_state.effective();
            }
            if let Some(event) = side_state.update_current(side) {
                actions.push(Action::Send(event));
            }
        }
        actions
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};
    use pong_core::Difficulty;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        }
    }

    fn release(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Release,
            state: KeyEventState::NONE,
        }
    }

    fn repeat(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::NONE,
            kind: KeyEventKind::Repeat,
            state: KeyEventState::NONE,
        }
    }

    /// Milliseconds after `base`.
    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    fn send(side: Side, direction: Option<Direction>, held: bool) -> Action {
        Action::Send(InputEvent::SetPaddleDirection {
            side,
            direction,
            held,
        })
    }

    /// The full press → silence → stop cycle of a quick tap.
    #[test]
    fn tap_moves_then_expires() {
        let base = Instant::now();
        let mut state = InputState::new();
        assert_eq!(
            state.handle_key(key(KeyCode::Char('w')), at(base, 0)),
            Some(send(Side::Left, Some(Direction::Up), false))
        );
        assert!(state.sweep(at(base, 100)).is_empty());
        assert!(
            state.sweep(at(base, 550)).is_empty(),
            "still inside the hold-start window"
        );
        assert_eq!(
            state.sweep(at(base, 610)),
            vec![send(Side::Left, None, false)]
        );
    }

    #[test]
    fn press_moves_and_silence_stops() {
        let base = Instant::now();
        let mut state = InputState::new();
        state.handle_key(key(KeyCode::Down), at(base, 0));
        assert_eq!(
            state.sweep(at(base, 601)),
            vec![send(Side::Right, None, false)]
        );
    }

    /// The first auto-repeat confirms the hold: the same direction is
    /// re-sent with the full-speed flag instead of the tap flag.
    #[test]
    fn confirming_repeat_upgrades_to_full_speed() {
        let base = Instant::now();
        let mut state = InputState::new();
        assert_eq!(
            state.handle_key(key(KeyCode::Char('w')), at(base, 0)),
            Some(send(Side::Left, Some(Direction::Up), false))
        );
        assert_eq!(
            state.handle_key(key(KeyCode::Char('w')), at(base, 500)),
            Some(send(Side::Left, Some(Direction::Up), true))
        );
        assert!(state.sweep(at(base, 550)).is_empty(), "still confirmed");
        // Stream goes silent: stop.
        assert_eq!(
            state.sweep(at(base, 700)),
            vec![send(Side::Left, None, false)]
        );
    }

    /// A flowing repeat stream keeps the key alive; its silence stops it.
    #[test]
    fn repeat_stream_keeps_key_alive_then_silence_stops() {
        let base = Instant::now();
        let mut state = InputState::new();
        // Press at 0, then the OS repeat delay passes and events flow
        // every ~33 ms (delivered as fresh presses on legacy terminals).
        state.handle_key(key(KeyCode::Char('s')), at(base, 0));
        state.handle_key(key(KeyCode::Char('s')), at(base, 500));
        state.handle_key(key(KeyCode::Char('s')), at(base, 533));
        state.handle_key(key(KeyCode::Char('s')), at(base, 566));
        // The measured 33 ms rhythm tightens the window to ~82.5 ms, so
        // this stream is alive only while events keep flowing.
        assert!(state.sweep(at(base, 640)).is_empty(), "still repeating");
        assert_eq!(
            state.sweep(at(base, 660)),
            vec![send(Side::Left, None, false)]
        );
    }

    /// The release window tightens to the stream's measured rhythm:
    /// two 30 ms samples give a 75 ms window — 90 ms of silence stops.
    #[test]
    fn release_window_adapts_to_the_repeat_rhythm() {
        let base = Instant::now();
        let mut state = InputState::new();
        state.handle_key(key(KeyCode::Char('w')), at(base, 0));
        state.handle_key(key(KeyCode::Char('w')), at(base, 500));
        state.handle_key(key(KeyCode::Char('w')), at(base, 530));
        state.handle_key(key(KeyCode::Char('w')), at(base, 560));
        assert!(
            state.sweep(at(base, 620)).is_empty(),
            "60 ms of silence is within the 75 ms window"
        );
        assert_eq!(
            state.sweep(at(base, 650)),
            vec![send(Side::Left, None, false)]
        );
    }

    /// A slow repeat stream (10 Hz) would want a 250 ms window; the
    /// ceiling keeps it at the conservative 150 ms.
    #[test]
    fn slow_repeat_stream_gets_the_conservative_window() {
        let base = Instant::now();
        let mut state = InputState::new();
        state.handle_key(key(KeyCode::Char('w')), at(base, 0));
        state.handle_key(key(KeyCode::Char('w')), at(base, 500));
        state.handle_key(key(KeyCode::Char('w')), at(base, 600));
        state.handle_key(key(KeyCode::Char('w')), at(base, 700));
        assert!(
            state.sweep(at(base, 840)).is_empty(),
            "140 ms of silence is within the clamped 150 ms window"
        );
        assert_eq!(
            state.sweep(at(base, 860)),
            vec![send(Side::Left, None, false)]
        );
    }

    /// A pathologically fast measurement must not shrink the window
    /// below the floor (60 ms), or stream jitter would false-release.
    #[test]
    fn fast_estimate_never_shrinks_below_the_floor() {
        let base = Instant::now();
        let mut state = InputState::new();
        state.handle_key(key(KeyCode::Char('w')), at(base, 0));
        state.handle_key(key(KeyCode::Char('w')), at(base, 500));
        state.handle_key(key(KeyCode::Char('w')), at(base, 520));
        state.handle_key(key(KeyCode::Char('w')), at(base, 540));
        assert!(
            state.sweep(at(base, 590)).is_empty(),
            "50 ms of silence is within the 60 ms floor"
        );
        assert_eq!(
            state.sweep(at(base, 615)),
            vec![send(Side::Left, None, false)]
        );
    }

    /// The OS repeat delay (~500 ms) between press and first repeat is
    /// not the rhythm: it must not be folded into the estimate, or the
    /// window would stay at the 150 ms ceiling forever.
    #[test]
    fn initial_repeat_delay_is_not_measured_as_rhythm() {
        let base = Instant::now();
        let mut state = InputState::new();
        state.handle_key(key(KeyCode::Char('w')), at(base, 0));
        state.handle_key(key(KeyCode::Char('w')), at(base, 500));
        state.handle_key(key(KeyCode::Char('w')), at(base, 530));
        state.handle_key(key(KeyCode::Char('w')), at(base, 560));
        // Rhythm is 30 ms → window 75 ms: 100 ms of silence must stop
        // the paddle. Had the 500 ms delay polluted the estimate, the
        // window would still sit at the 150 ms ceiling here.
        assert_eq!(
            state.sweep(at(base, 660)),
            vec![send(Side::Left, None, false)]
        );
    }

    /// Terminals that do report releases stop the key immediately.
    #[test]
    fn release_event_clears_immediately() {
        let base = Instant::now();
        let mut state = InputState::new();
        state.handle_key(key(KeyCode::Char('w')), at(base, 0));
        assert_eq!(
            state.handle_key(release(KeyCode::Char('w')), at(base, 10)),
            Some(send(Side::Left, None, false))
        );
        assert!(state.sweep(at(base, 20)).is_empty());
    }

    #[test]
    fn both_held_last_pressed_wins_and_release_falls_back() {
        let base = Instant::now();
        let mut state = InputState::new();
        state.handle_key(key(KeyCode::Char('w')), at(base, 0));
        assert_eq!(
            state.handle_key(key(KeyCode::Char('s')), at(base, 100)),
            Some(send(Side::Left, Some(Direction::Down), false))
        );
        assert_eq!(
            state.handle_key(release(KeyCode::Char('s')), at(base, 150)),
            Some(send(Side::Left, Some(Direction::Up), false))
        );
        assert_eq!(
            state.handle_key(release(KeyCode::Char('w')), at(base, 200)),
            Some(send(Side::Left, None, false))
        );
    }

    /// Legacy terminals have no releases: an expired tie-breaker key
    /// falls back to the still-repeating one.
    #[test]
    fn expired_tie_breaker_falls_back_to_the_still_repeating_key() {
        let base = Instant::now();
        let mut state = InputState::new();
        state.handle_key(key(KeyCode::Char('w')), at(base, 0));
        // Pressed last, so s wins the tie — then goes silent.
        state.handle_key(key(KeyCode::Char('s')), at(base, 100));
        // w keeps auto-repeating.
        for t in [500, 530, 560, 590, 620, 650, 680] {
            state.handle_key(key(KeyCode::Char('w')), at(base, t));
        }
        // s expired (610 ms silent), w alive (30 ms).
        assert_eq!(
            state.sweep(at(base, 710)),
            vec![send(Side::Left, Some(Direction::Up), true)]
        );
    }

    /// A dropped key (e.g. after a rendering stall) re-establishes
    /// itself when its repeat events resume.
    #[test]
    fn key_re_establishes_after_a_false_expiry() {
        let base = Instant::now();
        let mut state = InputState::new();
        state.handle_key(key(KeyCode::Char('w')), at(base, 0));
        state.handle_key(key(KeyCode::Char('w')), at(base, 500));
        state.handle_key(key(KeyCode::Char('w')), at(base, 530));
        state.handle_key(key(KeyCode::Char('w')), at(base, 560));
        assert_eq!(
            state.sweep(at(base, 720)),
            vec![send(Side::Left, None, false)]
        );
        // The repeat event arrives late but the key is still held.
        assert_eq!(
            state.handle_key(key(KeyCode::Char('w')), at(base, 725)),
            Some(send(Side::Left, Some(Direction::Up), false))
        );
        assert!(state.sweep(at(base, 740)).is_empty());
    }

    /// Terminals encoding repeats explicitly get the short timeout from
    /// the very first event.
    #[test]
    fn repeat_kind_event_establishes_a_repeating_hold() {
        let base = Instant::now();
        let mut state = InputState::new();
        assert_eq!(
            state.handle_key(repeat(KeyCode::Char('s')), at(base, 0)),
            Some(send(Side::Left, Some(Direction::Down), true))
        );
        assert!(state.sweep(at(base, 100)).is_empty());
        assert_eq!(
            state.sweep(at(base, 200)),
            vec![send(Side::Left, None, false)]
        );
    }

    #[test]
    fn sides_expire_independently() {
        let base = Instant::now();
        let mut state = InputState::new();
        state.handle_key(key(KeyCode::Char('w')), at(base, 0));
        state.handle_key(key(KeyCode::Up), at(base, 0));
        assert_eq!(
            state.sweep(at(base, 610)),
            vec![
                send(Side::Left, None, false),
                send(Side::Right, None, false),
            ]
        );
    }

    #[test]
    fn sweep_with_fresh_keys_sends_nothing() {
        let base = Instant::now();
        let mut state = InputState::new();
        state.handle_key(key(KeyCode::Char('s')), at(base, 0));
        assert!(state.sweep(at(base, 50)).is_empty());
    }

    #[test]
    fn release_of_an_unheld_key_changes_nothing() {
        let base = Instant::now();
        let mut state = InputState::new();
        assert_eq!(state.handle_key(release(KeyCode::Down), at(base, 0)), None);
        assert!(state.sweep(at(base, 700)).is_empty());
    }

    #[test]
    fn shifted_movement_keys_still_work() {
        let base = Instant::now();
        let mut state = InputState::new();
        assert_eq!(
            state.handle_key(key(KeyCode::Char('W')), at(base, 0)),
            Some(send(Side::Left, Some(Direction::Up), false))
        );
    }

    #[test]
    fn quit_and_restart_only_fire_on_press() {
        let base = Instant::now();
        let mut state = InputState::new();
        assert_eq!(
            state.handle_key(key(KeyCode::Char('q')), at(base, 0)),
            Some(Action::Quit)
        );
        assert_eq!(
            state.handle_key(key(KeyCode::Esc), at(base, 0)),
            Some(Action::Quit)
        );
        assert_eq!(
            state.handle_key(key(KeyCode::Char('r')), at(base, 0)),
            Some(Action::Send(InputEvent::Restart))
        );
        assert_eq!(
            state.handle_key(release(KeyCode::Char('q')), at(base, 0)),
            None
        );
        assert_eq!(
            state.handle_key(release(KeyCode::Char('r')), at(base, 0)),
            None
        );
    }

    #[test]
    fn control_c_quits_other_control_combos_ignored() {
        let base = Instant::now();
        let mut state = InputState::new();
        let ctrl_c = KeyEvent {
            code: KeyCode::Char('c'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        let ctrl_s = KeyEvent {
            code: KeyCode::Char('s'),
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: KeyEventState::NONE,
        };
        assert_eq!(state.handle_key(ctrl_c, at(base, 0)), Some(Action::Quit));
        assert_eq!(state.handle_key(ctrl_s, at(base, 0)), None);
    }

    #[test]
    fn unrelated_keys_are_ignored() {
        let base = Instant::now();
        let mut state = InputState::new();
        assert_eq!(state.handle_key(key(KeyCode::Char('x')), at(base, 0)), None);
        assert_eq!(state.handle_key(key(KeyCode::F(5)), at(base, 0)), None);
        assert_eq!(state.handle_key(key(KeyCode::Enter), at(base, 0)), None);
    }

    #[test]
    fn m_opens_the_menu_on_press_only() {
        let base = Instant::now();
        let mut state = InputState::new();
        assert_eq!(
            state.handle_key(key(KeyCode::Char('m')), at(base, 0)),
            Some(Action::Menu)
        );
        assert_eq!(
            state.handle_key(release(KeyCode::Char('m')), at(base, 0)),
            None
        );
    }

    /// AI matches remap the arrow keys to the left paddle: the right
    /// one belongs to the backend.
    #[test]
    fn arrow_keys_steer_the_left_paddle_in_ai_matches() {
        let base = Instant::now();
        let mut state = InputState::new();
        state.start_match(Opponent::Ai(Difficulty::Easy));
        assert_eq!(
            state.handle_key(key(KeyCode::Up), at(base, 0)),
            Some(send(Side::Left, Some(Direction::Up), false))
        );
        assert_eq!(
            state.handle_key(key(KeyCode::Down), at(base, 10)),
            Some(send(Side::Left, Some(Direction::Down), false))
        );
    }

    /// Two-player matches keep the phase-2 mapping: arrows are the
    /// right paddle's.
    #[test]
    fn arrow_keys_stay_on_the_right_paddle_in_two_player_matches() {
        let base = Instant::now();
        let mut state = InputState::new();
        state.start_match(Opponent::TwoPlayer);
        assert_eq!(
            state.handle_key(key(KeyCode::Up), at(base, 0)),
            Some(send(Side::Right, Some(Direction::Up), false))
        );
    }

    /// Starting a match drops stale held-key bookkeeping, but the
    /// backend is told nothing: its own `StartMatch` resets paddles.
    #[test]
    fn start_match_clears_held_keys_silently() {
        let base = Instant::now();
        let mut state = InputState::new();
        state.handle_key(key(KeyCode::Char('w')), at(base, 0));
        state.handle_key(key(KeyCode::Up), at(base, 0));
        state.start_match(Opponent::Ai(Difficulty::Hard));
        // Nothing held anymore: a long silence sweeps nothing.
        assert!(state.sweep(at(base, 10_000)).is_empty());
        // A fresh press works normally (and lands on the left side).
        assert_eq!(
            state.handle_key(key(KeyCode::Up), at(base, 10_050)),
            Some(send(Side::Left, Some(Direction::Up), false))
        );
    }
}
