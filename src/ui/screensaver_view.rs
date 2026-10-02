//! Draws the screen saver, over the whole window or inside the little monitor
//! in its settings dialog, and keeps the clock it starts by.
//!
//! The motion is [`crate::features::screensaver`]'s and the logos are
//! [`crate::ui::screensaver_logos`]'; this turns a scene into shapes, and
//! every size here is a fraction of the rect it is given, so the preview is
//! the real thing shrunk rather than a picture of it.

use std::time::{Duration, Instant};

use egui::{Align2, Color32, Context, FontId, Id, Painter, Pos2, Rect, Vec2};

use crate::features::screensaver::{Config, DvdContent, Kind, LogoContent, Saver, Scene};
use crate::ui::screensaver_image::{self, Picture};
use crate::ui::screensaver_logos as logos;
use crate::ui::shading::darken;

/// How often a scene that is moving asks to be drawn.
const MOVING: Duration = Duration::from_millis(33);

/// A saver and the time it was last stepped, which is what its `dt` comes
/// from.
pub struct SaverView {
    pub saver: Saver,
    last: Option<Instant>,
    /// When it started, which is the clock a GIF's frames are counted by.
    born: Instant,
    /// Why the custom image is not what is showing, if it is not.
    image_problem: Option<String>,
}

/// What the floating logo is drawing this frame, the image resolved.
enum Floating {
    Xp,
    Pirated,
    Text(String, Color32),
    Image(std::sync::Arc<Picture>),
}

