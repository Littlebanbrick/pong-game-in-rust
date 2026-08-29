//! Rendering: turns backend snapshots into terminal drawing.
//!
//! The whole module is a pure function of the snapshot: no game state is
//! read or kept here. It also owns the core-unit -> cell coordinate mapping,
//! because how the field appears on screen is purely a frontend concern.

use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Layout, Margin, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::symbols::border;
use ratatui::text::Line;
use ratatui::widgets::{Block, Clear, Paragraph};

use pong_core::{
    FIELD_HEIGHT, FIELD_WIDTH, GamePhase, GameSnapshot, PADDLE_HEIGHT, PADDLE_INSET, PADDLE_WIDTH,
    Side,
};

/// Cell geometry of the court: maps core field coordinates to cells.
#[derive(Debug, Clone, Copy)]
pub struct CourtGeometry {
    /// Court cells, excluding the border.
    area: Rect,
    x_scale: f32,
    y_scale: f32,
}

impl CourtGeometry {
    /// Chooses the largest court that fits `available` while keeping the
    /// core field's on-screen proportions (~7:3 horizontal rectangle).
    ///
    /// Character cells are roughly twice as tall as they are wide, so a
    /// court whose cell ratio `cols/rows` is ~14/3 shows up as ~7:3 — and
    /// the core's trajectories keep their on-screen angles.
    pub fn fit(available: Rect) -> Self {
        let rows_by_width = (available.width as u32 * 3 / 14) as u16;
        let rows = available.height.min(rows_by_width).max(1);
        let cols = available.width.min((rows as u32 * 14 / 3) as u16).max(1);
        let area = Rect {
            x: available.x + available.width.saturating_sub(cols) / 2,
            y: available.y + available.height.saturating_sub(rows) / 2,
            width: cols,
            height: rows,
        };
        Self {
            x_scale: cols as f32 / FIELD_WIDTH,
            y_scale: rows as f32 / FIELD_HEIGHT,
            area,
        }
    }

    /// Court cells, excluding the border.
    pub fn area(&self) -> Rect {
        self.area
    }

    /// Maps a core-field point to a cell inside the court (absolute screen
    /// coordinates, clamped to the court).
    pub fn cell(&self, x: f32, y: f32) -> (u16, u16) {
        let col = (x * self.x_scale)
            .round()
            .clamp(0.0, self.area.width as f32 - 1.0);
        let row = (y * self.y_scale)
            .round()
            .clamp(0.0, self.area.height as f32 - 1.0);
        (self.area.x + col as u16, self.area.y + row as u16)
    }
}

/// Draws one frame from a snapshot.
///
/// `score_flash` styles the score line while the score-flash effect is on
/// (the frontend blinks it for a short moment after a point).
/// `ai_opponent` switches the footer hints: in AI matches the arrow keys
/// belong to the left paddle.
pub fn draw(frame: &mut Frame<'_>, snapshot: &GameSnapshot, score_flash: bool, ai_opponent: bool) {
    // A zero-sized terminal (a headless pty, a pipe) cannot hold a
    // court; drawing into one would index outside the buffer.
    if frame.area().width == 0 || frame.area().height == 0 {
        return;
    }
    let [score_area, court_area, footer_area] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Min(0),
        Constraint::Length(1),
    ])
    .areas(frame.area());

    let mut score_line = Line::from(format!(
        "  {} : {}  ",
        snapshot.score.left, snapshot.score.right
    ));
    if let GamePhase::Serving { toward, ticks_left } = snapshot.phase {
        let seconds = ticks_left.div_ceil(pong_core::TICKS_PER_SEC as u16);
        let arrow = match toward {
            Side::Left => "◀",
            Side::Right => "▶",
        };
        score_line = Line::from(format!(
            "  {} : {}   ·   serve {} in {}s ",
            snapshot.score.left, snapshot.score.right, arrow, seconds
        ));
    }
    let score_style = if score_flash {
        Style::new().yellow().bold()
    } else {
        Style::new().bold()
    };
    frame.render_widget(
        Paragraph::new(score_line)
            .alignment(Alignment::Center)
            .style(score_style),
        score_area,
    );

    // Leave room for the border, then fit and center the court.
    let geometry = CourtGeometry::fit(court_area.inner(Margin::new(1, 1)));
    let court = geometry.area();
    let bordered = Rect {
        x: court.x.saturating_sub(1),
        y: court.y.saturating_sub(1),
        width: court.width + 2,
        height: court.height + 2,
    };
    frame.render_widget(
        Block::bordered()
            .border_set(border::PLAIN)
            .border_style(Style::new().cyan()),
        bordered,
    );

    draw_net(frame, &geometry);
    draw_paddle(frame, &geometry, Side::Left, snapshot.left_paddle_y);
    draw_paddle(frame, &geometry, Side::Right, snapshot.right_paddle_y);

    let (ball_col, ball_row) = geometry.cell(snapshot.ball_x, snapshot.ball_y);
    // A filled block, not a circle: round glyphs appear to hop between the
    // tall terminal cells, a solid square reads as smooth motion.
    frame.buffer_mut()[(ball_col, ball_row)]
        .set_char('█')
        .set_style(Style::new().yellow());

    if let GamePhase::GameOver { winner } = snapshot.phase {
        draw_game_over(frame, court, winner);
    }

    let footer = if ai_opponent {
        "left: w/s ↑/↓  ·  restart: r  ·  menu: m  ·  quit: q"
    } else {
        "left: w/s  ·  right: ↑/↓  ·  restart: r  ·  menu: m  ·  quit: q"
    };
    frame.render_widget(
        Paragraph::new(footer).alignment(Alignment::Center).dim(),
        footer_area,
    );
}

