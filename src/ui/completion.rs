//! The autocomplete popup, drawn under the cursor.
//!
//! What it offers and when is decided in [`crate::features::autocomplete`];
//! this only draws it. It takes no input of its own, deliberately: a widget
//! that answered a click would take the keyboard from the terminal, the
//! terminal losing focus closes the popup, and the click would land on nothing.
//! Its keys are the terminal's - see [`crate::ui::input::CompletionKey`].

use egui::{Align2, Color32, Context, Rect, RichText};

use crate::config::Theme;
use crate::features::autocomplete::Popup;
use crate::i18n::tr;

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
    egui::Area::new(egui::Id::new(("nit-completion", tab_uid)))
        .order(egui::Order::Foreground)
        .interactable(false)
        .pivot(Align2::LEFT_TOP)
        .fixed_pos(egui::pos2(left, caret.bottom()))
        .show(ctx, |ui| {
            egui::Frame::popup(ui.style())
                .fill(theme.background)
                .stroke(egui::Stroke::new(1.0_f32, theme.selection))
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let size = caret.height() * 0.75;
                    for (index, candidate) in popup.items.iter().enumerate() {
                        let selected = index == popup.selected;
                        let fill = if selected {
                            theme.selection
                        } else {
                            Color32::TRANSPARENT
                        };
                        egui::Frame::none()
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
                                        RichText::new(tr(candidate.category.label()))
                                            .size(size * 0.85)
                                            .color(theme.foreground.gamma_multiply(0.6)),
                                    );
                                });
                            });
                    }
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
    }
}
