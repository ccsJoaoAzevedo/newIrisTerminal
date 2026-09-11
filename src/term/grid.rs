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
///
/// A row holds only the columns something has been written to, however wide the
/// terminal claims to be. That is what makes a margin of
/// [`crate::ui::terminal_view::TERMINAL_COLS`] affordable: the width is
/// reported to IRIS so that it does not truncate what it writes, and a column
/// nobody has touched costs neither memory nor a pass of any per-frame scan.
/// Every reader bounds itself by `cells.len()` or [`Row::used_width`].
///
/// The one thing it gives up: a region erased with a coloured background has no
/// cells to carry the colour, so it is drawn in the theme background. The
/// scrollback has always behaved that way - `push_scrollback` trims it - and
/// nothing in IRIS paints with one.
#[derive(Clone, Debug, Default)]
pub struct Row {
    pub cells: Vec<Cell>,
    pub wrapped: bool,
}

impl Row {
    pub fn new() -> Self {
        Row::default()
    }

    /// The row as text, with trailing blanks trimmed. Used by logging, export,
    /// and clipboard copy — one implementation shared by all three.
    pub fn to_text(&self) -> String {
        self.cells[..self.used_width()]
            .iter()
            .map(|c| c.ch)
            .collect()
    }

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

    /// Replaces the row with `text`, one column per character.
    ///
    /// How a row is built from a string rather than received from the parser -
    /// which is what the tests want, and the only way to reach a column on a
    /// row that has never been printed into.
    pub fn set_text(&mut self, text: &str) {
        self.cells.clear();
        self.cells.extend(text.chars().map(|ch| Cell {
            ch,
            ..Cell::default()
        }));
    }

    /// Makes column `col` exist and hands it over, padding the gap with blanks
    /// if the row had not reached that far.
    fn reach(&mut self, col: usize) -> &mut Cell {
        if self.cells.len() <= col {
            self.cells.resize(col + 1, Cell::default());
        }
        &mut self.cells[col]
    }

    /// Whatever of `from..to` the row actually holds, for the erase operations.
    /// Erasing past the end is a no-op: there is nothing there to blank, and
    /// nothing is drawn there either.
    fn existing(&mut self, from: usize, to: usize) -> Option<&mut [Cell]> {
        let to = to.min(self.cells.len());
        (from < to).then(|| &mut self.cells[from..to])
    }

    /// Clamps the row to a narrower terminal. Never pads: a row is as long as
    /// what has been written into it.
    fn narrow_to(&mut self, cols: usize) {
        self.cells.truncate(cols);
    }

    /// Empties the row in place, keeping its buffer for the next thing printed
    /// there. What the rows recycled by scrolling get.
    fn reblank(&mut self) {
        self.cells.clear();
        self.wrapped = false;
    }

    /// A copy carrying only the columns in use — the shape a row reaches the
    /// scrollback in anyway, so the padding is never cloned in the first place.
    fn trimmed(&self) -> Row {
        Row {
            cells: self.cells[..self.used_width()].to_vec(),
            wrapped: self.wrapped,
        }
    }
}