fn draw_net(frame: &mut Frame<'_>, geometry: &CourtGeometry) {
    let (net_col, _) = geometry.cell(FIELD_WIDTH / 2.0, 0.0);
    let court = geometry.area();
    let buf = frame.buffer_mut();
    let mut row = court.y;
    while row < court.y + court.height {
        buf[(net_col, row)]
            .set_char('┊')
            .set_style(Style::new().dim());
        row += 2;
    }
}

fn draw_paddle(frame: &mut Frame<'_>, geometry: &CourtGeometry, side: Side, y_center: f32) {
    let center_x = match side {
        Side::Left => PADDLE_INSET,
        Side::Right => FIELD_WIDTH - PADDLE_INSET,
    };
    let width_cells = ((PADDLE_WIDTH * geometry.x_scale).round() as u16).max(1);
    let half_height = ((PADDLE_HEIGHT / 2.0 * geometry.y_scale).round() as u16).max(1);
    let (center_col, center_row) = geometry.cell(center_x, y_center);

    let court = geometry.area();
    let left = center_col.saturating_sub(width_cells / 2).max(court.x);
    let right = (left + width_cells).min(court.x + court.width);
    let top = center_row.saturating_sub(half_height).max(court.y);
    let bottom = (center_row + half_height).min(court.y + court.height);

    let buf = frame.buffer_mut();
    let style = Style::new().fg(Color::Cyan);
    for row in top..bottom {
        for col in left..right {
            buf[(col, row)].set_char('█').set_style(style);
        }
    }
}

fn draw_game_over(frame: &mut Frame<'_>, court: Rect, winner: Side) {
    let overlay = centered_rect(court, 36, 5);
    frame.render_widget(Clear, overlay);
    let who = match winner {
        Side::Left => "LEFT",
        Side::Right => "RIGHT",
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from("GAME OVER"),
            Line::from(format!("{who} wins")),
            Line::from("r: restart   m: menu   q: quit"),
        ])
        .alignment(Alignment::Center)
        .block(
            Block::bordered()
                .border_set(border::PLAIN)
                .border_style(Style::new().yellow()),
        ),
        overlay,
    );
}

fn centered_rect(area: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(area.width);
    let height = height.min(area.height);
    Rect {
        x: area.x + (area.width - width) / 2,
        y: area.y + (area.height - height) / 2,
        width,
        height,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(width: u16, height: u16) -> Rect {
        Rect {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    #[test]
    fn fit_keeps_the_screen_ratio_for_any_terminal_shape() {
        // cols/rows must stay ~14/3 so the court looks ~7:3 on screen.
        for (width, height) in [(120u16, 40u16), (80, 30), (200, 20), (40, 10)] {
            let area = CourtGeometry::fit(rect(width, height)).area();
            let ratio = area.width as f32 / area.height as f32;
            assert!(
                (ratio - 14.0 / 3.0).abs() < 0.5,
                "ratio {ratio} for terminal {width}x{height}"
            );
        }
    }

    #[test]
    fn fit_centers_the_court_in_extra_space() {
        let area = CourtGeometry::fit(rect(200, 30)).area();
        // rows: 30 (height-limited), cols: 140 → 30 cells of margin each side.
        assert_eq!(area.width, 140);
        assert_eq!(area.x, 30);
        assert_eq!(area.y, 0);
    }

    #[test]
    fn tiny_terminals_still_yield_a_non_empty_court() {
        let area = CourtGeometry::fit(rect(6, 2)).area();
        assert!(area.width >= 1);
        assert!(area.height >= 1);
    }

    /// A zero-sized area (headless pty, pipe) must not underflow the
    /// centering arithmetic.
    #[test]
    fn zero_sized_terminals_do_not_panic() {
        let area = CourtGeometry::fit(rect(0, 0)).area();
        assert!(area.width >= 1);
        assert!(area.height >= 1);
    }

    #[test]
    fn cell_maps_center_and_clamps_corners() {
        let geometry = CourtGeometry::fit(rect(112, 24));
        assert_eq!(geometry.cell(0.0, 0.0), (0, 0));
        assert_eq!(
            geometry.cell(FIELD_WIDTH / 2.0, FIELD_HEIGHT / 2.0),
            (56, 12)
        );
        assert_eq!(geometry.cell(FIELD_WIDTH, FIELD_HEIGHT), (111, 23));
    }
}
