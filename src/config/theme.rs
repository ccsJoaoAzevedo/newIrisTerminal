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

/// One complete set of ObjectScript colours, as hex strings.
///
/// Exists so a theme can adopt a palette in one line instead of sixteen, and so
/// the fallback palette is written down once.
#[derive(Clone, Copy, Debug)]
pub struct SyntaxPalette {
    pub label: &'static str,
    pub command: &'static str,
    pub string: &'static str,
    pub number: &'static str,
    pub delimiter: &'static str,
    pub operator: &'static str,
    pub preprocessor: &'static str,
    pub function: &'static str,
    pub global: &'static str,
    pub system_variable: &'static str,
    pub class: &'static str,
    pub method: &'static str,
    pub attribute: &'static str,
    pub member: &'static str,
    pub routine: &'static str,
    pub extrinsic: &'static str,
}

/// The palette every unset colour falls back to: the InterSystems VS Code
/// extension's own semantic token colours, so the terminal and the editor
/// agree about what a global or a macro looks like.
pub const DEFAULT_SYNTAX: SyntaxPalette = SyntaxPalette {
    label: "#2BED60",
    command: "#F98DCB",
    string: "#E2E974",
    number: "#D56260",
    delimiter: "#10DBFF",
    operator: "#FF76A5",
    preprocessor: "#FF8057",
    function: "#A851D3",
    global: "#FF3B25",
    system_variable: "#C0C000",
    class: "#A0A0FF",
    method: "#57FFFF",
    attribute: "#4B75FF",
    member: "#FF5780",
    routine: "#C49AFF",
    extrinsic: "#A0C0FF",
};

/// A greener reading of the same palette, for the phosphor theme: the
/// ObjectScript colours would fight a screen that is deliberately one hue.
const GREEN_SYNTAX: SyntaxPalette = SyntaxPalette {
    label: "#86efac",
    command: "#5eead4",
    string: "#bbf7d0",
    number: "#a3e635",
    delimiter: "#4ade80",
    operator: "#6ee7b7",
    preprocessor: "#a7f3d0",
    function: "#2dd4bf",
    global: "#5eead4",
    system_variable: "#84cc16",
    class: "#7dd3fc",
    method: "#99f6e4",
    attribute: "#67e8f9",
    member: "#bef264",
    routine: "#a7f3d0",
    extrinsic: "#7dd3fc",
};

/// The same hues darkened until they read on paper white.
const LIGHT_SYNTAX: SyntaxPalette = SyntaxPalette {
    label: "#0f7b33",
    command: "#a1157a",
    string: "#7a6300",
    number: "#a4262c",
    delimiter: "#0a6e8a",
    operator: "#b02a6b",
    preprocessor: "#a1490b",
    function: "#6f26b5",
    global: "#c02012",
    system_variable: "#7a6a00",
    class: "#3a3ac0",
    method: "#0e7490",
    attribute: "#2549c7",
    member: "#b31f4a",
    routine: "#6d3fb5",
    extrinsic: "#2f5fa8",
};

/// Fills the syntax fields of a theme file from a palette, leaving everything
/// else at its default so the caller can write only what it cares about.
fn with_syntax(palette: SyntaxPalette) -> ThemeFile {
    ThemeFile {
        syntax_global: palette.global.into(),
        syntax_string: palette.string.into(),
        syntax_label: palette.label.into(),
        syntax_command: palette.command.into(),
        syntax_number: palette.number.into(),
        syntax_delimiter: palette.delimiter.into(),
        syntax_operator: palette.operator.into(),
        syntax_preprocessor: palette.preprocessor.into(),
        syntax_function: palette.function.into(),
        syntax_system_variable: palette.system_variable.into(),
        syntax_class: palette.class.into(),
        syntax_method: palette.method.into(),
        syntax_attribute: palette.attribute.into(),
        syntax_member: palette.member.into(),
        syntax_routine: palette.routine.into(),
        syntax_extrinsic: palette.extrinsic.into(),
        ..ThemeFile::default()
    }
}

