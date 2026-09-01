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

use egui::{Context, Pos2, Ui, Vec2, ViewportClass};

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
///
/// The window opens in the middle of the main one, and after that wherever it
/// was last dragged to - see [`opening_position`].
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
    let position = opening_position(ctx, id, size);

    let mut builder = egui::ViewportBuilder::default()
        .with_title(title)
        .with_inner_size(size)
        .with_min_inner_size([360.0, 240.0])
        .with_decorations(native_decorations);
    // Only on the frame it opens. Asking for a position every frame would
    // fight the user dragging the window somewhere else.
    if let Some(position) = position {
        builder = builder.with_position(position);
    }
    // Where an embedded window goes instead: `position` is in monitor space,
    // which means nothing to a window drawn inside the main one.
    let embedded_position = ctx.screen_rect().center() - Vec2::from(size) / 2.0;

    ctx.show_viewport_immediate(egui::ViewportId::from_hash_of(id), builder, |ctx, class| {
        if class == ViewportClass::Embedded {
            // The backend cannot give us a real window - a headless run, or
            // a platform without multiple viewports. Drawn as an ordinary
            // window rather than dropped, so the dialog still opens.
            let mut showing = true;
            egui::Window::new(title)
                .id(egui::Id::new(id))
                .open(&mut showing)
                .default_size(size)
                .default_pos(embedded_position)
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
        remember_position(ctx, id);
        // The taskbar, Alt+F4, or the system menu.
        if ctx.input(|i| i.viewport().close_requested()) {
            closed = true;
        }
    });

    if closed {
        *open = false;
    }
}

/// Where a window should open, or `None` to leave the placing to the system.
///
/// The middle of the main window the first time, so the dialog lands over the
/// terminal it belongs to rather than wherever the window manager felt like
/// stacking it; after that, back where it was last left. "Last left" is only
/// remembered for the run: moving a dialog is a this-session arrangement, not
/// a setting, so nothing is written to settings.toml over it.
///
/// `None` on every frame but the one the window opens on. The position goes
/// into the viewport builder, and egui turns any change there into a command to
/// move the window - which, sent every frame, would drag the window back out of
/// the hand moving it.
fn opening_position(ctx: &Context, id: &'static str, size: [f32; 2]) -> Option<Pos2> {
    // Whether this is the frame it opened on, taken from the gap in the frames
    // it was drawn on. A flag set when it closes would not do: the callers
    // return before reaching here once their dialog is put away.
    let frame = ctx.frame_nr();
    let drawn = ctx.data_mut(|d| {
        let last = d.get_temp::<u64>(drawn_id(id));
        d.insert_temp(drawn_id(id), frame);
        last
    });
    if drawn.is_some_and(|last| last + 1 >= frame) {
        return None;
    }

    if let Some(left_at) = ctx.data_mut(|d| d.get_temp::<Pos2>(position_id(id))) {
        return Some(left_at);
    }

    // The centre of the main window, less half of what this window will
    // measure. `size` is the contents, and a position is the frame's, so the
    // decoration has to be added back first - which the main window is measured
    // for rather than guessed at, since it wears the same frame this one will,
    // whether that is the system's or the app's own.
    let (outer, inner) = ctx.input(|i| (i.viewport().outer_rect, i.viewport().inner_rect));
    let outer = outer.filter(|rect| rect.is_finite() && rect.width() > 0.0)?;
    let border = inner.map_or(Vec2::ZERO, |inner| outer.size() - inner.size());
    Some(outer.center() - (Vec2::from(size) + border) / 2.0)
}

/// Notes where the window is now, so the next time it opens it opens there.
///
/// Called from inside the window's own viewport, so the rect read here is that
/// window's and not the main one's.
fn remember_position(ctx: &Context, id: &'static str) {
    let rect = ctx.input(|i| {
        let viewport = i.viewport();
        if viewport.minimized.unwrap_or(false) {
            None
        } else {
            viewport.outer_rect
        }
    });
    // The off-screen parking spot some window managers use while a window is on
    // its way somewhere is not a position anyone chose.
    let Some(rect) = rect.filter(|rect| rect.is_finite() && rect.min.x > -30_000.0) else {
        return;
    };
    ctx.data_mut(|d| d.insert_temp(position_id(id), rect.min));
}

fn position_id(id: &'static str) -> egui::Id {
    egui::Id::new((id, "detached-position"))
}

fn drawn_id(id: &'static str) -> egui::Id {
    egui::Id::new((id, "detached-drawn-on"))
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
