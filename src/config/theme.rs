//! Colour and font definitions.
//!
//! The built-ins below live in the binary and are immutable: the theme manager
//! duplicates one to give the user something to edit. A duplicate is a plain
//! TOML file under `<config>/themes/`, and dropping a file in that folder by
//! hand makes it selectable just the same.

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

/// Aqua traffic lights, as Tiger drew them. Used for any slot an `aqua` theme
/// leaves unspecified, so a theme only has to ask for the style.
const AQUA_CLOSE: &str = "#ff6058";
const AQUA_MINIMIZE: &str = "#ffbd2e";
const AQUA_MAXIMIZE: &str = "#28ca42";

/// The blue of an Aqua scroll handle. Used when an `aqua` theme does not name
/// one, so a Tiger theme gets the capsule without having to describe it.
const AQUA_SCROLL: &str = "#4a90d9";

/// The same for Luna: the red of the XP close button, and the blue the other
/// two were tinted with. The painter shades each into a gradient, so these are
/// the mid-tone rather than either end of one.
const LUNA_CLOSE: &str = "#cf4a35";
const LUNA_BUTTON: &str = "#4b7fc4";

/// How the minimize / maximize / close controls are drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum WindowButtonStyle {
    /// Hand-stroked glyphs on a transparent square, filled on hover. What the
    /// app has always drawn, and what a theme that says nothing still gets.
    #[default]
    Stroke,
    /// Three filled circles, glyph only under the pointer.
    Aqua,
    /// Windows XP's Luna: rounded gradient tiles with the glyph always on.
    Luna,
}

impl WindowButtonStyle {
    pub const ALL: [WindowButtonStyle; 3] = [
        WindowButtonStyle::Stroke,
        WindowButtonStyle::Aqua,
        WindowButtonStyle::Luna,
    ];

    /// Empty, or anything unrecognised, means the stroked style: a theme file
    /// written before this existed has to keep looking the way it did.
    fn from_name(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "aqua" => WindowButtonStyle::Aqua,
            "luna" => WindowButtonStyle::Luna,
            _ => WindowButtonStyle::Stroke,
        }
    }

    /// The colour this style paints a control in when the theme names none.
    ///
    /// `None` for the stroked style, which fills nothing and leaves the glyph
    /// to the widget colours.
    pub fn default_colour(self, slot: WindowButtonSlot) -> Option<Color32> {
        let hex = match (self, slot) {
            (WindowButtonStyle::Stroke, _) => return None,
            (WindowButtonStyle::Aqua, WindowButtonSlot::Close) => AQUA_CLOSE,
            (WindowButtonStyle::Aqua, WindowButtonSlot::Minimize) => AQUA_MINIMIZE,
            (WindowButtonStyle::Aqua, WindowButtonSlot::Maximize) => AQUA_MAXIMIZE,
            (WindowButtonStyle::Luna, WindowButtonSlot::Close) => LUNA_CLOSE,
            (WindowButtonStyle::Luna, _) => LUNA_BUTTON,
        };
        parse_hex(hex)
    }

    pub fn name(self) -> &'static str {
        match self {
            WindowButtonStyle::Stroke => "stroke",
            WindowButtonStyle::Aqua => "aqua",
            WindowButtonStyle::Luna => "luna",
        }
    }
}

/// Which of the three controls a colour belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowButtonSlot {
    Close,
    Minimize,
    Maximize,
}

/// Resolved appearance of the three window controls.
///
/// Every colour is optional: `None` means "whatever the widget colours say",
/// which is exactly what the chrome did before a theme could speak about these,
/// so an old theme file is unchanged. The `aqua` style is the one exception -
/// it needs three fills to be three traffic lights at all, so it defaults them.
#[derive(Clone, Copy, Debug)]
pub struct WindowButtons {
    pub style: WindowButtonStyle,
    /// Draw them at the left-hand end of the title bar, as Aqua does.
    pub left: bool,
    /// Which of the three are drawn at all.
    ///
    /// A window that cannot be closed from its own title bar is a real choice
    /// some people make - Ctrl+W and Alt+F4 still work - and a terminal that
    /// nobody wants minimized is another. Hiding one takes it out of the row
    /// entirely rather than greying it out.
    pub show_close: bool,
    pub show_minimize: bool,
    pub show_maximize: bool,
    pub close: Option<Color32>,
    pub minimize: Option<Color32>,
    pub maximize: Option<Color32>,
    /// The glyph inside a button.
    pub icon: Option<Color32>,
    /// Fill behind the close glyph on hover. The one control with an
    /// irreversible effect, so it keeps a colour of its own.
    pub hover_close: Option<Color32>,
    /// The gear that opens Settings.
    ///
    /// Its own slot rather than borrowing one of the window controls': a gear
    /// painted in the minimize colour claims to be a window control, and on an
    /// `aqua` theme it would come out as a fourth traffic light. Unset leaves
    /// it following `icon`, which is where it was before there was a slot for
    /// it at all.
    pub settings: Option<Color32>,
    /// The `+` that opens a session.
    ///
    /// Not a window control either - it is the app's own button, at the other
    /// end of the row - and the two are the marks people actually aim at, so
    /// both are worth a theme being able to pick out.
    pub new_tab: Option<Color32>,
}

