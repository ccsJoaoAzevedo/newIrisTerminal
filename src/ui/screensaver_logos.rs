//! The logos the screen savers carry, drawn as shapes.
//!
//! Shapes rather than pictures: the saver draws them anywhere from the size of
//! a thumbnail in its settings dialog to a third of a 4K screen, and a bitmap
//! is only sharp at one of those. Every length is a fraction of the height it
//! is given, for the same reason.
//!
//! egui fills a shape with one colour, so the shading - the gloss on the XP
//! flag, the bone of the skull, the steel of the blades - is per-vertex colour
//! on meshes, the way [`crate::ui::shading`] does it.

use std::f32::consts::{PI, TAU};
use std::sync::Arc;

use egui::text::{LayoutJob, TextFormat};
use egui::{
    Align2, Color32, FontFamily, FontId, Galley, Mesh, Painter, Pos2, Rect, Shape, Stroke, Vec2,
};

use crate::ui::fonts::{display_family, Display};
use crate::ui::shading::{darken, lighten, radial};

fn family(painter: &Painter, display: Display) -> FontFamily {
    display_family(painter.ctx(), display).unwrap_or(FontFamily::Proportional)
}

fn faded(color: Color32, alpha: f32) -> Color32 {
    color.gamma_multiply(alpha.clamp(0.0, 1.0))
}

/// Text in the heavy face; `real_heavy` is whether there was one. Without it
/// the bundled light sans is drawn twice a hair apart, which is the nearest
/// it comes to bold.
fn heavy_text(painter: &Painter, at: Pos2, galley: Arc<Galley>, colour: Color32, real_heavy: bool) {
    if !real_heavy {
        let nudge = galley.size().y * 0.025;
        painter.galley_with_override_text_color(at + Vec2::new(nudge, 0.0), galley.clone(), colour);
    }
    painter.galley_with_override_text_color(at, galley, colour);
}

// ---------------------------------------------------------------------------
// Windows XP

/// The XP flag's panes, in the order they are laid out: top-left, top-right,
/// bottom-left, bottom-right.
const XP_PANES: [Color32; 4] = [
    Color32::from_rgb(240, 84, 34),
    Color32::from_rgb(118, 186, 33),
    Color32::from_rgb(28, 144, 226),
    Color32::from_rgb(255, 189, 20),
];

/// The red-orange of the superscript "xp".
const XP_RED: Color32 = Color32::from_rgb(232, 76, 30);

/// The wordmark, laid out at a flag height of `h`, without positions yet.
struct Wordmark {
    microsoft: Arc<Galley>,
    windows: Arc<Galley>,
    xp: Arc<Galley>,
    /// Whether "Windows" came out in a real heavy face.
    heavy: bool,
}

impl Wordmark {
    fn new(painter: &Painter, h: f32) -> Self {
        let heavy_family = family(painter, Display::Heavy);
        let heavy = heavy_family != FontFamily::Proportional;
        let italic = family(painter, Display::HeavyItalic);
        let lay = |text: &str, size: f32, family: FontFamily| {
            painter.layout_no_wrap(text.to_owned(), FontId::new(size, family), Color32::WHITE)
        };
        Wordmark {
            microsoft: lay("Microsoft\u{00ae}", h * 0.15, heavy_family.clone()),
            windows: lay("Windows", h * 0.50, heavy_family),
            xp: if italic == FontFamily::Proportional {
                // Without a real italic, egui's own slant is the nearest thing.
                let mut job = LayoutJob::default();
                job.append(
                    "xp",
                    0.0,
                    TextFormat {
                        font_id: FontId::proportional(h * 0.42),
                        color: Color32::WHITE,
                        italics: true,
                        ..Default::default()
                    },
                );
                painter.layout_job(job)
            } else {
                lay("xp", h * 0.44, italic)
            },
            heavy,
        }
    }

    /// Width taken beside the flag, past `gap`.
    fn width(&self, h: f32) -> f32 {
        (self.windows.size().x + h * 0.02 + self.xp.size().x).max(self.microsoft.size().x)
    }

