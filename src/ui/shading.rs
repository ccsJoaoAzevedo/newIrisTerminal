//! Gradients, glosses and bevels.
//!
//! egui paints flat fills and nothing else: every shape it draws is one colour.
//! Anything that has to look lit — an Aqua bubble, a Luna tile, the capsule on
//! an Aqua scrollbar — is therefore built here, out of meshes with the colour on
//! the vertices, which is the one place egui interpolates.
//!
//! Shared by the window chrome and the terminal's own scrollbar, because those
//! two have to agree: a Tiger theme with Aqua traffic lights and a flat grey
//! scroll handle looks like two applications in one window.

use egui::{Color32, Mesh, Painter, Pos2, Rect, Rounding, Shape, Stroke, Vec2};

/// Scales a colour towards black.
pub fn darken(color: Color32, factor: f32) -> Color32 {
    let s = |v: u8| (f32::from(v) * factor).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgb(s(color.r()), s(color.g()), s(color.b()))
}

/// Mixes a colour towards white.
pub fn lighten(color: Color32, t: f32) -> Color32 {
    let s = |v: u8| {
        (f32::from(v) + (255.0 - f32::from(v)) * t)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Color32::from_rgb(s(color.r()), s(color.g()), s(color.b()))
}

/// White at `alpha`, for a highlight.
pub fn white(alpha: u8) -> Color32 {
    Color32::from_rgba_unmultiplied(255, 255, 255, alpha)
}

/// A vertical gradient across `rect`, as a two-triangle mesh.
pub fn gradient(painter: &Painter, rect: Rect, top: Color32, bottom: Color32) {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), top);
    mesh.colored_vertex(rect.right_top(), top);
    mesh.colored_vertex(rect.left_bottom(), bottom);
    mesh.colored_vertex(rect.right_bottom(), bottom);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(2, 1, 3);
    painter.add(Shape::mesh(mesh));
}

/// The same across, for a bar whose light comes from the side.
pub fn gradient_across(painter: &Painter, rect: Rect, left: Color32, right: Color32) {
    let mut mesh = Mesh::default();
    mesh.colored_vertex(rect.left_top(), left);
    mesh.colored_vertex(rect.left_bottom(), left);
    mesh.colored_vertex(rect.right_top(), right);
    mesh.colored_vertex(rect.right_bottom(), right);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(2, 1, 3);
    painter.add(Shape::mesh(mesh));
}

/// A disc shaded from `inner` at a point `focus` from the centre out to `outer`
/// at the rim: a triangle fan, one vertex per step around the edge.
pub fn radial(
    painter: &Painter,
    center: Pos2,
    radius: f32,
    focus: Vec2,
    inner: Color32,
    outer: Color32,
) {
    const STEPS: usize = 40;
    let mut mesh = Mesh::default();
    mesh.colored_vertex(center + focus, inner);
    for step in 0..=STEPS {
        let angle = step as f32 / STEPS as f32 * std::f32::consts::TAU;
        mesh.colored_vertex(center + Vec2::angled(angle) * radius, outer);
    }
    for step in 1..=STEPS {
        mesh.add_triangle(0, step as u32, step as u32 + 1);
    }
    painter.add(Shape::mesh(mesh));
}

/// A white highlight ellipse, opaque where the light hits and fading to nothing
/// at its edge.
pub fn gloss(painter: &Painter, center: Pos2, rx: f32, ry: f32, alpha: u8) {
    const STEPS: usize = 32;
    let mut mesh = Mesh::default();
    mesh.colored_vertex(center - Vec2::new(0.0, ry * 0.35), white(alpha));
    for step in 0..=STEPS {
        let angle = step as f32 / STEPS as f32 * std::f32::consts::TAU;
        mesh.colored_vertex(
            center + Vec2::new(angle.cos() * rx, angle.sin() * ry),
            white(0),
        );
    }
    for step in 1..=STEPS {
        mesh.add_triangle(0, step as u32, step as u32 + 1);
    }
    painter.add(Shape::mesh(mesh));
}

/// The Aqua scroll handle: a blue capsule, lit from the left, with a white cap
/// over its top end and a dark rim all round.
///
/// Built for a vertical bar and turned on its side for a horizontal one, which
/// is what Aqua itself did — the gloss runs along the length either way, and
/// the shading across it.
pub fn aqua_capsule(painter: &Painter, rect: Rect, base: Color32, vertical: bool) {
    let thickness = if vertical {
        rect.width()
    } else {
        rect.height()
    };
    let rounding = Rounding::same(thickness * 0.5);

    // The body, then the shading across the short axis: bright at the lit edge,
    // deepening to the far one.
    painter.rect_filled(rect, rounding, base);
    let inner = rect.shrink(1.0);
    if vertical {
        gradient_across(painter, inner, lighten(base, 0.55), darken(base, 0.78));
    } else {
        gradient(painter, inner, lighten(base, 0.55), darken(base, 0.78));
    }

    // The specular streak along the lit edge, which is what makes it glass.
    let streak = if vertical {
        Rect::from_min_max(
            inner.left_top() + Vec2::new(thickness * 0.18, thickness * 0.35),
            Pos2::new(
                inner.left() + thickness * 0.46,
                inner.bottom() - thickness * 0.35,
            ),
        )
    } else {
        Rect::from_min_max(
            inner.left_top() + Vec2::new(thickness * 0.35, thickness * 0.18),
            Pos2::new(
                inner.right() - thickness * 0.35,
                inner.top() + thickness * 0.46,
            ),
        )
    };
    if streak.width() > 0.0 && streak.height() > 0.0 {
        painter.rect_filled(streak, Rounding::same(thickness * 0.25), white(120));
    }

    // The bright cap over the leading end, and the dark rim.
    let cap = if vertical {
        Rect::from_min_max(
            inner.left_top(),
            Pos2::new(inner.right(), inner.top() + thickness * 0.55),
        )
    } else {
        Rect::from_min_max(
            inner.left_top(),
            Pos2::new(inner.left() + thickness * 0.55, inner.bottom()),
        )
    };
    gloss(
        painter,
        cap.center(),
        cap.width() * 0.55,
        cap.height() * 0.55,
        110,
    );
    painter.rect_stroke(rect, rounding, Stroke::new(1.0_f32, darken(base, 0.55)));
}
