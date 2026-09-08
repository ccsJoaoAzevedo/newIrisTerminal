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

use crate::config::theme::{WindowButtonStyle, WindowButtons};
use crate::i18n::tr;
use crate::ui::icons::{self, Glyph};
use crate::ui::shading::{darken, gloss, gradient, lighten, radial, white};

/// How wide the grab area along each window edge is.
///
/// Three points rather than the five it started at, because everything the
/// grip overlaps is something else's: the terminal reaches three of the four
/// window edges, and a grip sits in a foreground layer that outranks whatever
/// is under it for hit-testing. At five, the terminal's first column could not
/// be clicked at all - the pointer turned into a resize arrow and a drag
/// resized the window instead of selecting the text. Three is still a band the
/// mouse finds, and it is held clear of the terminal twice over: by the inset
/// in [`crate::app::App::terminal_inset`], and by `keep_out` below.
pub const RESIZE_GRAB: f32 = 3.0;

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
    /// Opens the app's settings. Not a window control at all, but it sits in
    /// the row with them - next to minimize - and it has to be painted in
    /// whatever style the theme gives the others, or it would read as
    /// something from a different program bolted onto the title bar.
    Settings,
}

impl Icon {
    /// Which of the theme's three colours this control is drawn in.
    fn tint(self, style: &WindowButtons) -> Option<Color32> {
        match self {
            Icon::Close => style.close,
            Icon::Minimize => style.minimize,
            Icon::Maximize | Icon::Restore => style.maximize,
            // Deliberately not one of the three: a theme naming a colour for
            // "minimize" is naming it for the window control, and a gear
            // painted in it would claim to be one.
            Icon::Settings => None,
        }
    }

    /// Whether the theme asks for this control at all.
    fn shown(self, style: &WindowButtons) -> bool {
        match self {
            Icon::Close => style.show_close,
            Icon::Minimize => style.show_minimize,
            Icon::Maximize | Icon::Restore => style.show_maximize,
            // The way into Settings cannot be something a theme can take
            // away: there would then be no way in at all.
            Icon::Settings => true,
        }
    }

    /// The mark this control is drawn with.
    fn glyph(self) -> Glyph {
        match self {
            Icon::Minimize => Glyph::Bar,
            Icon::Maximize => Glyph::Window,
            Icon::Restore => Glyph::WindowStack,
            Icon::Close => Glyph::Cross,
            Icon::Settings => Glyph::Gear,
        }
    }
}

/// A control, or nothing at all when the theme has hidden it.
///
/// Hidden means gone: no space allocated, so the row closes up rather than
/// leaving a gap where the button was.
fn optional_button(ui: &mut Ui, icon: Icon, hint: &str, style: &WindowButtons) -> Option<Response> {
    icon.shown(style)
        .then(|| window_button(ui, icon, hint, style))
}

/// The grey Aqua greys its traffic lights out to when the window is not the
/// active one. Tiger did this, and it is the cheapest way for a window drawn by
/// the app to still say which one has the keyboard.
const AQUA_INACTIVE: Color32 = Color32::from_rgb(203, 203, 203);

/// A square control with a hand-drawn icon.
fn window_button(ui: &mut Ui, icon: Icon, hint: &str, style: &WindowButtons) -> Response {
    let side = ui.spacing().interact_size.y;
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(side), Sense::click());
    let hovered = response.hovered();
    match style.style {
        WindowButtonStyle::Stroke => paint_stroked(ui, rect, icon, hovered, style),
        WindowButtonStyle::Aqua => paint_aqua(ui, rect, icon, hovered, style),
        WindowButtonStyle::Luna => paint_luna(ui, rect, icon, hovered, style),
    }
    response.on_hover_text(hint)
}

