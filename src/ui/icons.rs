//! The marks the app draws for itself.
//!
//! Every icon here is stroked rather than set in text. The glyphs a title bar
//! wants — a gear, a window outline, a multiplication sign — are not in every
//! font egui falls back through, and a missing one renders as a tofu box; egui
//! draws its own window close button the same way, for the same reason.
//!
//! One module rather than a painter per call site, because the whole point of
//! this pass was that the marks were not distinct enough from each other: a
//! plain `+` beside a lowercase `x`, and a bar beside a square beside two
//! squares. Drawing them all in one place is what lets them be *designed*
//! against each other — the close mark is the only diagonal, the tab marks are
//! the only round-cornered ones, and the two window marks read as windows
//! rather than as boxes because both carry a title bar of their own.
//!
//! Sizes are all relative to the box each is handed, so one set of shapes works
//! for a 7-point title-bar control and a 12-point tab button alike.

use egui::{Color32, Painter, Pos2, Rect, Rounding, Stroke, Vec2};

/// A mark, and how big it wants to be inside the hit area it is given.
///
/// The fraction differs per glyph on purpose: a gear needs more room than a
/// bar to still read as a gear, and a diagonal cross looks larger than the
/// square it is drawn beside at the same nominal size.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Glyph {
    /// Minimize: a bar on the baseline of the box.
    Bar,
    /// Maximize: a window outline, with the heavier top edge that makes it a
    /// window rather than a rectangle.
    Window,
    /// Restore: that window, with the one it came from behind it.
    WindowStack,
    /// Close: the only diagonal mark in the set.
    Cross,
    /// Settings: a gear.
    Gear,
    /// New tab: a plus, with arms long enough not to be read as the cross.
    Plus,
    /// Close tab: a smaller cross, for a button that sits inside a tab.
    SmallCross,
    /// Pin to desktop, while it is off: a pushpin drawn in outline.
    Pin,
    /// Pin to desktop, while it is on: the same pushpin with its head filled,
    /// so the state reads without hovering for the tooltip.
    Pinned,
}

impl Glyph {
    /// How much of the hit area's shorter side the mark occupies.
    ///
    /// Every mark was drawn a step larger in this pass: at the sizes below the
    /// old ones the row read as a line of faint specks rather than as
    /// controls, and the gear in particular was small enough that its teeth
    /// were indistinguishable from noise.
    fn scale(self) -> f32 {
        match self {
            // Wider than tall: a minimize bar that is only as wide as a
            // maximize box reads as an underscore.
            Glyph::Bar => 0.50,
            Glyph::Window => 0.44,
            // Room for the offset copy behind it.
            Glyph::WindowStack => 0.40,
            Glyph::Cross => 0.40,
            // The most detailed mark in the set, and the one that needs the
            // most room before its teeth stop being teeth.
            Glyph::Gear => 0.60,
            Glyph::Plus => 0.52,
            // The one mark deliberately left where it was: it sits inside a
            // tab, beside a label, and growing it would make closing a tab
            // look like the thing the tab is for.
            Glyph::SmallCross => 0.40,
            Glyph::Pin | Glyph::Pinned => 0.50,
        }
    }

    /// How thick the mark is drawn, in points.
    ///
    /// Not scaled with the box: these are hairline marks on screen furniture,
    /// and a stroke that grows with the button turns the gear into a blob.
    /// Heavier than they were, though - a 1.0-point outline on a high-DPI
    /// screen is a grey suggestion of a line, and the window marks were the
    /// hardest of the set to see.
    fn width(self) -> f32 {
        match self {
            Glyph::Bar | Glyph::Plus => 1.8,
            Glyph::Cross => 1.7,
            Glyph::SmallCross => 1.3,
            Glyph::Gear => 1.5,
            Glyph::Window | Glyph::WindowStack | Glyph::Pin | Glyph::Pinned => 1.3,
        }
    }
}

