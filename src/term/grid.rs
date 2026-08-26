//! The screen model: a rectangle of cells, a cursor, and scrollback.
//!
//! The parser ([`crate::term::parser`]) mutates a `Grid`; the renderer
//! ([`crate::ui::terminal_view`]) reads it. Nothing here knows about egui or
//! about the PTY.

use std::collections::VecDeque;

use super::cell::{Cell, Pen};

/// A single line of the screen. `wrapped` records that this row continued onto
/// the next one because the text ran past the right margin, which is the only
/// way reflow-on-resize can tell a hard newline from a soft one.
#[derive(Clone, Debug)]
pub struct Row {
    pub cells: Vec<Cell>,
    pub wrapped: bool,
}

impl Row {
    pub fn new(cols: usize) -> Self {
        Row {
            cells: vec![Cell::default(); cols],
            wrapped: false,
        }
    }

    pub fn blank(cols: usize, pen: &Pen) -> Self {
        Row {
            cells: vec![Cell::blank(pen); cols],
            wrapped: false,
        }
    }

    /// The row as text, with trailing blanks trimmed. Used by logging, export,
    /// and clipboard copy — one implementation shared by all three.
    pub fn to_text(&self) -> String {
        let end = self
            .cells
            .iter()
            .rposition(|c| !c.is_blank())
            .map_or(0, |i| i + 1);
        self.cells[..end].iter().map(|c| c.ch).collect()
    }

