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
}

impl Glyph {
    /// How much of the hit area's shorter side the mark occupies.
    fn scale(self) -> f32 {
        match self {
            // Wider than tall: a minimize bar that is only as wide as a
            // maximize box reads as an underscore.
            Glyph::Bar => 0.44,
            Glyph::Window => 0.38,
            // Room for the offset copy behind it.
            Glyph::WindowStack => 0.34,
            Glyph::Cross => 0.34,
            // The most detailed mark in the set, and the one that needs the
            // most room before its teeth stop being teeth.
            Glyph::Gear => 0.52,
            Glyph::Plus => 0.46,
            Glyph::SmallCross => 0.40,
        }
    }

    /// How thick the mark is drawn, in points.
    ///
    /// Not scaled with the box: these are hairline marks on screen furniture,
    /// and a stroke that grows with the button turns the gear into a blob.
    fn width(self) -> f32 {
        match self {
            Glyph::Bar | Glyph::Plus => 1.4,
            Glyph::Cross | Glyph::SmallCross => 1.3,
            Glyph::Gear => 1.2,
            Glyph::Window | Glyph::WindowStack => 1.0,
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
fn plus(painter: &Painter, box_: Rect, stroke: Stroke) {
    let c = box_.center().round();
    let arm = box_.width() / 2.0;
    painter.line_segment(
        [
            Pos2::new(c.x - arm, c.y + 0.5),
            Pos2::new(c.x + arm, c.y + 0.5),
        ],
        stroke,
    );
    painter.line_segment(
        [
            Pos2::new(c.x + 0.5, c.y - arm),
            Pos2::new(c.x + 0.5, c.y + arm),
        ],
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
        ] {
            assert!(
                glyph.scale() > 0.0 && glyph.scale() <= 0.6,
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
