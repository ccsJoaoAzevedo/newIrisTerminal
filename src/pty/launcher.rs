//! Resolving *how* to start an IRIS session on this machine.
//!
//! This is the only module in the codebase that branches on the operating
//! system. Everything above it deals in a [`CommandBuilder`] and never asks
//! which platform it is running on.
//!
//! | Platform | Binary                | Invocation                    |
//! |----------|-----------------------|-------------------------------|
//! | Windows  | `irissession.exe`     | `irissession.exe INSTANCE`    |
//! | Linux    | `iris` (or `csession`)| `iris session INSTANCE`       |
//! | macOS    | `iris` (or `csession`)| `iris session INSTANCE`       |

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context, Result};
use portable_pty::CommandBuilder;

/// An IRIS instance as reported by `iris list` / discovered on disk.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Instance {
    pub name: String,
    /// Directory containing the session binary, when known.
    pub bin_dir: Option<PathBuf>,
}

/// Everything needed to start one session.
#[derive(Clone, Debug, Default)]
pub struct LaunchSpec {
    pub instance: String,
    /// Namespace to land in. Passed via `-U`; ignored when empty.
    pub namespace: Option<String>,
    /// Routine to run instead of the interactive prompt. Rarely used, but it is
    /// how a profile can open straight into a menu.
    pub routine: Option<String>,
    /// Overrides discovery entirely when the user has a non-standard install.
    pub binary_override: Option<PathBuf>,
}

/// Discovery shells out to `iris list` and scans the install directories,
/// which costs about 100 ms. `command()` needs it too, so without a cache
/// every new session paid that price again on the UI thread. Instances do not
/// appear and disappear while the app is open, so caching for the process
/// lifetime is safe; [`refresh_instances`] exists for when it is not.
static INSTANCE_CACHE: std::sync::OnceLock<std::sync::Mutex<Option<Vec<Instance>>>> =
    std::sync::OnceLock::new();

fn cache() -> &'static std::sync::Mutex<Option<Vec<Instance>>> {
    INSTANCE_CACHE.get_or_init(|| std::sync::Mutex::new(None))
}

/// Discovered instances, computed once and reused.
pub fn instances(launcher: &dyn IrisLauncher) -> Vec<Instance> {
    if let Ok(guard) = cache().lock() {
        if let Some(found) = guard.as_ref() {
            return found.clone();
        }
    }
    let found = launcher.discover();
    if let Ok(mut guard) = cache().lock() {
        *guard = Some(found.clone());
    }
    found
}

/// Drops the cache so the next lookup rediscovers.
pub fn refresh_instances() {
    if let Ok(mut guard) = cache().lock() {
        *guard = None;
    }
}

pub trait IrisLauncher {
    /// Instances this machine knows about. Never fails hard — an empty list
    /// just means the user must type the instance name themselves.
    fn discover(&self) -> Vec<Instance>;

    /// Build the command that starts a session.
    fn command(&self, spec: &LaunchSpec) -> Result<CommandBuilder>;
}

/// The launcher for the host platform.
pub fn launcher() -> Box<dyn IrisLauncher + Send + Sync> {
    #[cfg(windows)]
    {
        Box::new(windows::WindowsLauncher)
    }
    #[cfg(not(windows))]
    {
        Box::new(unix::UnixLauncher)
    }
}

/// Applies the settings every platform shares: namespace, routine, and the
/// environment a well-behaved terminal is expected to advertise.
fn finish(mut cmd: CommandBuilder, spec: &LaunchSpec) -> CommandBuilder {
    if let Some(ns) = spec.namespace.as_deref().filter(|s| !s.is_empty()) {
        cmd.arg("-U");
        cmd.arg(ns);
    }
    if let Some(routine) = spec.routine.as_deref().filter(|s| !s.is_empty()) {
        cmd.arg(routine);
    }
    // We implement a VT102-compatible subset; claiming xterm would invite
    // sequences (mouse, alt-screen) the grid does not model.
    cmd.env("TERM", "vt100");
    cmd
}