    fn resize(&mut self, cols: usize) {
        self.cells.resize(cols, Cell::default());
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cursor {
    pub row: usize,
    pub col: usize,
    pub visible: bool,
}

pub struct Grid {
    pub cols: usize,
    pub rows: usize,
    /// Visible screen, always exactly `rows` long.
    pub screen: Vec<Row>,
    /// Lines that have scrolled off the top, newest at the back.
    pub scrollback: VecDeque<Row>,
    pub scrollback_limit: usize,
    pub cursor: Cursor,
    pub pen: Pen,
    /// Inclusive top/bottom of the DECSTBM scrolling region, in screen coords.
    pub scroll_top: usize,
    pub scroll_bottom: usize,
    /// Set by OSC 0/2; the tab strip shows it when the user has not renamed the tab.
    pub title: Option<String>,
    /// Saved cursor for DECSC/DECRC.
    saved_cursor: Option<(Cursor, Pen)>,
    /// Deferred wrap: the cursor sits on the last column and the *next* printed
    /// character must move to the following line first. Without this, writing
    /// exactly `cols` characters would scroll one line too early.
    pending_wrap: bool,
    /// Bumped on every mutation so the UI can skip repainting an idle tab.
    pub revision: u64,
}

impl Grid {
    pub fn new(cols: usize, rows: usize, scrollback_limit: usize) -> Self {
        let cols = cols.max(1);
        let rows = rows.max(1);
        Grid {
            cols,
            rows,
            screen: (0..rows).map(|_| Row::new(cols)).collect(),
            scrollback: VecDeque::new(),
            scrollback_limit,
            cursor: Cursor {
                row: 0,
                col: 0,
                visible: true,
            },
            pen: Pen::default(),
            scroll_top: 0,
            scroll_bottom: rows - 1,
            title: None,
            saved_cursor: None,
            pending_wrap: false,
            revision: 0,
        }
    }

    #[inline]
    pub fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    /// Total lines available to the viewport, scrollback included.
    pub fn total_lines(&self) -> usize {
        self.scrollback.len() + self.rows
    }

    /// Line at absolute index, counting from the oldest scrollback line.
    pub fn line(&self, index: usize) -> Option<&Row> {
        if index < self.scrollback.len() {
            self.scrollback.get(index)
        } else {
            self.screen.get(index - self.scrollback.len())
        }
    }

    // ---- cursor movement -------------------------------------------------

    pub fn set_cursor(&mut self, row: usize, col: usize) {
        self.cursor.row = row.min(self.rows.saturating_sub(1));
        self.cursor.col = col.min(self.cols.saturating_sub(1));
        self.pending_wrap = false;
        self.touch();
    }

    pub fn move_up(&mut self, n: usize) {
        // Movement clamps at the scroll region's top edge, not the screen's.
        let limit = if self.cursor.row >= self.scroll_top {
            self.scroll_top
        } else {
            0
        };
        self.cursor.row = self.cursor.row.saturating_sub(n).max(limit);
        self.pending_wrap = false;
        self.touch();
    }

    pub fn move_down(&mut self, n: usize) {
        let limit = if self.cursor.row <= self.scroll_bottom {
            self.scroll_bottom
        } else {
            self.rows - 1
        };
        self.cursor.row = (self.cursor.row + n).min(limit);
        self.pending_wrap = false;
        self.touch();
    }

    pub fn move_left(&mut self, n: usize) {
        self.cursor.col = self.cursor.col.saturating_sub(n);
        self.pending_wrap = false;
        self.touch();
    }

    pub fn move_right(&mut self, n: usize) {
        self.cursor.col = (self.cursor.col + n).min(self.cols - 1);
        self.pending_wrap = false;
        self.touch();
    }

    pub fn carriage_return(&mut self) {
        self.cursor.col = 0;
        self.pending_wrap = false;
        self.touch();
    }

    pub fn backspace(&mut self) {
        self.cursor.col = self.cursor.col.saturating_sub(1);
        self.pending_wrap = false;
        self.touch();
    }

    pub fn tab(&mut self) {
        // Fixed 8-column tab stops; IRIS does not set custom stops.
        let next = ((self.cursor.col / 8) + 1) * 8;
        self.cursor.col = next.min(self.cols - 1);
        self.pending_wrap = false;
        self.touch();
    }

    pub fn save_cursor(&mut self) {
        self.saved_cursor = Some((self.cursor, self.pen));
    }

    pub fn restore_cursor(&mut self) {
        if let Some((cursor, pen)) = self.saved_cursor {
            self.cursor = cursor;
            self.pen = pen;
            self.pending_wrap = false;
            self.touch();
        }
    }

    // ---- printing --------------------------------------------------------

    pub fn print(&mut self, ch: char) {
        if self.pending_wrap {
            self.screen[self.cursor.row].wrapped = true;
            self.cursor.col = 0;
            self.line_feed();
            self.pending_wrap = false;
        }

        let (row, col) = (self.cursor.row, self.cursor.col);
        let cell = Cell::with_pen(ch, &self.pen);
        self.screen[row].cells[col] = cell;

        if col + 1 >= self.cols {
            self.pending_wrap = true;
        } else {
            self.cursor.col += 1;
        }
        self.touch();
    }

    /// LF / IND. Scrolls the region when already at its bottom.
    pub fn line_feed(&mut self) {
        if self.cursor.row == self.scroll_bottom {
            self.scroll_up(1);
        } else if self.cursor.row + 1 < self.rows {
            self.cursor.row += 1;
        }
        self.pending_wrap = false;
        self.touch();
    }

    /// RI — reverse index.
    pub fn reverse_line_feed(&mut self) {
        if self.cursor.row == self.scroll_top {
            self.scroll_down(1);
        } else {
            self.cursor.row = self.cursor.row.saturating_sub(1);
        }
        self.pending_wrap = false;
        self.touch();
    }

    // ---- scrolling -------------------------------------------------------

    /// Move the region's contents up by `n`, pushing lines into scrollback
    /// only when the region is the whole screen — a full-screen routine that
    /// scrolls an inner region must not pollute the transcript.
    pub fn scroll_up(&mut self, n: usize) {
        let n = n.min(self.scroll_bottom - self.scroll_top + 1);
        let full_screen = self.scroll_top == 0 && self.scroll_bottom == self.rows - 1;
        let pen = self.pen;

        for _ in 0..n {
            let row = self.screen.remove(self.scroll_top);
            if full_screen {
                self.push_scrollback(row);
            }
            self.screen
                .insert(self.scroll_bottom, Row::blank(self.cols, &pen));
        }
        self.touch();
    }

    pub fn scroll_down(&mut self, n: usize) {
        let n = n.min(self.scroll_bottom - self.scroll_top + 1);
        let pen = self.pen;
        for _ in 0..n {
            self.screen.remove(self.scroll_bottom);
            self.screen
                .insert(self.scroll_top, Row::blank(self.cols, &pen));
        }
        self.touch();
    }

    fn push_scrollback(&mut self, row: Row) {
        if self.scrollback_limit == 0 {
            return;
        }
        self.scrollback.push_back(row);
        while self.scrollback.len() > self.scrollback_limit {
            self.scrollback.pop_front();
        }
    }

    /// DECSTBM. Parameters arrive 1-based and inclusive.
    pub fn set_scroll_region(&mut self, top: usize, bottom: usize) {
        let top = top.min(self.rows - 1);
        let bottom = bottom.min(self.rows - 1);
        if top < bottom {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
            // DECSTBM homes the cursor.
            self.set_cursor(0, 0);
        }
    }

    pub fn reset_scroll_region(&mut self) {
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
    }

    // ---- erasing ---------------------------------------------------------

    /// ED. 0 = cursor to end, 1 = start to cursor, 2/3 = whole screen.
    pub fn erase_in_display(&mut self, mode: u16) {
        let pen = self.pen;
        let (row, col) = (self.cursor.row, self.cursor.col);
        match mode {
            0 => {
                self.erase_row_range(row, col, self.cols);
                for r in row + 1..self.rows {
                    self.screen[r] = Row::blank(self.cols, &pen);
                }
            }
            1 => {
                for r in 0..row {
                    self.screen[r] = Row::blank(self.cols, &pen);
                }
                self.erase_row_range(row, 0, col + 1);
            }
            _ => {
                for r in 0..self.rows {
                    self.screen[r] = Row::blank(self.cols, &pen);
                }
            }
        }
        self.touch();
    }

    /// EL. 0 = cursor to end of line, 1 = start to cursor, 2 = whole line.
    pub fn erase_in_line(&mut self, mode: u16) {
        let (row, col) = (self.cursor.row, self.cursor.col);
        match mode {
            0 => self.erase_row_range(row, col, self.cols),
            1 => self.erase_row_range(row, 0, col + 1),
            _ => self.erase_row_range(row, 0, self.cols),
        }
        self.touch();
    }

    fn erase_row_range(&mut self, row: usize, from: usize, to: usize) {
        let pen = self.pen;
        let to = to.min(self.cols);
        for c in from..to {
            self.screen[row].cells[c] = Cell::blank(&pen);
        }
        if from == 0 && to == self.cols {
            self.screen[row].wrapped = false;
        }
    }

    /// ECH — erase `n` characters at the cursor without moving it.
    pub fn erase_chars(&mut self, n: usize) {
        let (row, col) = (self.cursor.row, self.cursor.col);
        self.erase_row_range(row, col, col + n);
        self.touch();
    }

    /// ICH — insert `n` blanks at the cursor, shifting the rest right.
    pub fn insert_chars(&mut self, n: usize) {
        let pen = self.pen;
        let (row, col) = (self.cursor.row, self.cursor.col);
        let cols = self.cols;
        let cells = &mut self.screen[row].cells;
        for _ in 0..n.min(cols - col) {
            cells.insert(col, Cell::blank(&pen));
            cells.pop();
        }
        self.touch();
    }

    /// DCH — delete `n` characters at the cursor, shifting the rest left.
    pub fn delete_chars(&mut self, n: usize) {
        let pen = self.pen;
        let (row, col) = (self.cursor.row, self.cursor.col);
        let cols = self.cols;
        let cells = &mut self.screen[row].cells;
        for _ in 0..n.min(cols - col) {
            cells.remove(col);
            cells.push(Cell::blank(&pen));
        }
        self.touch();
    }

    /// IL — insert `n` blank lines at the cursor row, within the scroll region.
    pub fn insert_lines(&mut self, n: usize) {
        if self.cursor.row < self.scroll_top || self.cursor.row > self.scroll_bottom {
            return;
        }
        let pen = self.pen;
        let row = self.cursor.row;
        for _ in 0..n.min(self.scroll_bottom - row + 1) {
            self.screen.remove(self.scroll_bottom);
            self.screen.insert(row, Row::blank(self.cols, &pen));
        }
        self.touch();
    }

    /// DL — delete `n` lines at the cursor row, within the scroll region.
    pub fn delete_lines(&mut self, n: usize) {
        if self.cursor.row < self.scroll_top || self.cursor.row > self.scroll_bottom {
            return;
        }
        let pen = self.pen;
        let row = self.cursor.row;
        for _ in 0..n.min(self.scroll_bottom - row + 1) {
            self.screen.remove(row);
            self.screen
                .insert(self.scroll_bottom, Row::blank(self.cols, &pen));
        }
        self.touch();
    }

    /// RIS — full reset.
    pub fn reset(&mut self) {
        let cols = self.cols;
        for row in &mut self.screen {
            *row = Row::new(cols);
        }
        self.pen.reset();
        self.cursor = Cursor {
            row: 0,
            col: 0,
            visible: true,
        };
        self.reset_scroll_region();
        self.pending_wrap = false;
        self.saved_cursor = None;
        self.touch();
    }

    // ---- resize ----------------------------------------------------------

    /// Resize the screen, preserving as much content as possible.
    ///
    /// Growing taller pulls lines back out of scrollback rather than padding
    /// with blanks, so enlarging the window reveals history instead of empty
    /// space. Shrinking pushes the top lines into scrollback. Width changes
    /// truncate/pad each row; full re-wrapping of soft-wrapped paragraphs is
    /// deliberately not attempted — IRIS full-screen routines repaint on
    /// resize anyway, and a naive re-wrap corrupts their layout.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.cols && rows == self.rows {
            return;
        }

        if cols != self.cols {
            for row in &mut self.screen {
                row.resize(cols);
            }
            for row in &mut self.scrollback {
                row.resize(cols);
            }
            self.cols = cols;
        }

        match rows.cmp(&self.rows) {
            std::cmp::Ordering::Less => {
                // Prefer trimming blank lines below the cursor before pushing
                // real content into scrollback.
                let mut to_remove = self.rows - rows;
                while to_remove > 0 {
                    let last = self.screen.len() - 1;
                    if last > self.cursor.row && self.screen[last].to_text().is_empty() {
                        self.screen.pop();
                    } else {
                        let row = self.screen.remove(0);
                        self.push_scrollback(row);
                        self.cursor.row = self.cursor.row.saturating_sub(1);
                    }
                    to_remove -= 1;
                }
            }
            std::cmp::Ordering::Greater => {
                for _ in 0..rows - self.rows {
                    if let Some(row) = self.scrollback.pop_back() {
                        self.screen.insert(0, row);
                        self.cursor.row += 1;
                    } else {
                        self.screen.push(Row::new(cols));
                    }
                }
            }
            std::cmp::Ordering::Equal => {}
        }

        self.rows = rows;
        // A stale scroll region that outlives its screen would strand the
        // cursor, so re-home it and clamp.
        self.reset_scroll_region();
        self.cursor.row = self.cursor.row.min(rows - 1);
        self.cursor.col = self.cursor.col.min(cols - 1);
        self.pending_wrap = false;
        self.touch();
    }

    // ---- text extraction -------------------------------------------------

    /// Visible screen as text lines. Shared by export and the clean logger.
    pub fn screen_text(&self) -> Vec<String> {
        self.screen.iter().map(Row::to_text).collect()
    }

    /// Scrollback plus visible screen as text lines.
    pub fn all_text(&self) -> Vec<String> {
        self.scrollback
            .iter()
            .chain(self.screen.iter())
            .map(Row::to_text)
            .collect()
    }
}