/// One syntax colour: what the theme says, or the palette default when it says
/// nothing (or something unparseable).
fn syntax(hex: &str, fallback: &'static str) -> Color32 {
    parse_hex(hex).unwrap_or_else(|| c(fallback))
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
    /// ObjectScript colours for the terminal, one per token kind the scanner
    /// knows about ([`crate::term::syntax::Kind`]). Named after the semantic
    /// token scopes of the InterSystems VS Code extension, so an editor colour
    /// customisation can be copied across field by field.
    ///
    /// Every one of them is optional: a theme file written before they existed,
    /// or one that only cares about a few, falls back to [`DEFAULT_SYNTAX`] for
    /// the rest rather than losing the highlighting.
    #[serde(default)]
    pub syntax_global: String,
    #[serde(default)]
    pub syntax_string: String,
    #[serde(default)]
    pub syntax_label: String,
    #[serde(default)]
    pub syntax_command: String,
    #[serde(default)]
    pub syntax_number: String,
    #[serde(default)]
    pub syntax_delimiter: String,
    #[serde(default)]
    pub syntax_operator: String,
    #[serde(default)]
    pub syntax_preprocessor: String,
    #[serde(default)]
    pub syntax_function: String,
    #[serde(default)]
    pub syntax_system_variable: String,
    #[serde(default)]
    pub syntax_class: String,
    #[serde(default)]
    pub syntax_method: String,
    #[serde(default)]
    pub syntax_attribute: String,
    #[serde(default)]
    pub syntax_member: String,
    #[serde(default)]
    pub syntax_routine: String,
    #[serde(default)]
    pub syntax_extrinsic: String,
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

/// Everything empty and nothing claimed. Only useful as the base of a struct
/// update — [`with_syntax`] and the tests — never as a theme in its own right.
impl Default for ThemeFile {
    fn default() -> Self {
        ThemeFile {
            name: String::new(),
            background: String::new(),
            foreground: String::new(),
            cursor: String::new(),
            selection: String::new(),
            ansi: Vec::new(),
            ui_foreground: String::new(),
            ui_background: String::new(),
            syntax_global: String::new(),
            syntax_string: String::new(),
            syntax_label: String::new(),
            syntax_command: String::new(),
            syntax_number: String::new(),
            syntax_delimiter: String::new(),
            syntax_operator: String::new(),
            syntax_preprocessor: String::new(),
            syntax_function: String::new(),
            syntax_system_variable: String::new(),
            syntax_class: String::new(),
            syntax_method: String::new(),
            syntax_attribute: String::new(),
            syntax_member: String::new(),
            syntax_routine: String::new(),
            syntax_extrinsic: String::new(),
            font_family: default_font_family(),
            font_size: default_font_size(),
            dark: true,
            builtin: false,
        }
    }
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
    pub syntax_label: Color32,
    pub syntax_command: Color32,
    pub syntax_number: Color32,
    pub syntax_delimiter: Color32,
    pub syntax_operator: Color32,
    pub syntax_preprocessor: Color32,
    pub syntax_function: Color32,
    pub syntax_system_variable: Color32,
    pub syntax_class: Color32,
    pub syntax_method: Color32,
    pub syntax_attribute: Color32,
    pub syntax_member: Color32,
    pub syntax_routine: Color32,
    pub syntax_extrinsic: Color32,
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
            // Every syntax colour falls back to the ObjectScript palette
            // rather than to a terminal colour: a theme that says nothing about
            // syntax still highlights, and the fallback is a colour chosen for
            // the language rather than whichever ANSI slot was closest.
            syntax_global: syntax(&file.syntax_global, DEFAULT_SYNTAX.global),
            syntax_string: syntax(&file.syntax_string, DEFAULT_SYNTAX.string),
            syntax_label: syntax(&file.syntax_label, DEFAULT_SYNTAX.label),
            syntax_command: syntax(&file.syntax_command, DEFAULT_SYNTAX.command),
            syntax_number: syntax(&file.syntax_number, DEFAULT_SYNTAX.number),
            syntax_delimiter: syntax(&file.syntax_delimiter, DEFAULT_SYNTAX.delimiter),
            syntax_operator: syntax(&file.syntax_operator, DEFAULT_SYNTAX.operator),
            syntax_preprocessor: syntax(&file.syntax_preprocessor, DEFAULT_SYNTAX.preprocessor),
            syntax_function: syntax(&file.syntax_function, DEFAULT_SYNTAX.function),
            syntax_system_variable: syntax(
                &file.syntax_system_variable,
                DEFAULT_SYNTAX.system_variable,
            ),
            syntax_class: syntax(&file.syntax_class, DEFAULT_SYNTAX.class),
            syntax_method: syntax(&file.syntax_method, DEFAULT_SYNTAX.method),
            syntax_attribute: syntax(&file.syntax_attribute, DEFAULT_SYNTAX.attribute),
            syntax_member: syntax(&file.syntax_member, DEFAULT_SYNTAX.member),
            syntax_routine: syntax(&file.syntax_routine, DEFAULT_SYNTAX.routine),
            syntax_extrinsic: syntax(&file.syntax_extrinsic, DEFAULT_SYNTAX.extrinsic),
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
            syntax_label: to_hex(self.syntax_label),
            syntax_command: to_hex(self.syntax_command),
            syntax_number: to_hex(self.syntax_number),
            syntax_delimiter: to_hex(self.syntax_delimiter),
            syntax_operator: to_hex(self.syntax_operator),
            syntax_preprocessor: to_hex(self.syntax_preprocessor),
            syntax_function: to_hex(self.syntax_function),
            syntax_system_variable: to_hex(self.syntax_system_variable),
            syntax_class: to_hex(self.syntax_class),
            syntax_method: to_hex(self.syntax_method),
            syntax_attribute: to_hex(self.syntax_attribute),
            syntax_member: to_hex(self.syntax_member),
            syntax_routine: to_hex(self.syntax_routine),
            syntax_extrinsic: to_hex(self.syntax_extrinsic),
            font_family: self.font_family.clone(),
            font_size: self.font_size,
            dark: self.dark,
            // Anything serialised back out of a runtime theme is the user's
            // copy, never one we may overwrite on the next run.
            builtin: false,
        }
    }

    /// Colour for one scanned token kind. The single place the scanner's kinds
    /// meet a theme, so adding a kind is a compile error here rather than a
    /// token that silently comes out in the foreground colour.
    pub fn syntax_color(&self, kind: crate::term::syntax::Kind) -> Color32 {
        use crate::term::syntax::Kind;
        match kind {
            Kind::Label => self.syntax_label,
            Kind::Command => self.syntax_command,
            Kind::Str => self.syntax_string,
            Kind::Number => self.syntax_number,
            Kind::Delimiter => self.syntax_delimiter,
            Kind::Operator => self.syntax_operator,
            Kind::PreProcessor => self.syntax_preprocessor,
            Kind::Function => self.syntax_function,
            Kind::Global => self.syntax_global,
            Kind::SystemVariable => self.syntax_system_variable,
            Kind::ObjectClass => self.syntax_class,
            Kind::ObjectMethod => self.syntax_method,
            Kind::ObjectAttribute => self.syntax_attribute,
            Kind::ObjectMember => self.syntax_member,
            Kind::Routine => self.syntax_routine,
            Kind::Extrinsic => self.syntax_extrinsic,
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
/// and a full ObjectScript palette for the globals, strings, macros and class
/// references the ERP output is full of.
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
            font_family: default_font_family(),
            font_size: 14.0,
            dark: true,
            builtin: true,
            ..with_syntax(DEFAULT_SYNTAX)
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
            font_family: default_font_family(),
            font_size: 14.0,
            dark: true,
            builtin: true,
            ..with_syntax(GREEN_SYNTAX)
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
            font_family: default_font_family(),
            font_size: 14.0,
            dark: true,
            builtin: true,
            ..with_syntax(DEFAULT_SYNTAX)
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
            font_family: default_font_family(),
            font_size: 14.0,
            dark: false,
            builtin: true,
            ..with_syntax(LIGHT_SYNTAX)
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
            ..ThemeFile::default()
        };

        let theme = Theme::from_file(&file);
        assert_eq!(theme.ui_background, theme.background);
        assert_eq!(theme.ui_foreground, theme.foreground);
        // An unspecified syntax colour comes from the ObjectScript palette
        // rather than landing on the background and vanishing.
        assert_eq!(
            theme.syntax_global,
            parse_hex(DEFAULT_SYNTAX.global).unwrap()
        );
        assert_eq!(
            theme.syntax_string,
            parse_hex(DEFAULT_SYNTAX.string).unwrap()
        );
        assert_eq!(
            theme.syntax_command,
            parse_hex(DEFAULT_SYNTAX.command).unwrap()
        );
    }

    /// Every kind the scanner can produce has to resolve to a colour, and none
    /// of them may come out as the "this hex is broken" magenta.
    #[test]
    fn every_token_kind_has_a_colour_in_every_builtin() {
        use crate::term::syntax::Kind;
        const KINDS: [Kind; 16] = [
            Kind::Label,
            Kind::Command,
            Kind::Str,
            Kind::Number,
            Kind::Delimiter,
            Kind::Operator,
            Kind::PreProcessor,
            Kind::Function,
            Kind::Global,
            Kind::SystemVariable,
            Kind::ObjectClass,
            Kind::ObjectMethod,
            Kind::ObjectAttribute,
            Kind::ObjectMember,
            Kind::Routine,
            Kind::Extrinsic,
        ];
        let broken = Color32::from_rgb(255, 0, 255);
        for file in builtin_files() {
            let theme = Theme::from_file(&file);
            for kind in KINDS {
                assert_ne!(
                    theme.syntax_color(kind),
                    broken,
                    "{} has no colour for {kind:?}",
                    file.name
                );
                assert_ne!(
                    theme.syntax_color(kind),
                    theme.background,
                    "{} draws {kind:?} in the background colour",
                    file.name
                );
            }
        }
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
        assert_eq!(back.syntax_command, theme.syntax_command);
        assert_eq!(back.syntax_class, theme.syntax_class);
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