/// All three shown, stroked, on the right: what a theme that says nothing about
/// its window buttons gets.
impl Default for WindowButtons {
    fn default() -> Self {
        WindowButtons {
            style: WindowButtonStyle::default(),
            left: false,
            show_close: true,
            show_minimize: true,
            show_maximize: true,
            close: None,
            minimize: None,
            maximize: None,
            icon: None,
            hover_close: None,
            settings: None,
            new_tab: None,
        }
    }
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

/// Pinks and their neighbours, for the Hello Kitty theme: the ObjectScript
/// palette would fight a screen that is deliberately one hue, the same way it
/// does on the phosphor green.
const KITTY_SYNTAX: SyntaxPalette = SyntaxPalette {
    label: "#0f7b5c",
    command: "#c2185b",
    string: "#9c6b00",
    number: "#c2410c",
    delimiter: "#7b3fa0",
    operator: "#d81b60",
    preprocessor: "#b4551d",
    function: "#8e24aa",
    global: "#d1104a",
    system_variable: "#8d6e00",
    class: "#5b4bc4",
    method: "#0e7490",
    attribute: "#4055c8",
    member: "#c2185b",
    routine: "#7b3fa0",
    extrinsic: "#3f6fb5",
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
    /// How the minimize / maximize / close controls are drawn: `"stroke"` (the
    /// default, and what every theme written before this got) or `"aqua"`.
    #[serde(default)]
    pub window_button_style: String,
    /// Put the controls at the left-hand end of the title bar.
    #[serde(default)]
    pub window_buttons_left: bool,
    /// Which controls the title bar has. All three unless a theme says
    /// otherwise, which is what every theme written before this said.
    #[serde(default = "yes")]
    pub window_button_show_close: bool,
    #[serde(default = "yes")]
    pub window_button_show_minimize: bool,
    #[serde(default = "yes")]
    pub window_button_show_maximize: bool,
    /// Colours for the three controls. Empty leaves one to the widget colours,
    /// except under `aqua`, where an unset slot becomes its traffic light.
    #[serde(default)]
    pub window_button_close: String,
    #[serde(default)]
    pub window_button_minimize: String,
    #[serde(default)]
    pub window_button_maximize: String,
    #[serde(default)]
    pub window_button_icon: String,
    #[serde(default)]
    pub window_button_hover_close: String,
    /// The gear and the `+`. Empty leaves both following `window_button_icon`.
    #[serde(default)]
    pub settings_icon: String,
    #[serde(default)]
    pub new_tab_icon: String,
    /// The terminal's scroll handle. Empty leaves it derived from the selection
    /// and foreground colours, which is what every theme did before this - and
    /// what still happens under the stroked button style, where there is no
    /// period look to match.
    #[serde(default)]
    pub scrollbar_handle: String,
    #[serde(default = "default_font_family")]
    pub font_family: String,
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    /// Drives egui's own widget colours so the chrome matches the terminal.
    #[serde(default)]
    pub dark: bool,
    /// Marks a file as a copy of a built-in written by an earlier version of the
    /// app, which [`crate::config::load_themes`] drops in favour of the built-in
    /// itself. Themes the app writes now are always the user's, so it writes
    /// `false`; a built-in is one because it is in the binary, never because a
    /// file said so.
    #[serde(default)]
    pub builtin: bool,
}

fn default_font_family() -> String {
    "monospace".to_string()
}

/// `true`, for the fields whose absence has to mean "yes" rather than "no".
fn yes() -> bool {
    true
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
            window_button_style: String::new(),
            window_buttons_left: false,
            window_button_show_close: true,
            window_button_show_minimize: true,
            window_button_show_maximize: true,
            window_button_close: String::new(),
            window_button_minimize: String::new(),
            window_button_maximize: String::new(),
            window_button_icon: String::new(),
            window_button_hover_close: String::new(),
            settings_icon: String::new(),
            new_tab_icon: String::new(),
            scrollbar_handle: String::new(),
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

/// A colour the theme did not set stays unset, rather than becoming a hex
/// string that pins down whatever the widget colours happened to be.
fn hex_or_empty(color: Option<Color32>) -> String {
    color.map(to_hex).unwrap_or_default()
}

/// Resolves the window-control appearance from a theme file.
fn window_buttons(file: &ThemeFile) -> WindowButtons {
    let style = WindowButtonStyle::from_name(&file.window_button_style);
    // Under `aqua` and `luna` a missing fill is a missing button, so each
    // style supplies its own; the stroked style has nothing to fill and leaves
    // the slot to the widget colours.
    let fill = |hex: &str, slot: WindowButtonSlot| match parse_hex(hex) {
        Some(color) => Some(color),
        None => style.default_colour(slot),
    };
    WindowButtons {
        style,
        left: file.window_buttons_left,
        show_close: file.window_button_show_close,
        show_minimize: file.window_button_show_minimize,
        show_maximize: file.window_button_show_maximize,
        close: fill(&file.window_button_close, WindowButtonSlot::Close),
        minimize: fill(&file.window_button_minimize, WindowButtonSlot::Minimize),
        maximize: fill(&file.window_button_maximize, WindowButtonSlot::Maximize),
        icon: parse_hex(&file.window_button_icon),
        hover_close: parse_hex(&file.window_button_hover_close),
        settings: parse_hex(&file.settings_icon),
        new_tab: parse_hex(&file.new_tab_icon),
    }
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
    pub window_buttons: WindowButtons,
    /// Colour of the terminal's scroll handle, when the theme names one or its
    /// button style implies one.
    pub scrollbar_handle: Option<Color32>,
    pub font_family: String,
    pub font_size: f32,
    pub dark: bool,
    /// Shipped with the app, so the theme manager will not let it be edited or
    /// deleted. Never read from the file: a built-in is one because it came out
    /// of [`builtin_files`], not because a file claimed to be one.
    pub builtin: bool,
    /// The file this theme was read from, when it came from one. What the theme
    /// manager writes an edit back to, and what deleting it removes.
    pub path: Option<std::path::PathBuf>,
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
            window_buttons: window_buttons(file),
            scrollbar_handle: parse_hex(&file.scrollbar_handle).or_else(|| {
                // An Aqua theme gets the blue capsule for free: the scroll bar
                // is as much a part of that look as the traffic lights are.
                matches!(
                    WindowButtonStyle::from_name(&file.window_button_style),
                    WindowButtonStyle::Aqua
                )
                .then(|| parse_hex(AQUA_SCROLL))
                .flatten()
            }),
            font_family: file.font_family.clone(),
            font_size: file.font_size.clamp(6.0, 48.0),
            dark: file.dark,
            builtin: false,
            path: None,
        }
    }

