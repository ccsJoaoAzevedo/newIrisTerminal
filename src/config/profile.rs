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
    /// Character set IRIS speaks over Telnet on this session. Wrong values show
    /// up as mangled accented characters, not as an error, so it is per-profile
    /// rather than guessed at.
    ///
    /// Only consulted for a remote profile — see [`Profile::wire_encoding`],
    /// which is what the session actually reads and writes through.
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
    /// A shell to run instead of an IRIS session.
    ///
    /// Set when the tab was opened from the Shells section of the new-session
    /// menu; see [`crate::plugins::shells`]. It takes precedence over
    /// everything IRIS-shaped on this profile, because a `pwsh.exe` has no
    /// instance, no namespace and nothing to log in to - which is exactly what
    /// this field being set means.
    #[serde(default)]
    pub shell: Option<ShellCommand>,
}

/// The program a shell profile runs.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShellCommand {
    pub program: PathBuf,
    #[serde(default)]
    pub args: Vec<String>,
    /// Directory to start the program in. `None` leaves it wherever the app
    /// itself was started, which is what a shell from the Shells menu wants.
    ///
    /// Set by the Claude analysis tab, which runs in the folder holding the
    /// file it was handed: naming the file bare is what keeps a space in the
    /// path out of a `cmd` command line - see `escape_for_cmd` in
    /// [`crate::pty::launcher`].
    #[serde(default)]
    pub cwd: Option<PathBuf>,
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
            shell: None,
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

    /// The character set of the bytes actually on this session's wire.
    ///
    /// A remote profile speaks whatever [`Profile::encoding`] says: the socket
    /// carries the instance's own bytes and nothing translates them on the way.
    /// A local one is UTF-8 whatever the profile says, because a pseudo-console
    /// stands in the path and always hands the terminal UTF-8 — see
    /// [`crate::term::encoding`], which measures that against a live instance.
    ///
    /// Reading it here rather than at each call site is what keeps a stale
    /// value harmless. A profile that was remote and became local carries its
    /// old codepage across, and a profile written by an earlier build carries
    /// whatever that build put there; either would otherwise transcode a local
    /// session that needs no transcoding, and cost a column per accent.
    pub fn wire_encoding(&self) -> crate::term::Encoding {
        match self.remote {
            Some(_) => self.encoding,
            None => crate::term::Encoding::Utf8,
        }
    }

    /// Where this profile's session goes, in one line, for a tooltip or the
    /// status bar.
    pub fn endpoint(&self) -> String {
        if let Some(shell) = self.shell.as_ref() {
            // The program's own file name: `pwsh.exe` is what someone looking
            // at the bar wants, not the three directories above it.
            return shell
                .program
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_else(|| shell.program.display().to_string());
        }
        match self.remote.as_ref() {
            Some(remote) => format!("{}:{} (telnet)", remote.address, remote.port),
            None if self.instance.is_empty() => "no instance configured".to_string(),
            None => self.instance.clone(),
        }
    }

    /// A profile that opens one of the machine's shells.
    ///
    /// Deliberately not built from the current profile the way a server pick
    /// is: a shell inherits nothing IRIS-shaped, because none of it means
    /// anything to `bash`. Autologon would type a username at it, a namespace
    /// would be sent as a command, and the logging modes describe an IRIS
    /// transcript. What it does keep is the shell's own name, which is what
    /// names the tab.
    pub fn for_shell(shell: &crate::plugins::shells::Shell) -> Profile {
        Profile {
            name: shell.name.clone(),
            instance: shell.name.clone(),
            namespace: String::new(),
            shell: Some(ShellCommand {
                program: shell.program.clone(),
                args: shell.args.clone(),
                cwd: None,
            }),
            ..Profile::default()
        }
    }

    /// Whether this profile runs a shell rather than an IRIS session.
    ///
    /// Read by everything that would otherwise assume IRIS is at the other
    /// end: the ObjectScript colouring, the command history, and autologon.
    pub fn is_shell(&self) -> bool {
        self.shell.is_some()
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

    /// The bug this exists to make impossible: a codepage left over on a local
    /// profile transcoded a session that needs no transcoding, and cost a
    /// column per accent - which put recall, End and every rubout out by one.
    /// A local session is UTF-8 whatever the profile carries.
    #[test]
    fn a_local_session_is_utf8_whatever_the_profile_says() {
        use crate::term::Encoding;

        // The case above is exactly how one gets here: preferences carry over
        // from a Telnet base onto a local target.
        let local = Profile::for_server(
            &Server::for_instance("CONSISTEM"),
            &instances(),
            &telnet_base(),
        );
        assert!(local.remote.is_none());
        assert_eq!(
            local.encoding,
            Encoding::Cp850,
            "the setting is still there"
        );
        assert_eq!(
            local.wire_encoding(),
            Encoding::Utf8,
            "but it must not reach the wire"
        );

        // A remote profile speaks what it is set to: the socket carries the
        // instance's own bytes, with no console to have re-encoded them.
        let remote = telnet_base();
        assert!(remote.remote.is_some());
        assert_eq!(remote.wire_encoding(), Encoding::Cp850);

        // And every codepage, both ways, so this cannot rot into "Cp850 only".
        for enc in Encoding::ALL {
            let mut profile = telnet_base();
            profile.encoding = enc;
            assert_eq!(profile.wire_encoding(), enc, "remote {enc:?}");
            profile.remote = None;
            assert_eq!(profile.wire_encoding(), Encoding::Utf8, "local {enc:?}");
        }
    }
}