/// The app's own look: a glyph on a transparent square, filled on hover.
fn paint_stroked(ui: &Ui, rect: Rect, icon: Icon, hovered: bool, style: &WindowButtons) {
    let painter = ui.painter();
    // Close gets the conventional red. Worth more than consistency here: it is
    // the one control in the row with an irreversible effect.
    let danger = style.hover_close.unwrap_or(Color32::from_rgb(196, 43, 28));
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
    } else if let Some(tint) = icon.tint(style) {
        // A theme that names a colour for this control means it whether or not
        // the pointer is over it; only the close hover, which paints white on
        // red, is louder than the theme.
        tint
    } else if hovered {
        ui.visuals().widgets.hovered.fg_stroke.color
    } else {
        style
            .icon
            .unwrap_or_else(|| ui.visuals().widgets.inactive.fg_stroke.color)
    };
    // What the restore mark's front window is filled with, so the copy behind
    // it does not show through: the button's own background, which is the hover
    // fill while the pointer is on it and the panel otherwise.
    let behind = if hovered {
        ui.visuals().widgets.hovered.bg_fill
    } else {
        ui.visuals().panel_fill
    };
    icons::draw(painter, rect, icon.glyph(), colour, behind);
}

/// Mac OS X's traffic light: a glossy bubble whose glyph appears under the
/// pointer, greyed out while the window is not the active one.
///
/// Four passes, which is what makes it read as a lit object rather than a
/// coloured disc: the body shaded from a bright spot low down (the light
/// bouncing back up off the desk), a dark rim, a white gloss over the top half,
/// and a rim light along the bottom edge.
fn paint_aqua(ui: &Ui, rect: Rect, icon: Icon, hovered: bool, style: &WindowButtons) {
    let focused = ui.ctx().input(|i| i.viewport().focused.unwrap_or(true));
    let painter = ui.painter();
    let radius = (side_of(rect) * 0.30).clamp(5.0, 7.5);
    let center = rect.center();

    // Unfocused greys all three at once, which is why it is decided here rather
    // than left to the theme: it is a state of the window, not of the button.
    let mut base = match icon.tint(style) {
        Some(colour) if focused => colour,
        Some(_) => AQUA_INACTIVE,
        // Cannot happen for an `aqua` theme, which defaults its three fills,
        // but a hand-written one could still ask for the style and nothing else.
        None => ui.visuals().widgets.inactive.bg_fill,
    };
    if hovered {
        base = lighten(base, 0.12);
    }

    // The body. The bright spot sits below the middle because the strongest
    // light in an Aqua button is the bounce coming back up into it.
    radial(
        painter,
        center,
        radius,
        Vec2::new(0.0, radius * 0.30),
        lighten(base, 0.30),
        darken(base, 0.72),
    );
    // The bottom rim light, brightest directly under the bubble.
    radial(
        painter,
        center + Vec2::new(0.0, radius * 0.55),
        radius * 0.62,
        Vec2::ZERO,
        white(70),
        white(0),
    );
    painter.circle_stroke(center, radius, Stroke::new(1.0_f32, darken(base, 0.45)));
    // The gloss: a white cap over the top half, the whole of Aqua's look.
    gloss(
        painter,
        center - Vec2::new(0.0, radius * 0.34),
        radius * 0.72,
        radius * 0.50,
        215,
    );

    // Tiger showed the marks only under the pointer - x to close, - to
    // minimize, + to zoom - and hid them the rest of the time.
    if !hovered {
        return;
    }
    let stroke = Stroke::new(1.4_f32, darken(base, 0.30));
    let arm = radius * 0.46;
    match icon {
        // Aqua never had one of these, so it is simply the app's own gear in
        // Aqua's ink - drawn on hover like everything else in the row.
        Icon::Settings => {
            icons::draw(
                painter,
                Rect::from_center_size(center, Vec2::splat(radius * 2.0)),
                Glyph::Gear,
                darken(base, 0.30),
                Color32::TRANSPARENT,
            );
        }
        Icon::Close => {
            let d = arm * 0.78;
            painter.line_segment(
                [center + Vec2::new(-d, -d), center + Vec2::new(d, d)],
                stroke,
            );
            painter.line_segment(
                [center + Vec2::new(d, -d), center + Vec2::new(-d, d)],
                stroke,
            );
        }
        Icon::Minimize => {
            painter.line_segment(
                [center - Vec2::new(arm, 0.0), center + Vec2::new(arm, 0.0)],
                stroke,
            );
        }
        // Zoom is a plus, and coming back down from zoomed is a minus with the
        // plus's stem taken out - the same mark, one stroke short.
        Icon::Maximize => {
            painter.line_segment(
                [center - Vec2::new(arm, 0.0), center + Vec2::new(arm, 0.0)],
                stroke,
            );
            painter.line_segment(
                [center - Vec2::new(0.0, arm), center + Vec2::new(0.0, arm)],
                stroke,
            );
        }
        Icon::Restore => {
            painter.line_segment(
                [center - Vec2::new(arm, 0.0), center + Vec2::new(arm, 0.0)],
                stroke,
            );
        }
    }
}

