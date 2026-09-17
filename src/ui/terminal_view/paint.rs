//! Turning one display row of cells into shapes.
//!
//! Colour, selection, search highlighting and the runs they batch into. What
//! this produces goes to [`super::glyphs`], which gathers a row into a single
//! shape.

use super::*;

/// Why a run of columns is drawn on a coloured ground, if it is.
///
/// One value rather than a pair of flags, because they are mutually exclusive
/// and the run-batching compares whatever this is: two flags would have let a
/// selected hit and a plain selection batch together and then paint in two
/// different colours.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Highlight {
    None,
    Selected,
    /// A search hit, but not the one being looked at.
    Hit,
    CurrentHit,
}

/// Per-column syntax colour for one row, or an empty vector when the feature is
/// off.
///
/// Computed per row rather than per cell because the scan has to see a whole
/// line to know whether a `^` is inside quotes.
pub(super) fn syntax_overrides(cells: &[Cell], theme: &Theme, out: &mut Vec<Option<Color32>>) {
    out.clear();
    out.resize(cells.len(), None);
    for span in syntax::scan(cells) {
        let colour = theme.syntax_color(span.kind);
        out[span.start..span.end.min(cells.len())].fill(Some(colour));
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn paint_row(
    // Collected rather than drawn: `Painter::add` takes the paint list's lock
    // once per shape, and a screen of text is tens of thousands of shapes. The
    // caller hands the whole frame over in one `extend`.
    out: &mut Vec<egui::Shape>,
    glyphs: &mut GlyphCache,
    // The row's glyphs, gathered into one shape, and the underlines and strikes
    // that go over them.
    text: &mut RowMesh,
    marks: &mut Vec<egui::Shape>,
    ctx: &egui::Context,
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
    selection: Option<Selection>,
    // Search hits on this logical line, and the one of them being looked at.
    // Drawn behind the text like a selection, and in a stronger colour for the
    // current one so that stepping through them can be followed.
    hits: &[search::Match],
    current_hit: Option<search::Match>,
    font: &FontId,
    // Column whose glyph the cursor will draw itself, if it is on this row.
    hide_glyph_at: Option<usize>,
    // Per-column syntax colour, scanned over the whole row by the caller — not
    // the slice, because whether a `^` is inside quotes depends on text that
    // may be on an earlier display row. Empty when the feature is off.
    overrides: &[Option<Color32>],
) {
    let to = (from + view_cols).min(row.cells.len());

    // One allocation for the row's glyphs, rather than one every time the mesh
    // outgrows itself. Never short: a blank cell draws nothing, so the columns
    // in use are an upper bound on the characters drawn.
    text.reserve(row.used_width().min(to).saturating_sub(from));

    // Nothing selected and nothing found on this row is the ordinary case, and
    // it lets every column skip three range checks.
    let highlighted = selection.is_some() || current_hit.is_some() || !hits.is_empty();

    // Everything about how one column looks, in one place, so the run-batching
    // below compares exactly what it draws.
    let appearance = |col: usize| -> (Color32, Color32, Highlight) {
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
        // A selection the user made wins over a hit the search found: the
        // selection is what the next Ctrl+C will copy, and it must be visible.
        let highlight = if !highlighted {
            Highlight::None
        } else if selection.is_some_and(|s| s.contains(line_index, col)) {
            Highlight::Selected
        } else if current_hit.is_some_and(|m| m.covers(line_index, col)) {
            Highlight::CurrentHit
        } else if hits.iter().any(|m| m.covers(line_index, col)) {
            Highlight::Hit
        } else {
            Highlight::None
        };
        (fg, bg, highlight)
    };

    if from >= to {
        return;
    }

    let mut col = from;
    let mut look = appearance(from);
    while col < to {
        let (fg, bg, highlight) = look;

        // Extend the run while appearance is unchanged, so a line of plain
        // text becomes one background rect instead of `cols` of them. Each
        // column is costed once: the appearance that ended the run opens the
        // next one.
        let mut end = col + 1;
        while end < to {
            let next = appearance(end);
            if next != (fg, bg, highlight) {
                look = next;
                break;
            }
            end += 1;
        }

        let run_rect = Rect::from_min_size(
            Pos2::new(glyph_x(left, col - from, cell), y),
            Vec2::new((end - col) as f32 * cell.x, cell.y),
        );

        let effective_bg = match highlight {
            Highlight::None => bg,
            Highlight::Selected => theme.selection,
            // Derived from the selection colour rather than being settings of
            // their own: a theme that has been made readable has already
            // decided what highlighted text looks like in it, and two more
            // colours to keep in step across eleven themes would be two more
            // ways for one of them to come out unreadable.
            Highlight::Hit => palette::blend(theme.selection, theme.foreground, 0.22),
            Highlight::CurrentHit => palette::blend(theme.selection, theme.foreground, 0.55),
        };
        if effective_bg != theme.background {
            out.push(egui::Shape::rect_filled(run_rect, 0.0, effective_bg));
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
            // The lattice point the character sits on, as an offset from
            // where the row's own shape will be placed.
            let galley = glyphs.get(ctx, cell_at.ch, font);
            text.push(&galley, glyph_x(left, c - from, cell), fg);
        }

        if has_text {
            if row.cells[col].attrs.contains(crate::term::Attrs::UNDERLINE) {
                let uy = run_rect.bottom() - 1.0;
                marks.push(egui::Shape::line_segment(
                    [
                        Pos2::new(run_rect.left(), uy),
                        Pos2::new(run_rect.right(), uy),
                    ],
                    Stroke::new(1.0_f32, fg),
                ));
            }
            if row.cells[col].attrs.contains(crate::term::Attrs::STRIKE) {
                let sy = run_rect.center().y;
                marks.push(egui::Shape::line_segment(
                    [
                        Pos2::new(run_rect.left(), sy),
                        Pos2::new(run_rect.right(), sy),
                    ],
                    Stroke::new(1.0_f32, fg),
                ));
            }
        }

        col = end;
    }

    // Backgrounds are already in `out` and belong under the text; the marks
    // belong over it. One shape for every character on the row goes between.
    if let Some(shape) = text.take(y, ctx.pixels_per_point()) {
        out.push(shape);
    }
    out.append(marks);
}
