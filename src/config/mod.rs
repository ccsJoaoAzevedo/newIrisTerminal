//! Settings, profiles, and where they live on each platform.

pub mod profile;
pub mod servers;
pub mod theme;

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub use profile::{LogMode, Profile};
pub use servers::{Server, ServerList};
pub use theme::{Theme, ThemeFile};

/// Config root. `dirs` resolves this to `%APPDATA%`, `~/.config`, or
/// `~/Library/Application Support` as appropriate — never hardcode a path.
pub fn config_dir() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("newIrisTerminal")
}

pub fn themes_dir() -> PathBuf {
    config_dir().join("themes")
}

pub fn plugins_dir() -> PathBuf {
    config_dir().join("plugins")
}

pub fn default_log_dir() -> PathBuf {
    dirs::data_dir()
        .unwrap_or_else(config_dir)
        .join("newIrisTerminal")
        .join("logs")
}

pub fn settings_path() -> PathBuf {
    config_dir().join("settings.toml")
}

/// The user's own macros. Editable from the app.
pub fn personal_macros_path() -> PathBuf {
    config_dir().join("macros.xml")
}

/// Commands typed at an IRIS prompt, one per line, oldest first.
pub fn command_history_path() -> PathBuf {
    config_dir().join("history.txt")
}

/// Shows `path` in the platform's file manager, creating it first if it is not
/// there yet.
///
/// Themes are TOML files edited by hand, so the useful thing the app can do for
/// them is put the user in front of the folder. The command is spawned rather
/// than waited on: `explorer` returns a non-zero status even when it worked,
/// and there is nothing to read back either way.
pub fn open_in_file_manager(path: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(path).with_context(|| format!("creating {}", path.display()))?;
    let program = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program)
        .arg(path)
        .spawn()
        .with_context(|| format!("opening {}", path.display()))?;
    Ok(())
}

/// Terminal geometry a window opens at when the last one is not being
/// restored, in character cells.
///
/// Cells rather than pixels because that is the size the user cares about: a
/// window of "100x30" holds the same amount of IRIS output whatever the font
/// size is set to. The pixel size that produces it is worked out once the
/// window has measured a character.
pub const DEFAULT_TERMINAL_COLS: u16 = 100;
pub const DEFAULT_TERMINAL_ROWS: u16 = 30;

/// The geometries a terminal is conventionally set to, offered as one click
/// each: 80 and 132 columns are the two widths a VT had, and 24 and 48 the two
/// heights the InterSystems terminal offers.
pub const COMMON_COLS: [u16; 3] = [80, 100, 132];
pub const COMMON_ROWS: [u16; 3] = [24, 30, 48];

/// Shape the terminal cursor is drawn as.
///
/// A setting rather than something the session controls: IRIS never emits
/// DECSCUSR, so there is nothing to honour and the choice is purely the
/// user's.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CursorStyle {
    /// Fills the cell and inverts the character under it.
    #[default]
    Block,
    /// A vertical line at the left edge of the cell.
    Bar,
    /// A horizontal line along the bottom of the cell.
    ///
    /// Aliased, because settings files written before the rename say
    /// `"underline"` and must keep meaning this.
    #[serde(alias = "underline")]
    Underscore,
}

impl CursorStyle {
    pub const ALL: [CursorStyle; 3] = [
        CursorStyle::Block,
        CursorStyle::Bar,
        CursorStyle::Underscore,
    ];

