//! The Screen saver page, laid out the way macOS lays out its own: the saver
//! running live in a little monitor at the top, and what it is, when it
//! starts and what it shows in cards under it.
//!
//! It used to be a copy of Windows XP's Screen Saver tab, with OK, Cancel and
//! Apply, because the dialog it copied had them. They are gone: nothing about
//! a screen saver takes effect while the window is in use - it only starts
//! after minutes without a key or a movement - so a change applied as it is
//! made cannot get in anyone's way, and every other page here applies its
//! changes the same way. The monitor shows the setting itself, not a draft of
//! it, and Preview runs that over the whole window.

use std::sync::mpsc::Receiver;
use std::time::Duration;

use egui::{Color32, Context, Pos2, Rect, Rounding, Stroke, Ui, Vec2};

use super::{section, untitled, Category, Ctx, Item, Page, Section};
use crate::features::screensaver::{Config, DvdContent, Kind, LogoContent};
use crate::i18n::tr;
use crate::ui::panels::{UiRequest, WARNING};
use crate::ui::prefs::{self, Card, Row};
use crate::ui::screensaver_view::SaverView;
use crate::ui::shading::{darken, gradient};

/// What the page keeps between frames.
#[derive(Default)]
pub struct SaverPageState {
    /// The saver running in the little monitor.
    preview: Option<SaverView>,
    /// The file dialog picking the custom image, while it is open. It runs on
    /// a thread of its own: modal on this one, it would stop every session's
    /// output being drained for as long as it stayed up.
    picking: Option<Receiver<Option<String>>>,
}

pub(super) fn page() -> Page {
    super::page(Category::ScreenSaver, sections()).with_top(monitor_top)
}

fn config<'c>(c: &'c Ctx<'_>) -> &'c Config {
    &c.settings.screensaver
}

fn some(c: &Ctx<'_>) -> bool {
    config(c).kind != Kind::None
}

fn is_logo(c: &Ctx<'_>) -> bool {
    config(c).kind == Kind::FloatingLogo
}

fn is_dvd(c: &Ctx<'_>) -> bool {
    config(c).kind == Kind::Dvd
}

fn is_matrix(c: &Ctx<'_>) -> bool {
    config(c).kind == Kind::Matrix
}

fn sections() -> Vec<Section> {
    vec![
        untitled(vec![
            Item::control("screensaver", "Screen saver", kind)
                .sub("What covers the window after a while without a key or a mouse movement.")
                .keys(&[
                    "idle", "saver", "protecao", "ocioso", "matrix", "dvd", "logo",
                ]),
            Item::control("saver_wait", "Start after", wait)
                .when(some)
                .keys(&["wait", "minutes", "idle", "esperar", "minutos", "ocioso"]),
            Item::control("saver_speed", "Speed", speed)
                .when(some)
                .keys(&["velocidade", "fast", "slow", "rapido", "lento"]),
        ])
        .footer("Any key or mouse movement ends it. The key that does is not sent to the session."),
        section(
            "What it shows",
            vec![
                Item::control("saver_logo", "Shows", logo)
                    .when(is_logo)
                    .keys(&[
                        "logo",
                        "windows xp",
                        "pirated",
                        "text",
                        "image",
                        "texto",
                        "imagem",
                    ]),
                Item::rows("saver_logo_text", "Text", logo_text)
                    .when(|c| is_logo(c) && config(c).logo == LogoContent::CustomText)
                    .keys(&["logo", "texto", "colour", "color", "cor"]),
                Item::rows("saver_logo_image", "File", logo_image)
                    .when(|c| is_logo(c) && config(c).logo == LogoContent::CustomImage)
                    .keys(&[
                        "image", "picture", "gif", "png", "browse", "imagem", "arquivo", "procurar",
                    ]),
                Item::control("saver_logo_size", "Size", logo_size)
                    .when(is_logo)
                    .keys(&["logo", "scale", "tamanho", "escala"]),
                Item::control("saver_dvd", "Bounces", dvd)
                    .when(is_dvd)
                    .keys(&["dvd", "logo", "text", "texto"]),
                Item::rows("saver_dvd_text", "Text", dvd_text)
                    .when(|c| is_dvd(c) && config(c).dvd == DvdContent::CustomText)
                    .keys(&["dvd", "texto"]),
                Item::control("saver_rain", "Rain", |ui, c| {
                    rgb(ui, c, |s| &mut s.matrix_rain)
                })
                .when(is_matrix)
                .keys(&[
                    "matrix", "chuva", "colour", "color", "cor", "green", "verde",
                ]),
                Item::control("saver_head", "Leading glyph", |ui, c| {
                    rgb(ui, c, |s| &mut s.matrix_head)
                })
                .when(is_matrix)
                .keys(&["matrix", "head", "colour", "color", "cor"]),
                Item::control("saver_matrix_background", "Background", |ui, c| {
                    rgb(ui, c, |s| &mut s.matrix_background)
                })
                .when(is_matrix)
                .keys(&["matrix", "fundo", "colour", "color", "cor"]),
                Item::rows("saver_film", "Film colours", film_colours)
                    .when(is_matrix)
                    .keys(&["matrix", "reset", "original", "filme", "restaurar"]),
            ],
        ),
    ]
}

