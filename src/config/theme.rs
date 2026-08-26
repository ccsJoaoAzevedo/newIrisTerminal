//! Colour and font definitions.
//!
//! A theme is a plain TOML file under `<config>/themes/`. Dropping a file in
//! that folder makes it selectable; the built-ins below are written out on
//! first run so there is always something to copy.

use egui::Color32;
use serde::{Deserialize, Serialize};

/// Fallback for a malformed built-in colour. Deliberately garish so a typo in
/// a shipped theme is obvious rather than silently sane.
fn c(hex: &str) -> Color32 {
    parse_hex(hex).unwrap_or(Color32::from_rgb(255, 0, 255))
}

/// Accepts `#rgb`, `#rrggbb`, and the same without the leading `#`.
pub fn parse_hex(text: &str) -> Option<Color32> {
    let s = text.trim().trim_start_matches('#');
    match s.len() {
        3 => {
            let d = |i: usize| u8::from_str_radix(&s[i..i + 1], 16).ok().map(|v| v * 17);
            Some(Color32::from_rgb(d(0)?, d(1)?, d(2)?))
        }
        6 => {
            let d = |i: usize| u8::from_str_radix(&s[i..i + 2], 16).ok();
            Some(Color32::from_rgb(d(0)?, d(2)?, d(4)?))
        }
        _ => None,
    }
}

pub fn to_hex(color: Color32) -> String {
    format!("#{:02x}{:02x}{:02x}", color.r(), color.g(), color.b())
}

/// On-disk form. Kept separate from [`Theme`] so the runtime type can hold
/// resolved `Color32` values without serde noise on every field.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ThemeFile {
    pub name: String,
    pub background: String,
    pub foreground: String,
    pub cursor: String,
    pub selection: String,
    /// The 16 ANSI colours: 0-7 normal, 8-15 bright.
    pub ansi: Vec<String>,
    #[serde(default = "default_font_family")]
    pub font_family: String,
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    /// Drives egui's own widget colours so the chrome matches the terminal.
    #[serde(default)]
    pub dark: bool,
}

fn default_font_family() -> String {
    "monospace".to_string()
}

fn default_font_size() -> f32 {
    14.0
}

#[derive(Clone, Debug)]
pub struct Theme {
    pub name: String,
    pub background: Color32,
    pub foreground: Color32,
    pub cursor: Color32,
    pub selection: Color32,
    pub ansi: [Color32; 16],
    pub font_family: String,
    pub font_size: f32,
    pub dark: bool,
}

impl Default for Theme {
    fn default() -> Self {
        Theme::from_file(&builtin_files()[0])
    }
}

impl Theme {
    pub fn from_file(file: &ThemeFile) -> Self {
        let mut ansi = DEFAULT_ANSI.map(c);
        for (slot, hex) in ansi.iter_mut().zip(file.ansi.iter()) {
            if let Some(color) = parse_hex(hex) {
                *slot = color;
            }
        }
        Theme {
            name: file.name.clone(),
            background: parse_hex(&file.background).unwrap_or(Color32::BLACK),
            foreground: parse_hex(&file.foreground).unwrap_or(Color32::LIGHT_GRAY),
            cursor: parse_hex(&file.cursor).unwrap_or(Color32::WHITE),
            selection: parse_hex(&file.selection).unwrap_or(Color32::DARK_BLUE),
            ansi,
            font_family: file.font_family.clone(),
            font_size: file.font_size.clamp(6.0, 48.0),
            dark: file.dark,
        }
    }

    pub fn to_file(&self) -> ThemeFile {
        ThemeFile {
            name: self.name.clone(),
            background: to_hex(self.background),
            foreground: to_hex(self.foreground),
            cursor: to_hex(self.cursor),
            selection: to_hex(self.selection),
            ansi: self.ansi.iter().map(|c| to_hex(*c)).collect(),
            font_family: self.font_family.clone(),
            font_size: self.font_size,
            dark: self.dark,
        }
    }

    /// egui visuals that agree with the terminal colours, so the tab strip and
    /// dialogs do not fight the grid.
    pub fn visuals(&self) -> egui::Visuals {
        let mut v = if self.dark {
            egui::Visuals::dark()
        } else {
            egui::Visuals::light()
        };
        v.panel_fill = self.background;
        v.window_fill = self.background;
        v.extreme_bg_color = self.background;
        v.selection.bg_fill = self.selection;
        v.override_text_color = Some(self.foreground);
        v
    }
}

/// xterm's standard 16, used for any slot a theme leaves unspecified.
const DEFAULT_ANSI: [&str; 16] = [
    "#000000", "#cd0000", "#00cd00", "#cdcd00", "#0000ee", "#cd00cd", "#00cdcd", "#e5e5e5",
    "#7f7f7f", "#ff0000", "#00ff00", "#ffff00", "#5c5cff", "#ff00ff", "#00ffff", "#ffffff",
];

/// Themes written to disk on first run. The first entry is the default.
pub fn builtin_files() -> Vec<ThemeFile> {
    let ansi = |v: [&str; 16]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();

    vec![
        ThemeFile {
            name: "IRIS Dark".into(),
            background: "#101418".into(),
            foreground: "#d6dbe0".into(),
            cursor: "#4ec9b0".into(),
            selection: "#2a4a6b".into(),
            ansi: ansi(DEFAULT_ANSI),
            font_family: default_font_family(),
            font_size: 14.0,
            dark: true,
        },
        ThemeFile {
            name: "IRIS Classic Green".into(),
            background: "#0a0f0a".into(),
            foreground: "#33ff66".into(),
            cursor: "#88ffaa".into(),
            selection: "#1f4a2c".into(),
            ansi: ansi([
                "#0a0f0a", "#4ade80", "#33ff66", "#86efac", "#22c55e", "#4ade80", "#5eead4",
                "#bbf7d0", "#166534", "#4ade80", "#33ff66", "#bbf7d0", "#22c55e", "#86efac",
                "#5eead4", "#dcfce7",
            ]),
            font_family: default_font_family(),
            font_size: 14.0,
            dark: true,
        },
        ThemeFile {
            name: "Light".into(),
            background: "#fdfdfd".into(),
            foreground: "#1f2328".into(),
            cursor: "#0969da".into(),
            selection: "#b6d7ff".into(),
            ansi: ansi([
                "#24292f", "#cf222e", "#116329", "#8a6a00", "#0969da", "#8250df", "#1b7c83",
                "#6e7781", "#57606a", "#a40e26", "#1a7f37", "#a67c00", "#218bff", "#a475f9",
                "#3192aa", "#24292f",
            ]),
            font_family: default_font_family(),
            font_size: 14.0,
            dark: false,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_parses_both_lengths() {
        assert_eq!(parse_hex("#ff8000"), Some(Color32::from_rgb(255, 128, 0)));
        assert_eq!(parse_hex("f80"), Some(Color32::from_rgb(255, 136, 0)));
        assert_eq!(parse_hex("nope"), None);
    }

    #[test]
    fn hex_round_trips() {
        let c = Color32::from_rgb(18, 52, 86);
        assert_eq!(parse_hex(&to_hex(c)), Some(c));
    }

    #[test]
    fn builtins_all_load_with_16_ansi_colours() {
        for file in builtin_files() {
            let theme = Theme::from_file(&file);
            assert_eq!(theme.ansi.len(), 16, "{}", file.name);
        }
    }
}