    /// Paints "Microsoft®" over "Windows" with "xp" up at its shoulder, the
    /// top of "Windows" at `top`; returns the rect "Windows" took.
    fn paint(&self, painter: &Painter, left: f32, top: f32, h: f32, alpha: f32) -> Rect {
        let white = faded(Color32::WHITE, alpha);
        // Galleys carry their line gap above the caps; pulling them up by a
        // share of their own height sets the letters where the flag wants them.
        let windows_at = Pos2::new(left, top - self.windows.size().y * 0.12);
        painter.galley_with_override_text_color(
            Pos2::new(
                left + h * 0.02,
                windows_at.y - self.microsoft.size().y * 0.62,
            ),
            self.microsoft.clone(),
            faded(Color32::from_gray(225), alpha),
        );
        heavy_text(painter, windows_at, self.windows.clone(), white, self.heavy);
        let windows = Rect::from_min_size(windows_at, self.windows.size());
        painter.galley_with_override_text_color(
            Pos2::new(
                windows.right() + h * 0.02,
                windows.top() - self.xp.size().y * 0.18,
            ),
            self.xp.clone(),
            faded(XP_RED, alpha),
        );
        windows
    }
}

/// The XP logo's size for a flag `h` tall.
pub fn windows_xp_size(painter: &Painter, h: f32) -> Vec2 {
    Vec2::new(h * 1.12 + h * 0.16 + Wordmark::new(painter, h).width(h), h)
}

/// The four-pane flag, waving, and the wordmark beside it.
pub fn paint_windows_xp(painter: &Painter, rect: Rect, alpha: f32) {
    let h = rect.height();
    let flag = Rect::from_min_size(rect.min, Vec2::new(h * 1.12, h));
    paint_flag(painter, flag, alpha);
    let wordmark = Wordmark::new(painter, h);
    wordmark.paint(
        painter,
        flag.right() + h * 0.16,
        rect.top() + h * 0.30,
        h,
        alpha,
    );
}

/// Where a point of the flag, `u` across and `v` down (both 0 to 1), lands.
///
/// One mapping for the whole cloth, which is what makes four panes read as a
/// single flag in a breeze: the top and bottom edges ride the same wave, the
/// far side is a little shorter (it is further away), and the near edge bows.
fn flag_point(flag: Rect, u: f32, v: f32) -> Pos2 {
    let w = flag.width();
    let h = flag.height();
    let wave = -(PI * (0.1 + 0.95 * u)).sin() * h * 0.085 + (PI * 2.0 * u).sin() * h * 0.02;
    let taper = 1.0 - 0.12 * u;
    let bow = -(PI * v).sin() * w * 0.035 * (1.0 - u);
    let lean = (v - 0.5) * w * 0.06;
    Pos2::new(
        flag.left() + w * (0.045 + 0.92 * u) + bow + lean,
        flag.top() + h * 0.5 + (v - 0.5) * h * 0.86 * taper + wave + h * 0.02,
    )
}

/// The shading across one pane, `u` and `v` within it.
///
/// A plastic gloss: lit from the top left, a soft bevel round the rim - light
/// on the top and near edges, shadowed on the far ones - and a specular bloom
/// high on the pane.
fn pane_shade(base: Color32, u: f32, v: f32) -> Color32 {
    let lit = 1.0 - (u * 0.35 + v * 0.65);
    let mut c = if lit > 0.5 {
        lighten(base, (lit - 0.5) * 0.55)
    } else {
        darken(base, 1.0 - (0.5 - lit) * 0.35)
    };
    const RIM: f32 = 0.09;
    if v < RIM || u < RIM {
        let t = 1.0 - u.min(v) / RIM;
        c = lighten(c, 0.35 * t);
    }
    if v > 1.0 - RIM || u > 1.0 - RIM {
        let t = (u.max(v) - (1.0 - RIM)) / RIM;
        c = darken(c, 1.0 - 0.30 * t);
    }
    let d = (u - 0.32).powi(2) + (v - 0.26).powi(2) * 1.6;
    lighten(c, 0.38 * (-d / 0.035).exp())
}