/// Draws `glyph` centred in `rect`, in `colour`.
///
/// `behind` is what a mark that overlaps itself is filled with, so the front
/// window of the restore mark hides the one behind it instead of showing
/// through. Every other glyph ignores it.
pub fn draw(painter: &Painter, rect: Rect, glyph: Glyph, colour: Color32, behind: Color32) {
    let side = rect.width().min(rect.height());
    let size = (side * glyph.scale()).round().max(5.0);
    let box_ = Rect::from_center_size(rect.center().round(), Vec2::splat(size));
    let stroke = Stroke::new(glyph.width(), colour);

    match glyph {
        Glyph::Bar => bar(painter, box_, stroke),
        Glyph::Window => window(painter, box_, stroke),
        Glyph::WindowStack => window_stack(painter, box_, stroke, behind),
        Glyph::Cross | Glyph::SmallCross => cross(painter, box_, stroke),
        Glyph::Gear => gear(painter, box_, stroke),
        Glyph::Plus => plus(painter, box_, stroke),
        Glyph::Pin => pin(painter, box_, stroke, false),
        Glyph::Pinned => pin(painter, box_, stroke, true),
    }
}

/// Minimize. On the vertical centre, and wider than the other marks: it is the
/// one control whose mark is a single line, so length is all it has to say
/// what it is.
fn bar(painter: &Painter, box_: Rect, stroke: Stroke) {
    let y = box_.center().y.round() + 0.5;
    painter.line_segment(
        [Pos2::new(box_.left(), y), Pos2::new(box_.right(), y)],
        stroke,
    );
}

/// Maximize: a window, not a square. The doubled top edge is the difference,
/// and it is what stops this reading as the same mark as the restore one.
fn window(painter: &Painter, box_: Rect, stroke: Stroke) {
    let rect = align(box_);
    painter.rect_stroke(rect, Rounding::same(1.0), stroke);
    let y = (rect.top() + 2.0).round() + 0.5;
    painter.line_segment(
        [
            Pos2::new(rect.left() + 0.5, y),
            Pos2::new(rect.right() - 0.5, y),
        ],
        stroke,
    );
}

/// Restore: the window it will go back to, with the maximized one behind it.
///
/// The back copy is drawn first and then covered, so the two do not cross-hatch
/// where they overlap - which at this size is the difference between "two
/// windows" and "a smudge".
fn window_stack(painter: &Painter, box_: Rect, stroke: Stroke, behind: Color32) {
    let front = align(Rect::from_min_size(
        box_.min + Vec2::new(0.0, box_.height() * 0.28),
        box_.size() * 0.78,
    ));
    let back = front.translate(Vec2::new(front.width() * 0.30, -front.height() * 0.30));

    painter.rect_stroke(back, Rounding::same(1.0), stroke);
    painter.rect_filled(front.expand(1.0), Rounding::same(1.0), behind);
    window(painter, front, stroke);
}

/// Close, and the tab's own close: the only diagonals the app draws, which is
/// what makes them findable without reading them.
fn cross(painter: &Painter, box_: Rect, stroke: Stroke) {
    painter.line_segment([box_.left_top(), box_.right_bottom()], stroke);
    painter.line_segment([box_.right_top(), box_.left_bottom()], stroke);
}

/// New tab.
///
/// The half-point offset goes on the centre, not on one endpoint of each arm.
/// Putting it on the endpoints is what left the mark lopsided: the horizontal
/// bar still ran from `c.x - arm` to `c.x + arm` while the stem had moved half
/// a point right, so the left arm came out a point longer than the right one -
/// small, and plainly visible next to a symmetrical `x`.
fn plus(painter: &Painter, box_: Rect, stroke: Stroke) {
    let c = box_.center().round() + Vec2::splat(0.5);
    let arm = box_.width() / 2.0;
    painter.line_segment(
        [Pos2::new(c.x - arm, c.y), Pos2::new(c.x + arm, c.y)],
        stroke,
    );
    painter.line_segment(
        [Pos2::new(c.x, c.y - arm), Pos2::new(c.x, c.y + arm)],
        stroke,
    );
}