fn kind(ui: &mut Ui, c: &mut Ctx<'_>) {
    let saver = &mut c.settings.screensaver;
    let mut changed = false;
    prefs::popup(ui, "screensaver-kind", tr(saver.kind.label()), |ui| {
        for kind in Kind::ALL {
            changed |= ui
                .selectable_value(&mut saver.kind, kind, tr(kind.label()))
                .changed();
        }
    });
    c.changed |= changed;
}

fn wait(ui: &mut Ui, c: &mut Ctx<'_>) {
    ui.label(tr("minutes"));
    c.changed |= ui
        .add(
            egui::DragValue::new(&mut c.settings.screensaver.wait_minutes)
                .range(1..=240)
                .speed(0.2),
        )
        .changed();
}

fn speed(ui: &mut Ui, c: &mut Ctx<'_>) {
    c.changed |= prefs::slider(
        ui,
        &mut c.settings.screensaver.speed,
        0.25..=3.0,
        Some((tr("Slow"), tr("Fast"))),
        |s| s,
    )
    .changed();
}

/// What the floating logo shows.
fn logo(ui: &mut Ui, c: &mut Ctx<'_>) {
    let saver = &mut c.settings.screensaver;
    let mut changed = false;
    prefs::popup(ui, "screensaver-logo", tr(saver.logo.label()), |ui| {
        for logo in LogoContent::ALL {
            changed |= ui
                .selectable_value(&mut saver.logo, logo, tr(logo.label()))
                .changed();
        }
    });
    c.changed |= changed;
}

/// The text that floats, and its colour - with what happens when there is
/// none said under it.
fn logo_text(card: &mut Card<'_>, c: &mut Ctx<'_>) {
    let saver = &mut c.settings.screensaver;
    let note = saver
        .logo_text
        .trim()
        .is_empty()
        .then(|| tr("With no text, the Windows XP logo is shown."));
    let mut changed = false;
    card.row(Row::new(tr("Text")).subtitle(note), |ui| {
        changed |= ui
            .color_edit_button_srgb(&mut saver.logo_colour)
            .on_hover_text(tr("Text colour"))
            .changed();
        changed |= ui
            .add(
                egui::TextEdit::singleline(&mut saver.logo_text)
                    .desired_width(220.0)
                    .hint_text(tr("What floats across the screen")),
            )
            .changed();
    });
    c.changed |= changed;
}

/// The picture that floats, typed or browsed for, and why it is not the one
/// showing when it is not.
fn logo_image(card: &mut Card<'_>, c: &mut Ctx<'_>) {
    let image_problem = c
        .state
        .screensaver
        .preview
        .as_ref()
        .and_then(|v| v.image_problem().map(str::to_owned));
    let path = c.settings.screensaver.logo_image.trim().to_owned();
    let warning = if path.is_empty() {
        None
    } else if !crate::ui::screensaver_image::looks_like_picture(&path) {
        Some(tr("Only PNG and GIF files can be shown.").to_owned())
    } else {
        image_problem
    };
    let note = match &warning {
        Some(why) => Some(format!(
            "{why} {}",
            tr("The Windows XP logo is shown instead.")
        )),
        None if path.is_empty() => {
            Some(tr("With no file, the Windows XP logo is shown.").to_owned())
        }
        None => None,
    };
    let picking = &mut c.state.screensaver.picking;
    let saver = &mut c.settings.screensaver;
    let mut changed = false;
    card.row(Row::new(tr("File")).subtitle(note.as_deref()), |ui| {
        if cfg!(windows)
            && ui
                .add_enabled(picking.is_none(), prefs::button_widget(tr("Browse...")))
                .clicked()
        {
            *picking = Some(pick_image(ui.ctx(), &saver.logo_image));
        }
        let edit = egui::TextEdit::singleline(&mut saver.logo_image)
            .desired_width(220.0)
            .hint_text(tr("A PNG or GIF file"));
        let edit = if warning.is_some() {
            edit.text_color(WARNING)
        } else {
            edit
        };
        changed |= ui.add(edit).changed();
    });
    c.changed |= changed;
}