/// Samples across a pane: close together at the rim, where the bevel turns
/// quickly, and wide in the middle, where nothing does.
const PANE_STEPS: [f32; 13] = [
    0.0, 0.025, 0.055, 0.09, 0.15, 0.25, 0.4, 0.55, 0.7, 0.82, 0.9, 0.955, 1.0,
];

fn paint_flag(painter: &Painter, flag: Rect, alpha: f32) {
    const GAP: f32 = 0.045;
    let spans = [(0.0, 0.5 - GAP / 2.0), (0.5 + GAP / 2.0, 1.0)];
    for (i, base) in XP_PANES.into_iter().enumerate() {
        let (u0, u1) = spans[i % 2];
        let (v0, v1) = spans[i / 2];
        let at = |pu: f32, pv: f32| flag_point(flag, u0 + (u1 - u0) * pu, v0 + (v1 - v0) * pv);

        let n = PANE_STEPS.len() as u32;
        let mut mesh = Mesh::default();
        for &pv in &PANE_STEPS {
            for &pu in &PANE_STEPS {
                mesh.colored_vertex(at(pu, pv), faded(pane_shade(base, pu, pv), alpha));
            }
        }
        for row in 0..n - 1 {
            for col in 0..n - 1 {
                let a = row * n + col;
                mesh.add_triangle(a, a + 1, a + n);
                mesh.add_triangle(a + 1, a + n + 1, a + n);
            }
        }
        painter.add(Shape::mesh(mesh));

        // A mesh is not anti-aliased; a hairline round it in the rim's own
        // colour is, and takes the stair-steps off the curved edges.
        let mut rim: Vec<Pos2> = Vec::new();
        rim.extend(PANE_STEPS.iter().map(|&p| at(p, 0.0)));
        rim.extend(PANE_STEPS.iter().map(|&p| at(1.0, p)));
        rim.extend(PANE_STEPS.iter().rev().map(|&p| at(p, 1.0)));
        rim.extend(PANE_STEPS.iter().rev().map(|&p| at(0.0, p)));
        painter.add(Shape::closed_line(
            rim,
            Stroke::new(
                (flag.height() * 0.008).max(0.6),
                faded(darken(base, 0.85), alpha),
            ),
        ));
    }
}

// ---------------------------------------------------------------------------
// Windows XP Pirated Edition

const GOLD_DARK: Color32 = Color32::from_rgb(150, 104, 22);
const GOLD: Color32 = Color32::from_rgb(214, 168, 62);
const GOLD_LIGHT: Color32 = Color32::from_rgb(255, 226, 140);

fn pirated_lettering(painter: &Painter, h: f32) -> Arc<Galley> {
    painter.layout_no_wrap(
        "Pirated Edition".to_owned(),
        FontId::new(h * 0.19, family(painter, Display::Serif)),
        Color32::WHITE,
    )
}

/// The pirated logo's size for an emblem `h` tall.
pub fn pirated_size(painter: &Painter, h: f32) -> Vec2 {
    let wordmark = Wordmark::new(painter, h * 0.78).width(h * 0.78);
    let edition = pirated_lettering(painter, h).size().x;
    Vec2::new(h * 1.05 + h * 0.10 + wordmark.max(edition), h)
}

