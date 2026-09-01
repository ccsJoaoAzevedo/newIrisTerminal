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
    /// Remote server to log in to over Telnet instead of starting a local
    /// session. Set when the profile came from the launcher's Server Manager
    /// and the server is not on this machine; see
    /// [`crate::config::servers`].
    ///
    /// `instance` still carries the server's name, because that is what names
    /// the tab and what the user recognises. It is not an instance on this
    /// machine, so nothing may try to start it as one — which is exactly what
    /// this field being set means.
    #[serde(default)]
    pub remote: Option<Remote>,
}

/// Where a remote session connects to.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Remote {
    pub address: String,
    /// The instance's Telnet port, as recorded by the Server Manager.
    pub port: u16,
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
            remote: None,
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

    /// A profile that opens the given server, built from the launcher's own
    /// configuration.
    ///
    /// `base` carries the user's preferences — encoding, logging, macros — so
    /// picking a server from the menu changes *where* the session goes and
    /// nothing else. The name and instance both become the server's name: it is
    /// what the launcher calls it, and what the tab should say.
    pub fn for_server(
        server: &crate::config::servers::Server,
        instances: &[String],
        base: &Profile,
    ) -> Profile {
        use crate::config::servers::Target;
        let mut profile = Profile {
            name: server.name.clone(),
            instance: server.name.clone(),
            remote: None,
            // Autologon is deliberately not inherited. It is keyed by profile
            // name, and the name has just changed, so the stored password no
            // longer resolves — leaving the username behind would type one
            // server's account into another's login prompt and then stall.
            // A remote server's login is a different account anyway: often the
            // host's, not IRIS's.
            username: String::new(),
            autologon: false,
            ..base.clone()
        };
        match server.target(instances) {
            Target::Local { instance } => profile.instance = instance,
            Target::Telnet { address, port } => profile.remote = Some(Remote { address, port }),
        }
        profile
    }

    /// Where this profile's session goes, in one line, for a tooltip or the
    /// status bar.
    pub fn endpoint(&self) -> String {
        match self.remote.as_ref() {
            Some(remote) => format!("{}:{} (telnet)", remote.address, remote.port),
            None if self.instance.is_empty() => "no instance configured".to_string(),
            None => self.instance.clone(),
        }
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::servers::Server;

    fn instances() -> Vec<String> {
        vec!["CONSISTEM".to_string()]
    }

    /// A Telnet profile as the base, which is what the `+` menu holds once a
    /// remote server has been opened.
    fn telnet_base() -> Profile {
        Profile {
            name: "TESTES".into(),
            instance: "TESTES".into(),
            remote: Some(Remote {
                address: "10.0.0.102".into(),
                port: 23,
            }),
            encoding: crate::term::Encoding::Cp850,
            logging: LogMode::Clean,
            username: "someone".into(),
            autologon: true,
            ..Profile::default()
        }
    }

    /// The bug behind "the bottom `CONSISTEM` opened Telnet": a local target
    /// built from a remote base carried the base's `remote` across, so a local
    /// instance connected to the last server instead of starting.
    #[test]
    fn a_local_target_never_inherits_a_remote_from_its_base() {
        let profile = Profile::for_server(
            &Server::for_instance("CONSISTEM"),
            &instances(),
            &telnet_base(),
        );
        assert_eq!(profile.remote, None, "a local session must not be remote");
        assert_eq!(profile.instance, "CONSISTEM");
        assert_eq!(profile.name, "CONSISTEM");
    }

    /// The same in reverse: a remote target must not keep looking like the
    /// local instance the base named.
    #[test]
    fn a_remote_target_replaces_the_bases_local_instance() {
        let mut remote = Server::for_instance("TESTES");
        remote.address = "10.0.0.102".into();
        let base = Profile {
            name: "CONSISTEM".into(),
            instance: "CONSISTEM".into(),
            remote: None,
            ..Profile::default()
        };
        let profile = Profile::for_server(&remote, &instances(), &base);
        assert_eq!(
            profile.remote,
            Some(Remote {
                address: "10.0.0.102".into(),
                port: 23
            })
        );
        assert_eq!(profile.endpoint(), "10.0.0.102:23 (telnet)");
    }

    /// Preferences carry over — that is the point of taking a base at all —
    /// but the credentials do not, because they are keyed by the name that
    /// just changed.
    #[test]
    fn preferences_carry_over_but_credentials_do_not() {
        let profile = Profile::for_server(
            &Server::for_instance("CONSISTEM"),
            &instances(),
            &telnet_base(),
        );
        assert_eq!(profile.encoding, crate::term::Encoding::Cp850);
        assert_eq!(profile.logging, LogMode::Clean);
        assert_eq!(profile.username, "");
        assert!(!profile.autologon);
    }
}