/// How long after a resize a homing-and-erasing sweep is read as the
/// pseudoconsole repainting rather than as a clear-screen.
///
/// The repaint arrives in the same read as the resize goes out, so this only
/// has to cover the round trip. It lapses so that a resize on a connection that
/// does *not* repaint - a Telnet session, where the far side is told the new
/// size and says nothing back - cannot leave the next real clear-screen unable
/// to file its screen into the transcript.
pub(crate) const REPAINT_WINDOW: std::time::Duration = std::time::Duration::from_millis(500);

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
    /// The far side has asked for application cursor keys: DECCKM (`ESC [ ?
    /// 1 h`) or the keypad application mode (`ESC =`) that terminfo's `smkx`
    /// sends alongside it.
    ///
    /// It decides which of the two spellings of an arrow key IRIS is expecting
    /// back - `ESC O A` rather than `ESC [ A` - and IRIS 2023 is the first
    /// version to ask, which is why the arrow keys and click-to-position had
    /// stopped moving its cursor there. See [`crate::ui::input::key_bytes`].
    pub app_cursor_keys: bool,
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
    /// Until when a homing-and-erasing sweep is the pseudoconsole repainting
    /// after a resize rather than a clear-screen. See
    /// [`Grid::expect_repaint`].
    repaint_until: Option<std::time::Instant>,
    /// Set by [`Grid::purge_history_on_next_clear`].
    purge_on_clear: bool,
    /// The screen has just been cleared and nothing has been printed since.
    ///
    /// A Windows pseudoconsole ends every clear-screen with `ESC [ 3 J` - see
    /// [`Grid::erase_in_display`] - and this is what tells that one apart from
    /// a program asking for the history to be dropped on its own account.
    screen_cleared: bool,
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
            screen: (0..rows).map(|_| Row::new()).collect(),
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
            app_cursor_keys: false,
            widest: 0,
            pending_wrap: false,
            clear: None,
            repaint_until: None,
            purge_on_clear: false,
            screen_cleared: false,
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

        // Whatever came before, the screen is being written on again, so a
        // later `ESC [ 3 J` is not the tail of a clear-screen.
        self.screen_cleared = false;

        let (row, col) = (self.cursor.row, self.cursor.col);
        self.destroying_row(row);
        let cell = Cell::with_pen(ch, &self.pen);
        *self.screen[row].reach(col) = cell;
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

        // Rotating the region moves its top `n` rows to the bottom in one
        // pass, where they are recycled as the blank rows. The alternative -
        // `remove` plus `insert` per line - shifts the whole screen `n` times
        // over and allocates a row of cells each time round.
        self.screen[self.scroll_top..=self.scroll_bottom].rotate_left(n);
        for index in self.scroll_bottom + 1 - n..=self.scroll_bottom {
            if full_screen {
                let row = std::mem::replace(&mut self.screen[index], Row::new());
                self.push_scrollback(row);
            } else {
                self.screen[index].reblank();
            }
        }
        self.touch();
    }

    pub fn scroll_down(&mut self, n: usize) {
        self.cancel_clear();
        let n = n.min(self.scroll_bottom - self.scroll_top + 1);
        self.open_lines_at(self.scroll_top, self.scroll_bottom, n);
        self.touch();
    }

    /// Pushes `top..=bottom` down by `n`, blanking the rows that opens at the
    /// top and dropping what falls off the bottom. Shared by SD and IL.
    fn open_lines_at(&mut self, top: usize, bottom: usize, n: usize) {
        self.screen[top..=bottom].rotate_right(n);
        for row in &mut self.screen[top..top + n] {
            row.reblank();
        }
    }

    /// Pulls `top..=bottom` up by `n`, blanking the rows that opens at the
    /// bottom. The inner-region half of [`Grid::scroll_up`], shared with DL.
    fn drop_lines_at(&mut self, top: usize, bottom: usize, n: usize) {
        self.screen[top..=bottom].rotate_left(n);
        for row in &mut self.screen[bottom + 1 - n..=bottom] {
            row.reblank();
        }
    }

    fn push_scrollback(&mut self, mut row: Row) {
        if self.scrollback_limit == 0 {
            return;
        }
        // Trailing blanks are padding, and a line of history is never printed
        // into again, so they are dropped. A screen row is no different - see
        // [`Row`] - but it can still be holding blanks between two words, and
        // history cannot.
        row.cells.truncate(row.used_width());
        self.scrollback.push_back(row);
        let over = self.scrollback.len().saturating_sub(self.scrollback_limit);
        if over > 0 {
            self.scrollback.drain(..over);
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
    /// purpose is to drop the history - throws it away, and only when it is
    /// asked for on its own account rather than as the tail of a clear.
    pub fn erase_in_display(&mut self, mode: u16) {
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
                    self.screen[r] = Row::new();
                }
            }
            1 => {
                for r in 0..row {
                    self.screen[r] = Row::new();
                }
                self.erase_row_range(row, 0, col + 1);
            }
            _ => {
                // One sequence for the whole screen: nothing to piece together
                // a row at a time.
                self.cancel_clear();
                if mode == 3 {
                    // ED 3 drops the saved lines, and a program that sends it
                    // out of the blue gets exactly that.
                    //
                    // A Windows pseudoconsole, though, ends *every*
                    // clear-screen with it: `cls`, `clear` and `Clear-Host`
                    // all arrive as a row-by-row erase down the screen and
                    // then this. Obeying it there would delete the transcript
                    // the clear had just filed away - and the history above it
                    // - so a shell tab lost everything to a command that on
                    // the IRIS side loses nothing. Straight after a clear it
                    // is part of that clear, and the transcript stays.
                    if !self.screen_cleared {
                        self.scrollback.clear();
                    }
                } else {
                    self.archive_screen();
                }
                for r in 0..self.rows {
                    self.screen[r] = Row::new();
                }
                self.screen_cleared = true;
            }
        }
        self.touch();
    }

    // ---- clear-screen detection -----------------------------------------

    /// Notes that the cursor has been homed, which is how a clear-screen
    /// begins. Nothing is captured yet: most homings are an app about to
    /// repaint, and the sweep is dropped again the moment it behaves like one.
    fn begin_clear(&mut self) {
        // The pseudoconsole repainting itself after a resize, not a clear: it
        // hands back the same screen it is erasing, so there is nothing to
        // file away and filing it is what duplicated the screen.
        //
        // Unless a purge is armed, in which case the user has asked for a clear
        // and whatever comes back is it - a resize a moment earlier must not
        // leave Ctrl+Delete doing nothing.
        if self.repainting() && !self.purge_on_clear {
            return;
        }
        if self.clear.is_none() {
            self.clear = Some(ClearSweep {
                rows: vec![None; self.rows],
                at: 0,
            });
        }
    }

    /// Notes that a repaint of the whole screen is about to arrive, so that it
    /// is not mistaken for a clear-screen.
    ///
    /// A Windows pseudoconsole answers every resize by repainting: it homes the
    /// cursor and rewrites the screen a row at a time, erasing each row as it
    /// goes. That is character for character the shape of IRIS's own `W #` - see
    /// [`ClearSweep`] - so the sweep filed the screen it was about to be handed
    /// back into the transcript, and the repaint then landed underneath it.
    /// Dragging the window taller duplicated everything on screen, once per
    /// resize.
    ///
    /// Nothing is lost by declining to archive it: a repaint hands back the
    /// same screen it destroys.
    fn expect_repaint(&mut self) {
        self.repaint_until = Some(std::time::Instant::now() + REPAINT_WINDOW);
    }

    /// Whether a sweep starting now is that repaint.
    fn repainting(&self) -> bool {
        self.repaint_until
            .is_some_and(|until| std::time::Instant::now() < until)
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
            sweep.rows[row] = Some(self.screen[row].trimmed());
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
        self.screen_cleared = true;
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
        self.screen_cleared = true;
        if self.purging() {
            return;
        }
        let Some(last) = self.screen.iter().rposition(|r| r.used_width() > 0) else {
            return;
        };
        for index in 0..=last {
            let row = self.screen[index].trimmed();
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
        // To the end of the line, which is what IRIS does on every keystroke of
        // its own line editing: the tail is dropped rather than blanked, so the
        // row goes back to being as long as what is left on it.
        if to >= self.cols {
            self.screen[row].cells.truncate(from);
            if from == 0 {
                self.screen[row].wrapped = false;
            }
            return;
        }
        if let Some(cells) = self.screen[row].existing(from, to) {
            cells.fill(Cell::blank(&pen));
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
        let n = n.min(cols - col);
        let cells = &mut self.screen[row].cells;
        // Nothing written at or after the cursor, so there is nothing to shift:
        // inserting blanks into blanks leaves the row as it is.
        if col >= cells.len() || n == 0 {
            self.touch();
            return;
        }
        // The tail has to have somewhere to go. What would be pushed past the
        // margin is dropped, the way it is on a terminal of a fixed width.
        cells.resize((cells.len() + n).min(cols), Cell::default());
        // Shifting the tail right in one rotate, rather than inserting a blank
        // and popping the last cell `n` times over.
        cells[col..].rotate_right(n);
        cells[col..col + n].fill(Cell::blank(&pen));
        self.touch();
    }

    /// DCH — delete `n` characters at the cursor, shifting the rest left.
    pub fn delete_chars(&mut self, n: usize) {
        let (row, col) = (self.cursor.row, self.cursor.col);
        let cols = self.cols;
        let n = n.min(cols - col);
        let cells = &mut self.screen[row].cells;
        if col >= cells.len() || n == 0 {
            self.touch();
            return;
        }
        // The blanks a fixed-width terminal shifts in at the right margin are
        // exactly the cells that stop existing here.
        let n = n.min(cells.len() - col);
        cells[col..].rotate_left(n);
        let keep = cells.len() - n;
        cells.truncate(keep);
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
        let row = self.cursor.row;
        self.open_lines_at(row, self.scroll_bottom, n.min(self.scroll_bottom - row + 1));
        self.touch();
    }

    /// DL — delete `n` lines at the cursor row, within the scroll region.
    pub fn delete_lines(&mut self, n: usize) {
        if self.cursor.row < self.scroll_top || self.cursor.row > self.scroll_bottom {
            return;
        }
        self.cancel_clear();
        let row = self.cursor.row;
        self.drop_lines_at(row, self.scroll_bottom, n.min(self.scroll_bottom - row + 1));
        self.touch();
    }

    /// RIS — full reset. The screen is filed into scrollback on the way out,
    /// for the same reason a clear-screen is: a reset sent by IRIS must not
    /// take the transcript with it. Use [`Grid::hard_reset`] for the one the
    /// user asks for explicitly.
    pub fn reset(&mut self) {
        self.cancel_clear();
        self.archive_screen();
        self.blank_everything();
        // RIS is the far side saying it has forgotten its own modes, so the
        // keys it expects go back to the ANSI spelling. Not done by
        // `blank_everything`: `hard_reset` is the *user* clearing the screen,
        // and IRIS's idea of the mode is untouched by that.
        self.app_cursor_keys = false;
    }

    /// Reset *and* forget the history: the deliberate "give me a clean
    /// terminal" gesture, bound to Ctrl+Delete.
    pub fn hard_reset(&mut self) {
        self.cancel_clear();
        self.scrollback.clear();
        self.widest = 0;
        self.blank_everything();
    }

    /// The part of a reset both spellings share: an empty screen, the default
    /// pen, a homed cursor and no leftover modes.
    fn blank_everything(&mut self) {
        for row in &mut self.screen {
            row.reblank();
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

    // ---- resize ----------------------------------------------------------

    /// Resize the screen, preserving as much content as possible.
    ///
    /// Shrinking pushes the top lines into scrollback; growing adds the new
    /// rows at the bottom and leaves history where it is. That is what the far
    /// side does with its own buffer, and matching it is the point: the screen
    /// is the far side's to paint, and anything we put on a row it believes is
    /// empty gets erased by its next repaint. Nothing is hidden by this - the
    /// view draws history and screen as one stream, so a line in scrollback is
    /// still the line directly above the screen.
    ///
    /// Width changes truncate/pad each row; full re-wrapping of soft-wrapped
    /// paragraphs is deliberately not attempted — IRIS full-screen routines
    /// repaint on resize anyway, and a naive re-wrap corrupts their layout.
    pub fn resize(&mut self, cols: usize, rows: usize) {
        let cols = cols.max(1);
        let rows = rows.max(1);
        if cols == self.cols && rows == self.rows {
            return;
        }
        // A sweep is indexed by row, and the screen is about to be a different
        // height.
        self.cancel_clear();
        // Whatever the far side sends back about this is a repaint, not a
        // clear-screen.
        self.expect_repaint();

        if cols != self.cols {
            for row in &mut self.screen {
                row.narrow_to(cols);
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
                    if last > self.cursor.row && self.screen[last].used_width() == 0 {
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
                // Blank rows at the bottom, and history left where it is.
                //
                // Pulling lines back out of scrollback is what a terminal that
                // owns its buffer does, and it is wrong here: the buffer
                // belongs to the far side. A pseudoconsole that has grown
                // keeps its content at the top, keeps the cursor on the row it
                // was on, and adds the new rows below - so a line lifted back
                // onto the screen sits where the far side believes nothing is,
                // and its next repaint erases it. Having just been taken out
                // of scrollback, it is then gone for good.
                //
                // That is what emptied a session dragged short and then
                // maximized: every row the shrink had filed away was pulled
                // back onto the screen, the repaint wiped the screen, and the
                // transcript went with it. Left in scrollback the same rows
                // are still there, directly above the screen, because the view
                // draws history and screen as one stream.
                for _ in 0..rows - self.rows {
                    self.screen.push(Row::new());
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

    /// The invariant the whole width claim rests on: a column nobody has
    /// written to costs nothing. At a margin of 16384 the old shape of this -
    /// every screen row allocated to full width - was half a megabyte a row.
    #[test]
    fn a_row_holds_only_what_has_been_written_to_it() {
        let mut grid = Grid::new(16384, 24, 100);
        assert!(
            grid.screen.iter().all(|row| row.cells.is_empty()),
            "a fresh screen should hold no cells at all"
        );

        for ch in "USER>".chars() {
            grid.print(ch);
        }
        assert_eq!(grid.screen[0].cells.len(), 5);
        assert!(grid.screen[1].cells.is_empty());

        // And a long line is held whole, however far past the window it runs.
        grid.line_feed();
        grid.set_cursor(1, 0);
        for _ in 0..3000 {
            grid.print('X');
        }
        assert_eq!(grid.screen[1].to_text().chars().count(), 3000);
        assert_eq!(grid.widest_line(), 3000);
    }

    /// Erasing to the end of the line is what IRIS sends on every keystroke of
    /// its own line editing, so it has to leave the row as short as what is
    /// left on it rather than blanking thousands of columns.
    #[test]
    fn erasing_to_the_end_of_the_line_shortens_the_row() {
        let mut grid = Grid::new(16384, 4, 100);
        for ch in "USER>write 1".chars() {
            grid.print(ch);
        }
        grid.set_cursor(0, 5);
        grid.erase_in_line(0);
        assert_eq!(grid.screen[0].cells.len(), 5);
        assert_eq!(grid.screen[0].to_text(), "USER>");
    }

    /// ECH erases in the middle of a line, which is a blank rather than a
    /// truncation: what follows has to stay where it is.
    #[test]
    fn erasing_characters_in_the_middle_leaves_the_tail_in_place() {
        let mut grid = Grid::new(16384, 4, 100);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        grid.set_cursor(0, 2);
        grid.erase_chars(2);
        assert_eq!(grid.screen[0].to_text(), "ab  ef");
    }

    /// DCH pulls the tail left; the blanks a fixed-width terminal shifts in at
    /// the margin are the cells that stop existing.
    #[test]
    fn deleting_characters_pulls_the_tail_left() {
        let mut grid = Grid::new(16384, 4, 100);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        grid.set_cursor(0, 1);
        grid.delete_chars(2);
        assert_eq!(grid.screen[0].to_text(), "adef");
    }

    /// ICH pushes the tail right without dropping any of it, which is the part
    /// a row that only holds what is written could get wrong.
    #[test]
    fn inserting_characters_keeps_the_tail() {
        let mut grid = Grid::new(16384, 4, 100);
        for ch in "abcdef".chars() {
            grid.print(ch);
        }
        grid.set_cursor(0, 3);
        grid.insert_chars(2);
        assert_eq!(grid.screen[0].to_text(), "abc  def");
    }

    /// A row stays as short as what is on it - a resize pads nothing - and is
    /// printed into all the same, far to the right: the write grows it. The
    /// old shape of this was to pad every row out to full width, which is what
    /// a margin of 16384 made unaffordable.
    #[test]
    fn a_short_row_is_printed_into_without_being_padded() {
        let mut grid = Grid::new(200, 2, 100);
        for ch in "hi".chars() {
            grid.print(ch);
        }

        grid.resize(200, 4);
        for row in &grid.screen {
            assert!(
                row.cells.len() <= 2,
                "a screen row holds what is on it and no padding"
            );
        }

        // The write that would be out of bounds if the row did not grow.
        grid.set_cursor(0, 199);
        grid.print('X');
        assert_eq!(grid.screen[0].cells.len(), 200);
        assert_eq!(grid.screen[0].cells[199].ch, 'X');
        let text = grid.screen[0].to_text();
        assert!(text.starts_with("hi") && text.ends_with('X'));
        assert_eq!(text.chars().count(), 200, "the gap is blank, not missing");
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

    /// ED 3 means "and drop the saved lines" - but only when it is asked for
    /// on its own account. Straight after a clear-screen it is the tail of
    /// that clear, which is how a Windows pseudoconsole spells one.
    #[test]
    fn ed_3_drops_the_history_only_when_it_is_not_part_of_a_clear() {
        let mut grid = Grid::new(20, 3, 100);
        for ch in "gone".chars() {
            grid.print(ch);
        }
        grid.erase_in_display(2);
        assert_eq!(grid.scrollback.len(), 1);

        // Part of the clear that has just happened: the screen it filed stays.
        grid.erase_in_display(3);
        assert_eq!(grid.scrollback.len(), 1);

        // Printing ends the clear, so the next one is a program asking.
        for ch in "again".chars() {
            grid.print(ch);
        }
        grid.erase_in_display(3);
        assert!(grid.scrollback.is_empty());
    }

    /// The gesture behind Ctrl+Delete takes the history with it, always.
    #[test]
    fn hard_reset_drops_the_history() {
        let mut grid = Grid::new(20, 3, 100);
        for ch in "gone".chars() {
            grid.print(ch);
        }
        grid.erase_in_display(2);
        assert_eq!(grid.scrollback.len(), 1);

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
