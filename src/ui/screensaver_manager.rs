//! The screen saver dialog, laid out the way Windows XP's Screen Saver tab
//! was: a little monitor running the saver live, the picker with Settings and
//! Preview beside it, the wait in minutes, and OK / Cancel / Apply.
//!
//! Unlike the theme and macro managers this one has an OK button, because the
//! dialog it copies did: what is picked here is a draft until Apply or OK, and
//! Cancel leaves the setting as it was. Like them, it only decides - the app
//! saves.

use std::time::Duration;

use egui::{Color32, Context, Pos2, Rect, Rounding, Stroke, Ui, Vec2};

use crate::features::screensaver::{Config, Kind};
use crate::i18n::tr;
use crate::ui::screensaver_view::SaverView;
use crate::ui::shading::{darken, gradient};

/// Something the dialog asked for.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ScreensaverAction {
    /// Save this as the screen saver setting.
    Apply(Config),
    /// Run this over the whole window now, until the next key or movement.
    Preview(Config),
}

#[derive(Default)]
pub struct ScreensaverManagerState {
    pub open: bool,
    /// What the dialog shows, before Apply makes it the setting.
    draft: Option<Config>,
    /// The saver running in the little monitor.
    preview: Option<SaverView>,
    /// Whether the Settings button has unfolded the speed row.
    show_options: bool,
}

pub fn screensaver_manager(
    ctx: &Context,
    state: &mut ScreensaverManagerState,
    current: &Config,
    buttons: &crate::config::theme::WindowButtons,
) -> Vec<ScreensaverAction> {
    let mut actions = Vec::new();
    if !state.open {
        state.draft = None;
        state.preview = None;
        return actions;
    }
    let mut open = true;
    let mut close = false;
    let mut draft = state.draft.unwrap_or(*current);

    crate::ui::detach::shell(
        ctx,
        "nit-screensaver",
        tr("Screen Saver"),
        &mut open,
        [400.0, 470.0],
        buttons,
        None,
        |ui| {
            ui.spacing_mut().item_spacing.y = 8.0;
            ui.vertical_centered(|ui| {
                let view = state
                    .preview
                    .get_or_insert_with(|| SaverView::new(draft.kind));
                if view.kind() != draft.kind {
                    *view = SaverView::new(draft.kind);
                }
                monitor(ui, view, draft.speed);
            });

            group(ui, tr("Screen saver"), |ui| {
                ui.horizontal(|ui| {
                    egui::ComboBox::from_id_source("screensaver-kind")
                        .width(170.0)
                        .selected_text(tr(draft.kind.label()))
                        .show_ui(ui, |ui| {
                            for kind in Kind::ALL {
                                ui.selectable_value(&mut draft.kind, kind, tr(kind.label()));
                            }
                        });
                    let some = draft.kind != Kind::None;
                    if ui
                        .add_enabled(some, egui::Button::new(tr("Settings")))
                        .clicked()
                    {
                        state.show_options = !state.show_options;
                    }
                    if ui
                        .add_enabled(some, egui::Button::new(tr("Preview")))
                        .clicked()
                    {
                        actions.push(ScreensaverAction::Preview(draft));
                    }
                });
                ui.horizontal(|ui| {
                    ui.label(tr("Wait:"));
                    ui.add(
                        egui::DragValue::new(&mut draft.wait_minutes)
                            .range(1..=240)
                            .speed(0.2),
                    );
                    ui.label(tr("minutes"));
                });
                if state.show_options && draft.kind != Kind::None {
                    ui.horizontal(|ui| {
                        ui.label(tr("Speed"));
                        ui.add(egui::Slider::new(&mut draft.speed, 0.25..=3.0));
                    });
                }
                ui.small(tr("Any key or mouse movement ends it. The key that does is not sent to the session."));
            });

            ui.add_space(6.0);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let dirty = draft != *current;
                if ui
                    .add_enabled(dirty, egui::Button::new(tr("Apply")))
                    .clicked()
                {
                    actions.push(ScreensaverAction::Apply(draft));
                }
                if ui.button(tr("Cancel")).clicked() {
                    close = true;
                }
                if ui.button(tr("OK")).clicked() {
                    if dirty {
                        actions.push(ScreensaverAction::Apply(draft));
                    }
                    close = true;
                }
            });
        },
    );

    // The little monitor moves, and this window is drawn inside the main
    // one's frame, so it is the main one that has to ask for the next frame.
    ctx.request_repaint_after_for(Duration::from_millis(40), egui::ViewportId::ROOT);

    state.draft = Some(draft);
    if close || !open {
        // Closing by the title bar is Cancel, as it was on XP.
        state.open = false;
        state.draft = None;
        state.preview = None;
        state.show_options = false;
    }
    actions
}

/// A titled frame, like XP's group boxes.
fn group(ui: &mut Ui, title: &str, contents: impl FnOnce(&mut Ui)) {
    ui.label(title);
    egui::Frame::group(ui.style())
        .inner_margin(egui::Margin::same(10.0))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            contents(ui);
        });
}

/// The CRT the preview runs on: a beige-grey bezel, the screen, and a stand.
fn monitor(ui: &mut Ui, view: &mut SaverView, speed: f32) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(200.0, 175.0), egui::Sense::hover());
    let painter = ui.painter();
    let plastic = Color32::from_rgb(226, 226, 220);

    let bezel = Rect::from_min_size(rect.min + Vec2::new(10.0, 0.0), Vec2::new(180.0, 140.0));
    painter.rect_filled(bezel, Rounding::same(8.0), darken(plastic, 0.55));
    let face = bezel.shrink(1.5);
    painter.rect_filled(face, Rounding::same(7.0), plastic);
    gradient(
        painter,
        Rect::from_min_max(
            face.min + Vec2::new(4.0, 4.0),
            Pos2::new(face.right() - 4.0, face.center().y),
        ),
        Color32::from_rgba_unmultiplied(255, 255, 255, 90),
        Color32::from_rgba_unmultiplied(255, 255, 255, 0),
    );
    let screen = bezel.shrink2(Vec2::new(14.0, 13.0));
    painter.rect_stroke(
        screen.expand(1.0),
        2.0,
        Stroke::new(1.5_f32, darken(plastic, 0.45)),
    );
    view.paint(painter, screen, speed);

    // The neck and the foot.
    let neck = Rect::from_center_size(
        Pos2::new(rect.center().x, bezel.bottom() + 9.0),
        Vec2::new(46.0, 18.0),
    );
    painter.rect_filled(neck, 0.0, darken(plastic, 0.80));
    let foot = Rect::from_center_size(
        Pos2::new(rect.center().x, neck.bottom() + 6.0),
        Vec2::new(120.0, 12.0),
    );
    painter.rect_filled(foot, Rounding::same(6.0), darken(plastic, 0.70));
    painter.rect_filled(
        Rect::from_min_size(
            foot.min + Vec2::new(4.0, 1.0),
            Vec2::new(foot.width() - 8.0, 4.0),
        ),
        Rounding::same(2.0),
        darken(plastic, 0.92),
    );
}
