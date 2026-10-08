//! The autocomplete popup, drawn under the cursor.
//!
//! What it offers and when is decided in [`crate::features::autocomplete`];
//! this only draws it. It takes no input of its own, deliberately: a widget
//! that answered a click would take the keyboard from the terminal, the
//! terminal losing focus closes the popup, and the click would land on nothing.
//! Its keys are the terminal's - see [`crate::ui::input::CompletionKey`].

use egui::{Align2, Color32, Context, Rect, RichText};

use crate::config::Theme;
use crate::features::autocomplete::{Popup, MAX_SHOWN};

/// Draws `popup` just below `caret`, the cursor's cell on screen.
///
/// In the terminal's own colours and font size, so it reads as part of the
/// line it is completing rather than as a dialog that has opened over it.
pub fn show(ctx: &Context, popup: &Popup, caret: Rect, theme: &Theme, tab_uid: u64) {
    // Under the word being typed rather than under the cursor, so the
    // suggestions line up with the letters they continue - sigil and all,
    // since every suggestion in one popup shares the sigil that was typed.
    let sigil = popup
        .items
        .first()
        .map_or(0, |c| c.category.sigil().chars().count());
    let typed = (popup.token.text.chars().count() + sigil) as f32;
    let left = caret.left() - typed * caret.width();
    let id = egui::Id::new(("nit-completion", tab_uid));
    // Below the line, unless it would not fit there - then above it, if there
    // is more room above. Left to itself egui pushes an area that runs off the
    // bottom back up onto the screen, which put the list over the very line
    // being completed. The height is the one drawn last frame, or a guess from
    // the row count on the frame it first opens.
    let row = caret.height() * 1.1;
    let rows = popup.items.len() + usize::from(popup.hint.is_some());
    let height = ctx
        .memory(|m| m.area_rect(id))
        .map_or(rows as f32 * row + 12.0, |r| r.height());
    let screen = ctx.screen_rect();
    let room_below = screen.bottom() - caret.bottom();
    let room_above = caret.top() - screen.top();
    let below = height <= room_below || room_below >= room_above;
    let (pivot, top, room) = if below {
        (Align2::LEFT_TOP, caret.bottom(), room_below)
    } else {
        (Align2::LEFT_BOTTOM, caret.top(), room_above)
    };
    // A subscript's values can run to dozens, and a list that tall was cut
    // off by the edge of the window with no way to see the rest. So the list
    // scrolls: no taller than the room on its side - less the frame and the
    // hint - and never more than `MAX_SHOWN` rows, past which a list stops
    // being something read at a glance.
    let hint_height = if popup.hint.is_some() { row } else { 0.0 };
    let list_height = (room - 16.0 - hint_height)
        .min(MAX_SHOWN as f32 * row)
        .max(row);
    egui::Area::new(id)
        .order(egui::Order::Foreground)
        .interactable(false)
        .pivot(pivot)
        .fixed_pos(egui::pos2(left, top))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .fill(theme.background)
                .stroke(egui::Stroke::new(1.0_f32, theme.selection))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let size = caret.height() * 0.75;
                    if let Some(hint) = &popup.hint {
                        egui::Frame::none()
                            .inner_margin(egui::Margin::symmetric(4.0, 1.0))
                            .show(ui, |ui| {
                                ui.label(
                                    RichText::new(hint)
                                        .size(size * 0.85)
                                        .italics()
                                        .color(theme.foreground.gamma_multiply(0.75)),
                                );
                            });
                    }
                    egui::ScrollArea::vertical()
                        .id_source(("nit-completion-list", tab_uid))
                        .max_height(list_height)
                        // As tall as asked, not as tall as the area says there
                        // is room for. An area is laid out as if it grew down
                        // from its corner, so one opened upwards from the last
                        // line offered only the line or two below that corner,
                        // and the list never grew past it. Still shrinks to a
                        // short list: `auto_shrink` sees to that.
                        .min_scrolled_height(list_height)
                        .auto_shrink([true, true])
                        .show(ui, |ui| {
                            for (index, candidate) in popup.items.iter().enumerate() {
                                let selected = index == popup.selected;
                                let fill = if selected {
                                    theme.selection
                                } else {
                                    Color32::TRANSPARENT
                                };
                                let response = egui::Frame::none()
                                    .fill(fill)
                                    .inner_margin(egui::Margin::symmetric(4.0, 1.0))
                                    .show(ui, |ui| {
                                        ui.horizontal(|ui| {
                                            ui.label(
                                                RichText::new(candidate.display())
                                                    .monospace()
                                                    .size(size)
                                                    .color(theme.syntax_color(kind_of(candidate)))
                                                    .strong(),
                                            );
                                            ui.label(
                                                RichText::new(candidate.label())
                                                    .size(size * 0.85)
                                                    .color(theme.foreground.gamma_multiply(0.6)),
                                            );
                                        });
                                    })
                                    .response;
                                // The popup takes no input, so the wheel never
                                // reaches it: the list scrolls by following
                                // the selection the arrow keys move.
                                if selected {
                                    response.scroll_to_me(None);
                                }
                            }
                        });
                });
        });
}

/// The colour a suggestion is drawn in: the colour it will have on the line
/// once accepted, which says what it is before the label beside it is read.
fn kind_of(candidate: &crate::features::autocomplete::Candidate) -> crate::term::syntax::Kind {
    use crate::features::autocomplete::Category;
    use crate::term::syntax::Kind;
    match candidate.category {
        Category::Command | Category::SqlKeyword => Kind::Command,
        Category::Function | Category::SqlFunction => Kind::Function,
        Category::SystemVariable => Kind::SystemVariable,
        Category::SystemClass | Category::Class | Category::Table => Kind::ObjectClass,
        Category::Global => Kind::Global,
        Category::Routine => Kind::Routine,
        Category::Entry => Kind::Extrinsic,
        Category::Subscript => Kind::Number,
    }
}
