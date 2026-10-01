//! Draws the screen saver, over the whole window or inside the little monitor
//! in its settings dialog, and keeps the clock it starts by.
//!
//! The motion is [`crate::features::screensaver`]'s; this only turns a scene
//! into shapes, and every size here is a fraction of the rect it is given, so
//! the preview is the real thing shrunk rather than a picture of it.

use std::time::{Duration, Instant};

use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Color32, Context, FontId, Id, Mesh, Painter, Pos2, Rect, Shape, Stroke, Vec2};

use crate::features::screensaver::{Kind, Saver, Scene};

/// A saver and the time it was last stepped, which is what its `dt` comes
/// from.
pub struct SaverView {
    pub saver: Saver,
    last: Option<Instant>,
}

impl SaverView {
    pub fn new(kind: Kind) -> Self {
        // Seeded from the clock so two runs do not start in the same place;
        // nothing depends on which seed it was.
        let seed = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x5eed);
        SaverView {
            saver: Saver::new(kind, seed),
            last: None,
        }
    }

    pub fn kind(&self) -> Kind {
        self.saver.kind
    }

    /// Steps the scene to now and paints it filling `rect`.
    pub fn paint(&mut self, painter: &Painter, rect: Rect, speed: f32) {
        let now = Instant::now();
        let dt = self
            .last
            .map_or(0.0, |last| now.duration_since(last).as_secs_f32());
        self.last = Some(now);

        painter.rect_filled(rect, 0.0, Color32::BLACK);
        let painter = painter.with_clip_rect(rect);
        match self.saver.kind {
            Kind::None => {}
            Kind::XpLogo => {
                let logo = xp_size(rect);
                self.saver.step(dt, speed, room(rect, logo), (0, 0));
                if let Some(Scene::XpLogo(scene)) = &self.saver.scene {
                    let space = room(rect, logo);
                    let at = rect.min + Vec2::new(scene.at.0 * space.0, scene.at.1 * space.1);
                    paint_xp(&painter, Rect::from_min_size(at, logo), scene.alpha());
                }
            }
            Kind::Matrix => {
                let cell = matrix_cell(rect);
                let grid = (
                    (rect.width() / cell.x).ceil() as usize,
                    (rect.height() / cell.y).ceil() as usize,
                );
                self.saver.step(dt, speed, (0.0, 0.0), grid);
                if let Some(Scene::Matrix(rain)) = &self.saver.scene {
                    paint_matrix(&painter, rect, cell, rain);
                }
            }
            Kind::Dvd => {
                let logo = dvd_size(rect);
                self.saver.step(dt, speed, room(rect, logo), (0, 0));
                if let Some(Scene::Dvd(dvd)) = &self.saver.scene {
                    let at = rect.min + Vec2::new(dvd.pos.0, dvd.pos.1);
                    paint_dvd(
                        &painter,
                        Rect::from_min_size(at, logo),
                        DVD_HUES[dvd.hue % DVD_HUES.len()],
                    );
                }
            }
        }
    }
}

/// How far a logo of `logo` can travel inside `rect`.
fn room(rect: Rect, logo: Vec2) -> (f32, f32) {
    (
        (rect.width() - logo.x).max(1.0),
        (rect.height() - logo.y).max(1.0),
    )
}

fn faded(color: Color32, alpha: f32) -> Color32 {
    color.gamma_multiply(alpha.clamp(0.0, 1.0))
}

fn xp_size(rect: Rect) -> Vec2 {
    let h = (rect.height() * 0.16).clamp(18.0, 120.0);
    Vec2::new(h * 2.9, h)
}

