//! The start menu: chooses the opponent and the ball speed mode.
//!
//! Pure frontend state machine — the backend knows nothing about it
//! until the player confirms, at which point the resulting
//! [`GameOptions`] travel to it as one [`InputEvent::StartMatch`]
//! (see ARCHITECTURE.md).

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind};
use pong_core::{BallSpeedMode, Difficulty, GameOptions, InputEvent, Opponent};
use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::Style;
use ratatui::symbols::border;
use ratatui::text::Line;
use ratatui::widgets::{Block, Paragraph};

/// What the main loop should do after a menu key press.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuAction {
    /// The player confirmed a configuration; forward it and switch to
    /// the match. The action to send is ready-made.
    Start(InputEvent),
    /// Leave the application.
    Quit,
}

/// The four opponents, in cycling order.
const OPPONENTS: [Opponent; 4] = [
    Opponent::TwoPlayer,
    Opponent::Ai(Difficulty::Easy),
    Opponent::Ai(Difficulty::Normal),
    Opponent::Ai(Difficulty::Hard),
];

/// The three ball speed modes, in cycling order.
const BALL_SPEEDS: [BallSpeedMode; 3] = [
    BallSpeedMode::Slow,
    BallSpeedMode::Fast,
    BallSpeedMode::Mutable,
];

/// All UI text is English (project rule: the interface stays ASCII-safe
/// English; Chinese is for docs only).
const OPPONENT_LABELS: [&str; 4] = ["2 players", "AI · easy", "AI · normal", "AI · hard"];
const BALL_SPEED_LABELS: [&str; 3] = ["slow", "fast", "accelerating"];

/// The giant title logo, in the tradition of Angband's splash screens:
/// solid blocks spelling PONG, seven rows tall. [`Menu::draw`] doubles
/// it horizontally when the terminal is wide enough — the marquee look.
const LOGO: [&str; 7] = [
    "██████    █████   █     █   █████ ",
    "█     █  █     █  ██    █  █      ",
    "█     █  █     █  █ █   █  █    ██",
    "██████   █     █  █  █  █  █     █",
    "█        █     █  █   █ █  █     █",
    "█        █     █  █    ██  █     █",
    "█         █████   █     █   █████ ",
];

/// Width of the options box — wide enough for the longest hint line.
const MENU_BOX_WIDTH: u16 = 50;

/// Menu rows, in display order.
const ROWS: usize = 2;

/// The start menu's selection state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Menu {
    /// Which row is selected: 0 = opponent, 1 = ball speed.
    selected: usize,
    /// Index into [`OPPONENTS`].
    opponent: usize,
    /// Index into [`BALL_SPEEDS`].
    ball_speed: usize,
}

impl Menu {
    pub fn new() -> Self {
        Self {
            selected: 0,
            // The phase-2 defaults: two humans, fast ball.
            opponent: 0,
            ball_speed: 1,
        }
    }

    /// The configuration currently shown.
    pub fn options(&self) -> GameOptions {
        GameOptions {
            opponent: OPPONENTS[self.opponent],
            ball_speed: BALL_SPEEDS[self.ball_speed],
        }
    }