fn logo_size(ui: &mut Ui, c: &mut Ctx<'_>) {
    c.changed |= prefs::slider(
        ui,
        &mut c.settings.screensaver.logo_scale,
        0.5..=2.5,
        None,
        |s| s,
    )
    .changed();
}

/// What the DVD saver bounces. Text only: the colour is the saver's, a new
/// one at every wall.
fn dvd(ui: &mut Ui, c: &mut Ctx<'_>) {
    let saver = &mut c.settings.screensaver;
    let mut changed = false;
    prefs::popup(ui, "screensaver-dvd", tr(saver.dvd.label()), |ui| {
        for dvd in DvdContent::ALL {
            changed |= ui
                .selectable_value(&mut saver.dvd, dvd, tr(dvd.label()))
                .changed();
        }
    });
    c.changed |= changed;
}

fn dvd_text(card: &mut Card<'_>, c: &mut Ctx<'_>) {
    let saver = &mut c.settings.screensaver;
    let note = saver
        .dvd_text
        .trim()
        .is_empty()
        .then(|| tr("With no text, the DVD logo is shown."));
    let mut changed = false;
    card.row(Row::new(tr("Text")).subtitle(note), |ui| {
        changed |= ui
            .add(
                egui::TextEdit::singleline(&mut saver.dvd_text)
                    .desired_width(220.0)
                    .hint_text(tr("What bounces round the screen")),
            )
            .changed();
    });
    c.changed |= changed;
}

/// One of the rain's colours.
fn rgb(ui: &mut Ui, c: &mut Ctx<'_>, get: fn(&mut Config) -> &mut [u8; 3]) {
    c.changed |= ui
        .color_edit_button_srgb(get(&mut c.settings.screensaver))
        .changed();
}

/// The way back to the film's colours, once they have been changed.
fn film_colours(card: &mut Card<'_>, c: &mut Ctx<'_>) {
    use crate::features::screensaver::{MATRIX_BACKGROUND, MATRIX_HEAD, MATRIX_RAIN};
    let saver = &mut c.settings.screensaver;
    let film = (MATRIX_RAIN, MATRIX_HEAD, MATRIX_BACKGROUND);
    let now = (
        saver.matrix_rain,
        saver.matrix_head,
        saver.matrix_background,
    );
    let mut reset = false;
    card.buttons(|ui| {
        reset = ui
            .add_enabled(now != film, prefs::button_widget(tr("Film colours")))
            .clicked();
    });
    if reset {
        (
            saver.matrix_rain,
            saver.matrix_head,
            saver.matrix_background,
        ) = film;
        c.changed = true;
    }
}

/// The little monitor, and the button that runs the saver for real.
///
/// The monitor moves, and Settings is drawn inside the main window's frame,
/// so it is the main window that has to ask for the next frame - when the
/// saver in it next changes, and no faster than a small screen needs. Only
/// while the page is drawn: a closed window or another page asks for nothing.
fn monitor_top(ui: &mut Ui, c: &mut Ctx<'_>) {
    let config = c.settings.screensaver.clone();
    let mut next = Duration::MAX;
    let mut preview = false;
    ui.add_space(6.0);
    ui.vertical_centered(|ui| {
        let view = c
            .state
            .screensaver
            .preview
            .get_or_insert_with(|| SaverView::new(config.kind));
        if view.kind() != config.kind {
            *view = SaverView::new(config.kind);
        }
        next = monitor(ui, view, &config);
        ui.add_space(6.0);
        preview = ui
            .add_enabled(
                config.kind != Kind::None,
                prefs::button_widget(tr("Preview")),
            )
            .on_hover_text(tr(
                "Runs it over the whole window now, until the next key or movement.",
            ))
            .clicked();
    });
    ui.add_space(4.0);
    if preview {
        c.requests.push(UiRequest::PreviewScreensaver(config));
    }
    if next != Duration::MAX {
        ui.ctx()
            .request_repaint_after_for(next.max(Duration::from_millis(40)), egui::ViewportId::ROOT);
    }
}