/// Locates a binary by trying an explicit override, then a list of candidate
/// directories, then bare PATH lookup.
fn find_binary(override_path: Option<&Path>, dirs: &[PathBuf], names: &[&str]) -> Result<PathBuf> {
    if let Some(path) = override_path {
        return if path.is_file() {
            Ok(path.to_path_buf())
        } else {
            Err(anyhow!(
                "configured IRIS binary not found: {}",
                path.display()
            ))
        };
    }

    for dir in dirs {
        for name in names {
            let candidate = dir.join(name);
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }

    // Fall back to PATH — CommandBuilder resolves a bare name itself.
    Ok(PathBuf::from(names[0]))
}

#[cfg(windows)]
mod windows {
    use std::os::windows::process::CommandExt;

    use super::*;

    /// Runs a helper process with no console of its own.
    ///
    /// The app is a GUI process and so has no console to lend a child. Without
    /// this, spawning `iris.exe list` gives it a brand new one - and on Windows
    /// 11 a new console is handed to Windows Terminal, so a lookup that is meant
    /// to be invisible opens and closes a terminal window in front of the user
    /// just as the session starts. The output is captured through pipes either
    /// way, so there is nothing the console was needed for.
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    pub struct WindowsLauncher;

    /// Standard install roots. `iris.exe list` is authoritative when present,
    /// but scanning these covers a machine where IRIS is not on PATH.
    fn install_roots() -> Vec<PathBuf> {
        let mut roots = Vec::new();
        for base in [r"C:\InterSystems", r"C:\Program Files\InterSystems"] {
            let Ok(entries) = std::fs::read_dir(base) else {
                continue;
            };
            for entry in entries.flatten() {
                let bin = entry.path().join("bin");
                if bin.is_dir() {
                    roots.push(bin);
                }
            }
        }
        roots
    }

    impl IrisLauncher for WindowsLauncher {
        fn discover(&self) -> Vec<Instance> {
            let mut found: Vec<Instance> = Vec::new();

            // Preferred source: `iris.exe list`, which knows about instances
            // installed anywhere, including ones we would never scan for.
            for bin in install_roots() {
                let exe = bin.join("iris.exe");
                if !exe.is_file() {
                    continue;
                }
                if let Ok(output) = std::process::Command::new(&exe)
                    .arg("list")
                    .creation_flags(CREATE_NO_WINDOW)
                    .output()
                {
                    let text = String::from_utf8_lossy(&output.stdout);
                    for name in parse_iris_list(&text) {
                        if !found.iter().any(|i| i.name.eq_ignore_ascii_case(&name)) {
                            found.push(Instance {
                                name,
                                bin_dir: Some(bin.clone()),
                            });
                        }
                    }
                }
                if !found.is_empty() {
                    break;
                }
            }

            // Fallback: each install directory is usually named for its instance.
            if found.is_empty() {
                for bin in install_roots() {
                    if !bin.join("irissession.exe").is_file() {
                        continue;
                    }
                    if let Some(name) = bin
                        .parent()
                        .and_then(|p| p.file_name())
                        .map(|n| n.to_string_lossy().into_owned())
                    {
                        found.push(Instance {
                            name,
                            bin_dir: Some(bin),
                        });
                    }
                }
            }

            found
        }

        fn command(&self, spec: &LaunchSpec) -> Result<CommandBuilder> {
            // Prefer the bin directory of the instance we were asked for, so a
            // machine with several installs uses the matching binary.
            let mut dirs: Vec<PathBuf> = instances(self)
                .into_iter()
                .filter(|i| i.name.eq_ignore_ascii_case(&spec.instance))
                .filter_map(|i| i.bin_dir)
                .collect();
            dirs.extend(install_roots());

            let exe = find_binary(
                spec.binary_override.as_deref(),
                &dirs,
                &["irissession.exe", "csession.exe"],
            )
            .context("locating irissession.exe")?;

            let mut cmd = CommandBuilder::new(exe);
            cmd.arg(&spec.instance);
            Ok(finish(cmd, spec))
        }
    }

    /// `iris list` prints one stanza per instance, headed by a quoted name.
    ///
    /// The keyword varies by version and install type: standard installs say
    /// `Instance 'NAME'`, while 2023+ custom installs say `Configuration
    /// 'NAME'`. Both must be recognised, because the directory-name fallback is
    /// not merely less precise — it is often wrong. Real example: a directory
    /// named `CONSISTEM` whose registered instance is `CONSISETM`, which IRIS
    /// then refuses to start.
    fn parse_iris_list(text: &str) -> Vec<String> {
        text.lines()
            .filter_map(|line| {
                let line = line.trim();
                let rest = ["Instance ", "Configuration "]
                    .iter()
                    .find_map(|kw| line.strip_prefix(kw))?;
                let rest = rest.trim().strip_prefix('\'')?;
                let end = rest.find('\'')?;
                Some(rest[..end].to_string())
            })
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_instance_names_from_iris_list() {
            let text = "\
Instance 'CONSISTEM'   (C:\\InterSystems\\CONSISTEM)
    directory:  C:\\InterSystems\\CONSISTEM
    status:     running

Instance 'consistemdb'   (C:\\InterSystems\\consistemdb)
    status:     down
";
            assert_eq!(parse_iris_list(text), vec!["CONSISTEM", "consistemdb"]);
        }

        /// Verbatim from `iris.exe list` on IRIS 2025.1 with a custom install —
        /// note the keyword is `Configuration`, and the instance name differs
        /// from the directory name.
        #[test]
        fn parses_the_configuration_keyword_used_by_custom_installs() {
            let text = "\
Configuration 'CONSISETM'\t(Custom installation)
\tdirectory:    C:\\InterSystems\\CONSISTEM
\tversionid:    2025.1.3.481.1
\tstatus:       running, since Wed Aug 26 09:44:40 2026
";
            assert_eq!(parse_iris_list(text), vec!["CONSISETM"]);
        }
    }
}

#[cfg(not(windows))]
mod unix {
    use super::*;

    pub struct UnixLauncher;

    fn install_roots() -> Vec<PathBuf> {
        let mut roots: Vec<PathBuf> = vec![
            PathBuf::from("/usr/irissys/bin"),
            PathBuf::from("/usr/local/etc/irissys"),
            PathBuf::from("/usr/local/bin"),
            PathBuf::from("/usr/bin"),
        ];
        // Multi-instance installs live under /opt/<name>/bin or
        // /InterSystems/<name>/bin depending on the installer used.
        for base in ["/opt", "/InterSystems", "/usr/local"] {
            let Ok(entries) = std::fs::read_dir(base) else {
                continue;
            };
            for entry in entries.flatten() {
                let bin = entry.path().join("bin");
                if bin.is_dir() {
                    roots.push(bin);
                }
            }
        }
        roots
    }

    impl IrisLauncher for UnixLauncher {
        fn discover(&self) -> Vec<Instance> {
            let mut found: Vec<Instance> = Vec::new();
            for dir in install_roots() {
                let exe = dir.join("iris");
                if !exe.is_file() {
                    continue;
                }
                if let Ok(output) = std::process::Command::new(&exe).arg("list").output() {
                    let text = String::from_utf8_lossy(&output.stdout);
                    for name in parse_iris_list(&text) {
                        if !found.iter().any(|i| i.name.eq_ignore_ascii_case(&name)) {
                            found.push(Instance {
                                name,
                                bin_dir: Some(dir.clone()),
                            });
                        }
                    }
                }
                if !found.is_empty() {
                    break;
                }
            }
            found
        }

        fn command(&self, spec: &LaunchSpec) -> Result<CommandBuilder> {
            let exe = find_binary(
                spec.binary_override.as_deref(),
                &install_roots(),
                &["iris", "csession"],
            )
            .context("locating the iris binary")?;

            let mut cmd = CommandBuilder::new(&exe);
            // `csession` takes the instance directly; `iris` needs the
            // `session` subcommand first.
            let is_csession = exe
                .file_name()
                .map(|n| n.to_string_lossy().starts_with("csession"))
                .unwrap_or(false);
            if !is_csession {
                cmd.arg("session");
            }
            cmd.arg(&spec.instance);
            Ok(finish(cmd, spec))
        }
    }

    /// `iris list` prints one stanza per instance, headed by a quoted name.
    ///
    /// The keyword varies by version and install type: standard installs say
    /// `Instance 'NAME'`, while 2023+ custom installs say `Configuration
    /// 'NAME'`. Both must be recognised, because the directory-name fallback is
    /// not merely less precise — it is often wrong. Real example: a directory
    /// named `CONSISTEM` whose registered instance is `CONSISETM`, which IRIS
    /// then refuses to start.
    fn parse_iris_list(text: &str) -> Vec<String> {
        text.lines()
            .filter_map(|line| {
                let line = line.trim();
                let rest = ["Instance ", "Configuration "]
                    .iter()
                    .find_map(|kw| line.strip_prefix(kw))?;
                let rest = rest.trim().strip_prefix('\'')?;
                let end = rest.find('\'')?;
                Some(rest[..end].to_string())
            })
            .collect()
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn parses_instance_names_from_iris_list() {
            let text = "Instance 'IRISHEALTH'\t(/usr/irissys)\n    status:\trunning\n";
            assert_eq!(parse_iris_list(text), vec!["IRISHEALTH"]);
        }
    }
}