/// The four-pane flag, waving, and the wordmark beside it.
fn paint_xp(painter: &Painter, rect: Rect, alpha: f32) {
    let h = rect.height();
    let flag = Rect::from_min_size(rect.min, Vec2::new(h * 1.05, h));
    let gap = h * 0.05;
    let half = Vec2::new((flag.width() - gap) / 2.0, (flag.height() - gap) / 2.0);
    let panes = [
        (Vec2::ZERO, Color32::from_rgb(246, 83, 20)),
        (Vec2::new(half.x + gap, 0.0), Color32::from_rgb(124, 187, 0)),
        (Vec2::new(0.0, half.y + gap), Color32::from_rgb(0, 161, 241)),
        (
            Vec2::new(half.x + gap, half.y + gap),
            Color32::from_rgb(255, 187, 0),
        ),
    ];
    for (offset, colour) in panes {
        waved_pane(
            painter,
            Rect::from_min_size(flag.min + offset, half),
            flag.left(),
            flag.width(),
            h * 0.07,
            faded(colour, alpha),
        );
    }

    let text = Pos2::new(flag.right() + h * 0.18, rect.top() + h * 0.30);
    painter.text(
        text,
        Align2::LEFT_BOTTOM,
        "Microsoft\u{00ae}",
        FontId::proportional(h * 0.17),
        faded(Color32::WHITE, alpha),
    );
    let word = painter.text(
        text,
        Align2::LEFT_TOP,
        "Windows",
        FontId::proportional(h * 0.48),
        faded(Color32::WHITE, alpha),
    );
    painter.text(
        Pos2::new(word.right() + h * 0.04, word.top() - h * 0.06),
        Align2::LEFT_TOP,
        "xp",
        FontId::proportional(h * 0.36),
        faded(Color32::from_rgb(255, 120, 30), alpha),
    );
}

/// One pane of the flag, bent by the same wave across the whole flag so the
/// four read as one cloth rather than four tiles.
fn waved_pane(
    painter: &Painter,
    rect: Rect,
    flag_left: f32,
    flag_width: f32,
    amp: f32,
    colour: Color32,
) {
    const STRIPS: usize = 10;
    let wave = |x: f32| ((x - flag_left) / flag_width * std::f32::consts::PI * 1.4).sin() * amp;
    let mut mesh = Mesh::default();
    for i in 0..=STRIPS {
        let x = rect.left() + rect.width() * i as f32 / STRIPS as f32;
        let dy = wave(x);
        mesh.colored_vertex(Pos2::new(x, rect.top() + dy), colour);
        mesh.colored_vertex(Pos2::new(x, rect.bottom() + dy), colour);
    }
    for i in 0..STRIPS as u32 {
        let a = i * 2;
        mesh.add_triangle(a, a + 1, a + 2);
        mesh.add_triangle(a + 2, a + 1, a + 3);
    }
    painter.add(Shape::mesh(mesh));
}

fn matrix_cell(rect: Rect) -> Vec2 {
    let size = (rect.height() / 42.0).clamp(5.0, 16.0);
    Vec2::new(size * 0.75, size * 1.1)
}

fn paint_matrix(
    painter: &Painter,
    rect: Rect,
    cell: Vec2,
    rain: &crate::features::screensaver::Matrix,
) {
    let font = FontId::monospace(cell.y / 1.1);
    for (x, column) in rain.columns.iter().enumerate() {
        let head = column.head.floor() as i64;
        let tail = head - column.trail as i64;
        for row in tail.max(0)..=head.min(rain.rows as i64 - 1) {
            let Some(&glyph) = column.glyphs.get(row as usize) else {
                continue;
            };
            let colour = if row == head {
                Color32::from_rgb(210, 255, 210)
            } else {
                let fade = 1.0 - (head - row) as f32 / column.trail as f32;
                Color32::from_rgb(0, (90.0 + 165.0 * fade) as u8, (40.0 * fade) as u8)
                    .gamma_multiply(fade.max(0.15))
            };
            painter.text(
                rect.min + Vec2::new(x as f32 * cell.x, row as f32 * cell.y),
                Align2::LEFT_TOP,
                glyph,
                font.clone(),
                colour,
            );
        }
    }
}

fn dvd_size(rect: Rect) -> Vec2 {
    let w = (rect.width() * 0.17).clamp(36.0, 240.0);
    Vec2::new(w, w * 0.52)
}

/// The colours the logo changes through, one per bounce.
const DVD_HUES: [Color32; 7] = [
    Color32::from_rgb(255, 255, 255),
    Color32::from_rgb(255, 60, 60),
    Color32::from_rgb(60, 140, 255),
    Color32::from_rgb(255, 210, 40),
    Color32::from_rgb(80, 230, 120),
    Color32::from_rgb(230, 90, 230),
    Color32::from_rgb(255, 140, 30),
];