/// The skull and swords, and the XP wordmark with its gold "Pirated Edition".
pub fn paint_pirated(painter: &Painter, rect: Rect, alpha: f32) {
    let h = rect.height();
    let emblem = Rect::from_min_size(rect.min, Vec2::new(h * 1.05, h));
    let skull = Rect::from_center_size(emblem.center(), Vec2::splat(h));

    paint_cutlass(painter, skull, false, alpha);
    paint_cutlass(painter, skull, true, alpha);
    paint_skull(painter, skull, alpha);

    let wh = h * 0.78;
    let left = emblem.right() + h * 0.10;
    let wordmark = Wordmark::new(painter, wh);
    let windows = wordmark.paint(painter, left, rect.top() + h * 0.30, wh, alpha);

    // Gold in two tones: the whole of it in the deep shade, then the top half
    // again in the bright one, clipped - the band of light across a gilded
    // letter that a flat fill does not have.
    let edition = pirated_lettering(painter, h);
    let at = Pos2::new(left + h * 0.03, windows.bottom() - h * 0.04);
    let size = edition.size();
    painter.galley_with_override_text_color(
        at + Vec2::splat(h * 0.012),
        edition.clone(),
        faded(Color32::from_rgb(60, 36, 4), alpha),
    );
    painter.galley_with_override_text_color(at, edition.clone(), faded(GOLD_DARK, alpha));
    let upper = Rect::from_min_size(at, Vec2::new(size.x, size.y * 0.58));
    painter
        .with_clip_rect(upper)
        .galley_with_override_text_color(at, edition.clone(), faded(GOLD, alpha));
    let shine = Rect::from_min_size(
        at + Vec2::new(0.0, size.y * 0.28),
        Vec2::new(size.x, size.y * 0.14),
    );
    painter
        .with_clip_rect(shine)
        .galley_with_override_text_color(at, edition, faded(GOLD_LIGHT, alpha));

    // The glint off the tip of the right-hand blade.
    let tip = cutlass_point(skull, true, 1.0);
    lens_flare(painter, tip, h * 0.16, alpha);
}

/// A point along a cutlass's centre line, `t` from the pommel (0) to the tip
/// (1). `mirrored` is the one whose tip is top-left; the other, top-right.
fn cutlass_point(s: Rect, mirrored: bool, t: f32) -> Pos2 {
    let (x0, y0, x1, y1) = (0.16, 0.92, 0.90, 0.08);
    let base = Vec2::new(x0 + (x1 - x0) * t, y0 + (y1 - y0) * t);
    // The cutlass's curve: bowed away from the crossing, most near the tip.
    let normal = Vec2::new(0.74, 0.84).normalized();
    let bow = (PI * t).sin() * 0.07 * t.max(0.3);
    let mut p = base + normal * bow;
    if mirrored {
        p.x = 1.0 - p.x;
    }
    s.min + Vec2::new(p.x * s.width(), p.y * s.height())
}

fn paint_cutlass(painter: &Painter, s: Rect, mirrored: bool, alpha: f32) {
    let h = s.height();
    let at = |t: f32| cutlass_point(s, mirrored, t);
    let across = |t: f32| {
        let d = at((t + 0.01).min(1.0)) - at((t - 0.01).max(0.0));
        Vec2::new(-d.y, d.x).normalized()
    };

    // The blade, from the guard to the point: widening as a cutlass does, then
    // swept back to the tip. Steel shades from the bright spine to the edge.
    const GUARD: f32 = 0.17;
    const STEPS: usize = 24;
    let mut mesh = Mesh::default();
    for i in 0..=STEPS {
        let t = GUARD + (1.0 - GUARD) * i as f32 / STEPS as f32;
        let width = h * 0.04 * (0.7 + 0.7 * t) * if t > 0.86 { (1.0 - t) / 0.14 } else { 1.0 };
        let n = across(t);
        let spine = if mirrored { -n } else { n };
        mesh.colored_vertex(
            at(t) + spine * width * 0.8,
            faded(Color32::from_rgb(236, 238, 246), alpha),
        );
        mesh.colored_vertex(
            at(t) - spine * width * 1.2,
            faded(Color32::from_rgb(120, 126, 142), alpha),
        );
    }
    for i in 0..STEPS as u32 {
        let a = i * 2;
        mesh.add_triangle(a, a + 1, a + 2);
        mesh.add_triangle(a + 2, a + 1, a + 3);
    }
    painter.add(Shape::mesh(mesh));
    let fuller: Vec<Pos2> = (0..=12)
        .map(|i| at(GUARD + 0.6 * i as f32 / 12.0))
        .collect();
    painter.add(Shape::line(
        fuller,
        Stroke::new(h * 0.006, faded(Color32::from_white_alpha(140), alpha)),
    ));

    // Grip, guard and pommel, in leather and brass.
    let grip: Vec<Pos2> = (0..=6).map(|i| at(0.02 + 0.14 * i as f32 / 6.0)).collect();
    painter.add(Shape::line(
        grip,
        Stroke::new(h * 0.045, faded(Color32::from_rgb(84, 46, 22), alpha)),
    ));
    for i in 0..4 {
        let t = 0.04 + i as f32 * 0.035;
        let n = across(t) * h * 0.022;
        painter.line_segment(
            [at(t) - n, at(t) + n],
            Stroke::new(h * 0.008, faded(Color32::from_rgb(50, 26, 10), alpha)),
        );
    }
    let n = across(GUARD) * h * 0.075;
    painter.line_segment(
        [at(GUARD) - n, at(GUARD) + n],
        Stroke::new(h * 0.026, faded(GOLD, alpha)),
    );
    // The knuckle bow, from the guard round the outside of the grip to the
    // pommel.
    let outside = if mirrored { 1.0 } else { -1.0 };
    let bow: Vec<Pos2> = (0..=10)
        .map(|i| {
            let t = GUARD - GUARD * 0.95 * i as f32 / 10.0;
            at(t)
                + across(t)
                    * outside
                    * h
                    * (0.07 * (PI * i as f32 / 10.0).sin() + 0.075 * (1.0 - i as f32 / 10.0))
        })
        .collect();
    painter.add(Shape::line(
        bow,
        Stroke::new(h * 0.014, faded(GOLD_DARK, alpha)),
    ));
    radial(
        painter,
        at(0.0),
        h * 0.032,
        Vec2::splat(-h * 0.01),
        faded(GOLD_LIGHT, alpha),
        faded(GOLD_DARK, alpha),
    );
}

