//! What the pointer means over a character grid.
//!
//! Clicks, drags and double-clicks, in grid coordinates rather than points:
//! the cursor sits in the gap between two characters, so a click lands on a
//! boundary while a selection covers whole cells.

use super::*;

/// The column boundary nearest to `x`, in view columns from `left`.
///
/// What a cursor works in: it sits in the gap *between* two characters, so
/// clicking on the right half of one puts it after that one, the way a click
/// lands in any other text field.
fn boundary_at(x: f32, left: f32, cell_x: f32, view_cols: usize) -> usize {
    (((x - left) / cell_x).round().max(0.0) as usize).min(view_cols)
}

/// The column the pointer is over, in view columns from `left`.
///
/// What a drag works in: pressing anywhere on a character takes that whole
/// character, so the anchor is the cell itself and not the nearest gap between
/// two of them. Rounding here instead is what made a selection only reach a
/// character once the pointer was past its middle.
pub(super) fn column_at(x: f32, left: f32, cell_x: f32, view_cols: usize) -> usize {
    (((x - left) / cell_x).floor().max(0.0) as usize).min(view_cols.saturating_sub(1))
}

/// The run of like characters under `col` on `line`, as a selection.
///
/// The three runs a double-click can land on are a word, a stretch of blanks
/// and a stretch of symbols, exactly as in an editor: double-clicking `%CSW1A`
/// in `do ^%CSW1A` takes `CSW1A`, and the `^%` before it is a run of its own.
///
/// Bounded by the text on the line rather than by the width of the terminal, so
/// a click out in the right margin - past everything the line holds - selects
/// nothing instead of a mouthful of padding blanks.
fn word_at(grid: &Grid, line: usize, col: usize) -> Option<Selection> {
    let row = grid.line(line)?;
    let width = row.used_width();
    if col >= width {
        return None;
    }
    let chars: Vec<char> = row.cells[..width].iter().map(|c| c.ch).collect();
    let (start, end) = lineedit::word_bounds(&chars, col)?;
    Some(Selection::across(line, start, end))
}