/// Settings.
///
/// A ring with teeth around it and a hole in the middle. Six teeth rather than
/// the eight a real gear has: at the size a title-bar control gives, eight run
/// together into a fringe, and six still say "gear" while staying countable.
fn gear(painter: &Painter, box_: Rect, stroke: Stroke) {
    let centre = box_.center();
    let outer = box_.width() / 2.0;
    // The ring sits inside the teeth, so the whole mark still fits the box.
    let ring = outer * 0.64;
    painter.circle_stroke(centre, ring, stroke);
    // The hole. Small enough to read as a hole rather than as a second ring.
    painter.circle_stroke(centre, ring * 0.34, stroke);

    for step in 0..6 {
        let angle = std::f32::consts::TAU * step as f32 / 6.0;
        let (sin, cos) = angle.sin_cos();
        let dir = Vec2::new(cos, sin);
        painter.line_segment(
            [
                centre + dir * (ring - stroke.width * 0.5),
                centre + dir * outer,
            ],
            stroke,
        );
    }
}

/// Pin to desktop: a round head in the upper right and a needle running from it
/// to the lower left, the way a pushpin sits in a board.
///
/// Leaning rather than upright: an upright pin at this size is a circle on a
/// stick, which reads as a key or a lollipop before it reads as a pin.
fn pin(painter: &Painter, box_: Rect, stroke: Stroke, pinned: bool) {
    let side = box_.width();
    let head = box_.right_top() + Vec2::new(-side * 0.32, side * 0.32);
    let radius = side * 0.26;
    let dir = Vec2::new(-1.0, 1.0).normalized();
    painter.line_segment([head + dir * radius, box_.left_bottom()], stroke);
    // The collar under the head, across the needle, which is what makes the
    // round thing a pin's head rather than a balloon on a string.
    let across = Vec2::new(1.0, 1.0).normalized() * radius * 0.9;
    let collar = head + dir * (radius + stroke.width);
    painter.line_segment([collar - across, collar + across], stroke);
    if pinned {
        painter.circle_filled(head, radius, stroke.color);
    } else {
        painter.circle_stroke(head, radius, stroke);
    }
}

/// The marks the Settings window draws: one per sidebar category, and the few
/// a row needs - a magnifier for the search field, the chevrons on a pop-up
/// menu and on a row that leads somewhere.
///
/// Kept apart from [`Glyph`] because they are drawn to a different brief: white
/// on a small coloured tile, where the title-bar marks are hairlines on the
/// chrome, so they are heavier and fill more of their box.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Symbol {
    Gear,
    /// A circle half filled - light and dark, the mark for appearance.
    Contrast,
    /// A painter's palette: a ring with three dots of paint.
    Swatch,
    /// A monitor with a prompt on it.
    Display,
    Pencil,
    Window,
    /// Two windows, one behind the other.
    Windows,
    Person,
    /// A page of text.
    Lines,
    Bolt,
    /// `>_`, the shell prompt.
    Prompt,
    /// A lower-case "i".
    Info,
    Magnifier,
    ChevronRight,
    /// The pair of chevrons on a pop-up menu, saying it opens either way.
    ChevronUpDown,
}

impl Symbol {
    pub const ALL: [Symbol; 15] = [
        Symbol::Gear,
        Symbol::Contrast,
        Symbol::Swatch,
        Symbol::Display,
        Symbol::Pencil,
        Symbol::Window,
        Symbol::Windows,
        Symbol::Person,
        Symbol::Lines,
        Symbol::Bolt,
        Symbol::Prompt,
        Symbol::Info,
        Symbol::Magnifier,
        Symbol::ChevronRight,
        Symbol::ChevronUpDown,
    ];
}

