//! The scrollbars, and moving the view with the wheel or a drag.
//!
//! Both bars are drawn by hand rather than with egui's own: the view scrolls
//! in whole lines of a grid, not in points, and the scrollback is addressed by
//! line rather than by pixel.

use super::*;

/// Width of the scrollback scrollbar, in points.
pub(super) const SCROLLBAR_WIDTH: f32 = 10.0;

/// Shortest the thumb is allowed to get, so a long history still leaves
/// something you can actually grab.
const MIN_THUMB_HEIGHT: f32 = 24.0;

/// Draws the horizontal scrollbar and handles dragging it.
///
/// Only present when lines are clipped: that is the mode in which part of a line
/// is off to the side, and this is how it is reached without resizing the window.
#[allow(clippy::too_many_arguments)]
pub(super) fn h_scrollbar(
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
pub(super) fn autoscroll_lines(over: f32, cell_y: f32) -> i64 {
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

/// Moves the anchor by whole display rows and re-pins to the bottom on arrival.
///
/// Row-quantised on purpose: the terminal scrolls by rows, not pixels, so a
/// fractional offset would only ever be rounded away. Display rows rather than
/// logical lines is the whole of the fix for output that could not be read:
/// one notch over a `zwrite` that wraps two hundred times used to skip the
/// entire thing, so the only way to see the middle of a long line was to export
/// the transcript.
pub(super) fn scroll_lines(
    state: &mut ViewState,
    from: wrap::Top,
    lines: i64,
    max_top: wrap::Top,
    total: usize,
    mode: wrap::Mode,
    used: &dyn Fn(usize) -> usize,
) {
    // A positive `lines` means "towards the history", which is backwards
    // through the transcript.
    let next = wrap::step(total, from, -lines, mode, used).min(max_top);
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
pub(super) fn scrollbar(
    ui: &Ui,
    track: Rect,
    state: &mut ViewState,
    theme: &Theme,
    tab_uid: u64,
    total: usize,
    rows: usize,
    max_top: wrap::Top,
    mode: wrap::Mode,
    used: &dyn Fn(usize) -> usize,
) {
    let painter = ui.painter_at(track);
    // The track is drawn even with nothing to scroll, so the reserved strip
    // reads as part of the terminal rather than as a gap beside it.
    painter.rect_filled(
        track,
        0.0,
        palette::blend(theme.background, theme.foreground, 0.07),
    );

    if max_top == wrap::Top::default() {
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
        // Whole lines, except when the whole transcript is one line taller
        // than the window - a single enormous `zwrite` - and there are no lines
        // to interpolate over. Then the bar runs through that line's own rows,
        // which is the only way it can reach the end at all.
        let at = if max_top.line == 0 {
            wrap::Top {
                line: 0,
                skip: (wanted * max_top.skip as f32).round() as usize,
            }
        } else {
            wrap::Top::line((wanted * max_top.line as f32).round() as usize)
        };
        state.anchor = if at >= max_top {
            ScrollAnchor::Bottom
        } else {
            ScrollAnchor::At(at)
        };
    }

    // The wheel works over the bar too. The grid has its own handler, but it
    // only sees the pointer while it is over the grid, and the strip beside it
    // is exactly where people reach to scroll.
    let current = match state.anchor {
        ScrollAnchor::Bottom => max_top,
        ScrollAnchor::At(at) => at.min(max_top),
    };
    if response.hovered() {
        let scroll = ui.input(|i| i.raw_scroll_delta.y);
        let cell_y = track.height() / rows.max(1) as f32;
        let lines = (scroll / cell_y.max(1.0)).round() as i64;
        if lines != 0 {
            scroll_lines(state, current, lines, max_top, total, mode, used);
        }
    }

    // Read back from the anchor rather than reusing the caller's `top_line`,
    // which was worked out before the drag above could move it.
    let progress = match state.anchor {
        ScrollAnchor::Bottom => 1.0,
        ScrollAnchor::At(at) => {
            let at = at.min(max_top);
            if max_top.line == 0 {
                at.skip as f32 / max_top.skip.max(1) as f32
            } else {
                // The fraction of the line the top sits inside is counted too,
                // so the thumb keeps moving while a line taller than the window
                // is being scrolled through rather than sticking until it ends.
                let height = mode.rows_for(used(at.line)).max(1) as f32;
                (at.line as f32 + at.skip as f32 / height) / max_top.line as f32
            }
        }
    }
    .clamp(0.0, 1.0);
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

#[cfg(test)]
mod tests {
    use super::*;

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

    /// Shared by the wheel and by dragging the scrollbar, so the re-pinning
    /// rule has to hold for both: reaching the end goes back to following the
    /// live output rather than freezing on the last line.
    #[test]
    fn scrolling_to_the_end_re_pins_to_the_live_output() {
        let mut state = ViewState::default();
        // Every line one row tall, so a row is a line and the arithmetic is the
        // same one this test has always asserted.
        let short = |_: usize| 10usize;
        let mode = wrap::Mode::wrapping(80);
        let top = wrap::Top::line;

        // A positive wheel delta means "towards the history".
        scroll_lines(&mut state, top(100), 3, top(100), 200, mode, &short);
        assert_eq!(state.anchor, ScrollAnchor::At(top(97)));

        scroll_lines(&mut state, top(97), -3, top(100), 200, mode, &short);
        assert_eq!(state.anchor, ScrollAnchor::Bottom, "should follow again");

        // Past the oldest line clamps instead of underflowing.
        scroll_lines(&mut state, top(2), 40, top(100), 200, mode, &short);
        assert_eq!(state.anchor, ScrollAnchor::At(top(0)));

        // Nothing scrolled off at all: the only valid anchor is Bottom.
        scroll_lines(
            &mut state,
            wrap::Top::default(),
            5,
            wrap::Top::default(),
            1,
            mode,
            &short,
        );
        assert_eq!(state.anchor, ScrollAnchor::Bottom);
    }

    /// The report this was built for: a line long enough to wrap into far more
    /// rows than the window has could not be read, because every notch of the
    /// wheel skipped the whole line and landed on the command before or after
    /// it. Now a notch is a row.
    #[test]
    fn the_wheel_walks_through_a_line_that_wraps_instead_of_over_it() {
        let mut state = ViewState::default();
        let mode = wrap::Mode::wrapping(100);
        // One 200,000-character line in the middle: two thousand display rows.
        let used = |line: usize| if line == 1 { 200_000 } else { 8 };
        let max_top = wrap::Top {
            line: 1,
            skip: 1990,
        };

        // Three notches back from the bottom stay inside that same line.
        scroll_lines(&mut state, max_top, 3, max_top, 3, mode, &used);
        assert_eq!(
            state.anchor,
            ScrollAnchor::At(wrap::Top {
                line: 1,
                skip: 1987
            }),
            "the wheel should move three rows, not jump off the line"
        );
    }
}