/// What the floating logo draws, given the settings and, for an image, how
/// loading it went: anything that cannot be shown is the XP logo.
fn floating_content(config: &Config, image: Option<Result<(), &str>>) -> LogoContent {
    match (config.logo_shown(), image) {
        (LogoContent::CustomImage, Some(Ok(()))) => LogoContent::CustomImage,
        (LogoContent::CustomImage, _) => LogoContent::WindowsXp,
        (other, _) => other,
    }
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
            born: Instant::now(),
            image_problem: None,
        }
    }

    pub fn kind(&self) -> Kind {
        self.saver.kind
    }

    /// Why the custom image chosen is not being shown, as of the last paint.
    pub fn image_problem(&self) -> Option<&str> {
        self.image_problem.as_deref()
    }

    /// Steps the scene to now and paints it filling `rect`; returns how soon
    /// it wants painting again.
    ///
    /// Not always "next frame": the floating logo sits still between its
    /// fades, and a GIF has its own pace, so an idle saver costs what it
    /// shows rather than a steady thirty frames a second.
    pub fn paint(&mut self, painter: &Painter, rect: Rect, config: &Config) -> Duration {
        let now = Instant::now();
        let dt = self
            .last
            .map_or(0.0, |last| now.duration_since(last).as_secs_f32());
        self.last = Some(now);
        let speed = config.speed;

        let background = match self.saver.kind {
            Kind::Matrix => srgb(config.matrix_background),
            _ => Color32::BLACK,
        };
        painter.rect_filled(rect, 0.0, background);
        let painter = painter.with_clip_rect(rect);
        match self.saver.kind {
            Kind::None => Duration::MAX,
            Kind::FloatingLogo => self.paint_floating(&painter, rect, config, dt),
            Kind::Matrix => {
                let cell = matrix_cell(rect);
                let grid = (
                    (rect.width() / cell.x).ceil() as usize,
                    (rect.height() / cell.y).ceil() as usize,
                );
                self.saver.step(dt, speed, (0.0, 0.0), grid);
                if let Some(Scene::Matrix(rain)) = &self.saver.scene {
                    let colours = (srgb(config.matrix_rain), srgb(config.matrix_head));
                    paint_matrix(&painter, rect, cell, rain, colours);
                }
                MOVING
            }
            Kind::Dvd => {
                let width = (rect.width() * 0.17).clamp(36.0, 240.0);
                let text = match config.dvd_shown() {
                    DvdContent::DvdVideo => None,
                    DvdContent::CustomText => {
                        Some(logos::custom_text(&painter, &config.dvd_text, width * 0.30))
                    }
                };
                let logo = match &text {
                    None => logos::dvd_size(width),
                    Some((galley, _)) => galley.size(),
                };
                self.saver.step(dt, speed, room(rect, logo), (0, 0));
                if let Some(Scene::Dvd(dvd)) = &self.saver.scene {
                    let at = rect.min + Vec2::new(dvd.pos.0, dvd.pos.1);
                    let colour = DVD_HUES[dvd.hue % DVD_HUES.len()];
                    match text {
                        None => logos::paint_dvd(&painter, Rect::from_min_size(at, logo), colour),
                        Some((galley, heavy)) => {
                            logos::paint_custom_text(&painter, at, galley, heavy, colour)
                        }
                    }
                }
                MOVING
            }
        }
    }

    fn paint_floating(
        &mut self,
        painter: &Painter,
        rect: Rect,
        config: &Config,
        dt: f32,
    ) -> Duration {
        let scale = config.logo_scale.clamp(0.25, 4.0);
        let h = (rect.height() * 0.16).clamp(18.0, 120.0) * scale;

        let picture = (config.logo_shown() == LogoContent::CustomImage)
            .then(|| screensaver_image::picture(painter.ctx(), &config.logo_image));
        self.image_problem = match &picture {
            Some(Err(why)) => Some(why.clone()),
            _ => None,
        };
        let loaded = picture
            .as_ref()
            .map(|p| p.as_ref().map(|_| ()).map_err(String::as_str));
        let content = match floating_content(config, loaded) {
            LogoContent::WindowsXp => Floating::Xp,
            LogoContent::WindowsXpPirated => Floating::Pirated,
            LogoContent::CustomText => {
                let [r, g, b] = config.logo_colour;
                Floating::Text(config.logo_text.clone(), Color32::from_rgb(r, g, b))
            }
            LogoContent::CustomImage => match picture {
                Some(Ok(picture)) => Floating::Image(picture),
                _ => Floating::Xp,
            },
        };

        let text = match &content {
            Floating::Text(text, _) => Some(logos::custom_text(painter, text, h * 0.6)),
            _ => None,
        };
        let logo = match &content {
            Floating::Xp => logos::windows_xp_size(painter, h),
            Floating::Pirated => logos::pirated_size(painter, h * 1.4),
            Floating::Text(..) => text.as_ref().map_or(Vec2::ZERO, |(g, _)| g.size()),
            Floating::Image(picture) => {
                // As tall as the XP logo would be, or less if it is a wide
                // one that would not otherwise fit the window.
                let tall = h * 1.6;
                let size = picture.size * (tall / picture.size.y.max(1.0));
                size * (rect.width() * 0.8 / size.x.max(1.0)).min(1.0)
            }
        };
        let space = room(rect, logo);
        self.saver.step(dt, config.speed, space, (0, 0));
        let Some(Scene::FloatingLogo(scene)) = &self.saver.scene else {
            return Duration::MAX;
        };
        let at = rect.min + Vec2::new(scene.at.0 * space.0, scene.at.1 * space.1);
        let alpha = scene.alpha();
        let place = Rect::from_min_size(at, logo);
        let mut next = scene.held_for().map_or(MOVING, |secs| {
            Duration::from_secs_f32(secs / config.speed.clamp(0.1, 5.0))
        });
        match content {
            Floating::Xp => logos::paint_windows_xp(painter, place, alpha),
            Floating::Pirated => logos::paint_pirated(painter, place, alpha),
            Floating::Text(_, colour) => {
                if let Some((galley, heavy)) = text {
                    logos::paint_custom_text(
                        painter,
                        at,
                        galley,
                        heavy,
                        colour.gamma_multiply(alpha),
                    );
                }
            }
            Floating::Image(picture) => {
                let (texture, change) = picture.frame_at(self.born.elapsed());
                painter.image(
                    texture.id(),
                    place,
                    Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                    Color32::WHITE.gamma_multiply(alpha),
                );
                if let Some(change) = change {
                    next = next.min(change);
                }
            }
        }
        next
    }
}

fn srgb([r, g, b]: [u8; 3]) -> Color32 {
    Color32::from_rgb(r, g, b)
}

