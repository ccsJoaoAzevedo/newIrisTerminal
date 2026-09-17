//! Where the panes are: splitting, measuring, and fitting the window.
//!
//! Coordinates rather than content. The window is drawn without the system's
//! decorations, so the character grid's size has to be derived from the space
//! left over and fed back to the PTY, which is what [`WindowFit`] converges.

use super::*;

/// Fallback PTY size, used only before the first frame has measured the
/// window. After that, new sessions open at the size the terminal is actually
/// being drawn at.
///
/// This matters more than it looks: IRIS truncates output at the device right
/// margin rather than wrapping it, so a session opened at 80 columns loses
/// everything past column 80 until it is resized. Opening at the real size
/// means nothing is cut in the first place.
/// A strip of the title bar kept free of tabs, in front of the window
/// controls.
///
/// The handle that is always there. A row of tabs long enough to fill the bar
/// would otherwise leave nothing to drag the window by, and with the system's
/// frame turned off there is then no way to move it at all. Held back at the
/// end of the row rather than in front of the tabs, where an empty gap looks
/// like a tab that failed to draw.
pub(super) const TITLE_FREE_STRIP: f32 = 28.0;

/// How wide one title-bar control is, for working out what to keep back for
/// them. egui sizes them from the row height, which is `interact_size.y`.
pub(super) const TITLE_CONTROL_SIDE: f32 = 24.0;

pub(super) const FALLBACK_COLS: u16 = 80;

pub(super) const FALLBACK_ROWS: u16 = 24;

/// How many frames the opening size may take to settle. A correction normally
/// lands on the first one; the budget is there so a window manager that refuses
/// the size it is asked for cannot leave the app resizing itself forever.
pub(super) const FIT_ATTEMPTS: u8 = 8;

/// A window that has yet to be sized to the terminal geometry it was asked to
/// open at.
///
/// The size in the settings file is a character geometry, not a pixel one, and
/// the pixels it works out to depend on the font egui ends up with and on how
/// tall the panels above the terminal are laid out. Neither is known before the
/// first frame, so the window opens at an estimate and is corrected here once
/// there is a measurement to correct it with.
pub(super) struct WindowFit {
    pub(super) cols: u16,
    pub(super) rows: u16,
    /// Frames left to get there.
    pub(super) attempts: u8,
    /// Whether the window is being centred rather than opened at a saved
    /// position. Resizing anchors the top-left corner, which would walk a
    /// centred window off-centre, so its centre is held instead.
    pub(super) recentre: bool,
    /// The centre to hold, taken from the first frame that had a window rect to
    /// take it from.
    pub(super) centre: Option<egui::Pos2>,
}

/// Thickness of the divider between two panes, in points. Wide enough to be
/// aimed at with a mouse, which it has to be: it is the handle the split is
/// resized by.
pub(super) const SPLIT_DIVIDER: f32 = 6.0;

/// Smallest a pane may be dragged down to, in points.
///
/// A pane thinner than this is not a pane, and a session in one would be
/// reporting a terminal a couple of characters wide to IRIS - which truncates
/// its output at the margin it is told about, so everything past it would be
/// lost rather than merely hidden. The divider stops here instead.
const MIN_PANE: f32 = 80.0;

/// How much of `usable` the first pane gets at `ratio`, never letting either
/// side fall below [`MIN_PANE`].
///
/// Clamped here rather than where the ratio is stored: the room a split has
/// changes with the window, and a ratio that was reachable in a wide window
/// must not squeeze a pane out of existence in a narrow one.
pub(super) fn split_extent(usable: f32, ratio: f32) -> f32 {
    if usable <= MIN_PANE * 2.0 {
        // No room to honour a minimum on both sides, so the only fair split is
        // down the middle.
        return usable / 2.0;
    }
    (usable * ratio).clamp(MIN_PANE, usable - MIN_PANE)
}

/// The ratio a drag of the divider leaves behind.
///
/// Measured from where the divider actually is rather than added to the stored
/// ratio, so a drag that ran into the minimum does not bank the movement past
/// it and leave the handle lagging behind the pointer on the way back.
fn dragged_ratio(usable: f32, ratio: f32, delta: f32) -> f32 {
    if usable <= 0.0 {
        return ratio;
    }
    ((split_extent(usable, ratio) + delta) / usable).clamp(0.0, 1.0)
}

/// Which way the second pane sits next to the first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitDir {
    Right,
    Bottom,
}

/// A second session inside one tab, and which way it sits.
///
/// One split rather than a tree of them: two sessions in one tab is the thing
/// people actually ask for - a routine running in one and a global inspected in
/// the other - and a general pane tree would be a window manager inside a
/// terminal. So a split tab cannot be split again, and the strip entry has only
/// ever two names to carry.
pub struct Split {
    pub dir: SplitDir,
    /// The session the split opened. Boxed: a [`Tab`] holds one of these, so
    /// the type would otherwise have no size.
    pub tab: Box<Tab>,
    /// How much of the room the first pane gets, 0 to 1. Dragged by the
    /// divider between them; see `split_extent`, which is what turns it into
    /// a size and keeps either pane from being squeezed away.
    pub ratio: f32,
}

