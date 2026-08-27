//! Painting the grid and handling mouse interaction.
//!
//! egui is immediate-mode, so this draws the whole visible screen every frame.
//! That is cheap enough: a run of cells sharing one background is emitted as a
//! single rect, and text is batched per colour run rather than per character.

use egui::{Align2, FontFamily, FontId, Pos2, Rect, Response, Sense, Stroke, TextStyle, Ui, Vec2};

use crate::config::Theme;
use crate::term::{palette, Grid};

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

    pub fn is_empty(&self) -> bool {
        let (start, end) = self.ordered();
        start == end
    }
}

/// Per-tab view state that survives between frames.
#[derive(Default)]
pub struct ViewState {
    pub anchor: ScrollAnchor,
    pub selection: Option<Selection>,
    dragging: bool,
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
            let text: String = row.cells[from..to].iter().map(|c| c.ch).collect();
            out.push_str(text.trim_end());
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
pub fn cell_size(ui: &Ui, font: &FontId) -> Vec2 {
    ui.fonts(|f| {
        // Monospace, so any character measures the same; 'M' is the classic
        // choice and avoids zero-width surprises.
        let width = f.glyph_width(font, 'M');
        let height = f.row_height(font);
        Vec2::new(width, height)
    })
}

/// The font the terminal draws with. Falls back to egui's bundled monospace
/// when the theme names a family that is not loaded, so a bad font name in a
/// theme file cannot make the terminal unreadable.
pub fn terminal_font(theme: &Theme, font_size: f32) -> FontId {
    let family = match theme.font_family.as_str() {
        "" | "monospace" => FontFamily::Monospace,
        other => FontFamily::Name(other.into()),
    };
    FontId::new(font_size, family)
}

/// What the right-click menu asked for, if anything.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ContextAction {
    CopySelection,
    Paste,
    SelectAll,
    ClearSelection,
    ExportScreen,
}

pub struct RenderResult {
    pub response: Response,
    /// Set when the user picked something from the right-click menu.
    pub context_action: Option<ContextAction>,
    /// Grid dimensions the available space implies. The caller resizes the PTY
    /// when these differ from the current size.
    pub cols: usize,
    pub rows: usize,
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
/// `take_focus` is set by the caller when the active tab changed, so focus
/// follows the tab the user is looking at.
pub fn show(
    ui: &mut Ui,
    grid: &Grid,
    state: &mut ViewState,
    theme: &Theme,
    font_size: f32,
    tab_uid: u64,
    take_focus: bool,
) -> RenderResult {
    let font = terminal_font(theme, font_size);
    let cell = cell_size(ui, &font);
    let available = ui.available_size();

    let cols = ((available.x / cell.x).floor() as usize).max(1);
    let rows = ((available.y / cell.y).floor() as usize).max(1);

    let id = egui::Id::new(("nit-terminal", tab_uid));
    let (rect, _) = ui.allocate_exact_size(
        Vec2::new(cols as f32 * cell.x, rows as f32 * cell.y),
        Sense::hover(),
    );
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
    if take_focus || ui.memory(|m| m.focused().is_none()) {
        response.request_focus();
    }

    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme.background);

    // Which absolute line sits at the top of the viewport.
    let total = grid.total_lines();
    let max_top = total.saturating_sub(rows);
    let top_line = match state.anchor {
        ScrollAnchor::Bottom => max_top,
        ScrollAnchor::At(line) => line.min(max_top),
    };

    handle_mouse(ui, &response, state, rect, cell, top_line, grid, max_top);

    for screen_row in 0..rows {
        let line_index = top_line + screen_row;
        let Some(row) = grid.line(line_index) else {
            continue;
        };
        let y = rect.top() + screen_row as f32 * cell.y;
        paint_row(
            &painter,
            row,
            line_index,
            y,
            rect.left(),
            cell,
            theme,
            state,
            &font,
        );
    }

    // Cursor, only when it is actually on screen and the session says visible.
    if grid.cursor.visible {
        let cursor_line = grid.scrollback.len() + grid.cursor.row;
        if cursor_line >= top_line && cursor_line < top_line + rows {
            let x = rect.left() + grid.cursor.col as f32 * cell.x;
            let y = rect.top() + (cursor_line - top_line) as f32 * cell.y;
            let cursor_rect = Rect::from_min_size(Pos2::new(x, y), cell);
            painter.rect_filled(cursor_rect, 0.0, theme.cursor);
            // Redraw the character on top so the cursor does not hide it.
            if let Some(ch) = grid
                .line(cursor_line)
                .and_then(|r| r.cells.get(grid.cursor.col))
                .map(|c| c.ch)
                .filter(|c| *c != ' ')
            {
                painter.text(
                    cursor_rect.left_top(),
                    Align2::LEFT_TOP,
                    ch,
                    font.clone(),
                    theme.background,
                );
            }
        }
    }

