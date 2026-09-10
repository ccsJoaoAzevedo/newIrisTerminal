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
use crate::ui::wrap;

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
    /// the width it is given: a shell's full-screen program fills 16384
    /// columns with padding and box rules, and every one of those lines then
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
/// 16384 rather than something larger because of where the console stops.
/// A pseudoconsole resized to exactly 32767 columns - `SHRT_MAX` - stops
/// answering altogether: the session comes up as a black screen that ignores
/// every key, which is what `tests/live_width.rs` measures and pins. 32000
/// still worked, but sitting inside a thousand columns of an undocumented
/// cliff in someone else's process is not worth the characters; half of
/// `SHRT_MAX` is far past any real line and demonstrably fast at every window
/// height. What it costs is that a `Write` of more than 16384 characters is
/// still cut - IRIS itself stops at 32767 - which nothing in the ERP produces.
///
/// The one other thing the claim costs: anything positioning itself by the
/// width it is told - the `^%G` utility, a full-screen editor, a routine
/// drawing a rule across the screen - has a wrong idea of how wide the screen
/// is. That was already true at 512.
pub const TERMINAL_COLS: usize = 16384;

/// Width of the scrollback scrollbar, in points.
const SCROLLBAR_WIDTH: f32 = 10.0;

/// Shortest the thumb is allowed to get, so a long history still leaves
/// something you can actually grab.
const MIN_THUMB_HEIGHT: f32 = 24.0;

/// Where the viewport is anchored. Scrolling back pins the view so incoming
/// output does not yank the user to the bottom mid-read.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ScrollAnchor {
    #[default]
    Bottom,
    /// Absolute index of the top visible line, counting from oldest scrollback.
    At(usize),
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
fn column_at(x: f32, left: f32, cell_x: f32, view_cols: usize) -> usize {
    (((x - left) / cell_x).floor().max(0.0) as usize).min(view_cols.saturating_sub(1))
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

/// What the right-click menu asked for, if anything.
///
/// `Clone` rather than `Copy`: two of these carry a macro, and a macro carries
/// its body.
#[derive(Clone, Debug, PartialEq)]
pub enum ContextAction {
    CopySelection,
    /// Copy the selection and type it straight back into the session. What
    /// picking a global off the screen and putting it on the command line
    /// takes, in one entry instead of two.
    CopyAndPaste,
    Paste,
    SelectAll,
    ClearSelection,
    /// Write this much of the output to a file, in this format.
    Export(export::Format, export::Range),
    /// Put this much of the output on the clipboard.
    CopyRange(export::Range),
    /// Run a macro chosen from the menu.
    ///
    /// Nothing is filled in here: one that takes parameters, or that asks to be
    /// confirmed, opens a dialog over the terminal, and one that does neither
    /// goes straight out. The menu's job is to say *which* macro.
    RunMacro(Macro),
    /// Run an IRIS helper chosen from the menu. Always opens its dialog: every
    /// helper has at least one field to fill in.
    RunNative(Native),
    /// Hand this much of the output to a Claude Code session.
    Analyze(analyze::Scope, analyze::Panes),
    /// Layout of the tab this pane is in. Carried out by the app, which owns
    /// the tabs; the pane only says which one was asked for.
    SplitRight,
    SplitBottom,
    Unsplit,
    /// Close the session in this pane, and nothing else: the other pane of a
    /// split tab stays, unsplit.
    ClosePane,
    /// Reset the terminal and drop the scrollback. The same thing Ctrl+Delete
    /// does, put where it can be found.
    ClearTerminal,
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
    let top_line = match state.anchor {
        ScrollAnchor::Bottom => max_top,
        ScrollAnchor::At(line) => line.min(max_top),
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
    let segments = wrap::from_top(total, rows, top_line, mode, used);

    let mouse = handle_mouse(
        ui,
        &response,
        state,
        rect,
        cell,
        top_line,
        grid,
        max_top,
        max_h_offset,
        &segments,
        mode,
        opts.copy_on_select,
    );

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

    // One scan per logical line, reusing one buffer for the frame. A wrapped
    // line arrives as several consecutive segments, and rescanning it for each
    // of them was the most expensive thing a frame did.
    let mut overrides: Vec<Option<Color32>> = Vec::new();
    let mut scanned: Option<usize> = None;

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
        paint_row(
            &painter,
            row,
            segment.line,
            segment.start,
            view_cols,
            y,
            rect.left(),
            cell,
            theme,
            state,
            &font,
            hide_glyph_at,
            &overrides,
        );
    }

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
        scrollbar(ui, track, state, theme, tab_uid, total, rows, max_top);

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
            .on_hover_text(tr("Ctrl+Delete. Unlike IRIS's own clear-screen, this really does throw the history away. At an idle prompt it sends W # so IRIS puts its next prompt back at the top; the echo and the old screen are dropped rather than kept."))
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
    }
}