/// Draws `symbol` centred in `rect`, in `colour`, as large as `rect` allows.
pub fn symbol(painter: &Painter, rect: Rect, symbol: Symbol, colour: Color32) {
    let side = rect.width().min(rect.height());
    let b = Rect::from_center_size(rect.center(), Vec2::splat(side));
    // Proportional rather than the fixed hairline the title bar uses: these
    // are drawn from a 14-point tile up to a 20-point one, and a 1.3-point
    // line on the larger one looks like a sketch of the mark.
    let stroke = Stroke::new((side * 0.11).max(1.2), colour);
    let at = |x: f32, y: f32| Pos2::new(b.left() + b.width() * x, b.top() + b.height() * y);
    match symbol {
        Symbol::Gear => gear(painter, b.shrink(side * 0.04), stroke),
        Symbol::Swatch => {
            painter.circle_stroke(b.center(), side * 0.40, stroke);
            for (x, y) in [(0.36, 0.40), (0.56, 0.32), (0.66, 0.54)] {
                painter.circle_filled(at(x, y), side * 0.08, colour);
            }
        }
        Symbol::Display => {
            let screen = Rect::from_min_max(at(0.10, 0.16), at(0.90, 0.72));
            painter.rect_stroke(screen, Rounding::same(side * 0.06), stroke);
            painter.line_segment([at(0.36, 0.88), at(0.64, 0.88)], stroke);
            painter.line_segment([at(0.30, 0.34), at(0.42, 0.44)], stroke);
            painter.line_segment([at(0.42, 0.44), at(0.30, 0.54)], stroke);
        }
        Symbol::Pencil => {
            let thick = Stroke::new(stroke.width * 1.6, colour);
            painter.line_segment([at(0.74, 0.20), at(0.30, 0.64)], thick);
            painter.line_segment([at(0.30, 0.64), at(0.20, 0.80)], stroke);
            painter.line_segment([at(0.20, 0.80), at(0.36, 0.70)], stroke);
        }
        Symbol::Contrast => {
            let radius = side * 0.40;
            painter.circle_stroke(b.center(), radius, stroke);
            // The left half as a fan of points: egui has no arc to fill.
            let points: Vec<Pos2> = (0..=16)
                .map(|step| {
                    let angle =
                        std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * step as f32 / 16.0;
                    let (sin, cos) = angle.sin_cos();
                    b.center() + Vec2::new(cos, sin) * radius
                })
                .collect();
            painter.add(egui::Shape::convex_polygon(points, colour, Stroke::NONE));
        }
        Symbol::Window => window(painter, b.shrink(side * 0.14), stroke),
        Symbol::Windows => {
            let front = Rect::from_min_max(at(0.10, 0.34), at(0.70, 0.86));
            window(painter, front, stroke);
            // Only the two edges of the back window that show past the front
            // one: drawn whole, its lines would cross the front window's.
            let corner = at(0.90, 0.14);
            painter.line_segment([at(0.30, 0.14), corner], stroke);
            painter.line_segment([corner, at(0.90, 0.66)], stroke);
        }
        Symbol::Person => {
            painter.circle_stroke(at(0.5, 0.34), side * 0.17, stroke);
            // The shoulders: the top half of an ellipse, as a polyline since
            // egui has no arc.
            let points: Vec<Pos2> = (0..=12)
                .map(|step| {
                    let angle = std::f32::consts::PI * (1.0 + step as f32 / 12.0);
                    let (sin, cos) = angle.sin_cos();
                    at(0.5 + 0.32 * cos, 0.86 + 0.26 * sin)
                })
                .collect();
            painter.add(egui::Shape::line(points, stroke));
        }
        Symbol::Lines => {
            for (y, end) in [(0.28, 0.80), (0.46, 0.80), (0.64, 0.80), (0.82, 0.56)] {
                painter.line_segment([at(0.20, y), at(end, y)], stroke);
            }
        }
        Symbol::Bolt => {
            // Two triangles that overlap across the middle: egui fills convex
            // shapes only, and a lightning bolt is the textbook concave one.
            for points in [
                vec![at(0.62, 0.08), at(0.26, 0.58), at(0.58, 0.50)],
                vec![at(0.38, 0.92), at(0.74, 0.42), at(0.42, 0.50)],
            ] {
                painter.add(egui::Shape::convex_polygon(points, colour, Stroke::NONE));
            }
        }
        Symbol::Prompt => {
            painter.line_segment([at(0.18, 0.30), at(0.42, 0.50)], stroke);
            painter.line_segment([at(0.42, 0.50), at(0.18, 0.70)], stroke);
            painter.line_segment([at(0.50, 0.72), at(0.82, 0.72)], stroke);
        }
        Symbol::Info => {
            painter.circle_filled(at(0.5, 0.24), side * 0.09, colour);
            let thick = Stroke::new(stroke.width * 1.4, colour);
            painter.line_segment([at(0.5, 0.42), at(0.5, 0.82)], thick);
        }
        Symbol::Magnifier => {
            painter.circle_stroke(at(0.42, 0.42), side * 0.26, stroke);
            painter.line_segment([at(0.62, 0.62), at(0.86, 0.86)], stroke);
        }
        Symbol::ChevronRight => {
            painter.line_segment([at(0.38, 0.22), at(0.64, 0.50)], stroke);
            painter.line_segment([at(0.64, 0.50), at(0.38, 0.78)], stroke);
        }
        Symbol::ChevronUpDown => {
            painter.line_segment([at(0.28, 0.40), at(0.50, 0.20)], stroke);
            painter.line_segment([at(0.50, 0.20), at(0.72, 0.40)], stroke);
            painter.line_segment([at(0.28, 0.60), at(0.50, 0.80)], stroke);
            painter.line_segment([at(0.50, 0.80), at(0.72, 0.60)], stroke);
        }
    }
}

