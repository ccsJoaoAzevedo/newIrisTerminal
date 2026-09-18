//! Painting the grid and handling mouse interaction.
//!
//! egui is immediate-mode, so this draws the whole visible screen every frame.
//! That is cheap enough: a run of cells sharing one background is emitted as a
//! single rect, and text is batched per colour run rather than per character.

use egui::{Align2, Color32, FontFamily, FontId, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

use crate::config::{CursorStyle, Theme};
use crate::features::analyze;
use crate::features::export;
use crate::features::macros::{Macro, MacroGroup};
use crate::features::natives::Native;
use crate::i18n::tr;
use crate::term::cell::{Cell, Color};
use crate::term::{lineedit, palette, syntax, Attrs, Grid};

// Split by what each part answers for; the widget itself, its public
// types and the frame's entry point stay here.
mod glyphs;
mod menu;
mod mouse;
mod paint;
mod pieces;
mod scroll;

// Back into one scope, which every one of those modules also sees through
// the `use super::*` it starts with.
use crate::ui::search;
use crate::ui::wrap;
use glyphs::{GlyphCache, RowMesh, Scratch, SCRATCH};
pub use menu::ContextAction;
use menu::{analyze_scopes, export_menu, macro_menu, natives_menu};
use mouse::handle_mouse;
use paint::{paint_row, syntax_overrides};
pub use pieces::PieceSelection;
use scroll::{autoscroll_lines, h_scrollbar, scroll_lines, scrollbar, SCROLLBAR_WIDTH};

/// Everything about how the grid should be drawn that is not the grid itself.
///
/// A struct rather than more parameters: [`show`] already takes as many as it
/// can carry, and these all arrive together from `Settings` anyway.
#[derive(Clone, Debug)]
pub struct RenderOpts {
    pub font_size: f32,
    /// Font family, already known to be registered with egui. Empty means the
    /// bundled monospace.
    pub font_family: String,
    pub cursor_style: CursorStyle,
    pub cursor_blink: bool,
    /// Draw a scrollbar for the scrollback.
    pub scrollbar: bool,
    /// Colour globals and quoted strings in the output.
    pub syntax: bool,
    /// Continue a long line on the next display row instead of clipping it and
    /// letting the user scroll sideways. See [`TERMINAL_COLS`] for why both
    /// modes still receive the whole line.
    pub wrap: bool,
    /// Put a selection on the clipboard the moment the mouse is released.
    pub copy_on_select: bool,
    /// Tell the session the grid is [`TERMINAL_COLS`] wide rather than as wide
    /// as the window.
    ///
    /// Right for IRIS, which truncates a `Write` at the margin it was told and
    /// so must never be told a small one. Wrong for anything that *draws* to
    /// the width it is given: a shell's full-screen program fills every one of
    /// those columns with padding and box rules, and each of those lines then
    /// wraps into a hundred display rows of blanks. Shells therefore get the
    /// window's own width, which is what they are drawing into anyway.
    pub wide_grid: bool,
}

impl Default for RenderOpts {
    fn default() -> Self {
        RenderOpts {
            font_size: 14.0,
            font_family: String::new(),
            cursor_style: CursorStyle::default(),
            cursor_blink: false,
            scrollbar: true,
            syntax: true,
            wrap: true,
            copy_on_select: false,
            wide_grid: true,
        }
    }
}

/// Columns the terminal claims to have, however wide the window is.
///
/// IRIS truncates a `Write` at the device right margin rather than wrapping it,
/// so the tail of a line wider than the terminal is never sent and cannot be
/// recovered afterwards. The margin it is told is therefore the longest line
/// the session can ever produce - and the same limit applies to the echo of
/// what is typed, which is why a command longer than the margin looked like a
/// terminal that had stopped accepting keys. The window then shows a view onto
/// the wider grid, either wrapped or scrolled sideways.
///
/// It costs nothing to claim: a row holds only the columns something has been
/// written to (see [`crate::term::grid::Row`]), so the margin is a number the
/// far side is told, not an allocation.
///
/// 32000 because that is as far as the stack goes, and the margin is the whole
/// of what survives. Measured against a real instance: a `Write` of a million
/// characters arrives as exactly `cols` of them and the rest is discarded,
/// whatever `cols` is - 120 columns gives 120 characters, 16384 gives 16384.
/// The margin is therefore not a comfort setting, it is the line length limit,
/// and every column of it is one more character of a `zwrite` that can be read
/// back.
///
/// The ceiling above it is the console's, not ours. A pseudoconsole resized to
/// exactly 32767 columns - `SHRT_MAX` - stops answering altogether: the session
/// comes up as a black screen that ignores every key, which is what
/// `tests/live_width.rs` measures and pins. 32000 leaves most of a thousand
/// columns of clearance under that cliff and is demonstrably fast at every
/// window height. IRIS's own limit is 32767, so there is nothing meaningful
/// left above it either.
///
/// A line longer than this cannot be recovered on a console session at any
/// setting - it is gone before it is sent, and exporting or copying the
/// transcript cannot bring back what never arrived.
///
/// The one other thing the claim costs: anything positioning itself by the
/// width it is told - the `^%G` utility, a full-screen editor, a routine
/// drawing a rule across the screen - has a wrong idea of how wide the screen
/// is. That was already true at 512.
pub const TERMINAL_COLS: usize = 32000;

/// Where the viewport is anchored. Scrolling back pins the view so incoming
/// output does not yank the user to the bottom mid-read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ScrollAnchor {
    #[default]
    Bottom,
    /// Where the top of the view sits, as a line and a row within it. See
    /// [`wrap::Top`]: on a grid this wide one logical line routinely fills the
    /// window many times over, so naming only the line would make a wheel
    /// notch jump the whole of it.
    At(wrap::Top),
}

