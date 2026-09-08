//! Laying logical grid lines out over display rows.
//!
//! IRIS truncates a `Write` at the device right margin instead of wrapping it,
//! so a line wider than the terminal loses its tail before it is ever sent. The
//! grid is therefore kept wider than the window — see `TERMINAL_COLS` in
//! [`crate::ui::terminal_view`] — and what the window shows is a view onto it.
//! That is what this module works out, and it is why everything that used to be
//! `top_line + screen_row` goes through here instead.
//!
//! Two modes, the pair a text editor offers:
//!
//! - **wrap**: a long line continues on the next display row, breaking at the
//!   window width, so nothing is off-screen horizontally.
//! - **clip**: one row per line, showing the columns from a horizontal offset.
//!   The rest of the line is still there, reached by scrolling sideways or by
//!   making the window wider.
//!
//! Deliberately pure and free of egui: this is the arithmetic that the cursor
//! position, mouse hit-testing and both scrollbars have to agree on, and it is
//! only checkable if it can be called from a test.

/// One display row: a logical line and the column its visible slice starts at.
///
/// The slice runs `Mode::view_cols` wide, or to the end of the line, whichever
/// comes first; the caller already knows the width, so it is not repeated here.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Segment {
    pub line: usize,
    pub start: usize,
}

/// How the grid is being shown in the window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    /// Visible width, in columns.
    pub view_cols: usize,
    /// Continue a long line on the next row instead of clipping it.
    pub wrap: bool,
    /// First visible column when clipping. Ignored when wrapping, which always
    /// starts a line at column zero.
    pub offset: usize,
}

impl Mode {
    /// Wrapping at a given width, which is the default the app ships with.
    pub fn wrapping(view_cols: usize) -> Self {
        Mode {
            view_cols,
            wrap: true,
            offset: 0,
        }
    }

    /// Display rows a line of `used` characters needs.
    ///
    /// Always at least one: a blank line still takes a row.
    pub fn rows_for(&self, used: usize) -> usize {
        if !self.wrap || self.view_cols == 0 {
            return 1;
        }
        used.div_ceil(self.view_cols).max(1)
    }
}

/// The display rows filling a viewport of `rows`, starting at `top_line`.
///
/// `used` gives the used width of a logical line. It is a closure rather than a
/// slice because it is only asked about the lines on screen: measuring all ten
/// thousand scrollback lines every frame would not be viable.
pub fn from_top(
    total: usize,
    rows: usize,
    top_line: usize,
    mode: Mode,
    used: impl Fn(usize) -> usize,
) -> Vec<Segment> {
    let mut out = Vec::with_capacity(rows);
    let mut line = top_line;

    while out.len() < rows && line < total {
        if mode.wrap {
            let height = mode.rows_for(used(line));
            for segment in 0..height {
                if out.len() == rows {
                    break;
                }
                out.push(Segment {
                    line,
                    start: segment * mode.view_cols.max(1),
                });
            }
        } else {
            out.push(Segment {
                line,
                start: mode.offset,
            });
        }
        line += 1;
    }

    out
}

/// The logical line that has to be at the top for the last line to sit on the
/// bottom row — the largest vertical scroll offset there is.
///
/// Walks back from the end adding line heights, so it costs one measurement per
/// visible row rather than one per line in the scrollback.
pub fn top_for_bottom(
    total: usize,
    rows: usize,
    mode: Mode,
    used: impl Fn(usize) -> usize,
) -> usize {
    if total == 0 || rows == 0 {
        return 0;
    }
    if !mode.wrap {
        // Every line is exactly one row, so there is nothing to accumulate.
        return total.saturating_sub(rows);
    }

    let mut budget = rows;
    let mut line = total;
    while line > 0 {
        let height = mode.rows_for(used(line - 1));
        if height > budget {
            break;
        }
        budget -= height;
        line -= 1;
    }

    // A single line taller than the whole viewport fits nowhere, and returning
    // `total` would leave the screen blank. Showing the start of that line is
    // not ideal - the end is what you were waiting for - but it beats nothing.
    if line == total {
        total - 1
    } else {
        line
    }
}