/// Windows XP's Luna: a rounded, gradient-filled tile with a bevel along its
/// top edge and a white glyph that is always on.
///
/// Unlike Aqua's, these say what they do at rest - Luna drew the X, the dash
/// and the box whether or not the pointer was anywhere near - so nothing here
/// is hidden until hover; hover only lifts the colour.
fn paint_luna(ui: &Ui, rect: Rect, icon: Icon, hovered: bool, style: &WindowButtons) {
    let focused = ui.ctx().input(|i| i.viewport().focused.unwrap_or(true));
    let painter = ui.painter();
    // Slightly wider than tall, and inset from the hit area: Luna's buttons sat
    // in the title bar with air around them.
    let tile = Rect::from_center_size(
        rect.center(),
        Vec2::new(side_of(rect) * 0.82, side_of(rect) * 0.72),
    );
    let rounding = egui::Rounding::same(2.0);

    let mut base = match icon.tint(style) {
        Some(colour) if focused => colour,
        Some(colour) => darken(lighten(colour, 0.35), 0.85),
        None => ui.visuals().widgets.inactive.bg_fill,
    };
    if hovered {
        base = lighten(base, 0.18);
    }

    // The tile: a dark edge, then the fill, then the highlights on top of it.
    // Luna's buttons are lit from above and slightly left, with the deepest
    // tone about two thirds down and a thin bright rim along the very bottom -
    // that last one is what stops them looking painted on.
    painter.rect_filled(tile, rounding, darken(base, 0.55));
    let inner = tile.shrink(1.0);
    let upper = Rect::from_min_max(
        inner.left_top(),
        Pos2::new(inner.right(), inner.top() + inner.height() * 0.52),
    );
    let lower = Rect::from_min_max(upper.left_bottom(), inner.right_bottom());
    gradient(painter, upper, lighten(base, 0.52), lighten(base, 0.06));
    gradient(painter, lower, darken(base, 0.94), darken(base, 0.70));

    // The gloss over the top half, cut off square the way a Luna button's was.
    gradient(painter, upper, white(105), white(12));
    // Bevel: bright inside the top and left edges, and a light rim along the
    // bottom where the tile catches the desktop behind it.
    painter.line_segment(
        [
            Pos2::new(inner.left() + 1.0, inner.top() + 0.5),
            Pos2::new(inner.right() - 1.0, inner.top() + 0.5),
        ],
        Stroke::new(1.0_f32, white(165)),
    );
    painter.line_segment(
        [
            Pos2::new(inner.left() + 0.5, inner.top() + 1.5),
            Pos2::new(inner.left() + 0.5, inner.bottom() - 1.5),
        ],
        Stroke::new(1.0_f32, white(70)),
    );
    painter.line_segment(
        [
            Pos2::new(inner.left() + 1.5, inner.bottom() - 0.5),
            Pos2::new(inner.right() - 1.5, inner.bottom() - 0.5),
        ],
        Stroke::new(1.0_f32, lighten(base, 0.30)),
    );
    painter.rect_stroke(tile, rounding, Stroke::new(1.0_f32, darken(base, 0.42)));

    // The glyph, in the theme's colour or the white Luna always used.
    let colour = style.icon.unwrap_or(Color32::WHITE);
    let stroke = Stroke::new(1.3_f32, colour);
    let glyph = Rect::from_center_size(tile.center(), Vec2::splat((side_of(rect) * 0.26).max(5.0)));
    match icon {
        // Luna never had one either, and its glyphs are always on.
        Icon::Settings => icons::draw(painter, tile, Glyph::Gear, colour, darken(base, 0.80)),
        Icon::Minimize => {
            // Luna's minimize sat on the baseline rather than in the middle.
            let y = glyph.bottom();
            painter.line_segment(
                [Pos2::new(glyph.left(), y), Pos2::new(glyph.right(), y)],
                stroke,
            );
        }
        Icon::Maximize => {
            painter.rect_stroke(glyph, 0.0, stroke);
            // The heavier top edge of the little window.
            painter.line_segment(
                [
                    Pos2::new(glyph.left(), glyph.top() + 1.0),
                    Pos2::new(glyph.right(), glyph.top() + 1.0),
                ],
                stroke,
            );
        }
        Icon::Restore => {
            painter.rect_stroke(glyph.translate(Vec2::new(1.5, -1.5)), 0.0, stroke);
            painter.rect_filled(glyph, 0.0, darken(base, 0.80));
            painter.rect_stroke(glyph, 0.0, stroke);
        }
        Icon::Close => {
            painter.line_segment([glyph.left_top(), glyph.right_bottom()], stroke);
            painter.line_segment([glyph.right_top(), glyph.left_bottom()], stroke);
        }
    }
}

