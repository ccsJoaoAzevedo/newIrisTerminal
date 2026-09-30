//! The tabs that were open when the app last closed, for
//! [`super::Settings::remember_open_tabs`].
//!
//! A model of its own rather than serde on `Tab` or `Split`: those hold live
//! sessions, and the only part of a tab worth reopening is what it connects to
//! and what it was called. What it connects to is its [`Profile`], which holds
//! no password - that lives in the keychain - so nothing typed at a prompt, and
//! no secret, reaches this file.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::Profile;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedSession {
    /// Index into `tabs` of the tab that was in front.
    pub active: usize,
    pub tabs: Vec<SavedTab>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedTab {
    pub profile: Profile,
    // TOML has no way to write a `None`, so each of these is left out instead.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub split: Option<SavedSplit>,
    /// Whether the second pane had the keyboard. Meaningless without a split.
    pub second_focused: bool,
    /// The namespace the pane was last seen in, read off its prompt.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    /// What the pane showed when it closed, as plain text, oldest line first.
    /// Put back above the new session's output so the tab still says what was
    /// being done in it; the session behind it is a fresh one.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub screen: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedSplit {
    pub dir: SavedDir,
    pub ratio: f32,
    pub profile: Profile,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub screen: String,
}

/// Where the second pane sat. A mirror of the layout's own direction, so the
/// config tree does not reach into the app to describe a file.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SavedDir {
    #[default]
    Right,
    Bottom,
}

impl SavedSession {
    /// What `path` holds, or `None` when there is nothing usable there.
    ///
    /// A malformed file is logged and skipped rather than refused: it must not
    /// stop the app from starting, and the default profile is a fine thing to
    /// open instead.
    pub fn load(path: &Path) -> Option<SavedSession> {
        let text = std::fs::read_to_string(path).ok()?;
        match toml::from_str(&text) {
            Ok(session) => Some(session),
            Err(e) => {
                log::error!("{} is invalid ({e}); not restoring it", path.display());
                None
            }
        }
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .with_context(|| format!("creating {}", parent.display()))?;
        }
        let text = toml::to_string_pretty(self).context("serialising the session")?;
        std::fs::write(path, text).with_context(|| format!("writing {}", path.display()))
    }

    /// Forgets the saved session, so a switch turned off leaves nothing behind
    /// for it to find when it is turned back on months later.
    pub fn clear(path: &Path) {
        if let Err(e) = std::fs::remove_file(path) {
            if e.kind() != std::io::ErrorKind::NotFound {
                log::warn!("could not remove {}: {e}", path.display());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// A directory of this test's own, gone by the time it returns.
    fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nit-session-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create");
        dir
    }

    fn profile(name: &str) -> Profile {
        Profile {
            name: name.into(),
            instance: name.into(),
            ..Profile::default()
        }
    }

    #[test]
    fn a_saved_session_round_trips_with_and_without_a_split() {
        let dir = tempdir("round-trip");
        let path = dir.join("session.toml");
        let session = SavedSession {
            active: 1,
            tabs: vec![
                SavedTab {
                    profile: profile("ONE"),
                    ..SavedTab::default()
                },
                SavedTab {
                    profile: profile("TWO"),
                    title: Some("renamed".into()),
                    split: Some(SavedSplit {
                        dir: SavedDir::Bottom,
                        ratio: 0.3,
                        profile: profile("THREE"),
                        title: None,
                        screen: "COMP80>w 1\n1".into(),
                        namespace: Some("DESENV80".into()),
                    }),
                    second_focused: true,
                    namespace: None,
                    screen: String::new(),
                },
            ],
        };
        session.save(&path).expect("save");

        let back = SavedSession::load(&path).expect("load");
        assert_eq!(back.active, 1);
        assert_eq!(back.tabs.len(), 2);
        assert!(back.tabs[0].split.is_none() && back.tabs[0].title.is_none());
        assert_eq!(back.tabs[1].title.as_deref(), Some("renamed"));
        let split = back.tabs[1].split.as_ref().expect("split");
        assert_eq!(split.dir, SavedDir::Bottom);
        assert_eq!(split.ratio, 0.3);
        assert_eq!(split.profile.instance, "THREE");
        assert!(back.tabs[1].second_focused);
        assert_eq!(split.screen, "COMP80>w 1\n1");
        assert_eq!(split.namespace.as_deref(), Some("DESENV80"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_malformed_or_missing_session_file_reads_as_nothing() {
        let dir = tempdir("malformed");
        let path = dir.join("session.toml");
        assert!(SavedSession::load(&path).is_none());
        std::fs::write(&path, "tabs = 3").expect("write");
        assert!(SavedSession::load(&path).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn clearing_removes_the_file_and_tolerates_there_being_none() {
        let dir = tempdir("clear");
        let path = dir.join("session.toml");
        SavedSession::default().save(&path).expect("save");
        SavedSession::clear(&path);
        assert!(!path.exists());
        SavedSession::clear(&path);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
