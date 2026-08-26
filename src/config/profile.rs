//! Connection profiles — the unit every other feature hangs off.
//!
//! A profile names an instance and namespace, and carries the per-connection
//! switches for autologon, logging, and macros. Passwords are deliberately
//! absent: they live in the OS credential store, keyed by profile name.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// Service name under which passwords are stored in the OS keychain.
pub const KEYRING_SERVICE: &str = "newIrisTerminal";

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Profile {
    pub name: String,
    pub instance: String,
    #[serde(default)]
    pub namespace: String,
    /// Username for autologon. Empty disables it regardless of `autologon`.
    #[serde(default)]
    pub username: String,
    #[serde(default)]
    pub autologon: bool,
    /// Commands sent once the prompt appears (namespace switches, ZN, etc.).
    #[serde(default)]
    pub post_login: Vec<String>,
    /// Routine to run instead of the interactive prompt.
    #[serde(default)]
    pub routine: Option<String>,
    /// Non-standard install path, when discovery cannot find the binary.
    #[serde(default)]
    pub binary_override: Option<PathBuf>,
    /// Character set IRIS speaks on this instance. Wrong values show up as
    /// mangled accented characters, not as an error, so it is per-profile.
    #[serde(default)]
    pub encoding: crate::term::Encoding,
    #[serde(default)]
    pub logging: LogMode,
    /// Extra macro file for this profile, on top of the global one.
    #[serde(default)]
    pub macro_file: Option<PathBuf>,
}

impl Default for Profile {
    fn default() -> Self {
        Profile {
            name: "Default".into(),
            instance: String::new(),
            namespace: "USER".into(),
            username: String::new(),
            autologon: false,
            post_login: Vec::new(),
            routine: None,
            binary_override: None,
            encoding: crate::term::Encoding::default(),
            logging: LogMode::Off,
            macro_file: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogMode {
    #[default]
    Off,
    /// Rendered text lines as they scroll away — human readable.
    Clean,
    /// Raw bytes including escape sequences — replayable.
    Raw,
}

impl Profile {
    /// Whether autologon has everything it needs to run unattended.
    pub fn autologon_ready(&self) -> bool {
        self.autologon && !self.username.is_empty()
    }

    pub fn launch_spec(&self) -> crate::pty::launcher::LaunchSpec {
        crate::pty::launcher::LaunchSpec {
            instance: self.instance.clone(),
            namespace: if self.namespace.is_empty() {
                None
            } else {
                Some(self.namespace.clone())
            },
            routine: self.routine.clone(),
            binary_override: self.binary_override.clone(),
        }
    }

    /// Reads the stored password. A missing entry is not an error — it just
    /// means autologon will stop at the password prompt and hand over.
    pub fn password(&self) -> Option<String> {
        keyring::Entry::new(KEYRING_SERVICE, &self.name)
            .ok()?
            .get_password()
            .ok()
    }

    pub fn set_password(&self, password: &str) -> anyhow::Result<()> {
        let entry = keyring::Entry::new(KEYRING_SERVICE, &self.name)?;
        if password.is_empty() {
            // Treat an empty value as "forget this", so clearing the field in
            // the UI actually removes the secret rather than storing "".
            let _ = entry.delete_password();
            Ok(())
        } else {
            entry.set_password(password)?;
            Ok(())
        }
    }

    pub fn clear_password(&self) {
        if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, &self.name) {
            let _ = entry.delete_password();
        }
    }
}