/// The side of a square hit area, which both painters size their glyphs from.
fn side_of(rect: Rect) -> f32 {
    rect.height()
}

/// What the window buttons in the title bar were asked to do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowAction {
    Minimize,
    ToggleMaximize,
    Close,
}

/// Draws the three controls with nothing behind them, for a theme preview.
///
/// The clicks are dropped: this is a picture of the buttons, not the buttons.
/// Under the stroked style the glyph colour still comes from the surrounding
/// widget colours - that is what the style means - so what a preview shows of
/// it is whatever the *active* theme says, not the one being edited.
pub fn sample_buttons(ui: &mut Ui, style: &WindowButtons) {
    let _ = optional_button(ui, Icon::Close, tr("Close"), style);
    let _ = optional_button(ui, Icon::Minimize, tr("Minimize"), style);
    let _ = optional_button(ui, Icon::Maximize, tr("Maximize"), style);
}

/// The maximize control's icon and tooltip, which depend on where the window is
/// already.
fn maximize_icon(ui: &Ui) -> (Icon, &'static str) {
    if ui.ctx().input(|i| i.viewport().maximized.unwrap_or(false)) {
        (Icon::Restore, tr("Restore"))
    } else {
        (Icon::Maximize, tr("Maximize"))
    }
}

/// Draws the three controls at the left-hand end of the row, in Aqua's order.
///
/// Called before anything else in the title bar, so they sit where a Mac puts
/// them; the drag area is still claimed at the end by
/// [`title_bar_controls`].
pub fn leading_window_buttons(
    ui: &mut Ui,
    style: &WindowButtons,
    settings: Option<&mut bool>,
) -> Option<WindowAction> {
    let mut action = None;
    if clicked(optional_button(ui, Icon::Close, tr("Close"), style)) {
        action = Some(WindowAction::Close);
    }
    if clicked(optional_button(ui, Icon::Minimize, tr("Minimize"), style)) {
        action = Some(WindowAction::Minimize);
    }
    let (icon, hint) = maximize_icon(ui);
    if clicked(optional_button(ui, icon, hint, style)) {
        action = Some(WindowAction::ToggleMaximize);
    }
    // After the three, which in a left-hand group puts it beside minimize the
    // way it is beside minimize on the right.
    if let Some(open) = settings {
        settings_toggle(ui, style, open);
    }
    action
}

