//! Maps crossterm key events to backend input events.
//!
//! The terminal input model is **edge-triggered**: terminals report key
//! presses but usually no releases, so a paddle keeps moving in the last
//! pressed direction until the opposite key is pressed.

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use pong_core::{Direction, InputEvent, Side};

/// What the frontend should do with a key event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Forward this event to the backend.
    Send(InputEvent),
    /// Stop the frontend loop (the backend shuts down via `Drop`).
    Quit,
    /// Ignore this key.
    Ignore,
}

/// Translates one key event into an [`Action`].
pub fn map_key(key: KeyEvent) -> Action {
    if !matches!(key.kind, KeyEventKind::Press | KeyEventKind::Repeat) {
        return Action::Ignore;
    }
    if key.modifiers.contains(KeyModifiers::CONTROL) {
        return match key.code {
            KeyCode::Char('c') => Action::Quit,
            _ => Action::Ignore,
        };
    }
    match key.code {
        KeyCode::Up => paddle(Side::Right, Direction::Up),
        KeyCode::Down => paddle(Side::Right, Direction::Down),
        KeyCode::Char(c) => match c.to_ascii_lowercase() {
            'w' => paddle(Side::Left, Direction::Up),
            's' => paddle(Side::Left, Direction::Down),
            'r' => Action::Send(InputEvent::Restart),
            'q' => Action::Quit,
            _ => Action::Ignore,
        },
        KeyCode::Esc => Action::Quit,
        _ => Action::Ignore,
    }
}

fn paddle(side: Side, direction: Direction) -> Action {
    Action::Send(InputEvent::SetPaddleDirection {
        side,
        direction: Some(direction),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

    fn key(code: KeyCode) -> KeyEvent {
        key_with(code, KeyModifiers::NONE, KeyEventKind::Press)
    }

    fn key_with(code: KeyCode, modifiers: KeyModifiers, kind: KeyEventKind) -> KeyEvent {
        KeyEvent {
            code,
            modifiers,
            kind,
            state: KeyEventState::NONE,
        }
    }

    #[test]
    fn movement_keys_map_to_paddle_directions() {
        assert_eq!(
            map_key(key(KeyCode::Char('w'))),
            Action::Send(InputEvent::SetPaddleDirection {
                side: Side::Left,
                direction: Some(Direction::Up)
            })
        );
        assert_eq!(
            map_key(key(KeyCode::Char('s'))),
            Action::Send(InputEvent::SetPaddleDirection {
                side: Side::Left,
                direction: Some(Direction::Down)
            })
        );
        assert_eq!(
            map_key(key(KeyCode::Up)),
            Action::Send(InputEvent::SetPaddleDirection {
                side: Side::Right,
                direction: Some(Direction::Up)
            })
        );
        assert_eq!(
            map_key(key(KeyCode::Down)),
            Action::Send(InputEvent::SetPaddleDirection {
                side: Side::Right,
                direction: Some(Direction::Down)
            })
        );
    }

    #[test]
    fn shifted_movement_keys_still_work() {
        assert_eq!(
            map_key(key(KeyCode::Char('W'))),
            Action::Send(InputEvent::SetPaddleDirection {
                side: Side::Left,
                direction: Some(Direction::Up)
            })
        );
    }

    #[test]
    fn control_c_quits() {
        assert_eq!(
            map_key(key_with(
                KeyCode::Char('c'),
                KeyModifiers::CONTROL,
                KeyEventKind::Press
            )),
            Action::Quit
        );
    }

    #[test]
    fn other_control_combos_are_ignored() {
        assert_eq!(
            map_key(key_with(
                KeyCode::Char('s'),
                KeyModifiers::CONTROL,
                KeyEventKind::Press
            )),
            Action::Ignore
        );
    }

    #[test]
    fn quit_and_restart_keys() {
        assert_eq!(map_key(key(KeyCode::Char('q'))), Action::Quit);
        assert_eq!(map_key(key(KeyCode::Esc)), Action::Quit);
        assert_eq!(
            map_key(key(KeyCode::Char('r'))),
            Action::Send(InputEvent::Restart)
        );
    }

    #[test]
    fn repeat_events_are_mapped_like_presses() {
        assert_eq!(
            map_key(key_with(
                KeyCode::Char('w'),
                KeyModifiers::NONE,
                KeyEventKind::Repeat
            )),
            Action::Send(InputEvent::SetPaddleDirection {
                side: Side::Left,
                direction: Some(Direction::Up)
            })
        );
    }

    #[test]
    fn release_events_are_ignored() {
        assert_eq!(
            map_key(key_with(
                KeyCode::Char('w'),
                KeyModifiers::NONE,
                KeyEventKind::Release
            )),
            Action::Ignore
        );
    }

    #[test]
    fn unrelated_keys_are_ignored() {
        assert_eq!(map_key(key(KeyCode::Char('x'))), Action::Ignore);
        assert_eq!(map_key(key(KeyCode::F(5))), Action::Ignore);
        assert_eq!(map_key(key(KeyCode::Enter)), Action::Ignore);
    }
}