/// A text selection in absolute line coordinates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub start: (usize, usize),
    pub end: (usize, usize),
}

impl Selection {
    /// Normalised so `start` precedes `end` in reading order.
    fn ordered(&self) -> ((usize, usize), (usize, usize)) {
        if self.start <= self.end {
            (self.start, self.end)
        } else {
            (self.end, self.start)
        }
    }

    fn contains(&self, line: usize, col: usize) -> bool {
        let (start, end) = self.ordered();
        (line, col) >= start && (line, col) < end
    }

    /// Columns covered on `line`, when the whole selection sits on that one
    /// line. The end column is exclusive, as it is everywhere else here.
    ///
    /// `None` for a selection that spans several lines: the only selection the
    /// app can rub out of IRIS's read buffer is one inside the line being
    /// typed.
    pub fn span_on(&self, line: usize) -> Option<(usize, usize)> {
        let (start, end) = self.ordered();
        (start.0 == line && end.0 == line).then_some((start.1, end.1))
    }

    /// A selection covering the whole of one line's `start..end` columns.
    pub fn across(line: usize, start: usize, end: usize) -> Self {
        Selection {
            start: (line, start),
            end: (line, end),
        }
    }

    /// The selection covering every cell from `anchor` to `at`, both ends
    /// included, however the drag ran. The stored range stays half-open, so
    /// the cell the pointer is on is the one *past* the end column.
    fn over(anchor: (usize, usize), at: (usize, usize)) -> Self {
        let (first, last) = if anchor <= at {
            (anchor, at)
        } else {
            (at, anchor)
        };
        Selection {
            start: first,
            end: (last.0, last.1 + 1),
        }
    }

    pub fn is_empty(&self) -> bool {
        let (start, end) = self.ordered();
        start == end
    }
}

/// Per-tab view state that survives between frames.
#[derive(Default)]
pub struct ViewState {
    pub anchor: ScrollAnchor,
    /// First visible column, when lines are clipped rather than wrapped. Held
    /// per tab so scrolling sideways in one does not move another.
    pub h_offset: usize,
    pub selection: Option<Selection>,
    /// Ctrl+F: what is being looked for in this tab's transcript, and where it
    /// was found. See [`crate::ui::search`].
    pub search: search::Search,
    /// A cell to bring on screen before the next frame is laid out.
    ///
    /// Set by whatever moved to a hit, carried out by [`show`], which is the
    /// only place that knows how wide the window is and therefore which display
    /// row the cell is on. Taken when it is honoured, so it moves the view once
    /// rather than pinning it there.
    pub reveal: Option<(usize, usize)>,
    /// Cell the current drag started on, kept because the selection itself is
    /// normalised and so forgets which end the pointer left behind.
    drag_anchor: Option<(usize, usize)>,
}

impl ViewState {
    /// Text of the current selection, ready for the clipboard.
    pub fn selected_text(&self, grid: &Grid) -> Option<String> {
        let selection = self.selection?;
        if selection.is_empty() {
            return None;
        }
        let (start, end) = selection.ordered();

        let mut out = String::new();
        for line_index in start.0..=end.0 {
            let Some(row) = grid.line(line_index) else {
                continue;
            };
            let from = if line_index == start.0 { start.1 } else { 0 };
            let to = if line_index == end.0 {
                end.1.min(row.cells.len())
            } else {
                row.cells.len()
            };
            if from >= to {
                if line_index != end.0 {
                    out.push('\n');
                }
                continue;
            }
            let kept = out.len();
            out.extend(row.cells[from..to].iter().map(|c| c.ch));
            out.truncate(kept + out[kept..].trim_end().len());
            if line_index != end.0 {
                out.push('\n');
            }
        }
        Some(out)
    }

    pub fn clear_selection(&mut self) {
        self.selection = None;
    }

    pub fn scroll_to_bottom(&mut self) {
        self.anchor = ScrollAnchor::Bottom;
    }

    /// Selects every line, scrollback included.
    pub fn select_all(&mut self, grid: &Grid) {
        let last = grid.total_lines().saturating_sub(1);
        let width = grid.line(last).map(|r| r.cells.len()).unwrap_or(grid.cols);
        self.selection = Some(Selection {
            start: (0, 0),
            end: (last, width),
        });
    }
}