/// The gear on its own, for a window whose frame the system is drawing: there
/// are then no controls of the app's for it to sit beside, and it is still the
/// only way into Settings.
///
/// See [`settings_toggle`] for what it does.
///
/// A toggle rather than a button because that is what it replaced: clicking the
/// control that opened a window is how everyone expects to close it again, and
/// the pressed look is what says the window is already open somewhere.
pub fn settings_button(ui: &mut Ui, style: &WindowButtons, open: &mut bool) {
    settings_toggle(ui, style, open)
}

fn settings_toggle(ui: &mut Ui, style: &WindowButtons, open: &mut bool) {
    let response = window_button(ui, Icon::Settings, tr("Settings"), style);
    // Drawn over the button rather than by it: the three painters know nothing
    // about a pressed state, and a gear that looks pressed while the window is
    // open is worth more than making all three learn about one.
    if *open {
        ui.painter().rect_stroke(
            response.rect.shrink(1.0),
            egui::Rounding::same(2.0),
            Stroke::new(1.0_f32, ui.visuals().widgets.active.fg_stroke.color),
        );
    }
    if response.clicked() {
        *open = !*open;
    }
}

/// Whether a control that may not be there was clicked.
fn clicked(response: Option<Response>) -> bool {
    response.is_some_and(|r| r.clicked())
}

/// Makes `rect` a handle the window can be dragged by, and reads a double
/// click on it as the usual "maximize / restore".
fn drag_area(ui: &mut Ui, rect: Rect, id: Id) -> Option<WindowAction> {
    if rect.width() <= 0.0 || rect.height() <= 0.0 {
        return None;
    }
    let drag = ui.interact(rect, id, Sense::click_and_drag());
    if drag.drag_started() {
        ui.ctx().send_viewport_cmd(ViewportCommand::StartDrag);
    }
    drag.double_clicked()
        .then_some(WindowAction::ToggleMaximize)
}

/// Text in the title bar that the window can be dragged by.
///
/// Two widgets over one rectangle, and that is the point: the words are drawn
/// with no interaction of their own - not even egui's click-and-drag to select
/// a label's text, which is what was quietly eating the drag and leaving the
/// window stuck - and the handle is then claimed over exactly the space they
/// took. So the reading matter in the bar is only ever a picture, and pressing
/// anywhere on the bar moves the window, the way a title bar does.
///
/// `draggable` is false while the system is drawing the frame: the real title
/// bar is doing the moving, and the text here is then simply text.
pub fn drag_text(
    ui: &mut Ui,
    text: impl Into<egui::WidgetText>,
    draggable: bool,
    window: &'static str,
    tag: &'static str,
) -> Option<WindowAction> {
    let response = ui.add(egui::Label::new(text).selectable(false));
    if !draggable {
        return None;
    }
    // The full height of the row rather than the height of the glyphs: a title
    // bar you can only take hold of by hitting the letters is not one. The row
    // is as tall as the tallest thing already placed in it, which is a button.
    let handle = Rect::from_x_y_ranges(response.rect.x_range(), ui.min_rect().y_range());
    drag_area(ui, handle, Id::new(("nit-titlebar-text", window, tag)))
}

