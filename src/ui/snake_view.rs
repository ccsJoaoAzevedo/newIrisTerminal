//! The snake on screen: a square board in the middle of the pane.
//!
//! Everything here is drawing and keys; the rules are in
//! [`crate::features::snake`], which knows nothing about egui. The board is
//! square whatever shape the pane is, and centred in it, so the game is the
//! same game in a tall window as in a wide one.

use egui::{Align2, FontId, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

use crate::config::Theme;
use crate::features::snake::{Dir, Snake, Status, SIDE};
use crate::i18n::{tr, tr1};

/// How much room is kept around the board, which is also where the score and
/// the key hint are written. Wide enough for a line of text above and below
/// the board without either touching it.
const MARGIN: f32 = 30.0;

/// Gap between the board and the text on either side of it.
const TEXT_GAP: f32 = 7.0;

/// How much of a cell the snake fills, leaving the rest as the seam that makes
/// one segment readable as distinct from the next.
const SEGMENT_INSET: f32 = 1.0;

/// What the pane wants from the game this frame - the same two questions
/// [`crate::ui::terminal_view::PaneRole`] answers for a session.
#[derive(Clone, Copy, Debug)]
pub struct Role {
    /// The keyboard is meant to end up here when nothing else wants it.
    pub focused: bool,
    /// The tab has just been switched to, so take the keyboard now.
    pub take_focus: bool,
    /// Whether keys may be read at all this frame. False while a shortcut
    /// picker is recording a chord, or while the window is not the active one:
    /// an arrow key meant for something else must not steer the snake.
    pub keys: bool,
}

/// Draws the board, reads the keys, and moves the game on.
pub fn show(ui: &mut Ui, game: &mut Snake, theme: &Theme, tab_uid: u64, role: Role) -> Response {
    let (outer, _) = ui.allocate_exact_size(ui.available_size(), Sense::hover());
    let id = egui::Id::new(("nit-snake", tab_uid));
    let response = ui.interact(outer, id, Sense::click());

    // The arrows are the game, and egui reserves them for moving focus between
    // widgets unless a widget says otherwise - the same claim the terminal
    // makes for IRIS's sake.
    ui.memory_mut(|m| {
        m.set_focus_lock_filter(
            response.id,
            egui::EventFilter {
                tab: true,
                horizontal_arrows: true,
                vertical_arrows: true,
                escape: false,
            },
        )
    });
    if role.take_focus || (role.focused && ui.memory(|m| m.focused().is_none())) {
        response.request_focus();
    }
    if response.clicked() {
        response.request_focus();
    }

    if response.has_focus() && role.keys {
        for key in pressed_keys(ui) {
            match key {
                Pressed::Turn(dir) => game.turn(dir),
                Pressed::Start => game.start(),
            }
        }
    }

    game.advance(std::time::Instant::now());
    // Only while something is moving: a board waiting to be started, or one
    // with the game over on it, is a still picture and must cost no frames.
    if game.status() == Status::Running {
        ui.ctx().request_repaint_after(game.interval());
    }

    paint(ui, &response, outer, game, theme);
    response
}

/// Where the board goes: the biggest square of whole cells that fits inside
/// the margin, in the middle of the pane.
///
/// Whole cells rather than a square scaled to the room, so every segment of
/// the snake is the same size as every other one and the lattice the board is
/// drawn on lands on pixels rather than between them.
fn board_rect(outer: Rect) -> Rect {
    let room = (outer.width().min(outer.height()) - MARGIN * 2.0).max(SIDE as f32);
    let cell = (room / SIDE as f32).floor().max(1.0);
    let side = cell * SIDE as f32;
    let min = (outer.center() - Vec2::splat(side / 2.0)).round();
    Rect::from_min_size(min, Vec2::splat(side))
}

fn paint(ui: &Ui, response: &Response, outer: Rect, game: &Snake, theme: &Theme) {
    let painter = ui.painter_at(outer);
    painter.rect_filled(outer, 0.0, theme.background);

    let board = board_rect(outer);
    let cell = board.width() / SIDE as f32;
    // A wash of the foreground rather than a colour of its own, so the board
    // is visible as an area on every theme without any theme having to name a
    // colour for a game.
    painter.rect_filled(board, 2.0, theme.foreground.linear_multiply(0.05));
    painter.rect_stroke(
        board,
        2.0,
        Stroke::new(1.0_f32, theme.foreground.linear_multiply(0.35)),
    );

    if let Some((row, col)) = game.food() {
        let middle = board.min + Vec2::new((col as f32 + 0.5) * cell, (row as f32 + 0.5) * cell);
        painter.circle_filled(middle, (cell * 0.32).max(1.0), theme.ansi[9]);
    }

    for (index, (row, col)) in game.body().enumerate() {
        let at = Rect::from_min_size(
            board.min + Vec2::new(col as f32 * cell, row as f32 * cell),
            Vec2::splat(cell),
        );
        // The head picked out from the rest, so it is clear which way the
        // snake is about to go without having to watch it move.
        let colour = if index == 0 {
            theme.ansi[10]
        } else {
            theme.ansi[2]
        };
        painter.rect_filled(at.shrink(SEGMENT_INSET), 1.0, colour);
    }

    let font = FontId::proportional(14.0);
    let dim = theme.foreground.linear_multiply(0.6);
    painter.text(
        Pos2::new(board.left(), board.top() - TEXT_GAP),
        Align2::LEFT_BOTTOM,
        tr1("Score: {}", &game.score().to_string()),
        font.clone(),
        theme.foreground,
    );
    painter.text(
        Pos2::new(board.right(), board.top() - TEXT_GAP),
        Align2::RIGHT_BOTTOM,
        tr1("Best: {}", &game.high_score().to_string()),
        font.clone(),
        dim,
    );
    painter.text(
        Pos2::new(board.center().x, board.bottom() + TEXT_GAP),
        Align2::CENTER_TOP,
        hint(game.status(), response.has_focus()),
        font,
        dim,
    );

    if let Some(message) = message(game.status()) {
        // Over the board rather than beside it: the game is finished, and the
        // board underneath is the result being looked at.
        painter.rect_filled(board, 2.0, theme.background.linear_multiply(0.75));
        painter.text(
            board.center(),
            Align2::CENTER_CENTER,
            message,
            FontId::proportional(20.0),
            theme.foreground,
        );
    }
}

/// The line under the board: what to press, or that the board has to be
/// clicked before it will listen to anything at all.
fn hint(status: Status, focused: bool) -> String {
    if !focused {
        return tr("Click the board to play.").to_string();
    }
    match status {
        Status::Ready => tr("Arrows or WASD to steer. Space to start.").to_string(),
        Status::Running => tr("Arrows or WASD to steer.").to_string(),
        Status::Over | Status::Won => tr("Space to play again.").to_string(),
    }
}

/// What is written across the board when the game is not running.
fn message(status: Status) -> Option<String> {
    match status {
        Status::Ready => Some(tr("Ready").to_string()),
        Status::Running => None,
        Status::Over => Some(tr("Game over").to_string()),
        Status::Won => Some(tr("You filled the board!").to_string()),
    }
}

/// One thing a key press asks of the game.
enum Pressed {
    Turn(Dir),
    Start,
}

/// This frame's key presses, in the order they arrived, so two turns inside
/// one step are both offered to the game - which is what refuses the second
/// one when it would double the snake back along itself.
fn pressed_keys(ui: &Ui) -> Vec<Pressed> {
    ui.input(|i| {
        i.events
            .iter()
            .filter_map(|event| match event {
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } if modifiers.is_none() => asked_for(*key),
                _ => None,
            })
            .collect()
    })
}