    /// Marks this theme as one of the app's own, which the theme manager holds
    /// immutable.
    pub fn as_builtin(mut self) -> Self {
        self.builtin = true;
        self
    }

    /// Records where the theme was loaded from.
    pub fn at_path(mut self, path: std::path::PathBuf) -> Self {
        self.path = Some(path);
        self
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
            window_button_style: self.window_buttons.style.name().to_string(),
            window_buttons_left: self.window_buttons.left,
            window_button_show_close: self.window_buttons.show_close,
            window_button_show_minimize: self.window_buttons.show_minimize,
            window_button_show_maximize: self.window_buttons.show_maximize,
            window_button_close: hex_or_empty(self.window_buttons.close),
            window_button_minimize: hex_or_empty(self.window_buttons.minimize),
            window_button_maximize: hex_or_empty(self.window_buttons.maximize),
            window_button_icon: hex_or_empty(self.window_buttons.icon),
            window_button_hover_close: hex_or_empty(self.window_buttons.hover_close),
            settings_icon: hex_or_empty(self.window_buttons.settings),
            new_tab_icon: hex_or_empty(self.window_buttons.new_tab),
            scrollbar_handle: hex_or_empty(self.scrollbar_handle),
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

        // Everything egui fills - a button, a combo box, and above all a
        // scrollbar's handle - comes from these, and leaving them at the stock
        // dark/light grey is what made the scrollbars read as belonging to some
        // other application. Each state is the chrome background lifted a
        // little further towards the chrome text, so the ladder from resting to
        // pressed holds in a light theme and a dark one alike.
        let lift = |t: f32| crate::term::palette::blend(self.ui_background, self.ui_foreground, t);
        v.widgets.noninteractive.bg_fill = self.ui_background;
        v.widgets.noninteractive.weak_bg_fill = self.ui_background;
        v.widgets.noninteractive.bg_stroke.color = lift(0.20);
        v.widgets.inactive.bg_fill = lift(0.18);
        v.widgets.inactive.weak_bg_fill = lift(0.10);
        v.widgets.hovered.bg_fill = lift(0.30);
        v.widgets.hovered.weak_bg_fill = lift(0.22);
        v.widgets.hovered.bg_stroke.color = lift(0.40);
        v.widgets.active.bg_fill = lift(0.42);
        v.widgets.active.weak_bg_fill = lift(0.34);
        v.widgets.active.bg_stroke.color = lift(0.55);
        v.widgets.open.bg_fill = lift(0.24);
        v.widgets.open.weak_bg_fill = lift(0.16);

        // And the strokes, which are what a scrollbar handle is actually drawn
        // in: egui paints it with `fg_stroke.color` unless the scroll style
        // asks for the fill, so theming only the fills left the bars in the
        // stock grey. These also carry the checkmarks and the fold arrows;
        // label text does not come through here, since `override_text_color`
        // has already claimed it.
        v.widgets.noninteractive.fg_stroke.color = self.ui_foreground;
        v.widgets.inactive.fg_stroke.color = lift(0.62);
        v.widgets.hovered.fg_stroke.color = lift(0.82);
        v.widgets.active.fg_stroke.color = self.ui_foreground;
        v.widgets.open.fg_stroke.color = lift(0.72);
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
        // A pink one, asked for by name. Light: paper white under a strawberry
        // chrome, with the syntax palette pulled towards the same hues.
        ThemeFile {
            name: "Hello Kitty".into(),
            background: "#fff5f8".into(),
            foreground: "#3d2b33".into(),
            cursor: "#e75480".into(),
            selection: "#ffc9dd".into(),
            ansi: ansi([
                "#3d2b33", "#e0245e", "#3f9e5a", "#c98a00", "#3f7fd0", "#b45fc4", "#2f9fa8",
                "#f2dfe6", "#7a6670", "#ff4d7d", "#4fc07a", "#e3ad2b", "#5aa0ea", "#d47ae0",
                "#4fc4cd", "#fffafc",
            ]),
            ui_foreground: "#5a2233".into(),
            ui_background: "#ffd7e6".into(),
            window_button_close: "#e75480".into(),
            window_button_minimize: "#ffb3c9".into(),
            window_button_maximize: "#ff8fb1".into(),
            scrollbar_handle: "#f57fa8".into(),
            font_family: default_font_family(),
            font_size: 14.0,
            dark: false,
            builtin: true,
            ..with_syntax(KITTY_SYNTAX)
        },
        // And after dark: the same pink over a near-black, where it reads as
        // neon rather than as sugar.
        ThemeFile {
            name: "Hello Kitty Dark".into(),
            background: "#17111a".into(),
            foreground: "#f6dbe6".into(),
            cursor: "#ff5c9e".into(),
            selection: "#5c2340".into(),
            ansi: ansi([
                "#17111a", "#ff4d7d", "#5ce6a1", "#ffd166", "#7aa2f7", "#e56ee5", "#67e8f9",
                "#f6dbe6", "#4a3b48", "#ff85ad", "#8ff0c0", "#ffe08a", "#a8c4ff", "#f4a4f4",
                "#a5f3fc", "#fff5f8",
            ]),
            ui_foreground: "#ffd7e6".into(),
            ui_background: "#241a28".into(),
            window_button_close: "#ff5c9e".into(),
            window_button_minimize: "#ffa8c8".into(),
            window_button_maximize: "#ff85ad".into(),
            scrollbar_handle: "#ff5c9e".into(),
            font_family: default_font_family(),
            font_size: 14.0,
            dark: true,
            builtin: true,
            ..with_syntax(DEFAULT_SYNTAX)
        },
        // Windows XP: Luna blue chrome around the console black-and-silver,
        // with the console's own sixteen colours.
        ThemeFile {
            name: "Windows XP".into(),
            background: "#000000".into(),
            foreground: "#c0c0c0".into(),
            cursor: "#c0c0c0".into(),
            // XP's selection blue.
            selection: "#316ac5".into(),
            ansi: ansi([
                "#000000", "#800000", "#008000", "#808000", "#000080", "#800080", "#008080",
                "#c0c0c0", "#808080", "#ff0000", "#00ff00", "#ffff00", "#0000ff", "#ff00ff",
                "#00ffff", "#ffffff",
            ]),
            ui_foreground: "#ffffff".into(),
            // The Luna title bar, which is what anyone naming this theme is
            // asking for.
            ui_background: "#0a62c8".into(),
            // The Luna tiles themselves: a red close and two blue ones, each
            // shaded into a gradient by the painter.
            window_button_style: "luna".into(),
            window_button_icon: "#ffffff".into(),
            font_family: default_font_family(),
            font_size: 14.0,
            dark: true,
            builtin: true,
            ..with_syntax(DEFAULT_SYNTAX)
        },
        // KDE 3's Plastik: grey-blue widgets around a white Konsole, with the
        // palette Konsole shipped as "Linux colors".
        ThemeFile {
            name: "KDE Plastik".into(),
            background: "#ffffff".into(),
            foreground: "#1a1a1a".into(),
            cursor: "#678db2".into(),
            selection: "#b5cde4".into(),
            ansi: ansi([
                "#000000", "#b21818", "#18b218", "#b26818", "#1818b2", "#b218b2", "#18b2b2",
                "#b2b2b2", "#686868", "#ff5454", "#54ff54", "#ffff54", "#5454ff", "#ff54ff",
                "#54ffff", "#ffffff",
            ]),
            ui_foreground: "#202020".into(),
            ui_background: "#efefef".into(),
            window_button_icon: "#303030".into(),
            window_button_hover_close: "#b04040".into(),
            font_family: default_font_family(),
            font_size: 14.0,
            dark: false,
            builtin: true,
            ..with_syntax(LIGHT_SYNTAX)
        },
        // Mac OS X 10.4. Aqua traffic lights on the left, the brushed-metal
        // grey the windows of the era were framed in, and the colours Terminal
        // itself shipped with for the grid.
        ThemeFile {
            name: "Tiger Aqua".into(),
            background: "#ffffff".into(),
            foreground: "#1a1a1a".into(),
            cursor: "#3a6ea5".into(),
            // Aqua's own highlight blue.
            selection: "#b4d5fe".into(),
            ansi: ansi([
                "#000000", "#c23621", "#25bc24", "#adad27", "#492ee1", "#d338d3", "#33bbc8",
                "#cbcccd", "#818383", "#fc391f", "#31e722", "#adad27", "#5833ff", "#f935f8",
                "#14f0f0", "#e9ebeb",
            ]),
            ui_foreground: "#2b2b2b".into(),
            ui_background: "#dcdcdc".into(),
            window_button_style: "aqua".into(),
            window_buttons_left: true,
            font_family: default_font_family(),
            font_size: 14.0,
            dark: false,
            builtin: true,
            ..with_syntax(LIGHT_SYNTAX)
        },
        // The same frame in the graphite appearance, over a grid borrowed from
        // Tokyo: the Aqua palette has no dark reading of its own, and Tokyo's
        // is the one this app already renders IRIS output in well.
        ThemeFile {
            name: "Tiger Graphite".into(),
            background: "#0f0e13".into(),
            foreground: "#c8d3d5".into(),
            cursor: "#34e2e2".into(),
            selection: "#2d636f".into(),
            ansi: ansi([
                "#0f0e13", "#fc5698", "#7fff00", "#fe8019", "#3465a4", "#2a2436", "#116d61",
                "#aceeee", "#999988", "#ff3b3b", "#a3ff8c", "#ffe61c", "#0285f9", "#8b5cf6",
                "#34e2e2", "#eceff4",
            ]),
            ui_foreground: "#c0caf5".into(),
            // Graphite, not brushed steel: the same neutral grey pulled down
            // until it frames a dark grid instead of a white one.
            ui_background: "#26262b".into(),
            window_button_style: "aqua".into(),
            window_buttons_left: true,
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

    /// A theme's window-button style has to survive being written out and read
    /// back, or editing any other colour in the manager would quietly reset the
    /// buttons to the stroked default.
    #[test]
    fn the_window_button_style_round_trips() {
        for (name, expected) in [
            ("stroke", WindowButtonStyle::Stroke),
            ("aqua", WindowButtonStyle::Aqua),
            ("luna", WindowButtonStyle::Luna),
            ("LUNA", WindowButtonStyle::Luna),
            ("", WindowButtonStyle::Stroke),
            ("nonsense", WindowButtonStyle::Stroke),
        ] {
            let file = ThemeFile {
                name: "T".into(),
                window_button_style: name.into(),
                ..ThemeFile::default()
            };
            let theme = Theme::from_file(&file);
            assert_eq!(theme.window_buttons.style, expected, "{name:?}");
            let back = Theme::from_file(&theme.to_file());
            assert_eq!(
                back.window_buttons.style, expected,
                "{name:?} did not survive"
            );
            assert_eq!(back.window_buttons.close, theme.window_buttons.close);
        }
    }

    /// Every style that fills its buttons has to supply a colour for all three,
    /// or one of them is painted in nothing at all.
    #[test]
    fn a_filled_style_has_three_colours() {
        for name in ["aqua", "luna"] {
            let file = ThemeFile {
                name: "T".into(),
                window_button_style: name.into(),
                ..ThemeFile::default()
            };
            let buttons = Theme::from_file(&file).window_buttons;
            assert!(buttons.close.is_some(), "{name}: no close colour");
            assert!(buttons.minimize.is_some(), "{name}: no minimize colour");
            assert!(buttons.maximize.is_some(), "{name}: no maximize colour");
        }
        // The stroked style fills nothing, and must not invent a colour that
        // would then be painted over the chrome.
        let bare = Theme::from_file(&ThemeFile {
            name: "T".into(),
            ..ThemeFile::default()
        });
        assert!(bare.window_buttons.close.is_none());
    }

    /// The gear and the `+` are the two marks a theme could not speak about
    /// before, so both the file round trip and the silence of an older file
    /// are worth pinning.
    #[test]
    fn the_gear_and_the_plus_round_trip_and_default_to_unset() {
        let mut theme = Theme::from_file(&ThemeFile {
            name: "T".into(),
            ..ThemeFile::default()
        });
        assert!(
            theme.window_buttons.settings.is_none(),
            "an old file is silent"
        );
        assert!(theme.window_buttons.new_tab.is_none());

        theme.window_buttons.settings = parse_hex("#3366cc");
        theme.window_buttons.new_tab = parse_hex("#22aa55");
        let back = Theme::from_file(&theme.to_file());
        assert_eq!(back.window_buttons.settings, theme.window_buttons.settings);
        assert_eq!(back.window_buttons.new_tab, theme.window_buttons.new_tab);
    }

    /// Hiding a button has to survive the file, and a theme file written
    /// before the flags existed has to keep all three.
    #[test]
    fn hidden_buttons_round_trip_and_default_to_shown() {
        // A theme file as they were written before the flags existed.
        let text = r##"
            name = "Old"
            background = "#101010"
            foreground = "#e0e0e0"
            cursor = "#ffffff"
            selection = "#003366"
            ansi = []
        "##;
        let old_file: ThemeFile = toml::from_str(text).expect("parse");
        let old = Theme::from_file(&old_file).window_buttons;
        assert!(old.show_close && old.show_minimize && old.show_maximize);

        let mut theme = Theme::from_file(&old_file);
        theme.window_buttons.show_maximize = false;
        let back = Theme::from_file(&theme.to_file()).window_buttons;
        assert!(back.show_close);
        assert!(back.show_minimize);
        assert!(!back.show_maximize, "the hidden one came back");
    }

    #[test]
    fn builtins_all_load_with_16_ansi_colours() {
        for file in builtin_files() {
            let theme = Theme::from_file(&file);
            assert_eq!(theme.ansi.len(), 16, "{}", file.name);
        }
    }
}