/// The top line a live, bottom-anchored view sits at.
///
/// Two things have to hold at once, and each is a bug the other way round.
///
/// The cursor has to be on screen: a terminal always has blank rows below the
/// prompt - the screen is a fixed height and the prompt is somewhere up it -
/// and they are worth a display row each. Counting them into the range is
/// invisible while every line is one row tall, because then the whole screen
/// fits; once one line wraps into thirty, they push the prompt off the top and
/// leave the user scrolling up to find what they just ran. So the bottom of
/// the range is the cursor's line.
///
/// And the screen has to be the *whole* of what a live view shows: history
/// belongs above it, reached by scrolling. Anchoring on the cursor alone pulls
/// scrollback down into the window to fill the rows the blanks would have
/// taken - which is what made a clear-screen invisible. `W #` files the old
/// screen into history and prints its new prompt near the top; a view that
/// then drew forty lines of that history above the prompt showed exactly the
/// screen the clear had just thrown away.
///
/// The larger of the two is both: never above the screen's own first line, and
/// never so far down that the cursor falls off the bottom.
pub fn live_top(
    screen_top: usize,
    cursor_line: usize,
    rows: usize,
    mode: Mode,
    used: impl Fn(usize) -> usize,
) -> usize {
    top_for_bottom(cursor_line + 1, rows, mode, used).max(screen_top)
}