    pub fn label(self) -> &'static str {
        match self {
            CursorStyle::Block => "Block",
            CursorStyle::Bar => "Bar",
            CursorStyle::Underscore => "Underscore",
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Language the interface is drawn in. English unless asked otherwise: a
    /// guess from the system locale would change the language of an install
    /// that was happy, and the picker is one line into Settings.
    pub language: crate::i18n::Lang,
    pub theme: String,
    pub font_size: f32,
    /// Draw solid scrollbars instead of egui's floating ones, which are all but
    /// invisible until hovered. Reported as missing scrollbars often enough to
    /// be worth a switch of its own.
    pub show_scrollbars: bool,
    /// Monospace family the terminal draws with. Empty means egui's bundled
    /// font. Takes precedence over the active theme's `font_family`, which
    /// stays as that theme's suggestion.
    ///
    /// Only ever set from a name [`crate::ui::fonts::install`] has confirmed:
    /// egui panics when asked to measure a family it does not have.
    pub font_family: String,
    pub cursor_style: CursorStyle,
    pub cursor_blink: bool,
    /// Colour globals (`^ABC`) and quoted strings in the terminal. A heuristic
    /// over arbitrary output, so it is a switch; it never overrides a colour
    /// the remote side asked for.
    pub terminal_syntax_highlight: bool,
    /// Continue a long line on the next display row instead of clipping it at
    /// the window edge.
    ///
    /// Either way the whole line is received: IRIS truncates a `Write` at the
    /// device right margin rather than wrapping, so the terminal is always
    /// reported wider than the window and the window shows a view onto it. This
    /// only decides what happens to the part that does not fit — wrapped onto
    /// further rows, or reached by scrolling sideways.
    pub wrap_lines: bool,
    pub scrollback_limit: usize,
    /// Seconds a status message stays in the footer before it goes on its own.
    /// 0 keeps it until it is dismissed, which is what the app always did.
    pub status_timeout_secs: u32,
    /// Put a selection on the clipboard as soon as the mouse is released,
    /// without waiting for Ctrl+C — the way the native IrisTerm and PuTTY
    /// behave.
    pub copy_on_select: bool,
    /// Keep the commands typed at an IRIS prompt in a file, so Up recalls what
    /// was typed in earlier sessions and not only in this one.
    ///
    /// Recall itself is not optional; this decides only whether it outlives the
    /// session. Off also means nothing is written to disk.
    pub save_command_history: bool,
    pub log_dir: PathBuf,
    /// Applied to any profile whose own mode is `Off`.
    pub default_log_mode: LogMode,
    /// Delete logs older than this. 0 disables cleanup.
    pub log_retention_days: u32,
    /// Shared macro file supplied by the organisation. A UNC share, a mapped
    /// drive, or a local copy — anything readable. Empty means "none".
    ///
    /// Never written to: it is shared, so the app treats it as read-only and
    /// keeps personal edits in [`personal_macros_path`].
    pub org_macros_path: PathBuf,
    pub profiles: Vec<Profile>,
    /// Profile opened by Ctrl+T and at startup. Empty means "ask".
    pub default_profile: String,
    /// Open the default profile automatically at launch.
    pub open_on_start: bool,
    pub confirm_close_with_live_session: bool,
    /// Keep the operating system's title bar and window border.
    ///
    /// Off by default: the app draws its own, which is what puts the tab strip
    /// where the title bar would otherwise waste a row. An escape hatch for a
    /// window manager the custom chrome misbehaves under.
    pub native_decorations: bool,
    /// Draw the minimize / maximize / close controls in the app's own title
    /// bar. Off leaves the row to the tabs and the drag area: the window can
    /// still be moved, maximized by double-click, and closed with Ctrl+W or
    /// Alt+F4. Ignored while the system title bar is in use, which brings its
    /// own controls.
    pub show_window_buttons: bool,
    /// Reopen at the size the window was last closed at. Off opens every
    /// launch at [`Settings::default_geometry`].
    pub save_terminal_size: bool,
    /// Terminal size a window opens at when the last one is not being restored,
    /// in characters. The size anyone actually means by "how big is the
    /// terminal": 80x24 holds the same amount of IRIS output at any font size.
    pub default_cols: u16,
    pub default_rows: u16,
    /// Reopen where the window was last closed. Off centres it on the monitor.
    pub save_window_position: bool,
    /// Inner size of the window in egui points, as last closed. Recorded only
    /// while `save_terminal_size` is on, and ignored when it is off, so
    /// switching the setting back on restores what was there before rather
    /// than nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_size: Option<[f32; 2]>,
    /// Top-left of the window frame in egui points, as last closed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_position: Option<[f32; 2]>,
    /// Whether the window was maximized when it was last closed. Restored with
    /// the size, because the alternative is a window the size of the screen
    /// that the restore button cannot shrink.
    pub window_maximized: bool,
    pub enable_plugins: bool,
    /// Ask GitHub at startup whether a newer build has been released.
    ///
    /// One HTTPS request through the machine's own proxy, and nothing is
    /// downloaded or replaced without being asked for.
    pub check_for_updates: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            language: crate::i18n::Lang::default(),
            theme: "IRIS Dark".into(),
            font_size: 14.0,
            show_scrollbars: true,
            font_family: String::new(),
            cursor_style: CursorStyle::default(),
            cursor_blink: false,
            terminal_syntax_highlight: true,
            wrap_lines: true,
            scrollback_limit: 10_000,
            status_timeout_secs: 8,
            copy_on_select: true,
            save_command_history: true,
            log_dir: default_log_dir(),
            default_log_mode: LogMode::Off,
            log_retention_days: 30,
            org_macros_path: PathBuf::new(),
            profiles: Vec::new(),
            default_profile: String::new(),
            open_on_start: true,
            confirm_close_with_live_session: true,
            native_decorations: false,
            show_window_buttons: true,
            save_terminal_size: false,
            default_cols: DEFAULT_TERMINAL_COLS,
            default_rows: DEFAULT_TERMINAL_ROWS,
            save_window_position: false,
            window_size: None,
            window_position: None,
            window_maximized: false,
            enable_plugins: false,
            check_for_updates: true,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        let path = settings_path();
        match std::fs::read_to_string(&path) {
            Ok(text) => match toml::from_str::<Settings>(&text) {
                Ok(settings) => settings,
                Err(e) => {
                    // A malformed file must not stop the app from starting —
                    // fall back to defaults and say so.
                    log::error!("{} is invalid ({e}); using defaults", path.display());
                    Settings::default()
                }
            },
            Err(_) => Settings::default(),
        }
    }