/// How far a logo of `logo` can travel inside `rect`.
fn room(rect: Rect, logo: Vec2) -> (f32, f32) {
    (
        (rect.width() - logo.x).max(1.0),
        (rect.height() - logo.y).max(1.0),
    )
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
    (trail, head_colour): (Color32, Color32),
) {
    // A trail fades from the chosen colour at the head to a little over a
    // third of it at the tail, then out: the film's green, for any colour.
    let dim = darken(trail, 90.0 / 255.0);
    let font = FontId::monospace(cell.y / 1.1);
    for (x, column) in rain.columns.iter().enumerate() {
        let head = column.head.floor() as i64;
        let tail = head - column.trail as i64;
        for row in tail.max(0)..=head.min(rain.rows as i64 - 1) {
            let Some(&glyph) = column.glyphs.get(row as usize) else {
                continue;
            };
            let colour = if row == head {
                head_colour
            } else {
                let fade = 1.0 - (head - row) as f32 / column.trail as f32;
                lerp_colour(dim, trail, fade).gamma_multiply(fade.max(0.15))
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

fn lerp_colour(a: Color32, b: Color32, t: f32) -> Color32 {
    let m = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;
    Color32::from_rgb(m(a.r(), b.r()), m(a.g(), b.g()), m(a.b(), b.b()))
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
    /// The settings it was started with: a preview runs a draft that is not
    /// the setting yet.
    pub config: Config,
}

/// Input inside this long after the saver starts does not stop it. The click
/// on Preview, and the hand coming off the mouse after it, would otherwise
/// end the preview the moment it began.
const GRACE: Duration = Duration::from_millis(600);

impl Running {
    pub fn new(config: Config) -> Self {
        Running {
            view: SaverView::new(config.kind),
            started: Instant::now(),
            config,
        }
    }

    /// Whether anything has happened since it started that should end it.
    pub fn woken(&self, ctx: &Context) -> bool {
        last_activity(ctx) > self.started + GRACE
    }

    /// Paints it over everything in the window, and returns how soon it
    /// wants painting again.
    ///
    /// In an area of its own above every other layer, and one that senses
    /// clicks: the click that wakes it has to land on the saver, not on
    /// whatever in the terminal happened to be under the pointer.
    pub fn show(&mut self, ctx: &Context) -> Duration {
        let screen = ctx.screen_rect();
        let mut next = MOVING;
        egui::Area::new(Id::new("nit-screensaver"))
            .order(egui::Order::Debug)
            .fixed_pos(screen.min)
            .interactable(true)
            .show(ctx, |ui| {
                let (rect, _) =
                    ui.allocate_exact_size(screen.size(), egui::Sense::click_and_drag());
                next = self.view.paint(ui.painter(), rect, &self.config);
            });
        ctx.set_cursor_icon(egui::CursorIcon::None);
        next
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wanting(logo: LogoContent) -> Config {
        Config {
            kind: Kind::FloatingLogo,
            logo,
            logo_text: "IRIS".into(),
            logo_image: "picture.gif".into(),
            ..Config::default()
        }
    }

    #[test]
    fn an_image_that_will_not_load_floats_the_xp_logo_instead() {
        let config = wanting(LogoContent::CustomImage);
        assert_eq!(
            floating_content(&config, Some(Err("not a GIF"))),
            LogoContent::WindowsXp
        );
        assert_eq!(floating_content(&config, None), LogoContent::WindowsXp);
        assert_eq!(
            floating_content(&config, Some(Ok(()))),
            LogoContent::CustomImage
        );
        // What loads has no say over content that is not an image.
        assert_eq!(
            floating_content(&wanting(LogoContent::WindowsXpPirated), None),
            LogoContent::WindowsXpPirated
        );
    }

    /// End to end, through the cache: a file that does not exist leaves the
    /// view saying why, and painting the saver over it does not fail.
    #[test]
    fn a_missing_image_file_is_reported_and_the_saver_still_draws() {
        let ctx = Context::default();
        let gone = std::env::temp_dir().join("nit-screensaver-test-missing.gif");
        let config = Config {
            logo_image: gone.display().to_string(),
            ..wanting(LogoContent::CustomImage)
        };
        let mut view = SaverView::new(Kind::FloatingLogo);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 200.0));
                view.paint(ui.painter(), rect, &config);
            });
        });
        assert!(view.image_problem().is_some());
    }
}