/// The scope entries of the "Analyze with Claude" menu, all reporting the same
/// choice of panes.
///
/// A function rather than a closure because it is called from two arms of the
/// menu and a closure would have to borrow the answer twice.
/// The Export submenu: the six things the old Export dialog offered.
///
/// Every label is short, for the reason given at the Analyze submenu: a popup
/// is as wide as its widest entry and can only ever grow.
fn export_menu(ui: &mut Ui) -> Option<ContextAction> {
    let mut chosen = None;
    ui.label(tr("Save to a file"));
    for (label, format, range) in [
        (
            "Screen as text",
            export::Format::Text,
            export::Range::Screen,
        ),
        (
            "Everything as text",
            export::Format::Text,
            export::Range::All,
        ),
        (
            "Screen as HTML",
            export::Format::Html,
            export::Range::Screen,
        ),
        (
            "Everything as HTML",
            export::Format::Html,
            export::Range::All,
        ),
    ] {
        if ui.button(tr(label)).clicked() {
            chosen = Some(ContextAction::Export(format, range));
            ui.close_menu();
        }
    }
    ui.separator();
    ui.label(tr("Copy to the clipboard"));
    for (label, range) in [
        ("Screen", export::Range::Screen),
        ("Everything", export::Range::All),
    ] {
        if ui.button(tr(label)).clicked() {
            chosen = Some(ContextAction::CopyRange(range));
            ui.close_menu();
        }
    }
    chosen
}

/// The Macros submenu: a level per group, then the macros in it.
fn macro_menu(ui: &mut Ui, groups: &[MacroGroup]) -> Option<ContextAction> {
    let mut chosen = None;
    if groups.iter().all(|g| g.macros.is_empty()) {
        ui.weak(tr("No macros defined."));
        return chosen;
    }
    for group in groups {
        if group.macros.is_empty() {
            continue;
        }
        // A group with no name is not a level worth walking through: its macros
        // are offered directly, the way a single ungrouped file reads.
        if group.name.is_empty() {
            if let Some(picked) = group_entries(ui, group) {
                chosen = Some(picked);
            }
            continue;
        }
        ui.menu_button(&group.name, |ui| {
            if let Some(picked) = group_entries(ui, group) {
                chosen = Some(picked);
            }
        });
    }
    chosen
}

/// One group's macros, as menu entries.
///
/// Every entry is a plain press. What follows it depends on the macro: one
/// with parameters, or one marked `confirm`, opens a dialog over the terminal;
/// anything else is sent there and then.
fn group_entries(ui: &mut Ui, group: &MacroGroup) -> Option<ContextAction> {
    let mut chosen = None;
    for m in &group.macros {
        // Said on the entry rather than only in the dialog that follows: which
        // macros write is worth knowing *before* picking one. An ellipsis for
        // the ones that stop to ask, the way a menu entry opening a dialog is
        // spelled everywhere else.
        let label = match (m.confirm, m.needs_input()) {
            (true, _) => format!("{}...  ({})", m.name, tr("confirms")),
            (false, true) => format!("{}...", m.name),
            (false, false) => m.name.clone(),
        };
        let entry = ui.button(label);
        let entry = if m.description.is_empty() {
            entry
        } else {
            entry.on_hover_text(&m.description)
        };
        if entry.clicked() {
            chosen = Some(ContextAction::RunMacro(m.clone()));
            ui.close_menu();
        }
    }
    chosen
}