    pub fn save(&self) -> Result<()> {
        let path = settings_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serialising settings")?;
        std::fs::write(&path, text).with_context(|| format!("writing {}", path.display()))?;
        Ok(())
    }

    pub fn profile(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.name == name)
    }

    /// The profile a new tab should use, falling back to the first defined one.
    pub fn startup_profile(&self) -> Option<&Profile> {
        self.profile(&self.default_profile)
            .or_else(|| self.profiles.first())
    }

    /// The organisation macro file, if one is configured.
    pub fn org_macros(&self) -> Option<&std::path::Path> {
        (!self.org_macros_path.as_os_str().is_empty()).then_some(self.org_macros_path.as_path())
    }

    /// Terminal size a new window opens at, in characters.
    ///
    /// Clamped rather than trusted: the numbers come from a settings file that
    /// can be edited by hand, and a zero-column terminal is not something the
    /// rest of the app is prepared for.
    pub fn default_geometry(&self) -> (u16, u16) {
        (
            self.default_cols.clamp(20, 500),
            self.default_rows.clamp(5, 200),
        )
    }

    /// Inner size the window should open at, if a saved one is to be restored.
    ///
    /// `None` means "no size to restore": the caller opens at
    /// [`DEFAULT_TERMINAL_COLS`]x[`DEFAULT_TERMINAL_ROWS`] cells instead.
    pub fn restored_window_size(&self) -> Option<[f32; 2]> {
        self.save_terminal_size
            .then_some(self.window_size)
            .flatten()
    }

    /// Position the window should open at, or `None` to centre it.
    pub fn restored_window_position(&self) -> Option<[f32; 2]> {
        self.save_window_position
            .then_some(self.window_position)
            .flatten()
            .filter(|[x, y]| x.is_finite() && y.is_finite())
    }

    /// Whether the window should open maximized, which only a restored size
    /// can ask for.
    pub fn restored_maximized(&self) -> bool {
        self.save_terminal_size && self.window_maximized
    }

    /// Effective log mode for a profile, applying the global default.
    pub fn log_mode_for(&self, profile: &Profile) -> LogMode {
        if profile.logging == LogMode::Off {
            self.default_log_mode
        } else {
            profile.logging
        }
    }
}

