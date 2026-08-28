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
    /// Text colour for the chrome - tab strip, panels, dialogs. Empty falls
    /// back to `foreground`, which is what every theme did before the two were
    /// told apart, so the terminal and the UI around it can now differ.
    #[serde(default)]
    pub ui_foreground: String,
    /// Background for the chrome. Empty falls back to `background`.
    #[serde(default)]
    pub ui_background: String,
    /// Global references (`^ABC`) in terminal output. Empty falls back to a
    /// bright ANSI slot.
    #[serde(default)]
    pub syntax_global: String,
    /// Quoted strings in terminal output. Empty falls back to an ANSI slot.
    #[serde(default)]
    pub syntax_string: String,
    #[serde(default = "default_font_family")]
    pub font_family: String,
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    /// Drives egui's own widget colours so the chrome matches the terminal.
    #[serde(default)]
    pub dark: bool,
    /// Set on the copies [`crate::config::ensure_config_tree`] writes out, so a
    /// corrected built-in can replace a stale copy on disk. Clear it (or rename
    /// the theme) to claim the file as your own and stop it being overwritten.
    #[serde(default)]
    pub builtin: bool,
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
    pub ui_foreground: Color32,
    pub ui_background: Color32,
    pub syntax_global: Color32,
    pub syntax_string: Color32,
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
        // Resolved up front because the chrome and syntax colours fall back to
        // them, which keeps a theme that says nothing about either behaving
        // exactly as it did before those fields existed.
        let background = parse_hex(&file.background).unwrap_or(Color32::BLACK);
        let foreground = parse_hex(&file.foreground).unwrap_or(Color32::LIGHT_GRAY);

        Theme {
            name: file.name.clone(),
            background,
            foreground,
            cursor: parse_hex(&file.cursor).unwrap_or(Color32::WHITE),
            selection: parse_hex(&file.selection).unwrap_or(Color32::DARK_BLUE),
            ui_foreground: parse_hex(&file.ui_foreground).unwrap_or(foreground),
            ui_background: parse_hex(&file.ui_background).unwrap_or(background),
            syntax_global: parse_hex(&file.syntax_global).unwrap_or(ansi[14]),
            syntax_string: parse_hex(&file.syntax_string).unwrap_or(ansi[3]),
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
            ui_foreground: to_hex(self.ui_foreground),
            ui_background: to_hex(self.ui_background),
            syntax_global: to_hex(self.syntax_global),
            syntax_string: to_hex(self.syntax_string),
            font_family: self.font_family.clone(),
            font_size: self.font_size,
            dark: self.dark,
            // Anything serialised back out of a runtime theme is the user's
            // copy, never one we may overwrite on the next run.
            builtin: false,
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
        // The chrome gets its own pair. Painting it in the terminal's colours
        // left the two indistinguishable, which is what the theming issue is
        // about; `extreme_bg_color` stays on the terminal background so text
        // fields still read as part of the terminal.
        v.panel_fill = self.ui_background;
        v.window_fill = self.ui_background;
        v.extreme_bg_color = self.background;
        v.selection.bg_fill = self.selection;
        v.override_text_color = Some(self.ui_foreground);
        v
    }
}

/// xterm's standard 16, used for any slot a theme leaves unspecified.
const DEFAULT_ANSI: [&str; 16] = [
    "#000000", "#cd0000", "#00cd00", "#cdcd00", "#0000ee", "#cd00cd", "#00cdcd", "#e5e5e5",
    "#7f7f7f", "#ff0000", "#00ff00", "#ffff00", "#5c5cff", "#ff00ff", "#00ffff", "#ffffff",
];