    /// Handles one key event; `Some` means the menu is done.
    pub fn handle_key(&mut self, key: KeyEvent) -> Option<MenuAction> {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('w') => {
                self.selected = (self.selected + ROWS - 1) % ROWS;
                None
            }
            KeyCode::Down | KeyCode::Char('s') => {
                self.selected = (self.selected + 1) % ROWS;
                None
            }
            KeyCode::Left | KeyCode::Char('a') => {
                self.cycle(-1);
                None
            }
            KeyCode::Right | KeyCode::Char('d') => {
                self.cycle(1);
                None
            }
            KeyCode::Enter => Some(MenuAction::Start(InputEvent::StartMatch(self.options()))),
            KeyCode::Char('q') | KeyCode::Esc => Some(MenuAction::Quit),
            _ => None,
        }
    }

    /// Moves the selected row's value by `delta` (wraps around).
    fn cycle(&mut self, delta: i32) {
        match self.selected {
            0 => {
                let n = OPPONENTS.len() as i32;
                self.opponent = ((self.opponent as i32 + delta + n) % n) as usize;
            }
            _ => {
                let n = BALL_SPEEDS.len() as i32;
                self.ball_speed = ((self.ball_speed as i32 + delta + n) % n) as usize;
            }
        }
    }

    /// Draws the menu: the giant block logo above a bordered options
    /// box, the whole group vertically centered. On terminals narrower
    /// than the doubled logo the title falls back to single width, and
    /// below that it simply clips — the menu stays usable at any size.
    pub fn draw(&self, frame: &mut Frame<'_>) {
        let area = frame.area();
        const LOGO_HEIGHT: u16 = LOGO.len() as u16;
        const GAP: u16 = 1;
        const BOX_WIDTH: u16 = MENU_BOX_WIDTH;
        const BOX_HEIGHT: u16 = 7; // 2 border + 5 content rows
        let group_height = LOGO_HEIGHT + GAP + BOX_HEIGHT;
        let top = area.y + area.height.saturating_sub(group_height) / 2;

        draw_logo(frame, top);

        // The box is pinned right below the logo (the group as a whole
        // is already centered) and centered horizontally on its own.
        let box_top = top.saturating_add(LOGO_HEIGHT + GAP);
        let box_area = Rect {
            x: area.x + area.width.saturating_sub(BOX_WIDTH) / 2,
            y: box_top,
            width: BOX_WIDTH.min(area.width),
            height: BOX_HEIGHT.min(area.bottom().saturating_sub(box_top)),
        };
        let rows = [
            self.row(
                "Opponent",
                OPPONENT_LABELS[self.opponent],
                self.selected == 0,
            ),
            self.row(
                "Ball speed",
                BALL_SPEED_LABELS[self.ball_speed],
                self.selected == 1,
            ),
        ];
        let hint = Line::from("↑/↓ select · ←/→ change · Enter start · q quit");
        frame.render_widget(
            Paragraph::new(vec![
                Line::from(""),
                rows[0].clone(),
                rows[1].clone(),
                Line::from(""),
                hint,
            ])
            .block(Block::bordered().border_set(border::PLAIN)),
            box_area,
        );
    }

    /// One menu row: marker and label on the left, the value flush
    /// against the right edge. The selected and unselected value texts
    /// are the same width (`◀ v ▶` vs `  v  `), so moving the selection
    /// never shifts the value — only the markers appear around it.
    fn row(&self, label: &str, value: &str, selected: bool) -> Line<'static> {
        // The box's inner width: MENU_BOX_WIDTH minus its two borders.
        const CONTENT_WIDTH: usize = (MENU_BOX_WIDTH - 2) as usize;
        let (marker, value_text) = if selected {
            ("❯", format!("◀ {value} ▶"))
        } else {
            (" ", format!("  {value}  "))
        };
        let left = format!(" {marker} {label}");
        let pad =
            CONTENT_WIDTH.saturating_sub(left.chars().count() + value_text.chars().count() + 1);
        let mut text = left;
        text.extend(std::iter::repeat_n(' ', pad));
        text.push_str(&value_text);
        text.push(' ');
        if selected {
            Line::from(text).style(Style::new().reversed())
        } else {
            Line::from(text)
        }
    }
}

impl Default for Menu {
    fn default() -> Self {
        Self::new()
    }
}