/// A rectangle on whole pixels, so a one-point outline comes out crisp rather
/// than as two grey rows.
fn align(rect: Rect) -> Rect {
    Rect::from_min_max(
        Pos2::new(rect.left().round() + 0.5, rect.top().round() + 0.5),
        Pos2::new(rect.right().round() - 0.5, rect.bottom().round() - 0.5),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sidebar symbol sits on a tile barely larger than itself, so one that
    /// strays outside its box draws over the tile's edge and the label.
    #[test]
    fn every_symbol_stays_inside_the_box_it_is_drawn_in() {
        let ctx = egui::Context::default();
        let rect = Rect::from_min_size(Pos2::new(10.0, 10.0), Vec2::splat(16.0));
        for symbol_ in Symbol::ALL {
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                let painter = ctx.layer_painter(egui::LayerId::background());
                symbol(&painter, rect, symbol_, Color32::WHITE);
            });
            let mut drawn = Rect::NOTHING;
            for clipped in &output.shapes {
                drawn = drawn.union(clipped.shape.visual_bounding_rect());
            }
            assert!(drawn.is_positive(), "{symbol_:?} draws nothing");
            // Half a stroke of slack: a line ending on the box's edge is
            // drawn with its caps just past it.
            assert!(
                rect.expand(1.5).contains_rect(drawn),
                "{symbol_:?} reaches {drawn:?}, outside {rect:?}"
            );
        }
    }

    /// Every glyph has to fit the box it is handed, or a title-bar control
    /// would draw over its neighbour.
    #[test]
    fn every_glyph_fits_its_hit_area() {
        for glyph in [
            Glyph::Bar,
            Glyph::Window,
            Glyph::WindowStack,
            Glyph::Cross,
            Glyph::Gear,
            Glyph::Plus,
            Glyph::SmallCross,
            Glyph::Pin,
            Glyph::Pinned,
        ] {
            assert!(
                glyph.scale() > 0.0 && glyph.scale() <= 0.62,
                "{glyph:?} would not fit its button"
            );
            assert!(glyph.width() >= 1.0, "{glyph:?} would be invisible");
        }
    }

    /// The two window marks are the ones that were being confused for each
    /// other, and the restore mark is drawn smaller precisely so the copy
    /// behind it has somewhere to be.
    #[test]
    fn the_restore_mark_leaves_room_for_the_window_behind_it() {
        assert!(Glyph::WindowStack.scale() < Glyph::Window.scale());
    }

    /// The gear is the most detailed mark, so it gets the most room.
    #[test]
    fn the_gear_is_the_largest_mark() {
        for glyph in [Glyph::Bar, Glyph::Window, Glyph::Cross, Glyph::Plus] {
            assert!(glyph.scale() < Glyph::Gear.scale());
        }
    }
}
