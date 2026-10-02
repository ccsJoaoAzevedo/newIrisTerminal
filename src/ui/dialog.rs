//! The app's dialogs, drawn the way GNOME and macOS draw an alert: no title
//! bar, the title centred and bold inside a rounded card, the buttons along
//! the bottom on the right with the one that answers the question last and in
//! the accent colour, and the window behind veiled until it is answered.
//!
//! One shape for all of them, so a dialog reads as the app asking something
//! rather than as one of egui's debug windows - which is what the default
//! frame, with its title bar and its close cross, looks like.

use egui::{Align, Color32, Context, Key, Layout, Margin, Response, RichText, Rounding, Ui};

/// What a button does, which decides how it is drawn.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Role {
    Plain,
    /// The answer the dialog is there to get: filled with the accent colour.
    Suggested,
    /// An answer that deletes or closes something: filled red, as GNOME's
    /// destructive action and macOS's are.
    Destructive,
}

/// GNOME's destructive red.
const DESTRUCTIVE: Color32 = Color32::from_rgb(0xc0, 0x1c, 0x28);

/// Shows a dialog over a veil, centred, until `open` is cleared - by a button
/// in `contents`, or by Esc.
pub fn show<R>(
    ctx: &Context,
    id: &str,
    title: &str,
    open: &mut bool,
    contents: impl FnOnce(&mut Ui) -> R,
) {
    veil(ctx, id);
    let frame = egui::Frame::window(&ctx.style())
        .rounding(Rounding::same(12.0))
        .inner_margin(Margin::symmetric(22.0, 18.0));
    let shown = egui::Window::new(title)
        .id(egui::Id::new(id))
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .frame(frame)
        .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
        .show(ctx, |ui| {
            ui.set_min_width(320.0);
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(title).strong().size(16.0));
            });
            ui.add_space(4.0);
            contents(ui);
        });
    // The veil and the window are both areas in the same layer order, and a
    // click on an area brings it to the front of that order. Clicking the veil
    // therefore buried the dialog underneath it: still painted, but no longer
    // reachable by the pointer, so no button would answer and the dialog could
    // not be dismissed at all. Lifting the window every frame keeps it above
    // its own veil whatever was clicked.
    if let Some(shown) = shown {
        ctx.move_to_top(shown.response.layer_id);
    }
    // With the title bar gone there is no cross to close it by, and Esc is
    // what both desktops answer a dialog with instead.
    if ctx.input(|i| i.key_pressed(Key::Escape)) {
        *open = false;
    }
}

/// Dims the whole window and swallows clicks, so nothing behind a dialog can
/// be reached while it waits for an answer.
fn veil(ctx: &Context, id: &str) {
    let screen = ctx.screen_rect();
    egui::Area::new(egui::Id::new((id, "veil")))
        .order(egui::Order::Middle)
        .fixed_pos(screen.min)
        .interactable(true)
        .show(ctx, |ui| {
            // Allocated before it is painted, because an area's painter is
            // clipped to what the area has claimed - and the click-and-drag
            // sense is the half that makes it modal rather than decorative.
            ui.allocate_response(screen.size(), egui::Sense::click_and_drag());
            ui.painter()
                .rect_filled(screen, 0.0, Color32::from_black_alpha(110));
        });
}

/// The row of buttons along the bottom, against the right edge.
///
/// Laid out right to left, so `buttons` adds them from the right: the answer
/// first, then Cancel, then anything that belongs further off to the left.
pub fn actions(ui: &mut Ui, buttons: impl FnOnce(&mut Ui)) {
    ui.add_space(6.0);
    // A row of its own height. `with_layout` would hand the buttons all the
    // height the window offers, and a window offers the whole screen: the
    // card grew to fill it, the buttons floating in the middle of the space.
    let size = egui::vec2(ui.available_width(), 32.0);
    ui.allocate_ui_with_layout(size, Layout::right_to_left(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.x = 8.0;
        buttons(ui);
    });
}

/// A dialog button: rounded, roomy, and filled for the roles that are filled.
pub fn button(ui: &mut Ui, text: &str, role: Role) -> Response {
    let fill = match role {
        Role::Plain => None,
        Role::Suggested => Some(ui.visuals().selection.bg_fill),
        Role::Destructive => Some(DESTRUCTIVE),
    };
    let mut label = RichText::new(text);
    if fill.is_some() {
        label = label.color(Color32::WHITE).strong();
    }
    let mut button = egui::Button::new(label)
        .rounding(Rounding::same(8.0))
        .min_size(egui::vec2(88.0, 30.0));
    if let Some(fill) = fill {
        button = button.fill(fill);
    }
    ui.add(button)
}