const BONE: Color32 = Color32::from_rgb(240, 234, 214);
const BONE_SHADE: Color32 = Color32::from_rgb(176, 164, 132);
const SOCKET: Color32 = Color32::from_rgb(22, 12, 10);

fn ellipse(center: Pos2, radii: Vec2, from: f32, to: f32, steps: usize) -> Vec<Pos2> {
    (0..=steps)
        .map(|i| {
            let a = from + (to - from) * i as f32 / steps as f32;
            center + Vec2::new(a.cos() * radii.x, a.sin() * radii.y)
        })
        .collect()
}

fn paint_skull(painter: &Painter, s: Rect, alpha: f32) {
    let h = s.height();
    let p = |x: f32, y: f32| s.min + Vec2::new(x * s.width(), y * h);
    let f = |c: Color32| faded(c, alpha);

    // The jaw first, so the cranium covers the top of it.
    let jaw = Rect::from_min_max(p(0.37, 0.56), p(0.63, 0.82));
    painter.rect_filled(jaw, h * 0.06, f(darken(BONE, 0.9)));
    let cheeks = Rect::from_min_max(p(0.32, 0.50), p(0.68, 0.70));
    painter.rect_filled(cheeks, h * 0.08, f(BONE));
    radial(
        painter,
        p(0.5, 0.42),
        h * 0.25,
        Vec2::new(-h * 0.07, -h * 0.08),
        f(Color32::WHITE),
        f(BONE_SHADE),
    );
    // The cranium's fill ends in a hard edge against black; a soft ring the
    // colour of its rim takes it off.
    painter.circle_stroke(
        p(0.5, 0.42),
        h * 0.25,
        Stroke::new(h * 0.006, f(BONE_SHADE)),
    );

    for side in [-1.0, 1.0] {
        let eye = p(0.5 + side * 0.098, 0.505);
        painter.add(Shape::convex_polygon(
            ellipse(eye, Vec2::new(h * 0.07, h * 0.06), 0.0, TAU, 28),
            f(SOCKET),
            Stroke::new(h * 0.008, f(BONE_SHADE)),
        ));
        // A glint deep in the socket, which is what makes a skull look back.
        painter.circle_filled(
            eye + Vec2::new(side * h * 0.012, -h * 0.01),
            h * 0.009,
            f(Color32::from_rgb(200, 40, 30)),
        );
    }
    painter.add(Shape::convex_polygon(
        vec![
            p(0.5, 0.565),
            p(0.525, 0.625),
            p(0.5, 0.635),
            p(0.475, 0.625),
        ],
        f(SOCKET),
        Stroke::NONE,
    ));

    // The teeth: a dark bite line, and gaps between them.
    let bite = h * 0.71;
    painter.line_segment(
        [
            Pos2::new(jaw.left() + h * 0.02, s.min.y + bite),
            Pos2::new(jaw.right() - h * 0.02, s.min.y + bite),
        ],
        Stroke::new(h * 0.012, f(SOCKET)),
    );
    for i in 1..6 {
        let x = jaw.left() + jaw.width() * i as f32 / 6.0;
        painter.line_segment(
            [
                Pos2::new(x, s.min.y + h * 0.665),
                Pos2::new(x, s.min.y + h * 0.765),
            ],
            Stroke::new(h * 0.007, f(BONE_SHADE)),
        );
    }

    paint_bandana(painter, s, alpha);
}