/// What a frame's worth of mouse activity asked the caller to do.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct MouseOutcome {
    pub(super) copy_selection: bool,
    pub(super) cursor_move: Option<i64>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn handle_mouse(
    ui: &Ui,
    response: &Response,
    state: &mut ViewState,
    rect: Rect,
    cell: Vec2,
    top: wrap::Top,
    grid: &Grid,
    max_top: wrap::Top,
    max_h_offset: usize,
    segments: &[wrap::Segment],
    mode: wrap::Mode,
    copy_on_select: bool,
    total: usize,
    used: &dyn Fn(usize) -> usize,
) -> MouseOutcome {
    let mut outcome = MouseOutcome::default();
    // Wheel scrolling through scrollback.
    let (scroll, h_scroll, shift) = ui.input(|i| {
        (
            i.raw_scroll_delta.y,
            i.raw_scroll_delta.x,
            i.modifiers.shift,
        )
    });
    if response.hovered() {
        // Shift plus the wheel is the usual way to scroll sideways on a mouse
        // that has no tilt, and it must not also scroll vertically.
        let sideways = if shift && h_scroll == 0.0 {
            scroll
        } else {
            h_scroll
        };
        if sideways != 0.0 && max_h_offset > 0 {
            let columns = (sideways / cell.x).round() as i64;
            let next = (state.h_offset as i64 - columns).clamp(0, max_h_offset as i64);
            state.h_offset = next as usize;
        }

        let vertical = if shift && h_scroll == 0.0 {
            0.0
        } else {
            scroll
        };
        let lines = (vertical / cell.y).round() as i64;
        if lines != 0 {
            scroll_lines(state, top, lines, max_top, total, mode, used);
        }
    }

    // Screen position to grid coordinates, through the display layout: a
    // display row is not a line any more once a line can wrap over several of
    // them, so a selection dragged over a wrapped line has to resolve to the
    // columns it actually covers. `offset` is a column of the display row,
    // which the segment turns into a line and a grid column.
    let resolve = |pos: Pos2, offset: usize, last: usize| -> (usize, usize) {
        let row = ((pos.y - rect.top()) / cell.y).floor().max(0.0) as usize;

        match segments.get(row.min(segments.len().saturating_sub(1))) {
            Some(segment) => (segment.line, (segment.start + offset).min(last)),
            // Nothing laid out at all, which means an empty grid.
            None => (top.line, (mode.offset + offset).min(last)),
        }
    };
    // The cell under the pointer, for dragging out a selection.
    let pos_to_cell = |pos: Pos2| -> (usize, usize) {
        let offset = column_at(pos.x, rect.left(), cell.x, mode.view_cols);
        resolve(pos, offset, grid.cols.saturating_sub(1))
    };
    // The gap between cells nearest the pointer, for putting a cursor there:
    // clicking the right half of a character means after it, as anywhere else.
    let pos_to_boundary = |pos: Pos2| -> (usize, usize) {
        let offset = boundary_at(pos.x, rect.left(), cell.x, mode.view_cols);
        resolve(pos, offset, grid.cols)
    };

    // Double-click takes the word under the pointer and triple-click the whole
    // line, which is what every text editor does with the same two gestures.
    // Both are checked before the single click, which would otherwise clear the
    // selection again on the same frame - egui reports a multi-click as a click
    // as well.
    if response.triple_clicked() || response.double_clicked() {
        if let Some(pos) = response.interact_pointer_pos() {
            let (line, col) = pos_to_cell(pos);
            state.selection = if response.triple_clicked() {
                // To the last character, not to the width of the terminal: a
                // line selected out to column 200 pastes as a line with 150
                // spaces on the end of it.
                let width = grid.line(line).map_or(0, |row| row.used_width());
                (width > 0).then(|| Selection::across(line, 0, width))
            } else {
                word_at(grid, line, col)
            };
            // An empty line, or a click out past the end of one, selects
            // nothing - and clears what was selected before, the way clicking
            // into empty space in an editor does.
            state.drag_anchor = None;
            if copy_on_select && state.selection.is_some() {
                outcome.copy_selection = true;
            }
        }
        return outcome;
    }

    // A plain click clears the selection. This cannot be folded into the
    // `drag_stopped` branch below: egui only reports a drag once the pointer has
    // moved past its threshold, so pressing and releasing without moving never
    // started one, and the old selection stayed on screen still holding Ctrl+C.
    if response.clicked() {
        state.selection = None;

        // Clicking inside the line being typed puts IRIS's cursor there. Only
        // inside it: everywhere else a click is just a click, and off a command
        // line there is no cursor of ours to move.
        if let (Some(line), Some(pos)) = (lineedit::current(grid), response.interact_pointer_pos())
        {
            let cursor_line = grid.scrollback.len() + grid.cursor.row;
            let (clicked_line, col) = pos_to_boundary(pos);
            if clicked_line == cursor_line && (line.start..=line.end).contains(&col) {
                outcome.cursor_move = Some(col as i64 - line.cursor as i64);
            }
        }
    }

    // A drag takes every cell from the one it started on to the one under the
    // pointer, both of them included. The anchor cell is kept as it was
    // pressed: reading it back off the selection would lose it, since the
    // selection is stored in reading order whichever way the drag ran.
    if response.drag_started() {
        if let Some(pos) = response.interact_pointer_pos() {
            let at = pos_to_cell(pos);
            state.drag_anchor = Some(at);
            state.selection = Some(Selection::over(at, at));
        }
    } else if response.dragged() {
        if let (Some(pos), Some(anchor)) = (response.interact_pointer_pos(), state.drag_anchor) {
            state.selection = Some(Selection::over(anchor, pos_to_cell(pos)));
            // Dragging past the top or bottom edge keeps going: the view
            // follows the pointer a line at a time, the way selecting past the
            // edge of a text editor does. `resolve` has already clamped the
            // pointer to the first or last row on screen, so each line the view
            // moves is one more line taken into the selection.
            let over = if pos.y < rect.top() {
                pos.y - rect.top()
            } else if pos.y > rect.bottom() {
                pos.y - rect.bottom()
            } else {
                0.0
            };
            if over != 0.0 {
                let lines = autoscroll_lines(over, cell.y);
                scroll_lines(state, top, lines, max_top, total, mode, used);
                // An idle terminal draws no frame of its own, and without one
                // the scroll would stop the moment the pointer stopped moving.
                ui.ctx().request_repaint();
            }
        }
    } else if response.drag_stopped() {
        state.drag_anchor = None;
        // Never empty: a drag always holds at least the character it started
        // on. A press that never moved is a click, and is cleared above.
        if copy_on_select && state.selection.is_some() {
            outcome.copy_selection = true;
        }
    }

    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grid_with(lines: &[&str]) -> Grid {
        let mut grid = Grid::new(20, lines.len().max(1), 100);
        for (r, line) in lines.iter().enumerate() {
            grid.screen[r].set_text(line);
        }
        grid
    }

    /// A drag takes whole characters: anywhere on a character is that
    /// character, with no half-way point to reach first, and the two ends of
    /// the drag are both inside the selection whichever way round it was made.
    #[test]
    fn a_drag_takes_every_character_it_touches() {
        let (left, w, cols) = (4.0, 10.0, 80);
        let at = |x: f32| column_at(x, left, w, cols);

        // Every pixel of cell 3 is cell 3, from its left edge to its last.
        assert_eq!(at(left + 30.0), 3);
        assert_eq!(at(left + 35.0), 3);
        assert_eq!(at(left + 39.9), 3);
        assert_eq!(at(left + 40.0), 4);

        // Pressing on cell 3 and letting go on cell 5 takes 3, 4 and 5 — the
        // same three either way round.
        let forwards = Selection::over((7, at(left + 31.0)), (7, at(left + 55.0)));
        let backwards = Selection::over((7, at(left + 55.0)), (7, at(left + 31.0)));
        assert_eq!(forwards.span_on(7), Some((3, 6)));
        assert_eq!(backwards.span_on(7), forwards.span_on(7));

        // A drag that never leaves the cell it started on still holds it.
        assert_eq!(Selection::over((7, 3), (7, 3)).span_on(7), Some((3, 4)));

        // Off the left edge clamps to the first column, and past the right to
        // the last one rather than running off the grid.
        assert_eq!(at(left - 200.0), 0);
        assert_eq!(at(left + 10_000.0), cols - 1);
    }

    /// The cursor goes between characters, not on one, so a click on the right
    /// half of a character puts it after that character.
    #[test]
    fn a_click_puts_the_cursor_at_the_nearest_gap() {
        let (left, w, cols) = (4.0, 10.0, 80);
        let at = |x: f32| boundary_at(x, left, w, cols);

        assert_eq!(at(left + 31.0), 3);
        assert_eq!(at(left + 36.0), 4);
        assert_eq!(at(left - 200.0), 0);
        assert_eq!(at(left + 10_000.0), cols);
    }

    /// Double-click takes one run of like characters, the way an editor does:
    /// a word on its own, without the punctuation stuck to either end of it.
    #[test]
    fn a_double_click_takes_the_word_under_the_pointer() {
        let grid = grid_with(&["do ^%CSW1A"]);

        // Anywhere in the word is the whole word, first character to last.
        for col in 5..10 {
            assert_eq!(word_at(&grid, 0, col), Some(Selection::across(0, 5, 10)));
        }
        // The `^%` before it is a run of its own, and so is the blank.
        assert_eq!(word_at(&grid, 0, 3), Some(Selection::across(0, 3, 5)));
        assert_eq!(word_at(&grid, 0, 4), Some(Selection::across(0, 3, 5)));
        assert_eq!(word_at(&grid, 0, 2), Some(Selection::across(0, 2, 3)));
        assert_eq!(word_at(&grid, 0, 0), Some(Selection::across(0, 0, 2)));
    }

    /// Out past the text there is nothing to select: the cells are there, but
    /// they are the padding every row is stored with, not content.
    #[test]
    fn a_double_click_past_the_end_of_the_line_selects_nothing() {
        let grid = grid_with(&["do ^%CSW1A"]);
        assert_eq!(word_at(&grid, 0, 10), None);
        assert_eq!(word_at(&grid, 0, 19), None);
        assert_eq!(word_at(&grid, 1, 0), None);
    }

    /// The end column is exclusive everywhere, so the last character of a word
    /// - and of a line - has to be inside what the selection yields.
    #[test]
    fn a_selected_word_ends_on_its_last_character() {
        let grid = grid_with(&["do ^%CSW1A"]);
        let state = ViewState {
            selection: word_at(&grid, 0, 6),
            ..Default::default()
        };
        assert_eq!(state.selected_text(&grid).as_deref(), Some("CSW1A"));

        // What a triple-click makes: column zero out to the used width.
        let whole_line = ViewState {
            selection: Some(Selection::across(0, 0, 10)),
            ..Default::default()
        };
        assert_eq!(
            whole_line.selected_text(&grid).as_deref(),
            Some("do ^%CSW1A")
        );
    }
}