/// Which of a tab's two sessions.
///
/// `First` is the left or top pane and is the only one an unsplit tab has. The
/// numbers are what the strip entry shows - `1:` and `2:` - so they are the
/// panes' names as far as the user is concerned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pane {
    #[default]
    First,
    Second,
}

impl Pane {
    /// What the strip entry calls this pane.
    pub fn number(self) -> u8 {
        match self {
            Pane::First => 1,
            Pane::Second => 2,
        }
    }
}

/// A change to the tab layout, asked for from inside a pane.
///
/// Deferred rather than carried out on the spot: the right-click menu is drawn
/// while the panes are, and splitting or closing a tab there would move the
/// tabs under the loop that is drawing them. Applied once the central panel has
/// finished - the same reason the tab strip collects its own actions and
/// applies them after its loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum LayoutAction {
    Split(usize, SplitDir),
    Unsplit(usize),
    ClosePane(At),
}

/// One session on screen: the tab it is in, and which of its panes.
///
/// Everything that acts on "the session" takes one of these rather than a tab
/// index, because with a split tab an index no longer names a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct At {
    pub(super) tab: usize,
    pub(super) pane: Pane,
}

/// What drawing one pane produced.
///
/// `Default` stands for a pane there was nothing to draw in - a tab that went
/// away between frames. A zero cell size leaves the window geometry alone
/// rather than dividing by it.
#[derive(Default)]
pub(super) struct PaneMeasure {
    /// Grid size IRIS should be told about, which is wider than the window -
    /// see [`terminal_view::TERMINAL_COLS`].
    pub(super) grid: (usize, usize),
    /// Size of the pane in characters, as drawn.
    pub(super) view: (usize, usize),
    pub(super) cell: egui::Vec2,
    /// The user clicked into this pane's terminal this frame.
    pub(super) clicked: bool,
}

/// The handle between two panes: a separator that can be dragged.
///
/// Returns the new ratio while it is being dragged, and `None` otherwise. A
/// double-click puts the split back down the middle, which is the way out of a
/// layout dragged somewhere useless.
///
/// `across` is the length of the divider - the height of a left/right split,
/// the width of a top/bottom one - and `usable` is the room the two panes
/// share.
pub(super) fn split_divider(
    ui: &mut egui::Ui,
    dir: SplitDir,
    across: f32,
    usable: f32,
    ratio: f32,
) -> Option<f32> {
    let size = match dir {
        SplitDir::Right => egui::Vec2::new(SPLIT_DIVIDER, across),
        SplitDir::Bottom => egui::Vec2::new(across, SPLIT_DIVIDER),
    };
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());

    // A hairline down the middle of the hit area rather than a fill of it: the
    // grab area has to be wide enough to aim a mouse at, and a 6-point band of
    // colour between two terminals would read as a gap in the window.
    let visuals = ui.style().interact(&response);
    let active = response.hovered() || response.dragged();
    let stroke = if active {
        egui::Stroke::new(2.0_f32, visuals.fg_stroke.color)
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke
    };
    let painter = ui.painter();
    match dir {
        SplitDir::Right => {
            painter.vline(rect.center().x, rect.y_range(), stroke);
        }
        SplitDir::Bottom => {
            painter.hline(rect.x_range(), rect.center().y, stroke);
        }
    }
    if active {
        ui.ctx().set_cursor_icon(match dir {
            SplitDir::Right => egui::CursorIcon::ResizeHorizontal,
            SplitDir::Bottom => egui::CursorIcon::ResizeVertical,
        });
    }

    if response.double_clicked() {
        return Some(0.5);
    }
    if !response.dragged() {
        return None;
    }
    let delta = match dir {
        SplitDir::Right => response.drag_delta().x,
        SplitDir::Bottom => response.drag_delta().y,
    };
    (delta != 0.0).then(|| dragged_ratio(usable, ratio, delta))
}

/// Draws one session: the notes above its terminal, and the terminal.
///
/// A free function rather than a method because it needs the session and
/// nothing else of the app, which is what keeps the pane's own drawing out of
/// the way of everything [`App::terminal_pane`] then does with the result.
pub(super) fn draw_pane(
    ui: &mut egui::Ui,
    tab: &mut Tab,
    theme: &Theme,
    opts: &RenderOpts,
    role: terminal_view::PaneRole,
    macros: &[MacroGroup],
) -> terminal_view::RenderResult {
    // Autologon status belongs next to the terminal it applies to.
    if let Some(note) = tab.autologon.status_note() {
        ui.label(note);
    }
    if let Some(error) = tab.error.clone() {
        ui.colored_label(theme.ansi[9], error);
    }
    if tab.ended && tab.error.is_none() {
        let mut reconnect = false;
        ui.horizontal(|ui| {
            ui.label(tr("Session ended."));
            if ui.button(tr("Reconnect")).clicked() {
                reconnect = true;
            }
        });
        if reconnect {
            tab.start();
        }
    }

    let uid = tab.uid;
    terminal_view::show(ui, &tab.grid, &mut tab.view, theme, opts, uid, role, macros)
}

