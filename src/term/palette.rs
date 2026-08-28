//! Resolves the parser's abstract [`Color`] into concrete RGB for painting.
//!
//! Indexed colours 0..16 come from the active theme so a theme switch recolours
//! existing scrollback; 16..256 use the standard xterm cube, which no theme
//! overrides.

use egui::Color32;

use super::cell::{Attrs, Cell, Color};
use crate::config::theme::Theme;

/// Foreground and background for a cell, with REVERSE and HIDDEN already
/// applied so the renderer never has to think about them.
pub fn resolve(cell: &Cell, theme: &Theme) -> (Color32, Color32) {
    let mut fg = resolve_one(cell.fg, theme, theme.foreground);
    let mut bg = resolve_one(cell.bg, theme, theme.background);

    // Bold traditionally brightens the low 8 ANSI colours, which is what makes
    // IRIS error text legible against a dark background.
    if cell.attrs.contains(Attrs::BOLD) {
        if let Color::Indexed(i @ 0..=7) = cell.fg {
            fg = theme.ansi[(i + 8) as usize];
        }
    }

    if cell.attrs.contains(Attrs::DIM) {
        fg = blend(fg, bg, 0.5);
    }

    if cell.attrs.contains(Attrs::REVERSE) {
        std::mem::swap(&mut fg, &mut bg);
    }

    if cell.attrs.contains(Attrs::HIDDEN) {
        fg = bg;
    }

    (fg, bg)
}

fn resolve_one(color: Color, theme: &Theme, default: Color32) -> Color32 {
    match color {
        Color::Default => default,
        Color::Rgb(r, g, b) => Color32::from_rgb(r, g, b),
        Color::Indexed(i) => indexed(i, theme),
    }
}

/// The xterm 256-colour map: 0..16 themed, 16..232 a 6x6x6 cube, 232..256 grey.
pub fn indexed(index: u8, theme: &Theme) -> Color32 {
    match index {
        0..=15 => theme.ansi[index as usize],
        16..=231 => {
            let i = index - 16;
            let levels = [0u8, 95, 135, 175, 215, 255];
            let r = levels[(i / 36) as usize];
            let g = levels[((i % 36) / 6) as usize];
            let b = levels[(i % 6) as usize];
            Color32::from_rgb(r, g, b)
        }
        232..=255 => {
            let v = 8 + (index - 232) * 10;
            Color32::from_rgb(v, v, v)
        }
    }
}

/// How far apart a cursor and the background have to be for the cursor to be
/// worth drawing. In the same normalised units as [`distance`].
const MIN_CURSOR_DISTANCE: f32 = 0.25;

/// The colour to draw the cursor in.
///
/// Insert mode inverts the theme's cursor colour. Inverting rather than adding a
/// second colour keeps every existing theme file working, and it is the change
/// the eye notices without having to look for it.
///
/// The catch is that a colour can invert onto the background: a mid-grey cursor
/// on a mid-grey background inverts to the same mid-grey, and the cursor would
/// disappear at exactly the moment it is meant to be shouting. So a result too
/// close to the background is pushed towards whichever end of the scale the
/// background is furthest from, which keeps the hue recognisable while
/// guaranteeing it can be seen.
pub fn cursor(theme: &Theme, insert: bool) -> Color32 {
    if !insert {
        return theme.cursor;
    }

    let inverted = Color32::from_rgb(
        255 - theme.cursor.r(),
        255 - theme.cursor.g(),
        255 - theme.cursor.b(),
    );
    if distance(inverted, theme.background) >= MIN_CURSOR_DISTANCE {
        return inverted;
    }

    let escape = if luminance(theme.background) < 0.5 {
        Color32::WHITE
    } else {
        Color32::BLACK
    };
    blend(inverted, escape, 0.65)
}