/// Creates the config tree.
///
/// The built-in themes used to be written out here as well, and a copy on disk
/// replaced the built-in of the same name - so a file written by an earlier
/// version masked every later correction to that theme, and there was no such
/// thing as an immutable built-in. They now live only in the binary; the themes
/// folder holds the user's own, which nothing here ever writes over.
pub fn ensure_config_tree() -> Result<()> {
    std::fs::create_dir_all(config_dir())?;
    std::fs::create_dir_all(themes_dir())?;
    std::fs::create_dir_all(plugins_dir())?;
    Ok(())
}

/// Loads the built-in themes and then every theme in the themes directory.
///
/// A built-in can no longer be replaced by a file of the same name: it is
/// immutable, and shadowing was how a stale copy used to mask a correction. A
/// copy this app wrote itself is dropped, since it holds nothing of the user's;
/// anything else that collides keeps its colours under a name of its own.
pub fn load_themes() -> Vec<Theme> {
    let mut themes: Vec<Theme> = theme::builtin_files()
        .iter()
        .map(|file| Theme::from_file(file).as_builtin())
        .collect();

    let Ok(entries) = std::fs::read_dir(themes_dir()) else {
        return themes;
    };
    // Sorted, so which of two files that want the same name gets renamed does
    // not depend on the order the filesystem happens to hand them back in.
    let mut paths: Vec<PathBuf> = entries.flatten().map(|entry| entry.path()).collect();
    paths.sort();

    for path in paths {
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }
        let Ok(text) = std::fs::read_to_string(&path) else {
            continue;
        };
        let file = match toml::from_str::<ThemeFile>(&text) {
            Ok(file) => file,
            Err(e) => {
                log::warn!("skipping theme {}: {e}", path.display());
                continue;
            }
        };
        let taken = themes.iter().any(|t| t.name == file.name);
        // A copy of a built-in that an earlier version wrote. Nothing of the
        // user's is in it, so it is dropped rather than kept as a duplicate.
        if taken && file.builtin {
            log::info!("ignoring stale built-in copy {}", path.display());
            continue;
        }
        let mut loaded = Theme::from_file(&file).at_path(path);
        if taken {
            loaded.name = unclaimed_name(&themes, &file.name);
        }
        themes.push(loaded);
    }
    themes
}

/// `<name> (custom)`, and then `(custom 2)`, until nothing else answers to it.
fn unclaimed_name(themes: &[Theme], name: &str) -> String {
    let taken = |candidate: &str| themes.iter().any(|t| t.name == candidate);
    let first = format!("{name} (custom)");
    if !taken(&first) {
        return first;
    }
    (2..)
        .map(|n| format!("{name} (custom {n})"))
        .find(|candidate| !taken(candidate))
        .unwrap_or(first)
}

/// Filename a theme is stored under, unique within `themes`.
///
/// Derived from the name so the folder stays readable by hand, but never
/// allowed to land on a file that is already there: two themes whose names
/// slugify the same way would otherwise overwrite each other.
pub fn theme_path_for(name: &str) -> PathBuf {
    let stem = {
        let slug = slugify(name);
        let trimmed = slug.trim_matches('-').to_string();
        if trimmed.is_empty() {
            "theme".to_string()
        } else {
            trimmed
        }
    };
    let dir = themes_dir();
    let first = dir.join(format!("{stem}.toml"));
    if !first.exists() {
        return first;
    }
    (2..)
        .map(|n| dir.join(format!("{stem}-{n}.toml")))
        .find(|path| !path.exists())
        .unwrap_or(first)
}

