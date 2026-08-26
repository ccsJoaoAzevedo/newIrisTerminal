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

fn blend(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (x as f32 * (1.0 - t) + y as f32 * t) as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}