/// Size of one character cell for the given font.
///
/// Rounded up to whole pixels. The cell size defines the lattice that glyphs,
/// background rects and the cursor are all placed on, and a fractional width
/// puts each column at a different sub-pixel offset — which is how the cursor
/// came to sit over the wrong character. Rounding up rather than to nearest
/// also guarantees a cell is never narrower than the glyph it holds, so
/// neighbours cannot overlap.
pub fn cell_size(ui: &Ui, font: &FontId) -> Vec2 {
    ui.fonts(|f| {
        // Monospace, so any character measures the same; 'M' is the classic
        // choice and avoids zero-width surprises.
        let width = f.glyph_width(font, 'M');
        let height = f.row_height(font);
        Vec2::new(width.ceil().max(1.0), height.ceil().max(1.0))
    })
}

/// X coordinate of a column on the character lattice.
///
/// Everything that draws into the grid goes through this. Laying a run out as
/// one string instead lets the text renderer accumulate its own advances, and
/// the result drifts away from `col * cell.x` — the cursor and the background
/// rects are placed from the lattice, so the drift showed up as a cursor half
/// over its neighbour with the glyph beneath it appearing twice.
#[inline]
pub fn glyph_x(left: f32, col: usize, cell: Vec2) -> f32 {
    left + col as f32 * cell.x
}

/// The font the terminal draws with.
///
/// The family must already be registered with egui — see
/// [`crate::ui::fonts::install`], which is what decides whether a name is
/// usable. Asking for an unknown family panics inside glyph measurement rather
/// than falling back, so the caller resolves the name first and only a
/// confirmed one reaches here.
pub fn terminal_font(family: &str, font_size: f32) -> FontId {
    let family = match family {
        "" | "monospace" => FontFamily::Monospace,
        other => FontFamily::Name(other.into()),
    };
    FontId::new(font_size, family)
}

/// Where one pane stands among the panes on screen.
///
/// A struct rather than three more parameters, and one place to read what
/// "focused" buys a pane: the keyboard, and the cursor.
#[derive(Clone, Copy, Debug, Default)]
pub struct PaneRole {
    /// This is the pane the keyboard is in - the focused pane of a split tab,
    /// or the only pane of one that is not split.
    ///
    /// Two things follow from it. It is the pane that takes the keyboard back
    /// whenever no other widget wants it; two panes both claiming it would take
    /// it from each other every frame. And it is the only pane that draws a
    /// cursor: a caret sitting in a pane that is not listening says the
    /// keystrokes are going there, which is exactly wrong.
    pub focused: bool,
    /// The active tab changed this frame, so focus should follow it here.
    pub take_focus: bool,
    /// The tab holds a second session, so the menu can offer both of them and
    /// the split can be taken back.
    pub split: bool,
}

pub struct RenderResult {
    pub response: Response,
    /// Set when the user picked something from the right-click menu.
    pub context_action: Option<ContextAction>,
    /// A selection was just finished with copy-on-select turned on.
    pub copy_selection: bool,
    /// Columns to move IRIS's cursor by, after a click inside the line being
    /// typed. Negative is left.
    pub cursor_move: Option<i64>,
    /// Grid dimensions the caller should resize the PTY to.
    ///
    /// `cols` is the *grid* width - [`TERMINAL_COLS`], not the window - because
    /// it is what IRIS is told and therefore where IRIS truncates.
    pub cols: usize,
    pub rows: usize,
    /// Size of the window in character cells, which is what the user sees and
    /// what the status line reports.
    pub view_cols: usize,
    pub view_rows: usize,
    /// Size of one character cell, in points. What the window has to be grown
    /// or shrunk by to gain or lose a column or a row.
    pub cell: Vec2,
    /// The pointer is hovering a selection that is exactly one piece of a
    /// global's value - the trigger for the documentation tooltip. Only ever
    /// set alongside a real, non-empty selection; never speculative.
    pub piece_hover: Option<PieceSelection>,
}