const BANDANA: Color32 = Color32::from_rgb(196, 18, 26);

fn paint_bandana(painter: &Painter, s: Rect, alpha: f32) {
    let h = s.height();
    let p = |x: f32, y: f32| s.min + Vec2::new(x * s.width(), y * h);
    let f = |c: Color32| faded(c, alpha);
    let center = p(0.5, 0.42);

    // The cap: the top of the cranium, cut off by a brow line just above the
    // eyes.
    let cap = ellipse(center, Vec2::splat(h * 0.262), PI + 0.42, TAU - 0.42, 30);
    painter.add(Shape::convex_polygon(cap.clone(), f(BANDANA), Stroke::NONE));
    let fold = ellipse(
        center + Vec2::new(-h * 0.03, -h * 0.03),
        Vec2::splat(h * 0.18),
        PI + 0.7,
        TAU - 1.2,
        20,
    );
    painter.add(Shape::convex_polygon(
        fold,
        f(lighten(BANDANA, 0.25)),
        Stroke::NONE,
    ));
    painter.add(Shape::line(
        vec![cap[0], cap[cap.len() - 1]],
        Stroke::new(h * 0.03, f(darken(BANDANA, 0.65))),
    ));
    // The white spots of a pirate's kerchief.
    for (x, y) in [
        (0.40, 0.27),
        (0.52, 0.22),
        (0.62, 0.29),
        (0.47, 0.33),
        (0.35, 0.34),
        (0.57, 0.35),
    ] {
        painter.circle_filled(p(x, y), h * 0.014, f(Color32::from_rgb(250, 240, 236)));
    }

    // The knot at the side, and its two ends blowing out behind.
    let knot = p(0.75, 0.37);
    for tail in [
        [knot, p(0.92, 0.47), p(0.86, 0.52)],
        [knot, p(0.84, 0.56), p(0.77, 0.58)],
    ] {
        painter.add(Shape::convex_polygon(
            tail.to_vec(),
            f(darken(BANDANA, 0.85)),
            Stroke::NONE,
        ));
    }
    painter.circle_filled(knot, h * 0.035, f(BANDANA));
    painter.circle_filled(
        knot + Vec2::splat(-h * 0.01),
        h * 0.014,
        f(lighten(BANDANA, 0.3)),
    );
}