/// Themes written to disk on first run. The first entry is the default.
///
/// Each carries a separate pair of chrome colours, so the tab strip and the
/// dialogs read as a frame around the terminal rather than as more terminal,
/// and a pair of syntax colours for the globals and strings the ERP output is
/// full of.
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
            ui_foreground: "#aab4be".into(),
            ui_background: "#181d23".into(),
            syntax_global: "#4ec9b0".into(),
            syntax_string: "#ce9178".into(),
            font_family: default_font_family(),
            font_size: 14.0,
            dark: true,
            builtin: true,
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
            // Muted, so the chrome does not compete with the phosphor green the
            // terminal itself is drawn in.
            ui_foreground: "#8fbf9f".into(),
            ui_background: "#111a11".into(),
            syntax_global: "#5eead4".into(),
            syntax_string: "#bbf7d0".into(),
            font_family: default_font_family(),
            font_size: 14.0,
            dark: true,
            builtin: true,
        },
        // Ported from the author's VS Code theme, tokyo-terminal-codium:
        // terminal.* colours where it defines them, editor.* for the cursor.
        ThemeFile {
            name: "Tokyo".into(),
            background: "#060507".into(),
            foreground: "#34e2e2".into(),
            cursor: "#34e2e2".into(),
            // #34e2e255 composited over the background.
            selection: "#2d636f".into(),
            ansi: ansi([
                "#060507", "#fc5698", "#7fff00", "#fe8019", "#3465a4", "#2a2436", "#116d61",
                "#aceeee", "#999988", "#ff3b3b", "#a3ff8c", "#ffe61c", "#0285f9", "#8b5cf6",
                "#34e2e2", "#eceff4",
            ]),
            // Lifted off the deep black so the tabs and panels have an edge,
            // and neutral rather than cyan so the chrome is not more terminal.
            ui_foreground: "#c0caf5".into(),
            ui_background: "#16141c".into(),
            syntax_global: "#7dcfff".into(),
            syntax_string: "#9ece6a".into(),
            font_family: default_font_family(),
            font_size: 14.0,
            dark: true,
            builtin: true,
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
            ui_foreground: "#3a4148".into(),
            ui_background: "#f0f2f5".into(),
            syntax_global: "#0f766e".into(),
            syntax_string: "#a31515".into(),
            font_family: default_font_family(),
            font_size: 14.0,
            dark: false,
            builtin: true,
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

    /// A theme file written before the chrome and syntax colours existed must
    /// keep behaving exactly as it did: the terminal colours stand in for them.
    #[test]
    fn omitted_colours_fall_back_to_the_terminal_pair() {
        let file = ThemeFile {
            name: "Bare".into(),
            background: "#101010".into(),
            foreground: "#e0e0e0".into(),
            cursor: "#ffffff".into(),
            selection: "#003366".into(),
            ansi: Vec::new(),
            ui_foreground: String::new(),
            ui_background: String::new(),
            syntax_global: String::new(),
            syntax_string: String::new(),
            font_family: default_font_family(),
            font_size: 14.0,
            dark: true,
            builtin: false,
        };

        let theme = Theme::from_file(&file);
        assert_eq!(theme.ui_background, theme.background);
        assert_eq!(theme.ui_foreground, theme.foreground);
        // An unspecified syntax colour borrows a bright ANSI slot rather than
        // landing on the background and vanishing.
        assert_eq!(theme.syntax_global, theme.ansi[14]);
        assert_eq!(theme.syntax_string, theme.ansi[3]);
    }

    /// The built-ins are the answer to "the chrome looks like more terminal".
    #[test]
    fn every_builtin_separates_the_chrome_from_the_terminal() {
        for file in builtin_files() {
            let theme = Theme::from_file(&file);
            assert_ne!(
                theme.ui_background, theme.background,
                "{} has no chrome background of its own",
                file.name
            );
            assert_ne!(
                theme.ui_foreground, theme.foreground,
                "{} has no chrome text colour of its own",
                file.name
            );
        }
    }

    #[test]
    fn a_theme_round_trips_through_its_file_form() {
        let theme = Theme::from_file(&builtin_files()[2]);
        let back = Theme::from_file(&theme.to_file());
        assert_eq!(back.name, theme.name);
        assert_eq!(back.background, theme.background);
        assert_eq!(back.ui_background, theme.ui_background);
        assert_eq!(back.syntax_global, theme.syntax_global);
        assert_eq!(back.ansi, theme.ansi);
    }

    #[test]
    fn builtins_all_load_with_16_ansi_colours() {
        for file in builtin_files() {
            let theme = Theme::from_file(&file);
            assert_eq!(theme.ansi.len(), 16, "{}", file.name);
        }
    }
}
