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
        self.cells[..self.used_width()]
            .iter()
            .map(|c| c.ch)
            .collect()
    }

    // (see `Row::used_width` below)

    /// Columns up to and including the last non-blank cell.
    ///
    /// What the display layout measures a line by: trailing blanks are padding,
    /// not content, and wrapping on them would leave empty rows.
    pub fn used_width(&self) -> usize {
        self.cells
            .iter()
            .rposition(|c| !c.is_blank())
            .map_or(0, |i| i + 1)
    }

    fn resize(&mut self, cols: usize) {
        self.cells.resize(cols, Cell::default());
    }
}

/// A screen-clear caught in the act.
///
/// IRIS does not clear the screen with one escape sequence. `W #` homes the
/// cursor and then erases its way down the screen a row at a time - printing
/// the new prompt on the way past - so there is never a moment at which the old
/// screen still exists to be filed away in one piece. Each row has to be kept
/// as it is destroyed, and the collection handed to the scrollback only once
/// the sweep has reached the bottom and proved itself a clear rather than a
/// repaint.
struct ClearSweep {
    /// Old contents of each row the sweep has destroyed, indexed by row.
    rows: Vec<Option<Row>>,
    /// Furthest row it has reached. A clear works down the screen; a mutation
    /// above this is an app repainting, which must not eat the transcript.
    at: usize,
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
    /// Insert rather than replace, as far as we can tell.
    ///
    /// Set by IRM (`ESC [ 4 h` / `ESC [ 4 l`) when the remote side reports it,
    /// and by the Insert key otherwise: IRIS edits a line itself and repaints
    /// rather than announcing the mode, so the keystroke is the only signal
    /// there is. Purely a display hint - it drives the cursor colour and
    /// nothing else, so a flag that has drifted out of step with IRIS cannot
    /// corrupt what is on screen.
    pub insert_mode: bool,
    /// Widest line ever printed, in columns. Drives how far the view may be
    /// scrolled sideways when lines are clipped rather than wrapped.
    ///
    /// A high-water mark rather than a measurement: recomputing it would mean
    /// scanning the whole scrollback every frame, and deriving it from only the
    /// lines on screen made the view snap back to the left whenever scrolling
    /// vertically reached a run of short lines. It can therefore over-estimate
    /// once a long line has rotated out of the scrollback, which costs nothing
    /// but a slightly small scrollbar thumb.
    widest: usize,
    /// Deferred wrap: the cursor sits on the last column and the *next* printed
    /// character must move to the following line first. Without this, writing
    /// exactly `cols` characters would scroll one line too early.
    pending_wrap: bool,
    /// Set while a clear-screen is being carried out a row at a time. See
    /// [`ClearSweep`].
    clear: Option<ClearSweep>,
    /// Set by [`Grid::purge_history_on_next_clear`].
    purge_on_clear: bool,
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
            insert_mode: false,
            widest: 0,
            pending_wrap: false,
            clear: None,
            purge_on_clear: false,
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
        // Homing the cursor is how a clear-screen opens. See [`ClearSweep`].
        if self.cursor.row == 0 && self.cursor.col == 0 {
            self.begin_clear();
        }
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
        self.destroying_row(row);
        let cell = Cell::with_pen(ch, &self.pen);
        self.screen[row].cells[col] = cell;
        self.widest = self.widest.max(col + 1);

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
        // The screen is moving rather than being wiped, and what scrolls off
        // reaches the scrollback by itself.
        self.cancel_clear();
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
        self.cancel_clear();
        let n = n.min(self.scroll_bottom - self.scroll_top + 1);
        let pen = self.pen;
        for _ in 0..n {
            self.screen.remove(self.scroll_bottom);
            self.screen
                .insert(self.scroll_top, Row::blank(self.cols, &pen));
        }
        self.touch();
    }

    fn push_scrollback(&mut self, mut row: Row) {
        if self.scrollback_limit == 0 {
            return;
        }
        // Trailing blanks are padding, and a line of history is never printed
        // into again, so they are dropped. It matters once the grid can be much
        // wider than the window: at 240 columns, ten thousand lines of padding
        // is tens of megabytes of spaces. Every reader bounds itself by
        // `cells.len()` or `used_width()`, so a short row is safe; the one place
        // that needs full width back is a row promoted to the screen by
        // `resize`, which re-expands it.
        row.cells.truncate(row.used_width());
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

    /// Widest line printed so far, in columns. See [`Grid::widest`].
    pub fn widest_line(&self) -> usize {
        self.widest
    }

    pub fn reset_scroll_region(&mut self) {
        self.scroll_top = 0;
        self.scroll_bottom = self.rows - 1;
    }

    // ---- erasing ---------------------------------------------------------

    /// ED. 0 = cursor to end, 1 = start to cursor, 2 = whole screen,
    /// 3 = whole screen *and* the saved lines.
    ///
    /// Clearing the whole display files what was on it into scrollback first.
    /// IRIS's `W #` erases the screen where it stands rather than scrolling it
    /// off, so blanking those rows in place is what made the transcript
    /// disappear: nothing had ever been pushed into history, and there was
    /// nothing left to scroll back to. Only mode 3 - the sequence whose whole
    /// purpose is to drop the history - actually throws it away.
    pub fn erase_in_display(&mut self, mode: u16) {
        let pen = self.pen;
        let (row, col) = (self.cursor.row, self.cursor.col);
        match mode {
            0 => {
                // Terminfo spells "clear" both ways: `\E[H\E[2J` and
                // `\E[H\E[J`. An erase-to-end wipes every row below the
                // cursor, so a sweep already under way finishes here rather
                // than one row at a time.
                if self.clear.is_some() {
                    for r in row..self.rows {
                        self.destroying_row(r);
                    }
                } else if row == 0 && col == 0 {
                    self.archive_screen();
                }
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
                // One sequence for the whole screen: nothing to piece together
                // a row at a time.
                self.cancel_clear();
                if mode == 3 {
                    self.scrollback.clear();
                } else {
                    self.archive_screen();
                }
                for r in 0..self.rows {
                    self.screen[r] = Row::blank(self.cols, &pen);
                }
            }
        }
        self.touch();
    }

    // ---- clear-screen detection -----------------------------------------

    /// Notes that the cursor has been homed, which is how a clear-screen
    /// begins. Nothing is captured yet: most homings are an app about to
    /// repaint, and the sweep is dropped again the moment it behaves like one.
    fn begin_clear(&mut self) {
        if self.clear.is_none() {
            self.clear = Some(ClearSweep {
                rows: vec![None; self.rows],
                at: 0,
            });
        }
    }

    fn cancel_clear(&mut self) {
        self.clear = None;
    }

    /// Throw the history away at the next clear-screen instead of filing the
    /// screen into it.
    ///
    /// The "clear the terminal for real" gesture cannot just wipe the grid: the
    /// far side keeps its own idea of where the cursor is and repaints by
    /// absolute position, so a screen cleared behind its back leaves the next
    /// prompt painted back down at the row it had reached. The app therefore
    /// asks IRIS to clear the screen itself, and sets this so the clear that
    /// comes back drops the transcript - including the echo of the command that
    /// asked for it - rather than archiving it.
    pub fn purge_history_on_next_clear(&mut self) {
        self.purge_on_clear = true;
    }

    /// Forgets a purge that was asked for and never happened, so it cannot
    /// ambush a later clear-screen. See [`crate::app::Tab::pump`].
    pub fn cancel_purge(&mut self) {
        self.purge_on_clear = false;
    }

    /// Consumes a pending purge. Reports whether the caller should skip filing
    /// anything away, the history having just been dropped instead.
    fn purging(&mut self) -> bool {
        if !self.purge_on_clear {
            return false;
        }
        self.purge_on_clear = false;
        self.scrollback.clear();
        true
    }

    /// Keeps the old contents of a row that is about to be overwritten or
    /// erased, while a clear sweep is running, and files the sweep away once it
    /// has worked its way to the bottom of the screen.
    ///
    /// Both kinds of destruction count: `W #` erases most rows, but it prints
    /// the new prompt straight over one of them, and a transcript missing that
    /// one line would be its own small bug.
    fn destroying_row(&mut self, row: usize) {
        let Some(sweep) = self.clear.as_mut() else {
            return;
        };
        // A clear starts at the top and only ever moves down. Anything else is
        // an app painting its screen, and its previous screen is not history.
        let starting = sweep.at == 0 && sweep.rows[0].is_none();
        if row >= sweep.rows.len() || row < sweep.at || (starting && row != 0) {
            self.clear = None;
            return;
        }
        sweep.at = row;
        if sweep.rows[row].is_none() {
            sweep.rows[row] = Some(self.screen[row].clone());
        }

        // Reaching the last row ends the sweep. It only counts as a clear if
        // every row on the way down was destroyed: a repaint that starts at the
        // top and then jumps to the footer has skipped the middle, and its old
        // screen is not history.
        if row + 1 == self.rows {
            let Some(sweep) = self.clear.take() else {
                return;
            };
            if sweep.rows.iter().all(|r| r.is_some()) {
                self.file_sweep(sweep);
            }
        }
    }

    /// Hands a completed sweep to the scrollback.
    fn file_sweep(&mut self, sweep: ClearSweep) {
        if self.purging() {
            return;
        }
        let mut rows: Vec<Row> = sweep.rows.into_iter().flatten().collect();
        // Blank rows at the end of the screen are padding, exactly as they are
        // for [`Grid::archive_screen`].
        while rows.last().is_some_and(|r| r.used_width() == 0) {
            rows.pop();
        }
        for row in rows {
            self.push_scrollback(row);
        }
    }

    /// Files the visible screen into scrollback, so a clear-screen hides the
    /// text rather than destroying it.
    ///
    /// Blank rows below the last line of content are dropped: a routine that
    /// clears a 50-row screen holding three lines should cost three lines of
    /// history, not fifty blank ones.
    fn archive_screen(&mut self) {
        if self.purging() {
            return;
        }
        let Some(last) = self.screen.iter().rposition(|r| r.used_width() > 0) else {
            return;
        };
        for index in 0..=last {
            let row = self.screen[index].clone();
            self.push_scrollback(row);
        }
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
        self.destroying_row(row);
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
        // Row indices are about to shift, and a half-captured sweep is indexed
        // by row.
        self.cancel_clear();
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
        self.cancel_clear();
        let pen = self.pen;
        let row = self.cursor.row;
        for _ in 0..n.min(self.scroll_bottom - row + 1) {
            self.screen.remove(row);
            self.screen
                .insert(self.scroll_bottom, Row::blank(self.cols, &pen));
        }
        self.touch();
    }

    /// RIS — full reset. The screen is filed into scrollback on the way out,
    /// for the same reason a clear-screen is: a reset sent by IRIS must not
    /// take the transcript with it. Use [`Grid::hard_reset`] for the one the
    /// user asks for explicitly.
    pub fn reset(&mut self) {
        self.cancel_clear();
        self.archive_screen();
        let cols = self.cols;
        for row in &mut self.screen {
            *row = Row::new(cols);
        }
        self.pen.reset();
        self.insert_mode = false;
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

    /// Reset *and* forget the history: the deliberate "give me a clean
    /// terminal" gesture, bound to Ctrl+Delete.
    pub fn hard_reset(&mut self) {
        self.cancel_clear();
        self.scrollback.clear();
        let cols = self.cols;
        for row in &mut self.screen {
            *row = Row::new(cols);
        }
        self.pen.reset();
        self.insert_mode = false;
        self.widest = 0;
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
        // A sweep is indexed by row, and the screen is about to be a different
        // height.
        self.cancel_clear();

        if cols != self.cols {
            for row in &mut self.screen {
                row.resize(cols);
            }
            // Scrollback is deliberately left alone: history is not reflowed,
            // padding it back out would undo the trim in `push_scrollback`, and
            // a narrower window would otherwise truncate lines already
            // received. Readers clamp to each row's own length.
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
                    if let Some(mut row) = self.scrollback.pop_back() {
                        // Back to full width: unlike scrollback, a screen row
                        // is indexed directly by `print` and the erase
                        // operations, which assume `cols` cells are there.
                        row.resize(cols);
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The memory saving that makes a grid wider than the window affordable.
    #[test]
    fn a_line_pushed_to_scrollback_loses_its_trailing_blanks() {
        let mut grid = Grid::new(200, 2, 100);
        for ch in "hello".chars() {
            grid.print(ch);
        }
        grid.line_feed();
        grid.line_feed();

        let row = grid.scrollback.front().expect("a line scrolled off");
        assert_eq!(row.cells.len(), 5, "padding was kept");
        assert_eq!(row.to_text(), "hello");
    }

    /// A trimmed row promoted back onto the screen is printed into directly, so
    /// it has to be full width again or that write is out of bounds.
    #[test]
    fn a_row_promoted_out_of_scrollback_is_full_width_again() {
        let mut grid = Grid::new(200, 2, 100);
        for ch in "hi".chars() {
            grid.print(ch);
        }
        grid.line_feed();
        grid.line_feed();
        assert!(!grid.scrollback.is_empty());

        grid.resize(200, 4);
        for row in &grid.screen {
            assert_eq!(row.cells.len(), 200, "a screen row must be full width");
        }

        // The write that would be out of bounds on a short row.
        grid.set_cursor(0, 199);
        grid.print('X');
        assert_eq!(grid.screen[0].cells[199].ch, 'X');
    }

    /// Narrowing the window must not throw away text already received: only
    /// the screen is resized, and readers clamp to each row's own length.
    #[test]
    fn narrowing_the_grid_leaves_history_intact() {
        let mut grid = Grid::new(200, 2, 100);
        for _ in 0..150 {
            grid.print('x');
        }
        grid.line_feed();
        grid.line_feed();

        grid.resize(80, 2);
        assert_eq!(
            grid.scrollback.front().map(|r| r.to_text().len()),
            Some(150),
            "history was truncated by a window resize"
        );
    }

    /// What the horizontal scrollbar is sized from. A high-water mark on
    /// purpose: it must not shrink as the view scrolls onto short lines, or the
    /// sideways offset would be clamped away under the user.
    #[test]
    fn the_widest_line_is_remembered_across_short_ones() {
        let mut grid = Grid::new(200, 3, 100);
        assert_eq!(grid.widest_line(), 0);

        for _ in 0..150 {
            grid.print('x');
        }
        assert_eq!(grid.widest_line(), 150);

        // A short line after it must not lower the mark.
        grid.line_feed();
        grid.set_cursor(1, 0);
        grid.print('y');
        assert_eq!(grid.widest_line(), 150);

        // Nor may scrolling the long line into history.
        grid.line_feed();
        grid.line_feed();
        grid.line_feed();
        assert_eq!(grid.widest_line(), 150);

        // Clearing the history is the one point at which it is genuinely gone.
        grid.hard_reset();
        assert_eq!(grid.widest_line(), 0);
    }

    /// The bug behind "`W #` loses everything": IRIS clears the screen in
    /// place, so unless the rows are filed away first there is no history to
    /// scroll back to.
    #[test]
    fn clearing_the_screen_files_it_into_scrollback() {
        let mut grid = Grid::new(20, 4, 100);
        for ch in "USER>write 1".chars() {
            grid.print(ch);
        }
        grid.set_cursor(1, 0);
        for ch in "1".chars() {
            grid.print(ch);
        }

        grid.erase_in_display(2);

        assert_eq!(grid.screen[0].to_text(), "");
        let history: Vec<String> = grid.scrollback.iter().map(Row::to_text).collect();
        assert_eq!(history, vec!["USER>write 1".to_string(), "1".to_string()]);
    }

    /// The other spelling of "clear": home the cursor, then erase to the end.
    #[test]
    fn erase_to_end_from_the_home_position_is_a_clear_too() {
        let mut grid = Grid::new(20, 3, 100);
        for ch in "kept".chars() {
            grid.print(ch);
        }
        grid.set_cursor(0, 0);
        grid.erase_in_display(0);
        assert_eq!(
            grid.scrollback.front().map(Row::to_text),
            Some("kept".to_string())
        );
    }

    /// An erase-to-end from anywhere else is an ordinary partial erase and
    /// must not push a copy of the screen into the history.
    #[test]
    fn erase_to_end_below_the_home_position_files_nothing() {
        let mut grid = Grid::new(20, 3, 100);
        for ch in "kept".chars() {
            grid.print(ch);
        }
        grid.set_cursor(1, 0);
        grid.erase_in_display(0);
        assert!(grid.scrollback.is_empty());
        assert_eq!(grid.screen[0].to_text(), "kept");
    }

    /// A screen with nothing on it costs nothing: clearing twice must not add
    /// a screenful of blank lines.
    #[test]
    fn clearing_an_empty_screen_adds_no_history() {
        let mut grid = Grid::new(20, 24, 100);
        grid.erase_in_display(2);
        grid.erase_in_display(2);
        assert!(grid.scrollback.is_empty());
    }

    /// ED 3 is the sequence that means "and drop the saved lines", and the
    /// gesture behind Ctrl+Delete.
    #[test]
    fn ed_3_and_hard_reset_drop_the_history() {
        let mut grid = Grid::new(20, 3, 100);
        for ch in "gone".chars() {
            grid.print(ch);
        }
        grid.erase_in_display(2);
        assert_eq!(grid.scrollback.len(), 1);

        grid.erase_in_display(3);
        assert!(grid.scrollback.is_empty());

        for ch in "gone".chars() {
            grid.print(ch);
        }
        grid.erase_in_display(2);
        grid.hard_reset();
        assert!(grid.scrollback.is_empty());
        assert_eq!(grid.screen[0].to_text(), "");
    }

    #[test]
    fn used_width_ignores_trailing_blanks_only() {
        let mut grid = Grid::new(10, 1, 0);
        grid.set_cursor(0, 2);
        grid.print('a');
        grid.set_cursor(0, 5);
        grid.print('b');
        assert_eq!(grid.screen[0].used_width(), 6);
        assert_eq!(grid.screen[0].to_text(), "  a  b");
    }
}