/// Draws the grid into the remaining space of `ui`.
/// Draws the grid.
///
/// `tab_uid` must be stable for the lifetime of a tab and unique across tabs.
/// The widget id is derived from it rather than from egui's automatic
/// layout-position id, because that id shifts whenever the rows above the
/// terminal change — switching tabs, or an autologon banner appearing — which
/// silently moved keyboard focus to a widget that no longer existed and left
/// the terminal unable to receive keys at all.
///
/// `pane` says where this pane stands among the panes on screen - which is
/// what decides whether it claims the keyboard and whether it draws a cursor.
/// See [`PaneRole`].
///
/// `macros` is what the right-click menu offers under Macros. Borrowed rather
/// than owned here: the list belongs to the app, which reloads it when the
/// shared file changes, and is never written from this side.
#[allow(clippy::too_many_arguments)]
pub fn show(
    ui: &mut Ui,
    grid: &Grid,
    state: &mut ViewState,
    theme: &Theme,
    opts: &RenderOpts,
    tab_uid: u64,
    pane: PaneRole,
    macros: &[MacroGroup],
) -> RenderResult {
    let font = terminal_font(&opts.font_family, opts.font_size);
    let cell = cell_size(ui, &font);
    let available = ui.available_size();

    // The bar's width comes off before the grid is measured, and is reserved
    // whether or not there is history yet: taking it away the moment the first
    // line scrolls off would drop a column and reflow the PTY mid-session.
    let bar_width = if opts.scrollbar { SCROLLBAR_WIDTH } else { 0.0 };
    // Clipping is the mode where a line runs off the side, so that is the mode
    // that reserves room for a horizontal bar. Reserved whether or not anything
    // currently overflows: letting it come and go would change the row count
    // and reflow the PTY every time a long line arrived.
    let h_bar_height = if opts.scrollbar && !opts.wrap {
        SCROLLBAR_WIDTH
    } else {
        0.0
    };

    let view_cols = (((available.x - bar_width) / cell.x).floor() as usize).max(1);
    let rows = (((available.y - h_bar_height) / cell.y).floor() as usize).max(1);
    let grid_cols = if opts.wide_grid {
        TERMINAL_COLS.max(view_cols)
    } else {
        view_cols
    };

    let grid_size = Vec2::new(view_cols as f32 * cell.x, rows as f32 * cell.y);
    let (outer, _) = ui.allocate_exact_size(
        Vec2::new(grid_size.x + bar_width, grid_size.y + h_bar_height),
        Sense::hover(),
    );
    let rect = Rect::from_min_size(outer.min, grid_size);

    let id = egui::Id::new(("nit-terminal", tab_uid));
    let response = ui.interact(rect, id, Sense::click_and_drag());

    // A terminal needs every key, but egui reserves arrows, Tab and Escape for
    // moving focus between widgets and strips them from the event stream
    // before we ever see them. Claiming them here is what lets Up/Down reach
    // IRIS to cycle through command history.
    ui.memory_mut(|m| {
        m.set_focus_lock_filter(
            response.id,
            egui::EventFilter {
                tab: true,
                horizontal_arrows: true,
                vertical_arrows: true,
                escape: true,
            },
        )
    });

    // Without this the terminal is dead until clicked, and any click on the
    // chrome (a tab button, say) silently steals typing away again. Claim
    // focus when nothing else wants it, or when the caller says the active tab
    // just changed — but never off a dialog or text field that is in use.
    if pane.take_focus || (pane.focused && ui.memory(|m| m.focused().is_none())) {
        response.request_focus();
    }

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme.background);

    let total = grid.total_lines();
    let cursor_line = grid.scrollback.len() + grid.cursor.row;

    // How wide each line counts as, for laying it out. Trailing blanks are
    // padding, but the cursor needs a row to sit on even when it is past the
    // end of the text — otherwise the line it is on would be measured as
    // shorter than the cursor's own column and the cursor would have nowhere
    // to be drawn.
    let used = |line: usize| {
        let width = grid.line(line).map(|r| r.used_width()).unwrap_or(0);
        if line == cursor_line {
            width.max(grid.cursor.col + 1)
        } else {
            width
        }
    };

    let mode = wrap::Mode {
        view_cols,
        wrap: opts.wrap,
        offset: state.h_offset,
    };

    // The bottom of the view is the line the cursor is on, and its top is the
    // first line of the screen: the screen is the whole of what a live view
    // shows, and history belongs above it. See [`wrap::live_top`], which is
    // where both halves of that are argued - including the clear-screen this
    // was getting wrong.
    let max_top = wrap::live_top(grid.scrollback.len(), cursor_line, rows, mode, used);

    // A hit to bring on screen, now that the layout is known. Put a third of
    // the way down the window rather than on the top row, so what is around it
    // is readable - which is usually the point of having found it.
    if let Some((line, col)) = state.reveal.take() {
        let row_in_line = if mode.wrap {
            col / mode.view_cols.max(1)
        } else {
            0
        };
        let at = wrap::Top {
            line,
            skip: row_in_line,
        };
        let above = (rows / 3) as i64;
        let wanted = wrap::step(total, at, -above, mode, used).min(max_top);
        state.anchor = if wanted >= max_top {
            ScrollAnchor::Bottom
        } else {
            ScrollAnchor::At(wanted)
        };
        // Clipped lines are reached sideways instead, so the column has to be
        // brought into the view the same way.
        if !opts.wrap && (col < state.h_offset || col >= state.h_offset + view_cols) {
            state.h_offset = col.saturating_sub(view_cols / 3);
        }
    }

    let top = match state.anchor {
        ScrollAnchor::Bottom => max_top,
        ScrollAnchor::At(at) => at.min(max_top),
    };

    // How far sideways there is to go. The grid keeps this as a high-water
    // mark; measuring only the lines on screen made the view snap back to the
    // left as soon as scrolling vertically reached a run of short ones.
    let content_cols = grid.widest_line();
    let max_h_offset = content_cols.saturating_sub(view_cols);
    if opts.wrap {
        state.h_offset = 0;
    } else {
        state.h_offset = state.h_offset.min(max_h_offset);
    }
    let mode = wrap::Mode {
        offset: state.h_offset,
        ..mode
    };
    let segments = wrap::from_top(total, rows, top, mode, used);

    let mouse = handle_mouse(
        ui,
        &response,
        state,
        rect,
        cell,
        top,
        grid,
        max_top,
        max_h_offset,
        &segments,
        mode,
        opts.copy_on_select,
        total,
        &used,
    );

    // The piece tooltip's trigger: the pointer sitting over the selection,
    // and that selection being exactly one piece of a global's value. Read
    // after `handle_mouse`, so a selection just finished this same frame is
    // seen too.
    let piece_hover = response
        .hover_pos()
        .and_then(|pos| {
            let offset = mouse::column_at(pos.x, rect.left(), cell.x, mode.view_cols);
            let last = grid.cols.saturating_sub(1);
            let hovered = mouse::resolve(pos, rect, cell, top, mode, &segments, offset, last);
            state
                .selection
                .filter(|sel| sel.contains(hovered.0, hovered.1))
        })
        .and_then(|selection| pieces::piece_at_selection(grid, &selection));

    // Where the cursor is, in grid coordinates, when it is visible and its cell
    // is one of the ones on screen.
    //
    // Worked out before the rows are painted, because the cell underneath has
    // to skip its glyph: the cursor draws that character itself in the inverse
    // colour, and drawing it from both places is what made it look doubled.
    // Only in the pane that is listening: a caret in a pane the keyboard is
    // not in says the typing is going there, which is the one thing it must
    // never say. See [`PaneRole::focused`].
    let cursor_at = if pane.focused && grid.cursor.visible && cursor_phase_on(ui, opts) {
        wrap::row_of(&segments, mode, cursor_line, grid.cursor.col)
            .map(|screen_row| (screen_row, cursor_line, grid.cursor.col))
    } else {
        None
    };

    // Read once for the frame: `paint_row` borrows the search's hits, so the
    // one hit that is drawn differently cannot be read out of `state` while
    // that borrow is alive.
    let current_hit = state.search.current_match();

    // One scan per logical line, reusing one buffer for the frame. A wrapped
    // line arrives as several consecutive segments, and rescanning it for each
    // of them was the most expensive thing a frame did.
    let mut overrides: Vec<Option<Color32>> = Vec::new();
    let mut scanned: Option<usize> = None;

    // One borrow of the thread-local for the whole grid: taking it per
    // character was costing more than laying the characters out.
    SCRATCH.with(|scratch| {
        let mut scratch = scratch.borrow_mut();
        let Scratch {
            glyphs,
            shapes,
            marks,
        } = &mut *scratch;
        glyphs.prepare(ui.ctx(), &font);
        let mut text = RowMesh::new(ui.ctx());

        for (screen_row, segment) in segments.iter().enumerate() {
            let Some(row) = grid.line(segment.line) else {
                continue;
            };
            if opts.syntax && scanned != Some(segment.line) {
                scanned = Some(segment.line);
                if row.used_width() == 0 {
                    // Nothing on the row, so nothing to colour. Worth its own case:
                    // most of an idle screen is blank, and the scan would still
                    // walk every one of the grid's columns.
                    overrides.clear();
                } else {
                    syntax_overrides(&row.cells, theme, &mut overrides);
                }
            }
            let y = rect.top() + screen_row as f32 * cell.y;
            let hide_glyph_at = cursor_at
                .filter(|(row, _, _)| *row == screen_row && opts.cursor_style == CursorStyle::Block)
                .map(|(_, _, col)| col);
            let hits = state.search.on_line(segment.line);
            paint_row(
                shapes,
                glyphs,
                &mut text,
                marks,
                ui.ctx(),
                row,
                segment.line,
                segment.start,
                view_cols,
                y,
                rect.left(),
                cell,
                theme,
                state.selection,
                hits,
                current_hit,
                &font,
                hide_glyph_at,
                &overrides,
            );
        }
        // Drained rather than moved, so the buffer's capacity outlives the frame.
        painter.extend(shapes.drain(..));
    });

    if let Some((screen_row, _, col)) = cursor_at {
        // Inverted in insert mode, and moved clear of the background if the
        // inverse would have landed on it.
        let cursor_colour = palette::cursor(theme, grid.insert_mode);
        // Column relative to the slice on that row, so a cursor in the wrapped
        // tail of a line lands under the character it is actually on.
        let offset = col - segments[screen_row].start;
        let x = glyph_x(rect.left(), offset, cell);
        let y = rect.top() + screen_row as f32 * cell.y;
        let cell_rect = Rect::from_min_size(Pos2::new(x, y), cell);

        match opts.cursor_style {
            CursorStyle::Block => {
                painter.rect_filled(cell_rect, 0.0, cursor_colour);
                // The only draw of this character: `paint_row` left it out, so
                // it appears once, in the inverse colour, exactly on the
                // lattice position the block was filled at.
                if let Some(ch) = grid
                    .line(cursor_line)
                    .and_then(|r| r.cells.get(col))
                    .filter(|c| !c.is_blank())
                    .map(|c| c.ch)
                {
                    painter.text(
                        cell_rect.left_top(),
                        Align2::LEFT_TOP,
                        ch,
                        font.clone(),
                        theme.background,
                    );
                }
            }
            // Bar and underscore leave the character alone, so `paint_row` has
            // to draw it after all — see `hide_glyph_at` below.
            CursorStyle::Bar => {
                let width = (cell.x * 0.15).ceil().max(1.0);
                painter.rect_filled(
                    Rect::from_min_size(cell_rect.left_top(), Vec2::new(width, cell.y)),
                    0.0,
                    cursor_colour,
                );
            }
            CursorStyle::Underscore => {
                let height = (cell.y * 0.12).ceil().max(1.0);
                painter.rect_filled(
                    Rect::from_min_size(
                        Pos2::new(cell_rect.left(), cell_rect.bottom() - height),
                        Vec2::new(cell.x, height),
                    ),
                    0.0,
                    cursor_colour,
                );
            }
        }
    }

    if opts.scrollbar {
        let track = Rect::from_min_max(
            Pos2::new(rect.right(), rect.top()),
            Pos2::new(outer.right(), rect.bottom()),
        );
        scrollbar(
            ui, track, state, theme, tab_uid, total, rows, max_top, mode, &used,
        );

        if h_bar_height > 0.0 {
            let track = Rect::from_min_max(
                Pos2::new(rect.left(), rect.bottom()),
                Pos2::new(rect.right(), outer.bottom()),
            );
            h_scrollbar(
                ui,
                track,
                state,
                theme,
                tab_uid,
                content_cols,
                view_cols,
                max_h_offset,
            );
        }
    }

    // Where the system should put an input method's candidate window - and, the
    // reason this is here at all, the fact that this window takes text input in
    // the first place.
    //
    // egui reports an area only for a `TextEdit`, and `egui-winit` turns that
    // report straight into `Window::set_ime_allowed`. With no `TextEdit`
    // anywhere in the app, the window was telling the system it accepts no text
    // at all - so every surface the shell inserts text through had nothing to
    // talk to: a CJK input method, the emoji panel, the touch keyboard, the
    // clipboard history. A terminal is a text input area with a caret in it,
    // which is exactly what this says.
    //
    // Only the pane holding the keyboard says it, and only until something with
    // a real text field - a dialog's own `TextEdit`, drawn later in the frame -
    // says otherwise.
    if response.has_focus() {
        let caret = wrap::row_of(&segments, mode, cursor_line, grid.cursor.col)
            .map(|screen_row| {
                let offset = grid.cursor.col.saturating_sub(segments[screen_row].start);
                Rect::from_min_size(
                    Pos2::new(
                        glyph_x(rect.left(), offset, cell),
                        rect.top() + screen_row as f32 * cell.y,
                    ),
                    cell,
                )
            })
            // Off screen: the top-left of the terminal is where a candidate
            // window is least in the way.
            .unwrap_or_else(|| Rect::from_min_size(rect.min, cell));
        ui.ctx().output_mut(|o| {
            o.ime = Some(egui::output::IMEOutput {
                rect,
                cursor_rect: caret,
            });
        });
    }

    // Right-click menu. Copy is disabled without a selection so the menu
    // states plainly what is available, rather than silently doing nothing.
    let has_selection = state.selection.map(|s| !s.is_empty()).unwrap_or(false);
    let mut context_action = None;

    response.context_menu(|ui| {
        if ui
            .add_enabled(has_selection, egui::Button::new(tr("Copy")))
            .clicked()
        {
            context_action = Some(ContextAction::CopySelection);
            ui.close_menu();
        }
        if ui.button(tr("Paste")).clicked() {
            context_action = Some(ContextAction::Paste);
            ui.close_menu();
        }
        if ui
            .add_enabled(has_selection, egui::Button::new(tr("Copy and paste")))
            .on_hover_text(tr(
                "Puts the selection on the clipboard and types it at the prompt.",
            ))
            .clicked()
        {
            context_action = Some(ContextAction::CopyAndPaste);
            ui.close_menu();
        }
        ui.separator();
        if ui.button(tr("Select all")).clicked() {
            context_action = Some(ContextAction::SelectAll);
            ui.close_menu();
        }
        if ui
            .add_enabled(has_selection, egui::Button::new(tr("Clear selection")))
            .clicked()
        {
            context_action = Some(ContextAction::ClearSelection);
            ui.close_menu();
        }
        ui.separator();
        // Macros, where the output they are run against is. The manager in
        // Settings is for writing them; this is for using them, which is a
        // different gesture at a different moment.
        ui.menu_button(tr("Macros"), |ui| {
            if let Some(chosen) = macro_menu(ui, macros) {
                context_action = Some(chosen);
            }
        });
        // The IRIS helpers, beside the macros: both compose a line and send it
        // to the session, and both belong where that session's output is.
        ui.menu_button(tr("IRIS utilities"), |ui| {
            if let Some(chosen) = natives_menu(ui) {
                context_action = Some(chosen);
            }
        });
        // Exporting used to be a button in the menu bar opening a dialog of six
        // choices. All six are here, which is one press each instead of two,
        // and they are next to the output they act on.
        ui.menu_button(tr("Export"), |ui| {
            if let Some(chosen) = export_menu(ui) {
                context_action = Some(chosen);
            }
        });
        // A submenu rather than three entries: the scope is the only question
        // it asks, and asking it in the menu saves a dialog. A split tab is
        // asked one more - which of its two sessions - and only then, because
        // there is nothing to choose between when there is one pane.
        //
        // Nothing in here explains itself, and that is deliberate: a menu popup
        // sizes its rectangle from its own contents and lays them out
        // justified, so a wide entry stretches every later one to match and the
        // width can only ever grow. One line of explanation was enough to latch
        // it, and the submenu it latched stayed that wide for entries two words
        // long - which is how hovering this before splitting a tab left the
        // menu stretched most of the way across the window afterwards. Every
        // entry here is short, so the menu is the size of its longest label.
        ui.menu_button(tr("Analyze with Claude"), |ui| {
            if pane.split {
                for panes in [analyze::Panes::Focused, analyze::Panes::Both] {
                    ui.menu_button(tr(panes.label()), |ui| {
                        if let Some(chosen) = analyze_scopes(ui, has_selection, panes) {
                            context_action = Some(chosen);
                        }
                    });
                }
            } else if let Some(chosen) = analyze_scopes(ui, has_selection, analyze::Panes::Focused) {
                context_action = Some(chosen);
            }
        });
        if ui
            .button(tr("Clear terminal and scrollback"))
            .on_hover_text(tr("Ctrl+Delete. Unlike a clear-screen from the session itself, this really does throw the history away. It asks the far side to clear - W # at an idle IRIS prompt, Ctrl+L in a shell - so the next prompt goes back to the top; the echo and the old screen are dropped rather than kept."))
            .clicked()
        {
            context_action = Some(ContextAction::ClearTerminal);
            ui.close_menu();
        }
        // The layout of the tab this pane is in, offered where the pane is
        // rather than only up in the strip: splitting is something you decide
        // while looking at the output, not while looking at the tab's name.
        // The same three entries the strip's own menu has, and they do the
        // same things.
        ui.separator();
        if pane.split {
            if ui
                .button(tr("Remove split"))
                .on_hover_text(tr(
                    "Gives the second session a tab of its own. Nothing is closed.",
                ))
                .clicked()
            {
                context_action = Some(ContextAction::Unsplit);
                ui.close_menu();
            }
        } else {
            if ui
                .button(tr("Split to right"))
                .on_hover_text(tr(
                    "Opens a second session in this tab, beside this one. Click into a pane to type in it.",
                ))
                .clicked()
            {
                context_action = Some(ContextAction::SplitRight);
                ui.close_menu();
            }
            if ui.button(tr("Split to bottom")).clicked() {
                context_action = Some(ContextAction::SplitBottom);
                ui.close_menu();
            }
        }
        if ui
            .button(tr("Close pane"))
            .on_hover_text(if pane.split {
                tr("Closes this session. The other pane stays, in a tab of its own.")
            } else {
                tr("Closes this session.")
            })
            .clicked()
        {
            context_action = Some(ContextAction::ClosePane);
            ui.close_menu();
        }
    });

    RenderResult {
        response,
        context_action,
        copy_selection: mouse.copy_selection,
        cursor_move: mouse.cursor_move,
        cols: grid_cols,
        rows,
        view_cols,
        view_rows: rows,
        cell,
        piece_hover,
    }
}