/// WASD beside the arrows, because both are what a snake is played with and
/// neither costs anything here: this pane holds no session, so no key on it
/// has another job.
fn asked_for(key: egui::Key) -> Option<Pressed> {
    use egui::Key;
    match key {
        Key::ArrowUp | Key::W => Some(Pressed::Turn(Dir::Up)),
        Key::ArrowDown | Key::S => Some(Pressed::Turn(Dir::Down)),
        Key::ArrowLeft | Key::A => Some(Pressed::Turn(Dir::Left)),
        Key::ArrowRight | Key::D => Some(Pressed::Turn(Dir::Right)),
        Key::Space | Key::Enter => Some(Pressed::Start),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The board is square and sits in the middle of the pane, whichever way
    /// the pane is the long way round.
    #[test]
    fn the_board_is_a_centred_square_in_a_pane_of_any_shape() {
        for size in [Vec2::new(1200.0, 400.0), Vec2::new(400.0, 900.0)] {
            let outer = Rect::from_min_size(Pos2::new(17.0, 5.0), size);
            let board = board_rect(outer);
            assert!(
                (board.width() - board.height()).abs() < f32::EPSILON,
                "{board:?} is not square"
            );
            assert!(
                (board.center() - outer.center()).length() <= 1.0,
                "{board:?} is not centred in {outer:?}"
            );
            assert!(outer.contains_rect(board), "{board:?} left {outer:?}");
        }
    }

    /// Every cell the same whole number of points across: a board scaled to
    /// the room instead would put some segments a pixel wider than their
    /// neighbours.
    #[test]
    fn every_cell_of_the_board_is_the_same_whole_size() {
        let outer = Rect::from_min_size(Pos2::ZERO, Vec2::new(813.0, 517.0));
        let cell = board_rect(outer).width() / SIDE as f32;
        assert_eq!(cell, cell.floor());
        assert!(cell >= 1.0);
    }

    /// A pane too small for the margins still produces a board rather than an
    /// inside-out rectangle.
    #[test]
    fn a_pane_smaller_than_the_margins_still_gets_a_board() {
        let outer = Rect::from_min_size(Pos2::ZERO, Vec2::new(40.0, 20.0));
        let board = board_rect(outer);
        assert!(board.width() > 0.0 && board.height() > 0.0);
    }
}