/// Inner window size that turns a terminal of `view` characters into one of
/// `target` characters.
///
/// Only the difference is worked out, so everything around the terminal - the
/// menu bar, the tab strip, the scrollbar it reserves - drops out of the sum
/// without having to be known. The half point of slack covers the view flooring
/// the space it is given: a window a rounding error short of a whole column
/// would otherwise come out one column narrower than asked for.
pub(super) fn fitted_inner_size(
    inner: egui::Vec2,
    view: (usize, usize),
    target: (usize, usize),
    cell: egui::Vec2,
) -> egui::Vec2 {
    egui::Vec2::new(
        inner.x + (target.0 as f32 - view.0 as f32) * cell.x + 0.5,
        inner.y + (target.1 as f32 - view.1 as f32) * cell.y + 0.5,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The divider can be dragged anywhere, and neither pane may be squeezed
    /// away: a pane a couple of characters wide would have IRIS truncating
    /// every line it wrote at that margin.
    #[test]
    fn a_dragged_split_never_squeezes_a_pane_below_the_minimum() {
        let usable = 900.0_f32;
        for ratio in [-1.0, 0.0, 0.01, 0.5, 0.99, 1.0, 2.0] {
            let first = split_extent(usable, ratio);
            assert!(
                first >= MIN_PANE && usable - first >= MIN_PANE,
                "ratio {ratio} left {first} of {usable}"
            );
        }
    }

    /// A window too narrow to give both sides a minimum has no fair answer but
    /// half each, and must not come back with a negative pane.
    #[test]
    fn a_window_with_no_room_for_two_minimums_splits_evenly() {
        let usable = MIN_PANE;
        assert_eq!(split_extent(usable, 0.9), usable / 2.0);
    }

    /// Dragging left then right by the same amount comes back to where it
    /// started, which is what stops the handle lagging behind the pointer.
    #[test]
    fn dragging_the_divider_and_back_returns_to_the_same_place() {
        let usable = 800.0_f32;
        let moved = dragged_ratio(usable, 0.5, 120.0);
        let back = dragged_ratio(usable, moved, -120.0);
        assert!((back - 0.5).abs() < 0.001, "came back to {back}");
        assert!(split_extent(usable, moved) > split_extent(usable, 0.5));
    }

    /// A drag past the end stops at the minimum rather than banking movement
    /// nothing can be done with.
    #[test]
    fn a_drag_past_the_edge_stops_at_the_minimum() {
        let usable = 600.0_f32;
        let far = dragged_ratio(usable, 0.5, -10_000.0);
        assert_eq!(split_extent(usable, far), MIN_PANE);
        // And one step back off the edge moves again straight away.
        let back = dragged_ratio(usable, far, 50.0);
        assert!((split_extent(usable, back) - (MIN_PANE + 50.0)).abs() < 0.001);
    }

    /// The window is corrected from a measurement, so the check that matters is
    /// that measuring the corrected window gives the geometry that was asked
    /// for - in one step, at any font size and whatever the chrome around the
    /// terminal happens to take up.
    #[test]
    fn one_correction_lands_on_the_geometry_that_was_asked_for() {
        let target = (
            config::DEFAULT_TERMINAL_COLS as usize,
            config::DEFAULT_TERMINAL_ROWS as usize,
        );

        for cell in [
            egui::Vec2::new(8.0, 16.0),
            egui::Vec2::new(9.0, 21.0),
            egui::Vec2::new(13.0, 30.0),
        ] {
            for chrome in [egui::Vec2::new(24.0, 78.0), egui::Vec2::new(12.0, 61.0)] {
                for inner in [
                    egui::Vec2::new(1000.0, 640.0),
                    egui::Vec2::new(400.0, 240.0),
                    egui::Vec2::new(1913.0, 1027.0),
                ] {
                    // What `terminal_view::show` would measure in that window.
                    let measure = |inner: egui::Vec2| {
                        (
                            (((inner.x - chrome.x) / cell.x).floor() as usize).max(1),
                            (((inner.y - chrome.y) / cell.y).floor() as usize).max(1),
                        )
                    };

                    let fitted = fitted_inner_size(inner, measure(inner), target, cell);
                    assert_eq!(
                        measure(fitted),
                        target,
                        "cell {cell:?}, chrome {chrome:?}, from {inner:?}"
                    );
                }
            }
        }
    }

    /// A window already at the right size is left exactly where it is, so the
    /// fit cannot drift the window it was meant to leave alone.
    #[test]
    fn a_window_that_already_fits_is_not_moved() {
        let inner = egui::Vec2::new(1000.0, 640.0);
        let size = fitted_inner_size(inner, (100, 30), (100, 30), egui::Vec2::new(8.0, 16.0));
        assert!((size.x - inner.x).abs() <= 0.5 && (size.y - inner.y).abs() <= 0.5);
    }
}