/// Halves of the blink cycle per second: ~0.6 s lit, ~0.6 s dark, which is
/// roughly where every other terminal sits.
const BLINK_HALVES_PER_SEC: f64 = 1.6;

/// Whether a blinking cursor is in its visible half right now.
///
/// Always true when blinking is off. A repaint has to be pending for this to
/// animate; the caller schedules one for each half's end while blink is
/// enabled, since a terminal sitting at an idle prompt gets no other reason to
/// redraw.
fn cursor_phase_on(ui: &Ui, opts: &RenderOpts) -> bool {
    if !opts.cursor_blink {
        return true;
    }
    let time = ui.input(|i| i.time);
    (time * BLINK_HALVES_PER_SEC).floor() as i64 % 2 == 0
}

/// How long until the blinking cursor next changes halves.
///
/// The only moment a blinking cursor needs a frame for. Asking for one any
/// sooner redraws the same pixels, which is what an idle terminal used to do
/// sixty times a second.
pub fn until_cursor_phase_flip(ctx: &egui::Context) -> std::time::Duration {
    phase_flip_after(ctx.input(|i| i.time))
}

/// [`until_cursor_phase_flip`] without the context, which is the whole of it.
fn phase_flip_after(time: f64) -> std::time::Duration {
    let elapsed = time * BLINK_HALVES_PER_SEC;
    let left = (elapsed.floor() + 1.0 - elapsed) / BLINK_HALVES_PER_SEC;
    // Never zero: a zero-length wait is a request for the next frame as fast as
    // the display can give it, which is the loop this replaced.
    std::time::Duration::from_secs_f64(left.clamp(0.001, 1.0 / BLINK_HALVES_PER_SEC))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_blink_asks_for_a_frame_at_each_half_and_not_before() {
        let half = 1.0 / super::BLINK_HALVES_PER_SEC;
        // Right at a boundary: a whole half to wait.
        let at_edge = phase_flip_after(0.0).as_secs_f64();
        assert!((at_edge - half).abs() < 1e-6, "{at_edge} != {half}");
        // Part way through one: only what is left of it.
        let part_way = phase_flip_after(half / 2.0).as_secs_f64();
        assert!((part_way - half / 2.0).abs() < 1e-6, "{part_way}");
        // Never a busy loop, and never longer than a half.
        for step in 0..2000 {
            let left = phase_flip_after(f64::from(step) * 0.0037).as_secs_f64();
            assert!(left > 0.0 && left <= half, "{step}: {left}");
        }
    }
    use crate::term::Grid;

    /// The span is what decides whether an erase key can act on a selection, so
    /// a selection that reaches off the line must not produce one.
    #[test]
    fn a_selection_reports_its_columns_only_while_it_stays_on_one_line() {
        let one_line = Selection::across(7, 5, 12);
        assert_eq!(one_line.span_on(7), Some((5, 12)));
        assert_eq!(one_line.span_on(6), None);

        let across_lines = Selection {
            start: (6, 3),
            end: (7, 9),
        };
        assert_eq!(across_lines.span_on(7), None);
        assert_eq!(across_lines.span_on(6), None);
    }

    /// Dragging right-to-left is the same selection as dragging left-to-right.
    #[test]
    fn a_backwards_selection_reports_the_same_span() {
        let backwards = Selection {
            start: (7, 12),
            end: (7, 5),
        };
        assert_eq!(backwards.span_on(7), Some((5, 12)));
    }

    fn grid_with(lines: &[&str]) -> Grid {
        let mut grid = Grid::new(20, lines.len().max(1), 100);
        for (r, line) in lines.iter().enumerate() {
            grid.screen[r].set_text(line);
        }
        grid
    }

    /// The invariant the cursor bug came down to: text, background rects and
    /// the cursor must all agree on where a column starts, with no drift as
    /// the column index grows.
    #[test]
    fn every_column_sits_on_a_whole_multiple_of_the_cell_width() {
        let cell = Vec2::new(9.0, 17.0);
        for col in 0..200 {
            assert_eq!(glyph_x(4.0, col, cell), 4.0 + col as f32 * 9.0);
        }
        // Adjacent columns are exactly one cell apart, however far along.
        assert_eq!(
            glyph_x(0.0, 138, cell) - glyph_x(0.0, 137, cell),
            cell.x,
            "columns drifted apart"
        );
    }

    #[test]
    fn selection_spanning_lines_joins_with_newlines() {
        let grid = grid_with(&["hello", "world"]);
        let state = ViewState {
            selection: Some(Selection {
                start: (0, 0),
                end: (1, 5),
            }),
            ..ViewState::default()
        };
        assert_eq!(state.selected_text(&grid).as_deref(), Some("hello\nworld"));
    }

    #[test]
    fn selection_within_one_line_is_exclusive_of_the_end_column() {
        let grid = grid_with(&["abcdef"]);
        let state = ViewState {
            selection: Some(Selection {
                start: (0, 1),
                end: (0, 4),
            }),
            ..ViewState::default()
        };
        assert_eq!(state.selected_text(&grid).as_deref(), Some("bcd"));
    }

    #[test]
    fn a_backwards_drag_selects_the_same_text() {
        let grid = grid_with(&["abcdef"]);
        let state = ViewState {
            selection: Some(Selection {
                start: (0, 4),
                end: (0, 1),
            }),
            ..ViewState::default()
        };
        assert_eq!(state.selected_text(&grid).as_deref(), Some("bcd"));
    }

    #[test]
    fn an_empty_selection_yields_nothing() {
        let grid = grid_with(&["abc"]);
        let state = ViewState {
            selection: Some(Selection {
                start: (0, 2),
                end: (0, 2),
            }),
            ..ViewState::default()
        };
        assert!(state.selected_text(&grid).is_none());
    }
}