/// Renders the giant logo, each cell doubled horizontally when the
/// terminal is wide enough for the marquee look.
fn draw_logo(frame: &mut Frame<'_>, top: u16) {
    let area = frame.area();
    let doubled = area.width >= 2 * LOGO[0].chars().count() as u16 + 2;
    for (offset, row) in LOGO.iter().enumerate() {
        let Some(y) = top.checked_add(offset as u16) else {
            break;
        };
        if y >= area.bottom() {
            break;
        }
        let line = if doubled {
            row.chars().flat_map(|c| [c, c]).collect::<String>()
        } else {
            (*row).to_string()
        };
        frame.render_widget(
            Paragraph::new(Line::from(line)).alignment(Alignment::Center),
            Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            },
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::{KeyEvent, KeyEventKind, KeyEventState, KeyModifiers};

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

    /// Values sit flush against the box's right edge, and moving the
    /// selection never shifts them — only the ◀ ▶ markers appear.
    #[test]
    fn rows_right_align_their_values_without_selection_jumps() {
        // Column (character offset) of `needle` in `s` — byte offsets
        // would skew on the multi-byte markers.
        fn col(s: &str, needle: &str) -> usize {
            let byte = s.find(needle).expect("needle present");
            s[..byte].chars().count()
        }

        let menu = Menu::new();
        let selected = menu.row("Ball speed", "accelerating", true);
        let plain = menu.row("Ball speed", "accelerating", false);
        let sel: String = selected.spans[0].content.to_string();
        let pln: String = plain.spans[0].content.to_string();
        // Both rows fill the box's content width exactly.
        assert_eq!(sel.chars().count(), (MENU_BOX_WIDTH - 2) as usize);
        assert_eq!(sel.chars().count(), pln.chars().count());
        // The value itself sits at the same column in both forms.
        assert_eq!(col(&sel, "accelerating"), col(&pln, "accelerating"));
        assert!(sel.starts_with(" ❯ Ball speed"));
        assert!(pln.starts_with("   Ball speed"));
        // Different labels still end their values at the same edge.
        let opponent = menu.row("Opponent", "AI · normal", false);
        let opp: String = opponent.spans[0].content.to_string();
        assert_eq!(
            sel.chars().count() - col(&sel, "accelerating"),
            opp.chars().count() - col(&opp, "AI · normal") + 1,
            "values end one column apart: rows are flush right"
        );
    }

    /// The logo must stay rectangular (hand-drawn art rots silently
    /// otherwise) and consist of nothing but blocks and spaces.
    #[test]
    fn logo_is_rectangular_blocks_only() {
        assert_eq!(LOGO.len(), 7);
        let width = LOGO[0].chars().count();
        assert!(width >= 30, "the logo must be grand, got {width} cols");
        for row in LOGO {
            assert_eq!(row.chars().count(), width, "row {row:?}");
            assert!(
                row.chars().all(|c| c == '█' || c == ' '),
                "row {row:?} has non-block characters"
            );
        }
    }

    /// The menu renders without panicking at any terminal size, from a
    /// sliver to a cinema screen.
    #[test]
    fn menu_renders_at_every_terminal_size() {
        let menu = Menu::new();
        for (width, height) in [
            (0u16, 0u16),
            (10, 3),
            (20, 10),
            (36, 12),
            (40, 24),
            (68, 20),
            (120, 40),
            (200, 60),
        ] {
            let backend = ratatui::backend::TestBackend::new(width, height);
            let mut terminal = ratatui::Terminal::new(backend).unwrap();
            terminal.draw(|frame| menu.draw(frame)).unwrap();
        }
    }

    #[test]
    fn defaults_match_the_phase2_configuration() {
        let menu = Menu::new();
        assert_eq!(
            menu.options(),
            GameOptions {
                opponent: Opponent::TwoPlayer,
                ball_speed: BallSpeedMode::Fast,
            }
        );
    }

    #[test]
    fn enter_returns_a_start_match_event() {
        let mut menu = Menu::new();
        assert_eq!(
            menu.handle_key(key(KeyCode::Enter)),
            Some(MenuAction::Start(InputEvent::StartMatch(
                GameOptions::default()
            )))
        );
    }

    #[test]
    fn q_quits_and_releases_are_ignored() {
        let mut menu = Menu::new();
        assert_eq!(
            menu.handle_key(key(KeyCode::Char('q'))),
            Some(MenuAction::Quit)
        );
        assert_eq!(menu.handle_key(key(KeyCode::Esc)), Some(MenuAction::Quit));
        assert_eq!(menu.handle_key(release(KeyCode::Char('q'))), None);
        assert_eq!(menu.handle_key(release(KeyCode::Enter)), None);
    }

    /// Down moves to the ball-speed row; changing it there cycles the
    /// speed, not the opponent.
    #[test]
    fn row_selection_routes_value_changes() {
        let mut menu = Menu::new();
        menu.handle_key(key(KeyCode::Down));
        menu.handle_key(key(KeyCode::Right));
        assert_eq!(menu.options().ball_speed, BallSpeedMode::Mutable);
        menu.handle_key(key(KeyCode::Left));
        assert_eq!(menu.options().ball_speed, BallSpeedMode::Fast);
        // The opponent is untouched.
        assert_eq!(menu.options().opponent, Opponent::TwoPlayer);
    }

    #[test]
    fn selection_wraps_around_both_rows() {
        let mut menu = Menu::new();
        menu.handle_key(key(KeyCode::Up)); // wraps down to the last row
        menu.handle_key(key(KeyCode::Right)); // ...which is the speed
        assert_eq!(menu.options().ball_speed, BallSpeedMode::Mutable);
        menu.handle_key(key(KeyCode::Down)); // wraps back up to row 0
        menu.handle_key(key(KeyCode::Right)); // ...which is the opponent
        assert_eq!(menu.options().opponent, Opponent::Ai(Difficulty::Easy));
    }

    /// Values wrap in both directions: left from the first opponent
    /// reaches the last one.
    #[test]
    fn values_wrap_in_both_directions() {
        let mut menu = Menu::new();
        menu.handle_key(key(KeyCode::Left));
        assert_eq!(menu.options().opponent, Opponent::Ai(Difficulty::Hard));
        menu.handle_key(key(KeyCode::Right));
        assert_eq!(menu.options().opponent, Opponent::TwoPlayer);
    }

    #[test]
    fn all_opponents_and_speeds_are_reachable() {
        let mut menu = Menu::new();
        for expected in OPPONENTS {
            assert_eq!(menu.options().opponent, expected);
            menu.handle_key(key(KeyCode::Right));
        }
        menu.handle_key(key(KeyCode::Down));
        // The menu starts on Fast; cycling right walks all three.
        for expected in [
            BallSpeedMode::Fast,
            BallSpeedMode::Mutable,
            BallSpeedMode::Slow,
        ] {
            assert_eq!(menu.options().ball_speed, expected);
            menu.handle_key(key(KeyCode::Right));
        }
    }

    /// The wasd aliases work like the arrows.
    #[test]
    fn wasd_aliases_drive_the_menu() {
        let mut menu = Menu::new();
        menu.handle_key(key(KeyCode::Char('s'))); // down to speed
        menu.handle_key(key(KeyCode::Char('d'))); // right
        assert_eq!(menu.options().ball_speed, BallSpeedMode::Mutable);
        menu.handle_key(key(KeyCode::Char('a'))); // left
        assert_eq!(menu.options().ball_speed, BallSpeedMode::Fast);
        menu.handle_key(key(KeyCode::Char('w'))); // back up to opponent
        menu.handle_key(key(KeyCode::Char('d'))); // right
        assert_eq!(menu.options().opponent, Opponent::Ai(Difficulty::Easy));
    }
}
