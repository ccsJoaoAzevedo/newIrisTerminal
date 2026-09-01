//! The two dialogs that live in windows of their own.
//!
//! Settings and the theme manager are the ones you keep open *while* watching
//! the terminal — turning a switch on, or dragging a colour, and looking at
//! what it did. An `egui::Window` inside the app covers exactly what you are
//! trying to see, so these two are operating-system windows instead. Nothing
//! else here is: the rest are either momentary (export, a confirmation) or
//! already beside the terminal rather than over it.
//!
//! They are drawn as *immediate* viewports, not deferred ones: the contents
//! borrow the app's settings and themes, and a deferred viewport's callback has
//! to be `'static + Send + Sync`, which nothing that edits live state can be.
//!
//! The frame is the app's own, drawn by [`crate::ui::chrome`] exactly as the
//! main window's is, and it follows the same "use the system title bar" setting.

use egui::{Context, Ui, ViewportClass};

use crate::config::theme::WindowButtons;
use crate::ui::chrome::{self, WindowAction};

/// Draws `contents` in a window of its own.
///
/// `open` is cleared when the window is closed, whichever way it was closed, so
/// the caller's "is this dialog showing" flag stays the one source of truth.
///
/// `buttons` is the active theme's window-control style, so a detached window
/// is framed like the main one; `native_decorations` is the same setting the
/// main window follows, and hands the frame back to the system when it is on.
pub fn shell(
    ctx: &Context,
    id: &'static str,
    title: &str,
    open: &mut bool,
    size: [f32; 2],
    buttons: &WindowButtons,
    native_decorations: bool,
    contents: impl FnOnce(&mut Ui),
) {
    if !*open {
        return;
    }
    let mut closed = false;

    ctx.show_viewport_immediate(
        egui::ViewportId::from_hash_of(id),
        egui::ViewportBuilder::default()
            .with_title(title)
            .with_inner_size(size)
            .with_min_inner_size([360.0, 240.0])
            .with_decorations(native_decorations),
        |ctx, class| {
            if class == ViewportClass::Embedded {
                // The backend cannot give us a real window - a headless run, or
                // a platform without multiple viewports. Drawn as an ordinary
                // window rather than dropped, so the dialog still opens.
                let mut showing = true;
                egui::Window::new(title)
                    .id(egui::Id::new(id))
                    .open(&mut showing)
                    .default_size(size)
                    .collapsible(false)
                    .show(ctx, contents);
                if !showing {
                    closed = true;
                }
                return;
            }

            if !native_decorations {
                egui::TopBottomPanel::top(egui::Id::new((id, "title-bar"))).show(ctx, |ui| {
                    if let Some(action) = title_bar(ui, id, title, buttons) {
                        match action {
                            // Closing this window is not closing the app: it
                            // puts the dialog away, which is what the caller's
                            // flag means.
                            WindowAction::Close => closed = true,
                            other => chrome::apply(ctx, other),
                        }
                    }
                });
            }
            egui::CentralPanel::default().show(ctx, contents);
            if !native_decorations {
                // Last, and in a foreground layer, for the same reason the main
                // window does it last.
                chrome::resize_grips(ctx, id);
            }
            // The taskbar, Alt+F4, or the system menu.
            if ctx.input(|i| i.viewport().close_requested()) {
                closed = true;
            }
        },
    );

    if closed {
        *open = false;
    }
}

/// The window's own title bar: the name on one side, the controls on the other,
/// and everything between them draggable.
///
/// Laid out the way the main window's is, down to which end the buttons sit at,
/// so a detached window does not read as something from a different app.
fn title_bar(
    ui: &mut Ui,
    window: &'static str,
    title: &str,
    buttons: &WindowButtons,
) -> Option<WindowAction> {
    let mut action = None;
    ui.horizontal(|ui| {
        if buttons.left {
            if let Some(asked) = chrome::leading_window_buttons(ui, buttons) {
                action = Some(asked);
            }
            ui.add_space(6.0);
        }
        ui.label(egui::RichText::new(title).strong());
        // Claims the rest of the row for dragging either way; without it the
        // window could not be moved at all.
        if let Some(asked) = chrome::title_bar_controls(ui, buttons, !buttons.left, window) {
            action = Some(asked);
        }
    });
    action
}