/// The IRIS utilities submenu: one entry per helper.
///
/// Every one of them opens its dialog - they all take at least one field, so
/// there is no such thing here as a helper that can be run by picking it. The
/// fields used to be a flyout hanging off the entry, which put text fields
/// inside a menu: a menu closes when the pointer wanders onto a sibling, and a
/// half-typed package name went with it. A dialog over the terminal stays until
/// it is answered.
fn natives_menu(ui: &mut Ui) -> Option<ContextAction> {
    let mut chosen = None;
    for native in Native::ALL {
        if ui.button(tr(native.label())).clicked() {
            chosen = Some(ContextAction::RunNative(native));
            ui.close_menu();
        }
    }
    chosen
}

fn analyze_scopes(
    ui: &mut Ui,
    has_selection: bool,
    panes: analyze::Panes,
) -> Option<ContextAction> {
    let mut chosen = None;
    for scope in analyze::Scope::ALL {
        // Asking about the selection needs one, so the entry says as much
        // rather than opening a session on nothing.
        let usable = has_selection || !scope.is_selection();
        if ui
            .add_enabled(usable, egui::Button::new(tr(scope.label())))
            .clicked()
        {
            chosen = Some(ContextAction::Analyze(scope, panes));
            ui.close_menu();
        }
    }
    chosen
}

/// Per-column syntax colour for one row, or an empty vector when the feature is
/// off.
///
/// Computed per row rather than per cell because the scan has to see a whole
/// line to know whether a `^` is inside quotes.
fn syntax_overrides(cells: &[Cell], theme: &Theme, out: &mut Vec<Option<Color32>>) {
    out.clear();
    out.resize(cells.len(), None);
    for span in syntax::scan(cells) {
        let colour = theme.syntax_color(span.kind);
        out[span.start..span.end.min(cells.len())].fill(Some(colour));
    }
}

