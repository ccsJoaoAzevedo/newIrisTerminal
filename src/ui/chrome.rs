//! The window frame the app draws for itself.
//!
//! With `decorations(false)` the operating system stops drawing a title bar —
//! and stops providing the resize borders and the move-by-dragging that came
//! with it. Everything the frame used to do has to be provided here, or the
//! window cannot be moved or resized at all.

use egui::viewport::ResizeDirection;
use egui::{
    Align, Color32, Context, CursorIcon, Id, Layout, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2,
    ViewportCommand,
};

/// How wide the grab area along each window edge is.
const RESIZE_GRAB: f32 = 5.0;

/// Which control to draw.
///
/// The icons are stroked rather than set in text: the glyphs for window
/// controls are not in every font egui falls back through, and a missing one
/// would render as a tofu box in the title bar. egui draws its own window close
/// button the same way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Icon {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// A square control with a hand-drawn icon.
fn window_button(ui: &mut Ui, icon: Icon, hint: &str) -> Response {
    let side = ui.spacing().interact_size.y;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(side), Sense::click());
    let painter = ui.painter();

    // Close gets the conventional red. Worth more than consistency here: it is
    // the one control in the row with an irreversible effect.
    let danger = Color32::from_rgb(196, 43, 28);
    let hovered = response.hovered();
    if hovered {
        let fill = if icon == Icon::Close {
            danger
        } else {
            ui.visuals().widgets.hovered.bg_fill
        };
        painter.rect_filled(rect, 2.0, fill);
    }

    let colour = if hovered && icon == Icon::Close {
        Color32::WHITE
    } else if hovered {
        ui.visuals().widgets.hovered.fg_stroke.color
    } else {
        ui.visuals().widgets.inactive.fg_stroke.color
    };
    let stroke = Stroke::new(1.0_f32, colour);

    // A small box in the middle of the hit area, so the icons come out the same
    // size whatever the row height works out to.
    let glyph = Rect::from_center_size(rect.center(), Vec2::splat((side * 0.36).round().max(6.0)));
    match icon {
        Icon::Minimize => {
            painter.line_segment(
                [
                    Pos2::new(glyph.left(), glyph.center().y),
                    Pos2::new(glyph.right(), glyph.center().y),
                ],
                stroke,
            );
        }
        Icon::Maximize => {
            painter.rect_stroke(glyph, 0.0, stroke);
        }
        Icon::Restore => {
            // Two offset outlines, the usual "back to the previous size" mark.
            painter.rect_stroke(glyph.translate(Vec2::new(2.0, -2.0)), 0.0, stroke);
            painter.rect_filled(glyph, 0.0, ui.visuals().panel_fill);
            painter.rect_stroke(glyph, 0.0, stroke);
        }
        Icon::Close => {
            painter.line_segment([glyph.left_top(), glyph.right_bottom()], stroke);
            painter.line_segment([glyph.right_top(), glyph.left_bottom()], stroke);
        }
    }

    response.on_hover_text(hint)
}

/// What the window buttons in the title bar were asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowAction {
    Minimize,
    ToggleMaximize,
    Close,
}

/// Draws minimize / maximize / close at the right-hand end of the row, then
/// makes whatever space is left draggable.
///
/// Buttons first, dragging second: the drag area is the leftover rectangle, so
/// it cannot swallow the buttons however narrow the window gets.
pub fn title_bar_controls(ui: &mut Ui) -> Option<WindowAction> {
    let mut action = None;
    let maximized = ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false));

    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if window_button(ui, Icon::Close, "Close").clicked() {
            action = Some(WindowAction::Close);
        }
        let (icon, hint) = if maximized {
            (Icon::Restore, "Restore")
        } else {
            (Icon::Maximize, "Maximize")
        };
        if window_button(ui, icon, hint).clicked() {
            action = Some(WindowAction::ToggleMaximize);
        }
        if window_button(ui, Icon::Minimize, "Minimize").clicked() {
            action = Some(WindowAction::Minimize);
        }

        // The gap between the buttons and the items already placed on the left.
        let rest = ui.available_rect_before_wrap();
        if rest.width() > 0.0 {
            let drag = ui.interact(rest, Id::new("nit-titlebar-drag"), Sense::click_and_drag());
            if drag.drag_started() {
                ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
            }
            if drag.double_clicked() {
                action = Some(WindowAction::ToggleMaximize);
            }
        }
    });

    action
}