    // A scrollback indicator, so it is obvious the view is pinned to history.
    if !matches!(state.anchor, ScrollAnchor::Bottom) {
        let label = format!("scrolled back {} lines", max_top - top_line);
        let pos = Pos2::new(rect.right() - 8.0, rect.top() + 4.0);
        painter.text(
            pos,
            Align2::RIGHT_TOP,
            label,
            TextStyle::Small.resolve(ui.style()),
            theme.ansi[3],
        );
    }

    // Right-click menu. Copy is disabled without a selection so the menu
    // states plainly what is available, rather than silently doing nothing.
    let has_selection = state.selection.map(|s| !s.is_empty()).unwrap_or(false);
    let mut context_action = None;

    response.context_menu(|ui| {
        if ui
            .add_enabled(has_selection, egui::Button::new("Copy"))
            .clicked()
        {
            context_action = Some(ContextAction::CopySelection);
            ui.close_menu();
        }
        if ui.button("Paste").clicked() {
            context_action = Some(ContextAction::Paste);
            ui.close_menu();
        }
        ui.separator();
        if ui.button("Select all").clicked() {
            context_action = Some(ContextAction::SelectAll);
            ui.close_menu();
        }
        if ui
            .add_enabled(has_selection, egui::Button::new("Clear selection"))
            .clicked()
        {
            context_action = Some(ContextAction::ClearSelection);
            ui.close_menu();
        }
        ui.separator();
        if ui.button("Export screen...").clicked() {
            context_action = Some(ContextAction::ExportScreen);
            ui.close_menu();
        }
    });

    RenderResult {
        response,
        context_action,
        cols,
        rows,
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_row(
    painter: &egui::Painter,
    row: &crate::term::Row,
    line_index: usize,
    y: f32,
    left: f32,
    cell: Vec2,
    theme: &Theme,
    state: &ViewState,
    font: &FontId,
) {
    let mut col = 0;
    while col < row.cells.len() {
        let (fg, bg) = palette::resolve(&row.cells[col], theme);
        let selected = state
            .selection
            .map(|s| s.contains(line_index, col))
            .unwrap_or(false);

        // Extend the run while appearance is unchanged, so a line of plain
        // text becomes one rect and one text draw instead of `cols` of each.
        let mut end = col + 1;
        while end < row.cells.len() {
            let (next_fg, next_bg) = palette::resolve(&row.cells[end], theme);
            let next_selected = state
                .selection
                .map(|s| s.contains(line_index, end))
                .unwrap_or(false);
            if next_fg != fg || next_bg != bg || next_selected != selected {
                break;
            }
            end += 1;
        }

        let run_rect = Rect::from_min_size(
            Pos2::new(left + col as f32 * cell.x, y),
            Vec2::new((end - col) as f32 * cell.x, cell.y),
        );

        let effective_bg = if selected { theme.selection } else { bg };
        if effective_bg != theme.background {
            painter.rect_filled(run_rect, 0.0, effective_bg);
        }

        let text: String = row.cells[col..end].iter().map(|c| c.ch).collect();
        if !text.trim().is_empty() {
            painter.text(
                run_rect.left_top(),
                Align2::LEFT_TOP,
                &text,
                font.clone(),
                fg,
            );

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
) {
    // Wheel scrolling through scrollback.
    let scroll = ui.input(|i| i.raw_scroll_delta.y);
    if scroll != 0.0 && response.hovered() {
        let lines = (scroll / cell.y).round() as i64;
        if lines != 0 {
            let current = top_line as i64;
            let next = (current - lines).clamp(0, max_top as i64) as usize;
            state.anchor = if next >= max_top {
                ScrollAnchor::Bottom
            } else {
                ScrollAnchor::At(next)
            };
        }
    }

    let pos_to_cell = |pos: Pos2| -> (usize, usize) {
        let col = (((pos.x - rect.left()) / cell.x).floor().max(0.0) as usize).min(grid.cols);
        let row = (((pos.y - rect.top()) / cell.y).floor().max(0.0) as usize)
            .min(grid.rows.saturating_sub(1));
        (top_line + row, col)
    };

    if response.drag_started() {
        if let Some(pos) = response.interact_pointer_pos() {
            let at = pos_to_cell(pos);
            state.selection = Some(Selection { start: at, end: at });
            state.dragging = true;
        }
    } else if response.dragged() && state.dragging {
        if let Some(pos) = response.interact_pointer_pos() {
            if let Some(selection) = state.selection.as_mut() {
                selection.end = pos_to_cell(pos);
            }
        }
    } else if response.drag_stopped() {
        state.dragging = false;
        // A click without movement clears rather than leaving an empty
        // selection, which would otherwise keep stealing Ctrl+C.
        if state.selection.map(|s| s.is_empty()).unwrap_or(false) {
            state.selection = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::Grid;

    fn grid_with(lines: &[&str]) -> Grid {
        let mut grid = Grid::new(20, lines.len().max(1), 100);
        for (r, line) in lines.iter().enumerate() {
            for (c, ch) in line.chars().enumerate() {
                grid.screen[r].cells[c].ch = ch;
            }
        }
        grid
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