/// Writes a theme to its own file, which is by definition the user's.
pub fn save_theme(theme: &Theme, path: &std::path::Path) -> Result<()> {
    std::fs::create_dir_all(themes_dir())?;
    let text = toml::to_string_pretty(&theme.to_file()).context("serialising theme")?;
    std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

fn slugify(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_round_trip_through_toml() {
        let mut settings = Settings::default();
        settings.profiles.push(Profile {
            name: "CONSISTEM".into(),
            instance: "CONSISTEM".into(),
            namespace: "USER".into(),
            username: "dev".into(),
            autologon: true,
            ..Profile::default()
        });

        let text = toml::to_string_pretty(&settings).expect("serialise");
        let back: Settings = toml::from_str(&text).expect("deserialise");

        assert_eq!(back.profiles.len(), 1);
        assert_eq!(back.profiles[0].instance, "CONSISTEM");
        assert!(back.profiles[0].autologon);
    }

    #[test]
    fn missing_fields_fall_back_to_defaults() {
        // Old config files must keep loading as new fields are added.
        let settings: Settings = toml::from_str("theme = \"Light\"").expect("parse");
        assert_eq!(settings.theme, "Light");
        assert_eq!(settings.scrollback_limit, 10_000);
    }

    /// The three cases `ensure_config_tree` has to tell apart before it
    /// overwrites anything.
    #[test]
    fn cursor_style_round_trips_as_a_lowercase_name() {
        let settings = Settings {
            cursor_style: CursorStyle::Underscore,
            cursor_blink: true,
            ..Settings::default()
        };
        let text = toml::to_string_pretty(&settings).expect("serialise");
        assert!(
            text.contains("cursor_style = \"underscore\""),
            "unexpected form: {text}"
        );

        let back: Settings = toml::from_str(&text).expect("deserialise");
        assert_eq!(back.cursor_style, CursorStyle::Underscore);
        assert!(back.cursor_blink);
    }

    /// The variant used to be spelled `underline`, and a settings file written
    /// then has to keep meaning what it said.
    #[test]
    fn the_old_spelling_of_the_underscore_cursor_still_loads() {
        let settings: Settings = toml::from_str("cursor_style = \"underline\"").expect("parse");
        assert_eq!(settings.cursor_style, CursorStyle::Underscore);
    }

    /// Settings files predate the appearance fields, so every one of them has
    /// to have a default.
    #[test]
    fn an_old_settings_file_still_loads_with_appearance_defaults() {
        let settings: Settings = toml::from_str("theme = \"Tokyo\"").expect("parse");
        assert_eq!(settings.cursor_style, CursorStyle::Block);
        assert!(!settings.cursor_blink);
        assert!(settings.show_scrollbars);
        assert!(settings.font_family.is_empty());
        assert!(settings.copy_on_select);
        assert!(settings.save_command_history);
        assert!(settings.show_window_buttons);
        assert_eq!(settings.status_timeout_secs, 8);
        assert_eq!(
            settings.default_geometry(),
            (DEFAULT_TERMINAL_COLS, DEFAULT_TERMINAL_ROWS)
        );
    }

    /// The geometry is written down by hand as often as it is set in the
    /// dialog, and nothing downstream survives a terminal with no columns.
    #[test]
    fn a_hand_written_geometry_is_clamped() {
        let settings: Settings =
            toml::from_str("default_cols = 0\ndefault_rows = 60000").expect("parse");
        assert_eq!(settings.default_geometry(), (20, 200));
    }

    /// The window geometry is only ever restored through the switch that asks
    /// for it, so a stored value has to stay inert while its switch is off.
    #[test]
    fn window_geometry_is_only_restored_when_it_was_asked_for() {
        let stored = Settings {
            window_size: Some([1200.0, 800.0]),
            window_position: Some([120.0, 40.0]),
            window_maximized: true,
            ..Settings::default()
        };
        assert_eq!(stored.restored_window_size(), None);
        assert_eq!(stored.restored_window_position(), None);
        assert!(!stored.restored_maximized());

        let restoring = Settings {
            save_terminal_size: true,
            save_window_position: true,
            ..stored.clone()
        };
        assert_eq!(restoring.restored_window_size(), Some([1200.0, 800.0]));
        assert_eq!(restoring.restored_window_position(), Some([120.0, 40.0]));
        assert!(restoring.restored_maximized());

        // Switched on before anything has been recorded: the caller opens at
        // the default geometry rather than at nothing.
        let first_run = Settings {
            save_terminal_size: true,
            save_window_position: true,
            ..Settings::default()
        };
        assert_eq!(first_run.restored_window_size(), None);
        assert_eq!(first_run.restored_window_position(), None);
    }

    /// `toml` refuses to serialise a `None`, so the geometry fields have to be
    /// skipped rather than written - which the whole settings file depends on,
    /// since they are `None` until a window has been closed once.
    #[test]
    fn window_geometry_survives_the_settings_file() {
        let text = toml::to_string_pretty(&Settings::default()).expect("serialise defaults");
        assert!(!text.contains("window_size"), "unexpected form: {text}");

        let saved = Settings {
            save_terminal_size: true,
            window_size: Some([1024.5, 640.0]),
            window_position: Some([-8.0, 300.0]),
            ..Settings::default()
        };
        let back: Settings =
            toml::from_str(&toml::to_string_pretty(&saved).expect("serialise")).expect("parse");
        assert_eq!(back.window_size, Some([1024.5, 640.0]));
        assert_eq!(back.window_position, Some([-8.0, 300.0]));
        assert!(back.save_terminal_size);
        assert!(!back.save_window_position);
    }

    /// Every settings file on disk predates the window geometry, so its absence
    /// has to mean the default geometry and a centred window.
    #[test]
    fn an_old_settings_file_opens_at_the_default_geometry() {
        let settings: Settings = toml::from_str("theme = \"Tokyo\"").expect("parse");
        assert!(!settings.save_terminal_size);
        assert!(!settings.save_window_position);
        assert_eq!(settings.window_size, None);
        assert_eq!(settings.window_position, None);
        assert!(!settings.window_maximized);
    }

    /// The marker is what lets `load_themes` recognise a copy of a built-in
    /// that an earlier version left on disk, so every built-in must carry it.
    #[test]
    fn every_builtin_theme_is_marked_as_one() {
        for file in theme::builtin_files() {
            assert!(file.builtin, "{} is not marked builtin", file.name);
        }
    }

    /// Theme files predate the `builtin` field, so it must be optional.
    #[test]
    fn a_theme_file_without_the_builtin_key_still_loads() {
        let text = r##"
            name = "Hand written"
            background = "#000000"
            foreground = "#ffffff"
            cursor = "#ff0000"
            selection = "#0000ff"
            ansi = []
        "##;
        let file: ThemeFile = toml::from_str(text).expect("parse");
        assert!(!file.builtin);
        assert_eq!(file.font_family, "monospace");
    }

    #[test]
    fn a_profiles_own_log_mode_wins_over_the_global_default() {
        let settings = Settings {
            default_log_mode: LogMode::Clean,
            ..Settings::default()
        };

        let off = Profile::default();
        assert_eq!(settings.log_mode_for(&off), LogMode::Clean);

        let raw = Profile {
            logging: LogMode::Raw,
            ..Profile::default()
        };
        assert_eq!(settings.log_mode_for(&raw), LogMode::Raw);
    }
}