/// Draws the horizontal scrollbar and handles dragging it.
///
/// Only present when lines are clipped: that is the mode in which part of a line
/// is off to the side, and this is how it is reached without resizing the window.
#[allow(clippy::too_many_arguments)]
fn h_scrollbar(
    ui: &Ui,
    track: Rect,
    state: &mut ViewState,
    theme: &Theme,
    tab_uid: u64,
    content_cols: usize,
    view_cols: usize,
    max_offset: usize,
) {
    let painter = ui.painter_at(track);
    painter.rect_filled(
        track,
        0.0,
        palette::blend(theme.background, theme.foreground, 0.07),
    );

    if max_offset == 0 {
        return;
    }

    let response = ui.interact(
        track,
        egui::Id::new(("nit-terminal-h-scrollbar", tab_uid)),
        Sense::click_and_drag(),
    );

    let visible = (view_cols as f32 / content_cols.max(1) as f32).clamp(0.0, 1.0);
    let thumb_width = (track.width() * visible).max(MIN_THUMB_HEIGHT.min(track.width()));
    let span = (track.width() - thumb_width).max(0.0);

    if let Some(pos) = response.interact_pointer_pos() {
        let wanted = if span > 0.0 {
            ((pos.x - track.left() - thumb_width * 0.5) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        state.h_offset = (wanted * max_offset as f32).round() as usize;
    }

    let progress = state.h_offset.min(max_offset) as f32 / max_offset as f32;
    let thumb = Rect::from_min_size(
        Pos2::new(track.left() + span * progress, track.top() + 1.0),
        Vec2::new(thumb_width, (track.height() - 2.0).max(1.0)),
    );
    paint_thumb(
        &painter,
        thumb,
        theme,
        response.hovered() || response.dragged(),
        false,
    );
}

/// The handle, in whichever look the theme calls for.
///
/// An Aqua theme gets the glass capsule, because a Tiger window with a flat
/// grey scroll handle reads as two applications in one frame; everything else
/// keeps the flat bar, which is what a modern theme wants.
fn paint_thumb(painter: &egui::Painter, thumb: Rect, theme: &Theme, active: bool, vertical: bool) {
    use crate::config::theme::WindowButtonStyle;
    use crate::ui::shading;

    match (theme.window_buttons.style, theme.scrollbar_handle) {
        (WindowButtonStyle::Aqua, Some(base)) => {
            let base = if active {
                shading::lighten(base, 0.12)
            } else {
                base
            };
            shading::aqua_capsule(painter, thumb, base, vertical);
        }
        (_, handle) => {
            let base = handle.unwrap_or(theme.selection);
            let colour = if active {
                palette::blend(base, theme.foreground, 0.35)
            } else {
                palette::blend(base, theme.foreground, 0.1)
            };
            painter.rect_filled(thumb, 2.0, colour);
        }
    }
}

/// Lines to scroll for a pointer `over` points past the edge of the view,
/// positive being downwards.
///
/// One line per cell of overshoot, so the further out the pointer is dragged
/// the faster the selection grows, and always at least one so that resting a
/// pixel past the edge still moves. Capped: a pointer flung to the far side of
/// the screen should not clear the whole scrollback in three frames.
fn autoscroll_lines(over: f32, cell_y: f32) -> i64 {
    const MAX: f32 = 8.0;
    let lines = (over / cell_y.max(1.0)).abs().ceil().clamp(1.0, MAX) as i64;
    // `scroll_lines` counts a positive `lines` as scrolling *back*, the way a
    // wheel does, and dragging below the bottom edge goes forwards.
    if over < 0.0 {
        lines
    } else {
        -lines
    }
}

/// Moves the anchor by whole lines and re-pins to the bottom on arrival.
///
/// Line-quantised on purpose: the terminal scrolls by rows, not pixels, so a
/// fractional offset would only ever be rounded away.
fn scroll_lines(state: &mut ViewState, from: usize, lines: i64, max_top: usize) {
    let next = (from as i64 - lines).clamp(0, max_top as i64) as usize;
    state.anchor = if next >= max_top {
        ScrollAnchor::Bottom
    } else {
        ScrollAnchor::At(next)
    };
}

/// Draws the scrollback scrollbar and handles dragging it.
///
/// Hand-drawn because the terminal is not an `egui::ScrollArea`: scrolling here
/// is an anchor into the scrollback, not a pixel offset over a laid-out widget,
/// so there is no scroll area to borrow a bar from.
#[allow(clippy::too_many_arguments)]
fn scrollbar(
    ui: &Ui,
    track: Rect,
    state: &mut ViewState,
    theme: &Theme,
    tab_uid: u64,
    total: usize,
    rows: usize,
    max_top: usize,
) {
    let painter = ui.painter_at(track);
    // The track is drawn even with nothing to scroll, so the reserved strip
    // reads as part of the terminal rather than as a gap beside it.
    painter.rect_filled(
        track,
        0.0,
        palette::blend(theme.background, theme.foreground, 0.07),
    );

    if max_top == 0 {
        return;
    }

    // Its own id. Sharing the terminal's would give the bar the keyboard focus
    // the terminal defends with a focus-lock filter, and typing would stop
    // reaching IRIS.
    let response = ui.interact(
        track,
        egui::Id::new(("nit-terminal-scrollbar", tab_uid)),
        Sense::click_and_drag(),
    );

    let visible = (rows as f32 / total as f32).clamp(0.0, 1.0);
    let thumb_height = (track.height() * visible).max(MIN_THUMB_HEIGHT.min(track.height()));
    let span = (track.height() - thumb_height).max(0.0);

    if let Some(pos) = response.interact_pointer_pos() {
        // Centred on the pointer, so grabbing the thumb feels like holding it
        // rather than snapping it somewhere else first.
        let wanted = if span > 0.0 {
            ((pos.y - track.top() - thumb_height * 0.5) / span).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let line = (wanted * max_top as f32).round() as usize;
        state.anchor = if line >= max_top {
            ScrollAnchor::Bottom
        } else {
            ScrollAnchor::At(line)
        };
    }

    // The wheel works over the bar too. The grid has its own handler, but it
    // only sees the pointer while it is over the grid, and the strip beside it
    // is exactly where people reach to scroll.
    let current = match state.anchor {
        ScrollAnchor::Bottom => max_top,
        ScrollAnchor::At(line) => line.min(max_top),
    };
    if response.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        let cell_y = track.height() / rows.max(1) as f32;
        let lines = (scroll / cell_y.max(1.0)).round() as i64;
        if lines != 0 {
            scroll_lines(state, current, lines, max_top);
        }
    }

    // Read back from the anchor rather than reusing the caller's `top_line`,
    // which was worked out before the drag above could move it.
    let progress = match state.anchor {
        ScrollAnchor::Bottom => 1.0,
        ScrollAnchor::At(line) => line.min(max_top) as f32 / max_top as f32,
    };
    let thumb = Rect::from_min_size(
        Pos2::new(track.left() + 1.0, track.top() + span * progress),
        Vec2::new((track.width() - 2.0).max(1.0), thumb_height),
    );
    paint_thumb(
        &painter,
        thumb,
        theme,
        response.hovered() || response.dragged(),
        true,
    );
}

/// Whether a blinking cursor is in its visible half right now.
///
/// Always true when blinking is off. A repaint has to be pending for this to
/// animate; the caller keeps one scheduled while blink is enabled, since a
/// terminal sitting at an idle prompt gets no other reason to redraw.
fn cursor_phase_on(ui: &Ui, opts: &RenderOpts) -> bool {
    if !opts.cursor_blink {
        return true;
    }
    // ~0.6 s per half, which is roughly where every other terminal sits.
    let time = ui.input(|i| i.time);
    (time * 1.6).floor() as i64 % 2 == 0
}

#[allow(clippy::too_many_arguments)]
fn paint_row(
    painter: &egui::Painter,
    row: &crate::term::Row,
    line_index: usize,
    // First grid column of the slice on this display row, and how many columns
    // fit. Both are grid coordinates; only the drawing is shifted, so
    // selection, syntax and the cursor keep working in one coordinate system.
    from: usize,
    view_cols: usize,
    y: f32,
    left: f32,
    cell: Vec2,
    theme: &Theme,
    state: &ViewState,
    font: &FontId,
    // Column whose glyph the cursor will draw itself, if it is on this row.
    hide_glyph_at: Option<usize>,
    // Per-column syntax colour, scanned over the whole row by the caller — not
    // the slice, because whether a `^` is inside quotes depends on text that
    // may be on an earlier display row. Empty when the feature is off.
    overrides: &[Option<Color32>],
) {
    let to = (from + view_cols).min(row.cells.len());
    let selection = state.selection;

    // Everything about how one column looks, in one place, so the run-batching
    // below compares exactly what it draws.
    let appearance = |col: usize| -> (Color32, Color32, bool) {
        let cell = &row.cells[col];
        let (mut fg, bg) = palette::resolve(cell, theme);
        if let Some(Some(colour)) = overrides.get(col) {
            // An override, not a replacement. A cell the remote side coloured
            // deliberately keeps that colour: the scan is a guess about
            // arbitrary text, and it must never overrule an SGR sequence.
            if cell.fg == Color::Default && !cell.attrs.contains(Attrs::REVERSE) {
                fg = *colour;
            }
        }
        let selected = selection.is_some_and(|s| s.contains(line_index, col));
        (fg, bg, selected)
    };

    if from >= to {
        return;
    }

    let mut col = from;
    let mut look = appearance(from);
    while col < to {
        let (fg, bg, selected) = look;

        // Extend the run while appearance is unchanged, so a line of plain
        // text becomes one background rect instead of `cols` of them. Each
        // column is costed once: the appearance that ended the run opens the
        // next one.
        let mut end = col + 1;
        while end < to {
            let next = appearance(end);
            if next != (fg, bg, selected) {
                look = next;
                break;
            }
            end += 1;
        }

        let run_rect = Rect::from_min_size(
            Pos2::new(glyph_x(left, col - from, cell), y),
            Vec2::new((end - col) as f32 * cell.x, cell.y),
        );

        let effective_bg = if selected { theme.selection } else { bg };
        if effective_bg != theme.background {
            painter.rect_filled(run_rect, 0.0, effective_bg);
        }

        // One draw per glyph, positioned on the lattice. Letting the text
        // renderer lay out the whole run instead is what let the text drift
        // away from the columns the cursor and the rects are drawn at. Blank
        // cells are skipped, so a mostly empty row still costs a handful of
        // draws rather than one per column.
        let mut has_text = false;
        for (c, cell_at) in (col..end).zip(&row.cells[col..end]) {
            if cell_at.is_blank() {
                continue;
            }
            has_text = true;
            if hide_glyph_at == Some(c) {
                continue;
            }
            painter.text(
                Pos2::new(glyph_x(left, c - from, cell), y),
                Align2::LEFT_TOP,
                cell_at.ch,
                font.clone(),
                fg,
            );
        }

        if has_text {
            if row.cells[col].attrs.contains(crate::term::Attrs::UNDERLINE) {
                let uy = run_rect.bottom() - 1.0;
                painter.line_segment(
                    [
                        Pos2::new(run_rect.left(), uy),
                        Pos2::new(run_rect.right(), uy),
                    ],
                    Stroke::new(1.0_f32, fg),
                );
            }
            if row.cells[col].attrs.contains(crate::term::Attrs::STRIKE) {
                let sy = run_rect.center().y;
                painter.line_segment(
                    [
                        Pos2::new(run_rect.left(), sy),
                        Pos2::new(run_rect.right(), sy),
                    ],
                    Stroke::new(1.0_f32, fg),
                );
            }
        }

        col = end;
    }
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
struct MouseOutcome {
    copy_selection: bool,
    cursor_move: Option<i64>,
}

#[allow(clippy::too_many_arguments)]
fn handle_mouse(
    ui: &Ui,
    response: &Response,
    state: &mut ViewState,
    rect: Rect,
    cell: Vec2,
    top_line: usize,
    grid: &Grid,
    max_top: usize,
    max_h_offset: usize,
    segments: &[wrap::Segment],
    mode: wrap::Mode,
    copy_on_select: bool,
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
            scroll_lines(state, top_line, lines, max_top);
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
            None => (top_line, (mode.offset + offset).min(last)),
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
                scroll_lines(state, top_line, lines, max_top);
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
    use crate::term::Grid;

    /// Dragging a selection past the edge of the view scrolls it, the way it
    /// does in a text editor: away from the edge the pointer is past, faster
    /// the further out it is dragged, and never nothing at all.
    #[test]
    fn dragging_past_an_edge_scrolls_towards_it() {
        // Above the top edge: back through the scrollback, which is the
        // direction a positive count means to `scroll_lines`.
        assert_eq!(autoscroll_lines(-1.0, 14.0), 1, "a pixel past the top");
        assert_eq!(autoscroll_lines(-30.0, 14.0), 3);
        // Below the bottom edge: forwards.
        assert_eq!(autoscroll_lines(1.0, 14.0), -1, "a pixel past the bottom");
        assert_eq!(autoscroll_lines(-1000.0, 14.0), 8, "and capped");
        assert_eq!(autoscroll_lines(1000.0, 14.0), -8);
    }

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

    /// Shared by the wheel and by dragging the scrollbar, so the re-pinning
    /// rule has to hold for both: reaching the end goes back to following the
    /// live output rather than freezing on the last line.
    #[test]
    fn scrolling_to_the_end_re_pins_to_the_live_output() {
        let mut state = ViewState::default();

        // A positive wheel delta means "towards the history".
        scroll_lines(&mut state, 100, 3, 100);
        assert_eq!(state.anchor, ScrollAnchor::At(97));

        scroll_lines(&mut state, 97, -3, 100);
        assert_eq!(state.anchor, ScrollAnchor::Bottom, "should follow again");

        // Past the oldest line clamps instead of underflowing.
        scroll_lines(&mut state, 2, 40, 100);
        assert_eq!(state.anchor, ScrollAnchor::At(0));

        // Nothing scrolled off at all: the only valid anchor is Bottom.
        scroll_lines(&mut state, 0, 5, 0);
        assert_eq!(state.anchor, ScrollAnchor::Bottom);
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