/// A camera's star of light: a glow, and four rays fading to nothing.
fn lens_flare(painter: &Painter, at: Pos2, size: f32, alpha: f32) {
    let white = |a: u8| faded(Color32::from_white_alpha(a), alpha);
    radial(
        painter,
        at,
        size * 0.5,
        Vec2::ZERO,
        white(170),
        Color32::TRANSPARENT,
    );
    for (long, short, turn) in [
        (size * 1.4, size * 0.05, 0.0),
        (size * 0.9, size * 0.045, PI / 2.0),
        (size * 0.45, size * 0.03, PI / 4.0),
        (size * 0.45, size * 0.03, -PI / 4.0),
    ] {
        let along = Vec2::angled(turn);
        let side = along.rot90();
        let mut ray = Mesh::default();
        ray.colored_vertex(at, white(255));
        for corner in [
            along * long,
            side * short,
            -along * long,
            -side * short,
            along * long,
        ] {
            ray.colored_vertex(at + corner, Color32::TRANSPARENT);
        }
        for i in 1..5 {
            ray.add_triangle(0, i, i + 1);
        }
        painter.add(Shape::mesh(ray));
    }
    painter.circle_filled(at, size * 0.07, white(255));
    // The little rainbow ring a cheap lens adds.
    painter.circle_stroke(
        at,
        size * 0.28,
        Stroke::new(
            size * 0.02,
            faded(Color32::from_rgba_unmultiplied(140, 200, 255, 60), alpha),
        ),
    );
}

// ---------------------------------------------------------------------------
// DVD Video

/// The DVD logo's size for a logo `w` wide.
pub fn dvd_size(w: f32) -> Vec2 {
    Vec2::new(w, w * 0.52)
}

pub fn paint_dvd(painter: &Painter, rect: Rect, colour: Color32) {
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
    let points = ellipse(disc.center(), disc.size() / 2.0, 0.0, TAU, 40);
    painter.add(Shape::convex_polygon(points, colour, Stroke::NONE));
    painter.text(
        disc.center(),
        Align2::CENTER_CENTER,
        "VIDEO",
        FontId::proportional(disc.height() * 0.62),
        Color32::BLACK,
    );
}

// ---------------------------------------------------------------------------
// Custom text

/// Text laid out to float or bounce: in the heavy face where there is one.
pub fn custom_text(painter: &Painter, text: &str, size: f32) -> (Arc<Galley>, bool) {
    let heavy = family(painter, Display::Heavy);
    let real = heavy != FontFamily::Proportional;
    (
        painter.layout_no_wrap(
            text.trim().to_owned(),
            FontId::new(size, heavy),
            Color32::WHITE,
        ),
        real,
    )
}

pub fn paint_custom_text(
    painter: &Painter,
    at: Pos2,
    galley: Arc<Galley>,
    heavy: bool,
    colour: Color32,
) {
    heavy_text(painter, at, galley, colour, heavy);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four panes share one wave, so the gap between two neighbours is
    /// the same width all along it: the cloth is cut, not torn.
    #[test]
    fn neighbouring_panes_of_the_flag_keep_an_even_gap_along_their_seam() {
        let flag = Rect::from_min_size(Pos2::ZERO, Vec2::new(112.0, 100.0));
        let gaps: Vec<f32> = (0..=10)
            .map(|i| {
                let v = i as f32 / 10.0;
                flag_point(flag, 0.5225, v).x - flag_point(flag, 0.4775, v).x
            })
            .collect();
        let (lo, hi) = gaps
            .iter()
            .fold((f32::MAX, f32::MIN), |(lo, hi), &g| (lo.min(g), hi.max(g)));
        assert!(lo > 0.0, "the panes overlap");
        assert!(hi - lo < 0.5, "the seam pinches: {gaps:?}");
    }

    #[test]
    fn the_flag_stays_inside_the_room_it_was_given() {
        let flag = Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(112.0, 100.0));
        for i in 0..=20 {
            for j in 0..=20 {
                let p = flag_point(flag, i as f32 / 20.0, j as f32 / 20.0);
                assert!(flag.expand(0.5).contains(p), "{p:?} is outside {flag:?}");
            }
        }
    }

    /// Lit from the top left: the corner the light comes from is brighter
    /// than the one it does not reach.
    #[test]
    fn a_pane_is_brighter_at_its_lit_corner_than_its_shadowed_one() {
        let lum = |c: Color32| u32::from(c.r()) + u32::from(c.g()) + u32::from(c.b());
        for base in XP_PANES {
            assert!(lum(pane_shade(base, 0.2, 0.2)) > lum(pane_shade(base, 0.97, 0.97)));
        }
    }
}