/// Draws minimize / maximize / close at the right-hand end of the row, then
/// makes whatever space is left draggable.
///
/// Buttons first, dragging second: the drag area is the leftover rectangle, so
/// it cannot swallow the buttons however narrow the window gets.
///
/// `buttons` is false when the controls are not wanted here at all - either the
/// setting has them hidden, or the theme has already had them drawn on the left
/// by [`leading_window_buttons`]. The drag area is claimed either way: without
/// it the window could not be moved.
///
/// `settings` is the gear, drawn beside minimize when this is the group minimize
/// is in; `None` for a window that has no settings to open.
pub fn title_bar_controls(
    ui: &mut Ui,
    style: &WindowButtons,
    buttons: bool,
    window: &'static str,
    settings: Option<&mut bool>,
) -> Option<WindowAction> {
    let mut action = None;

    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
        if buttons {
            if clicked(optional_button(ui, Icon::Close, tr("Close"), style)) {
                action = Some(WindowAction::Close);
            }
            let (icon, hint) = maximize_icon(ui);
            if clicked(optional_button(ui, icon, hint, style)) {
                action = Some(WindowAction::ToggleMaximize);
            }
            if clicked(optional_button(ui, Icon::Minimize, tr("Minimize"), style)) {
                action = Some(WindowAction::Minimize);
            }
        }
        // Last in a right-to-left row, so it lands to the left of minimize -
        // which is where it was asked for, and which keeps the three controls
        // together at the corner where the pointer goes looking for them.
        if let Some(open) = settings {
            settings_toggle(ui, style, open);
        }

        // Everything between the buttons and whatever the caller has already
        // put on the left, all of it: a title bar you can only take hold of by
        // hitting its name is not one. What the caller placed - the tabs, when
        // the setting puts them up here - has already advanced the row's
        // cursor, so this is exactly the space they left.
        let rest = ui.available_rect_before_wrap();
        if rest.width() > 0.0 {
            // Keyed by the window that asked for the bar. Three of them draw
            // one now - the main window and the two dialogs that live in
            // windows of their own - and a shared id makes them one widget as
            // far as egui is concerned, so only whichever was drawn last would
            // answer to the mouse.
            let id = Id::new(("nit-titlebar-drag", window));
            if let Some(asked) = drag_area(ui, rest, id) {
                action = Some(asked);
            }
        }
    });

    action
}

/// Makes an existing stretch of the row a handle for the window, without
/// taking any space for it.
///
/// The space between two widgets is already there - it is the row's own item
/// spacing - and this is what makes it drag the window rather than do nothing.
/// Nothing is allocated: the caller has drawn what is on either side, and the
/// gap between them is what is claimed. That is the whole point, because a gap
/// wide enough to be worth allocating reads as a slot with something missing
/// from it.
///
/// `draggable` is false while the system is drawing the frame, when the real
/// title bar is doing the moving and a gap here is only a gap.
pub fn drag_span(
    ui: &mut Ui,
    x: std::ops::Range<f32>,
    draggable: bool,
    window: &'static str,
    tag: &'static str,
) -> Option<WindowAction> {
    if !draggable || x.end <= x.start {
        return None;
    }
    let rect = Rect::from_x_y_ranges(x.start..=x.end, ui.min_rect().y_range());
    drag_area(ui, rect, Id::new(("nit-titlebar-gap", window, tag)))
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

/// Puts a resize grip under the pointer when it is at the edge of `window`.
///
/// `keep_out` is the rectangles that are somebody else's whatever the edge
/// arithmetic says - the terminal panes, which reach the window edge and sense
/// drags of their own.
///
/// Only one grip exists, and only while the pointer is actually within
/// [`RESIZE_GRAB`] of an edge. Eight permanent ones seemed simpler, but a grip
/// has to sit in a foreground layer to beat the panels and the terminal, which
/// reach the window edge and sense drags of their own — and `layer_id_at` walks
/// layers back to front, so a foreground layer that is always present outranks
/// every window for hit-testing. That is what stopped the mouse wheel reaching
/// the Settings scroll area. Existing only under the pointer, at the very edge
/// of the window, it cannot be in anything's way.
pub fn resize_grips(ctx: &Context, window: &'static str, keep_out: &[Rect]) {
    // A maximized window has no edges to drag.
    if ctx.input(|i| i.viewport().maximized.unwrap_or(false)) {
        return;
    }

    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else {
        return;
    };
    // Whatever the arithmetic below works out, a rectangle the caller has
    // declared its own is not a resize handle. The terminal is the one that
    // matters: it reaches three window edges, and a grip over its first column
    // means that column cannot be clicked. Belt as well as braces - the inset
    // already keeps the two apart - because the failure is silent and the
    // person hitting it has no way to tell what took their click.
    if keep_out.iter().any(|rect| rect.contains(pos)) {
        return;
    }
    let Some((direction, cursor)) = edge_at(ctx.screen_rect(), pos) else {
        return;
    };

    egui::Area::new(Id::new(("nit-resize", window)))
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