/// Carries out a title-bar action. Closing is left to the caller, which may
/// want to ask first.
pub fn apply(ctx: &Context, action: WindowAction) {
    match action {
        WindowAction::Minimize => ctx.send_viewport_cmd(ViewportCommand::Minimized(true)),
        WindowAction::ToggleMaximize => {
            let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
            ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
        }
        WindowAction::Close => ctx.send_viewport_cmd(ViewportCommand::Close),
    }
}

/// Puts a resize grip under the pointer when it is at a window edge.
///
/// Only one grip exists, and only while the pointer is actually within
/// [`RESIZE_GRAB`] of an edge. Eight permanent ones seemed simpler, but a grip
/// has to sit in a foreground layer to beat the panels and the terminal, which
/// reach the window edge and sense drags of their own — and `layer_id_at` walks
/// layers back to front, so a foreground layer that is always present outranks
/// every window for hit-testing. That is what stopped the mouse wheel reaching
/// the Settings scroll area. Existing only under the pointer, at the very edge
/// of the window, it cannot be in anything's way.
pub fn resize_grips(ctx: &Context) {
    // A maximized window has no edges to drag.
    if ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
        return;
    }

    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    let Some((direction, cursor)) = edge_at(ctx.screen_rect(), pos) else {
        return;
    };

    egui::Area::new(Id::new("nit-resize"))
        .order(egui::Order::Foreground)
        .fixed_pos(pos - Vec2::splat(RESIZE_GRAB))
        .interactable(true)
        .show(ctx, |ui| {
            let rect = Rect::from_center_size(pos, Vec2::splat(RESIZE_GRAB * 2.0));
            let response = ui.allocate_rect(rect, Sense::drag());
            ui.ctx().set_cursor_icon(cursor);
            if response.drag_started() {
                ui.ctx()
                    .send_viewport_cmd(ViewportCommand::BeginResize(direction));
            }
        });
}

/// Which window edge or corner `pos` is on, if any.
///
/// Corners win over edges where they overlap: aiming for a corner and getting a
/// one-axis resize is the annoying way round.
fn edge_at(screen: Rect, pos: Pos2) -> Option<(ResizeDirection, CursorIcon)> {
    let g = RESIZE_GRAB;
    let west = pos.x <= screen.left() + g;
    let east = pos.x >= screen.right() - g;
    let north = pos.y <= screen.top() + g;
    let south = pos.y >= screen.bottom() - g;

    // A corner is a generous square, so it is reachable without pixel-hunting.
    let corner = g * 3.0;
    let near_west = pos.x <= screen.left() + corner;
    let near_east = pos.x >= screen.right() - corner;
    let near_north = pos.y <= screen.top() + corner;
    let near_south = pos.y >= screen.bottom() - corner;

    let pair = |a: bool, b: bool| a && b;
    if pair(west || near_west, north || near_north) && (west || north) {
        return Some((ResizeDirection::NorthWest, CursorIcon::ResizeNorthWest));
    }
    if pair(east || near_east, north || near_north) && (east || north) {
        return Some((ResizeDirection::NorthEast, CursorIcon::ResizeNorthEast));
    }
    if pair(west || near_west, south || near_south) && (west || south) {
        return Some((ResizeDirection::SouthWest, CursorIcon::ResizeSouthWest));
    }
    if pair(east || near_east, south || near_south) && (east || south) {
        return Some((ResizeDirection::SouthEast, CursorIcon::ResizeSouthEast));
    }

    if west {
        Some((ResizeDirection::West, CursorIcon::ResizeWest))
    } else if east {
        Some((ResizeDirection::East, CursorIcon::ResizeEast))
    } else if north {
        Some((ResizeDirection::North, CursorIcon::ResizeNorth))
    } else if south {
        Some((ResizeDirection::South, CursorIcon::ResizeSouth))
    } else {
        None
    }
}
