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

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: String,
    pub font_size: f32,
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
    pub enable_plugins: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            theme: "IRIS Dark".into(),
            font_size: 14.0,
            scrollback_limit: 10_000,
            log_dir: default_log_dir(),
            default_log_mode: LogMode::Off,
            log_retention_days: 30,
            org_macros_path: PathBuf::new(),
            profiles: Vec::new(),
            default_profile: String::new(),
            open_on_start: true,
            confirm_close_with_live_session: true,
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

/// Creates the config tree and writes the built-in themes on first run.
pub fn ensure_config_tree() -> Result<()> {
    std::fs::create_dir_all(config_dir())?;
    std::fs::create_dir_all(themes_dir())?;
    std::fs::create_dir_all(plugins_dir())?;

    for file in theme::builtin_files() {
        let path = themes_dir().join(format!("{}.toml", slugify(&file.name)));
        // Never overwrite: the user may have edited a built-in.
        if path.exists() {
            continue;
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