/// Which display row holds a given cell, if it is on screen.
///
/// Answers for both modes: a clipped row's slice starts at the horizontal
/// offset, so a column scrolled off to the left is correctly reported as absent.
pub fn row_of(segments: &[Segment], mode: Mode, line: usize, col: usize) -> Option<usize> {
    let width = mode.view_cols.max(1);
    segments
        .iter()
        .position(|s| s.line == line && col >= s.start && col < s.start + width)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Widths for a scrollback where every line is `w` wide.
    fn flat(w: usize) -> impl Fn(usize) -> usize {
        move |_| w
    }

    fn clipping(view_cols: usize, offset: usize) -> Mode {
        Mode {
            view_cols,
            wrap: false,
            offset,
        }
    }

    /// The bug this pins, as it looked: three `zwrite`s of a 3000-character
    /// global on a 47-row screen, and the prompt you had just typed at was off
    /// the top of the window, with blank space below it.
    ///
    /// A terminal screen is a fixed height with the cursor somewhere up it, so
    /// the rows after the cursor are blank - and each is a display row.
    /// Counting them into the scroll range is invisible while every line is one
    /// row tall, because then the whole screen fits either way; once one line
    /// wraps into thirty, they push the cursor off the top. So the bottom of
    /// the range is the cursor's line, which is the total `terminal_view`
    /// passes here.
    #[test]
    fn the_cursor_stays_on_screen_when_a_line_wraps_into_dozens_of_rows() {
        let rows = 47;
        let mode = Mode::wrapping(102);
        // Three `zwrite`s of 3000 characters, then the prompt on line 3, then
        // the untouched bottom of the screen.
        let cursor_line = 3;
        let used = move |line: usize| match line {
            0..=2 => 3000,
            3 => 8,
            _ => 0,
        };

        // Anchored on the whole grid, the blank rows take up the whole budget
        // and the prompt lands on the *first* row of the window, with nothing
        // above it and forty blank rows below - which is the bug, exactly as
        // reported: every command put the new prompt at the top, and seeing
        // its output meant scrolling up.
        let whole = top_for_bottom(47, rows, mode, used);
        let showing = from_top(47, rows, whole, mode, used);
        assert_eq!(
            showing.first().map(|s| s.line),
            Some(cursor_line),
            "this is the bug: the prompt is the top row"
        );
        assert!(
            !showing.iter().any(|s| s.line == cursor_line - 1),
            "and the output it answered is off the top of the window"
        );

        // Anchored on the cursor's line, the prompt is on screen with the
        // output it answered above it.
        let to_cursor = top_for_bottom(cursor_line + 1, rows, mode, used);
        assert!(
            to_cursor < whole,
            "and that is further up, not further down"
        );
        let showing = from_top(47, rows, to_cursor, mode, used);
        assert!(
            showing.iter().any(|s| s.line == cursor_line),
            "the prompt should be on screen"
        );
        assert!(
            showing.iter().any(|s| s.line == cursor_line - 1),
            "and so should what it is answering"
        );
    }

    /// The regression behind "`W #` stopped clearing the screen".
    ///
    /// IRIS clears by filing the old screen into history and printing its new
    /// prompt near the top of a blank one. Anchored on the cursor's line alone,
    /// the window filled the rows below the prompt with the history that had
    /// just been filed - so the clear put the same screen back, one line lower,
    /// and looked like a command that did nothing.
    #[test]
    fn a_cleared_screen_shows_the_screen_and_not_the_history_it_just_filed() {
        let rows = 40;
        let mode = Mode::wrapping(100);
        // 200 lines of history, then a 40-row screen holding a prompt on its
        // second row and nothing else - which is what a screen looks like the
        // moment after `W #`.
        let screen_top = 200;
        let total = screen_top + rows;
        let cursor_line = screen_top + 1;
        let used = move |line: usize| {
            if line < screen_top || line == cursor_line {
                6
            } else {
                0
            }
        };

        // Anchored on the cursor alone, the window is 38 lines of history with
        // the new prompt at the bottom: the bug, exactly as reported.
        let cursor_only = top_for_bottom(cursor_line + 1, rows, mode, used);
        assert!(
            cursor_only < screen_top,
            "this is the bug: the view reaches back into the history"
        );

        let top = live_top(screen_top, cursor_line, rows, mode, used);
        assert_eq!(top, screen_top, "the cleared screen starts at its own top");
        let showing = from_top(total, rows, top, mode, used);
        assert!(
            !showing.iter().any(|s| s.line < screen_top),
            "and nothing that was filed into history is on screen"
        );
        assert_eq!(
            showing.iter().position(|s| s.line == cursor_line),
            Some(1),
            "the prompt is on the second row, where IRIS printed it"
        );
    }

    /// And the case `live_top` must not undo: the prompt stays on screen when
    /// the lines above it wrap into more rows than the window has, even though
    /// that means scrolling past the top of the screen.
    #[test]
    fn a_screen_taller_than_the_window_still_keeps_the_cursor_on_it() {
        let rows = 47;
        let mode = Mode::wrapping(102);
        let screen_top = 100;
        let cursor_line = screen_top + 3;
        let used = move |line: usize| match line.checked_sub(screen_top) {
            Some(0..=2) => 3000,
            Some(3) => 8,
            _ => 0,
        };

        let top = live_top(screen_top, cursor_line, rows, mode, used);
        assert!(top > screen_top, "the top of the screen has to give way");
        let showing = from_top(screen_top + rows, rows, top, mode, used);
        assert!(showing.iter().any(|s| s.line == cursor_line));
        assert!(showing.iter().any(|s| s.line == cursor_line - 1));
    }

    /// The ordinary screen must not move: while every line is one row tall the
    /// whole thing fits, and the blank rows below the prompt stay on screen
    /// where they have always been.
    #[test]
    fn a_screen_that_fits_is_not_scrolled_at_all() {
        let rows = 47;
        let mode = Mode::wrapping(102);
        let cursor_line = 3;
        let used = move |line: usize| if line <= cursor_line { 40 } else { 0 };

        assert_eq!(top_for_bottom(cursor_line + 1, rows, mode, used), 0);
        let showing = from_top(47, rows, 0, mode, used);
        assert_eq!(showing.len(), rows, "the blank rows are still drawn");
        assert_eq!(showing[cursor_line].line, cursor_line);
    }

    #[test]
    fn a_line_narrower_than_the_window_takes_one_row() {
        let mode = Mode::wrapping(90);
        assert_eq!(mode.rows_for(0), 1, "a blank line still takes a row");
        assert_eq!(mode.rows_for(1), 1);
        assert_eq!(mode.rows_for(90), 1);
    }

    #[test]
    fn a_wider_line_takes_as_many_rows_as_it_needs() {
        let mode = Mode::wrapping(90);
        assert_eq!(mode.rows_for(91), 2);
        assert_eq!(mode.rows_for(180), 2);
        assert_eq!(mode.rows_for(181), 3);
    }

    /// With clipping there is no such thing as a tall line, however long it is:
    /// the rest of it is reached sideways.
    #[test]
    fn clipping_gives_every_line_exactly_one_row() {
        let mode = clipping(90, 0);
        assert_eq!(mode.rows_for(0), 1);
        assert_eq!(mode.rows_for(5000), 1);
    }

    /// The case that has to keep working: lines that fit are laid out exactly as
    /// the plain `top_line + screen_row` this replaced.
    #[test]
    fn short_lines_are_one_row_each_in_either_mode() {
        let expected = vec![
            Segment { line: 40, start: 0 },
            Segment { line: 41, start: 0 },
            Segment { line: 42, start: 0 },
            Segment { line: 43, start: 0 },
            Segment { line: 44, start: 0 },
        ];
        assert_eq!(from_top(100, 5, 40, Mode::wrapping(90), flat(90)), expected);
        assert_eq!(from_top(100, 5, 40, clipping(90, 0), flat(90)), expected);

        assert_eq!(top_for_bottom(100, 5, Mode::wrapping(90), flat(90)), 95);
        assert_eq!(top_for_bottom(100, 5, clipping(90, 0), flat(90)), 95);
    }

    #[test]
    fn a_long_line_is_split_across_consecutive_rows_when_wrapping() {
        // 200 characters in a 90-column window: 90, 90, then 20.
        let used = |line: usize| if line == 1 { 200 } else { 10 };
        assert_eq!(
            from_top(4, 6, 0, Mode::wrapping(90), used),
            vec![
                Segment { line: 0, start: 0 },
                Segment { line: 1, start: 0 },
                Segment { line: 1, start: 90 },
                Segment {
                    line: 1,
                    start: 180
                },
                Segment { line: 2, start: 0 },
                Segment { line: 3, start: 0 },
            ]
        );
    }

    /// The same content clipped: one row each, all showing the same slice.
    #[test]
    fn a_long_line_stays_on_one_row_when_clipping() {
        let used = |line: usize| if line == 1 { 200 } else { 10 };
        assert_eq!(
            from_top(4, 6, 0, clipping(90, 45), used),
            vec![
                Segment { line: 0, start: 45 },
                Segment { line: 1, start: 45 },
                Segment { line: 2, start: 45 },
                Segment { line: 3, start: 45 },
            ]
        );
    }

    /// A viewport must never be given more rows than it has, even when the line
    /// it stops in the middle of has segments left.
    #[test]
    fn the_layout_stops_at_the_row_count() {
        assert_eq!(
            from_top(10, 2, 0, Mode::wrapping(90), flat(500)),
            vec![
                Segment { line: 0, start: 0 },
                Segment { line: 0, start: 90 },
            ]
        );
    }

    #[test]
    fn scrolling_to_the_bottom_accounts_for_wrapped_lines() {
        // Every line takes two rows, so six rows hold three lines.
        assert_eq!(top_for_bottom(100, 6, Mode::wrapping(90), flat(180)), 97);

        // Mixed heights: the last line takes 3 rows, the two before it 1 each.
        let used = |line: usize| if line == 9 { 200 } else { 10 };
        assert_eq!(top_for_bottom(10, 5, Mode::wrapping(90), used), 7);

        // Clipping ignores the heights entirely.
        assert_eq!(top_for_bottom(10, 5, clipping(90, 0), used), 5);
    }

    #[test]
    fn a_short_history_starts_at_the_first_line() {
        assert_eq!(top_for_bottom(3, 40, Mode::wrapping(90), flat(10)), 0);
        assert_eq!(top_for_bottom(0, 40, Mode::wrapping(90), flat(10)), 0);
        assert_eq!(top_for_bottom(3, 40, clipping(90, 0), flat(10)), 0);
    }

    /// Returning `total` here would leave the viewport empty.
    #[test]
    fn a_line_taller_than_the_viewport_still_shows_something() {
        let mode = Mode::wrapping(10);
        let top = top_for_bottom(4, 3, mode, flat(1000));
        assert_eq!(top, 3);
        assert_eq!(from_top(4, 3, top, mode, flat(1000)).len(), 3);
    }

    #[test]
    fn a_cell_is_found_on_the_row_its_slice_is_on() {
        let mode = Mode::wrapping(90);
        let used = |line: usize| if line == 1 { 200 } else { 10 };
        let segments = from_top(4, 6, 0, mode, used);

        assert_eq!(row_of(&segments, mode, 0, 5), Some(0));
        assert_eq!(row_of(&segments, mode, 1, 0), Some(1));
        assert_eq!(row_of(&segments, mode, 1, 89), Some(1));
        assert_eq!(
            row_of(&segments, mode, 1, 90),
            Some(2),
            "start of the second slice"
        );
        assert_eq!(row_of(&segments, mode, 1, 185), Some(3));
        assert_eq!(row_of(&segments, mode, 3, 0), Some(5));

        // Off the bottom, and past the end of what is laid out.
        assert_eq!(row_of(&segments, mode, 9, 0), None);
        assert_eq!(row_of(&segments, mode, 1, 400), None);
    }

    /// Scrolled sideways, the columns to the left of the offset are genuinely
    /// not on screen — which is how the cursor knows not to draw itself.
    #[test]
    fn a_column_scrolled_off_to_the_left_is_not_on_screen() {
        let mode = clipping(90, 100);
        let segments = from_top(3, 3, 0, mode, flat(300));

        assert_eq!(row_of(&segments, mode, 0, 99), None, "left of the view");
        assert_eq!(row_of(&segments, mode, 0, 100), Some(0));
        assert_eq!(row_of(&segments, mode, 0, 189), Some(0));
        assert_eq!(row_of(&segments, mode, 0, 190), None, "right of the view");
    }
}