/// The answer of a file dialog opened from the page, whatever page is on
/// screen when it comes.
pub(super) fn collect_picked_image(ctx: &Context, c: &mut Ctx<'_>) {
    let state = &mut c.state.screensaver;
    let Some(picking) = state.picking.as_ref() else {
        return;
    };
    match picking.try_recv() {
        Ok(picked) => {
            state.picking = None;
            if let Some(path) = picked {
                c.settings.screensaver.logo_image = path;
                c.changed = true;
            }
        }
        // Often enough to see its answer, and no more: the thread wakes the
        // window itself when it has one.
        Err(std::sync::mpsc::TryRecvError::Empty) => {
            ctx.request_repaint_after_for(Duration::from_millis(250), egui::ViewportId::ROOT);
        }
        Err(std::sync::mpsc::TryRecvError::Disconnected) => state.picking = None,
    }
}

/// Opens the system's file dialog for a PNG or GIF, starting at `current`;
/// the answer arrives on the receiver, `None` if it was cancelled.
fn pick_image(ctx: &Context, current: &str) -> Receiver<Option<String>> {
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx = ctx.clone();
    let current = current.trim().to_owned();
    let title = tr("Choose a picture");
    let kind = tr("Images");
    std::thread::spawn(move || {
        let _ = tx.send(open_file_dialog(title, kind, &current));
        ctx.request_repaint_of(egui::ViewportId::ROOT);
    });
    rx
}

#[cfg(windows)]
fn open_file_dialog(title: &str, kind: &str, current: &str) -> Option<String> {
    use windows_sys::Win32::UI::Controls::Dialogs::{
        GetOpenFileNameW, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW,
    };
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    // Pairs of name and pattern, each ended by a NUL, and the list by one more.
    let filter: Vec<u16> = format!("{kind} (*.png, *.gif)\0*.png;*.gif\0\0")
        .encode_utf16()
        .collect();
    let title = wide(title);
    let mut file = vec![0u16; 4096];
    // Starting from the file already chosen opens the dialog in its folder.
    for (slot, unit) in file.iter_mut().zip(current.encode_utf16().take(4095)) {
        *slot = unit;
    }
    // SAFETY: OPENFILENAMEW is plain data - integers, pointers and an
    // optional callback - for which all zeroes is the documented "unset".
    let mut ofn: OPENFILENAMEW = unsafe { std::mem::zeroed() };
    ofn.lStructSize = std::mem::size_of::<OPENFILENAMEW>() as u32;
    ofn.lpstrFilter = filter.as_ptr();
    ofn.lpstrFile = file.as_mut_ptr();
    ofn.nMaxFile = file.len() as u32;
    ofn.lpstrTitle = title.as_ptr();
    // NOCHANGEDIR: left to itself the dialog moves the whole process's
    // working directory to wherever the file was.
    ofn.Flags = OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR;
    // SAFETY: every buffer the struct points at outlives the call.
    if unsafe { GetOpenFileNameW(&mut ofn) } == 0 {
        return None;
    }
    let end = file.iter().position(|&c| c == 0).unwrap_or(file.len());
    Some(String::from_utf16_lossy(&file[..end]))
}

/// Elsewhere there is no dialog to open, and the path is typed instead.
#[cfg(not(windows))]
fn open_file_dialog(_title: &str, _kind: &str, _current: &str) -> Option<String> {
    None
}

/// The CRT the preview runs on: a beige-grey bezel, the screen, and a stand.
fn monitor(ui: &mut Ui, view: &mut SaverView, config: &Config) -> Duration {
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
    let next = view.paint(painter, screen, config);

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
    next
}