fn paint_dvd(painter: &Painter, rect: Rect, colour: Color32) {
    let w = rect.width();
    let mut job = LayoutJob::default();
    job.append(
        "DVD",
        0.0,
        TextFormat {
            font_id: FontId::proportional(w * 0.42),
            color: colour,
            italics: true,
            ..Default::default()
        },
    );
    let galley = painter.layout_job(job);
    let at = Pos2::new(rect.center().x - galley.size().x / 2.0, rect.top());
    // Twice, a hair apart: the bundled font has no bold, and the logo is
    // nothing if not heavy.
    painter.galley(at, galley.clone(), colour);
    painter.galley(at + Vec2::new(w * 0.012, 0.0), galley, colour);

    // The disc under the letters, with VIDEO knocked out of it.
    let disc = Rect::from_center_size(
        Pos2::new(rect.center().x, rect.bottom() - rect.height() * 0.17),
        Vec2::new(w * 0.92, rect.height() * 0.30),
    );
    let points: Vec<Pos2> = (0..40)
        .map(|i| {
            let a = i as f32 / 40.0 * std::f32::consts::TAU;
            disc.center() + Vec2::new(a.cos() * disc.width() / 2.0, a.sin() * disc.height() / 2.0)
        })
        .collect();
    painter.add(Shape::convex_polygon(points, colour, Stroke::NONE));
    painter.text(
        disc.center(),
        Align2::CENTER_CENTER,
        "VIDEO",
        FontId::proportional(disc.height() * 0.62),
        Color32::BLACK,
    );
}

fn activity_id() -> Id {
    Id::new("nit-screensaver-activity")
}

/// Records a key or a mouse movement in the viewport being drawn, if there was
/// one this frame.
///
/// Called from every window the app opens, not only the main one: typing in
/// Settings for ten minutes is not ten minutes idle, and the saver would start
/// over the terminal behind it.
pub fn note_activity(ctx: &Context) {
    use egui::Event;
    let active = ctx.input(|i| {
        i.events.iter().any(|e| {
            matches!(
                e,
                Event::Key { .. }
                    | Event::Text(_)
                    | Event::PointerMoved(_)
                    | Event::PointerButton { .. }
                    | Event::MouseWheel { .. }
                    | Event::Zoom(_)
                    | Event::Paste(_)
                    | Event::Copy
                    | Event::Cut
                    | Event::Touch { .. }
            )
        })
    });
    ctx.data_mut(|d| {
        // The first frame counts as activity, or the clock would have nothing
        // to count from.
        if active || d.get_temp::<Instant>(activity_id()).is_none() {
            d.insert_temp(activity_id(), Instant::now());
        }
    });
}

/// When the last key or mouse movement was, in any of the app's windows.
pub fn last_activity(ctx: &Context) -> Instant {
    ctx.data(|d| d.get_temp::<Instant>(activity_id()))
        .unwrap_or_else(Instant::now)
}

/// A screen saver covering the main window.
pub struct Running {
    pub view: SaverView,
    pub started: Instant,
    pub speed: f32,
}

/// Input inside this long after the saver starts does not stop it. The click
/// on Preview, and the hand coming off the mouse after it, would otherwise
/// end the preview the moment it began.
const GRACE: Duration = Duration::from_millis(600);

impl Running {
    pub fn new(kind: Kind, speed: f32) -> Self {
        Running {
            view: SaverView::new(kind),
            started: Instant::now(),
            speed,
        }
    }

    /// Whether anything has happened since it started that should end it.
    pub fn woken(&self, ctx: &Context) -> bool {
        last_activity(ctx) > self.started + GRACE
    }

    /// Paints it over everything in the window.
    ///
    /// In an area of its own above every other layer, and one that senses
    /// clicks: the click that wakes it has to land on the saver, not on
    /// whatever in the terminal happened to be under the pointer.
    pub fn show(&mut self, ctx: &Context) {
        let screen = ctx.screen_rect();
        egui::Area::new(Id::new("nit-screensaver"))
            .order(egui::Order::Debug)
            .fixed_pos(screen.min)
            .interactable(true)
            .show(ctx, |ui| {
                let (rect, _) =
                    ui.allocate_exact_size(screen.size(), egui::Sense::click_and_drag());
                self.view.paint(ui.painter(), rect, self.speed);
            });
        ctx.set_cursor_icon(egui::CursorIcon::None);
    }
}