/// Distance between two colours: the RGB cube diagonal, normalised to 0..1.
///
/// Crude next to a perceptual metric, but it is monotonic, cheap and easy to
/// reason about, and the only question being asked is "can this be seen against
/// that".
fn distance(a: Color32, b: Color32) -> f32 {
    let d = |x: u8, y: u8| (x as f32 - y as f32) / 255.0;
    let (dr, dg, db) = (d(a.r(), b.r()), d(a.g(), b.g()), d(a.b(), b.b()));
    (dr * dr + dg * dg + db * db).sqrt() / 3.0_f32.sqrt()
}

/// Relative luminance, with the usual sRGB weights.
fn luminance(c: Color32) -> f32 {
    (0.2126 * c.r() as f32 + 0.7152 * c.g() as f32 + 0.0722 * c.b() as f32) / 255.0
}

/// Mixes `a` towards `b`. Also used by the chrome the terminal draws itself,
/// such as the scrollbar, so that it shades from the theme rather than needing
/// colours of its own in every theme file.
pub fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (x as f32 * (1.0 - t) + y as f32 * t) as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::theme::{builtin_files, to_hex, ThemeFile};

    fn theme_with(background: &str, cursor: &str) -> Theme {
        let mut file: ThemeFile = builtin_files()[0].clone();
        file.background = background.to_string();
        file.cursor = cursor.to_string();
        Theme::from_file(&file)
    }

    #[test]
    fn replace_mode_leaves_the_cursor_colour_alone() {
        let theme = theme_with("#101418", "#4ec9b0");
        assert_eq!(cursor(&theme, false), theme.cursor);
    }

    /// The example the behaviour was asked for with: a green cursor becomes
    /// pink.
    #[test]
    fn insert_mode_inverts_the_cursor() {
        let theme = theme_with("#101418", "#00ff00");
        assert_eq!(cursor(&theme, true), Color32::from_rgb(255, 0, 255));
    }

    /// The case worth thinking about: a cursor that inverts onto the
    /// background. Left alone it would vanish exactly when it should stand out.
    #[test]
    fn an_inverse_that_lands_on_the_background_is_moved_off_it() {
        // Mid grey on mid grey: the inverse is the background.
        let theme = theme_with("#808080", "#7f7f7f");
        let colour = cursor(&theme, true);
        assert!(
            distance(colour, theme.background) >= MIN_CURSOR_DISTANCE,
            "cursor {colour:?} is invisible on {:?}",
            theme.background
        );
        assert_ne!(colour, theme.background);
    }

    /// The guarantee, over every combination coarse enough to enumerate: the
    /// cursor is always far enough from the background to be seen.
    #[test]
    fn the_insert_cursor_is_always_visible() {
        let steps = [0u8, 63, 127, 128, 191, 255];
        for &br in &steps {
            for &bg in &steps {
                for &bb in &steps {
                    for &cr in &steps {
                        for &cg in &steps {
                            let background = Color32::from_rgb(br, bg, bb);
                            // The blue channel of the cursor is the one varied
                            // least; five nested loops is already 7776 cases.
                            let mut file: ThemeFile = builtin_files()[0].clone();
                            file.background = to_hex(background);
                            file.cursor = to_hex(Color32::from_rgb(cr, cg, bb));
                            let theme = Theme::from_file(&file);

                            let colour = cursor(&theme, true);
                            assert!(
                                distance(colour, theme.background) >= MIN_CURSOR_DISTANCE,
                                "cursor {colour:?} invisible on {background:?}"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn distance_is_zero_for_a_colour_against_itself_and_one_across_the_cube() {
        let c = Color32::from_rgb(12, 34, 56);
        assert_eq!(distance(c, c), 0.0);
        assert!((distance(Color32::BLACK, Color32::WHITE) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn every_builtin_theme_gets_a_visible_insert_cursor() {
        for file in builtin_files() {
            let theme = Theme::from_file(&file);
            let colour = cursor(&theme, true);
            assert!(
                distance(colour, theme.background) >= MIN_CURSOR_DISTANCE,
                "{} has an invisible insert cursor",
                file.name
            );
            assert_ne!(colour, theme.cursor, "{} does not change", file.name);
        }
    }
}
