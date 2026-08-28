//! Settings, profiles, and where they live on each platform.

pub mod profile;
pub mod theme;

use std::path::PathBuf;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub use profile::{LogMode, Profile};
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
    pub enable_plugins: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: "IRIS Dark".into(),
            font_size: 14.0,
            show_scrollbars: true,
            font_family: String::new(),
            cursor_style: CursorStyle::default(),
            cursor_blink: false,
            terminal_syntax_highlight: true,
            wrap_lines: true,
            scrollback_limit: 10_000,
            log_dir: default_log_dir(),
            default_log_mode: LogMode::Off,
            log_retention_days: 30,
            org_macros_path: PathBuf::new(),
            profiles: Vec::new(),
            default_profile: String::new(),
            open_on_start: true,
            confirm_close_with_live_session: true,
            native_decorations: false,
            enable_plugins: false,
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

    /// Effective log mode for a profile, applying the global default.
    pub fn log_mode_for(&self, profile: &Profile) -> LogMode {
        if profile.logging == LogMode::Off {
            self.default_log_mode
        } else {
            profile.logging
        }
    }
}

/// Who owns a theme file that is already on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owner {
    /// Marked `builtin = true` — a copy we wrote and nobody has claimed.
    Ours,
    /// No `builtin` key at all, so it predates the marker. Every install that
    /// ran an earlier version has one of these.
    Unmarked,
    /// The user's, either by `builtin = false` or because it will not parse.
    /// Left strictly alone.
    User,
}

fn owner_of(path: &std::path::Path) -> Owner {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Owner::User;
    };
    // Parsed as a bare table rather than a `ThemeFile`, so a file that is
    // missing a required colour is still classified rather than treated as
    // unreadable.
    let Ok(table) = toml::from_str::<toml::Table>(&text) else {
        return Owner::User;
    };
    match table.get("builtin") {
        Some(value) => {
            if value.as_bool().unwrap_or(false) {
                Owner::Ours
            } else {
                Owner::User
            }
        }
        None => Owner::Unmarked,
    }
}

/// Creates the config tree and refreshes the built-in theme files.
///
/// The refresh is the point: [`load_themes`] lets a file in the themes folder
/// replace the built-in of the same name, so a copy written by an earlier
/// version masks every later correction to that theme. Files we still own are
/// rewritten; a file predating the `builtin` marker is rewritten too, but only
/// after being kept as `<name>.toml.bak` so an edit is never destroyed.
/// Clearing `builtin` (or renaming the theme) claims the file for good.
pub fn ensure_config_tree() -> Result<()> {
    std::fs::create_dir_all(config_dir())?;
    std::fs::create_dir_all(themes_dir())?;
    std::fs::create_dir_all(plugins_dir())?;

    for file in theme::builtin_files() {
        let path = themes_dir().join(format!("{}.toml", slugify(&file.name)));
        if path.exists() {
            match owner_of(&path) {
                Owner::User => continue,
                Owner::Ours => {}
                Owner::Unmarked => {
                    // One-time migration. `.bak` is not `.toml`, so the copy is
                    // invisible to `load_themes` and cannot shadow anything.
                    let backup = path.with_extension("toml.bak");
                    if !backup.exists() {
                        let _ = std::fs::copy(&path, &backup);
                    }
                }
            }
        }
        if let Ok(text) = toml::to_string_pretty(&file) {
            let _ = std::fs::write(&path, text);
        }
    }
    Ok(())
}

/// Loads every theme in the themes directory, always including the built-ins
/// so a deleted or broken file cannot leave the app with nothing to render.
pub fn load_themes() -> Vec<Theme> {
    let mut themes: Vec<Theme> = theme::builtin_files()
        .iter()
        .map(Theme::from_file)
        .collect();

    if let Ok(entries) = std::fs::read_dir(themes_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("toml") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            match toml::from_str::<ThemeFile>(&text) {
                Ok(file) => {
                    let loaded = Theme::from_file(&file);
                    // A file that shares a built-in's name replaces it.
                    if let Some(slot) = themes.iter_mut().find(|t| t.name == loaded.name) {
                        *slot = loaded;
                    } else {
                        themes.push(loaded);
                    }
                }
                Err(e) => log::warn!("skipping theme {}: {e}", path.display()),
            }
        }
    }
    themes
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
    }

    #[test]
    fn theme_files_are_classified_by_who_owns_them() {
        let dir = std::env::temp_dir().join("nit-owner-test");
        let _ = std::fs::create_dir_all(&dir);

        let write = |name: &str, body: &str| {
            let path = dir.join(name);
            std::fs::write(&path, body).expect("write");
            path
        };

        let ours = write(
            "ours.toml",
            "name = \"X\"
builtin = true
",
        );
        assert_eq!(owner_of(&ours), Owner::Ours);

        // What every install that ran an earlier version has on disk.
        let unmarked = write(
            "unmarked.toml",
            "name = \"X\"
background = \"#000\"
",
        );
        assert_eq!(owner_of(&unmarked), Owner::Unmarked);

        let claimed = write(
            "claimed.toml",
            "name = \"X\"
builtin = false
",
        );
        assert_eq!(owner_of(&claimed), Owner::User);

        // A half-written file is the user's business, not ours to replace.
        let broken = write(
            "broken.toml",
            "name = \"X\"
this is not toml",
        );
        assert_eq!(owner_of(&broken), Owner::User);

        assert_eq!(owner_of(&dir.join("absent.toml")), Owner::User);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The marker is what lets a corrected built-in reach a disk that already
    /// has a stale copy, so every built-in must carry it.
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
