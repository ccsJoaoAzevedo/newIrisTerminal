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

/// The arguments a session takes after the instance name: the namespace to
/// open in, and a routine to run instead of the prompt.
fn session_args(spec: &LaunchSpec) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(ns) = spec.namespace.as_deref().filter(|s| !s.is_empty()) {
        args.push("-U".to_string());
        args.push(ns.to_string());
    }
    if let Some(routine) = spec.routine.as_deref().filter(|s| !s.is_empty()) {
        args.push(routine.to_string());
    }
    args
}

/// The environment a well-behaved terminal is expected to advertise.
///
/// We implement a VT102-compatible subset; claiming xterm would invite
/// sequences (mouse, alt-screen) the grid does not model.
fn announce_terminal(cmd: &mut CommandBuilder) {
    cmd.env("TERM", "vt100");
}

/// The environment a *shell* is expected to find instead.
///
/// A shell does not read `TERM` directly - it looks the name up in terminfo,
/// and the databases shells ship with no longer carry the entry IRIS is told
/// about. Git Bash's holds `xterm`, `cygwin` and `screen` and nothing else, so
/// being told `vt100` left it with no terminal description at all: `clear`
/// exited 1 having cleared nothing, and Ctrl+L in `bash` did nothing either,
/// both of them for want of a name they could look up.
///
/// `xterm-256color` is the entry all of them have, and the colours are the
/// reason for that spelling over bare `xterm`: a shell picks its prompt and its
/// `ls` colours from what terminfo says the terminal can do.
///
/// What it licenses that the grid does not model - alt screen, mouse
/// reporting, bracketed paste - is ignored rather than drawn, and on Windows it
/// does not even reach us: the pseudoconsole renders those itself and hands
/// back an ordinary repaint.
fn announce_shell_terminal(cmd: &mut CommandBuilder) {
    cmd.env("TERM", "xterm-256color");
}

/// The command that starts a shell rather than an IRIS session.
///
/// Here rather than in [`crate::plugins::shells`] for the reason this module
/// exists at all: it is the one place allowed to know which operating system it
/// is running on, and what a shell needs before it will speak UTF-8 is entirely
/// a question about the platform.
///
/// On Windows that means the same wrapper an IRIS session gets - `cmd /c chcp
/// 65001 & ...` - because a console starts on the OEM codepage and a shell
/// inherits it. Without it `cmd.exe` prints accented text as mojibake and no
/// decoding on this side can rescue it; see [`crate::term::encoding`]. The
/// shells that already speak UTF-8, PowerShell 7 among them, are unaffected by
/// being told to.
///
/// `cwd` is the directory the program starts in, when the caller cares. Only
/// the Claude analysis tab does: it runs in the folder holding the file it was
/// handed, so the file can be named bare on a `cmd` command line that has no
/// way to quote a space.
pub fn shell_command(program: &Path, args: &[String], cwd: Option<&Path>) -> CommandBuilder {
    #[cfg(windows)]
    {
        let mut cmd = windows::shell_command(program, args);
        if let Some(dir) = cwd {
            cmd.cwd(dir);
        }
        cmd
    }
    #[cfg(not(windows))]
    {
        // Nothing to arrange: the terminal is already UTF-8 and the shell reads
        // it from the locale.
        let mut cmd = CommandBuilder::new(program);
        for arg in args {
            cmd.arg(arg);
        }
        if let Some(dir) = cwd {
            cmd.cwd(dir);
        }
        announce_shell_terminal(&mut cmd);
        cmd
    }
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

            // The arguments are already inside the command line `cmd` runs,
            // so only the environment is left to apply.
            let mut cmd = session_command(&exe, spec);
            announce_terminal(&mut cmd);
            Ok(cmd)
        }
    }

    /// One argument, safe to put in a command line `cmd` will parse.
    ///
    /// `^` is the character cmd escapes with, and an IRIS routine name starts
    /// with one: a profile set to run `^MYROUTINE` would otherwise have the
    /// caret eaten on the way through. Quoting is not the alternative it looks
    /// like - the command builder escapes a quote in a way cmd does not
    /// understand - so every character cmd would act on gets a caret of its
    /// own.
    ///
    /// The one thing it cannot do anything about is a **space**, because a
    /// space is not something cmd escapes: it is the delimiter, and the only
    /// way past it is a quote. So nothing with a space in it may be put on this
    /// line - which is why both callers name their program bare and put its
    /// directory on PATH instead.
    fn escape_for_cmd(arg: &str) -> String {
        let mut out = String::with_capacity(arg.len());
        for ch in arg.chars() {
            if matches!(ch, '^' | '&' | '|' | '<' | '>' | '(' | ')' | '"') {
                out.push('^');
            }
            out.push(ch);
        }
        out
    }

    /// The command that opens a session, with the console put into UTF-8
    /// first.
    ///
    /// A pseudo-console starts on the machine's OEM codepage, and IRIS speaks
    /// UTF-8: the console reads the instance's bytes as CP850 and re-encodes
    /// them, so `Nó` arrives as `├│`, a typed `ó` reaches IRIS as `?`, and the
    /// accent costs a column that only one side of the connection knows about -
    /// which is what left a character behind every time IRIS repainted a
    /// recalled line by absolute position. `chcp 65001` in front of the session
    /// settles all three: the bytes pass through untouched and both sides count
    /// the same columns.
    ///
    /// `cmd` is the only way to run two commands in one console, and it takes
    /// the program name unquoted - the escaping a command builder applies to a
    /// quoted path is not something `cmd` understands. So the name is bare and
    /// the directory it lives in goes on PATH, which works whatever spaces the
    /// path has.
    /// A shell, with the console put into UTF-8 first.
    ///
    /// The same wrapper [`session_command`] uses, and - as it turns out - for
    /// the same *two* reasons. The console opens on the OEM codepage and a
    /// shell that inherits it prints accented text as bytes this side cannot
    /// decode; and the program is named bare, with its directory prepended to
    /// PATH, because the command line inside `cmd /c` is one `cmd` parses and
    /// a path with a space in it is three words to it. Passing the full path
    /// is what produced `'C:\Program' is not recognized` for Git Bash. There
    /// is no quoting round it: see [`escape_for_cmd`] for why a quote cannot
    /// survive the trip.
    pub fn shell_command(program: &Path, args: &[String]) -> CommandBuilder {
        let (Some(name), Some(dir)) = (program.file_name(), program.parent()) else {
            // Neither a file name nor a parent directory: nothing to put on
            // PATH, so start it directly and leave the console on whatever
            // codepage it opened on. Accented output will not survive that.
            // Reachable only for a program that is a bare relative name, which
            // the shell files never hold - they are absolute paths, checked to
            // be files before they are offered.
            let mut cmd = CommandBuilder::new(program);
            for arg in args {
                cmd.arg(arg);
            }
            announce_shell_terminal(&mut cmd);
            return cmd;
        };

        let mut line = escape_for_cmd(&name.to_string_lossy());
        for arg in args {
            line.push(' ');
            line.push_str(&escape_for_cmd(arg));
        }
        let mut cmd = CommandBuilder::new("cmd.exe");
        cmd.arg("/s");
        cmd.arg("/c");
        cmd.arg(format!("chcp 65001>nul & {line}"));
        cmd.env("PATH", prepend_to_path(dir));
        announce_shell_terminal(&mut cmd);
        cmd
    }

    /// `dir` in front of the inherited PATH, which is how a program with a
    /// space in its path is named without quoting it.
    fn prepend_to_path(dir: &Path) -> String {
        match std::env::var("PATH") {
            Ok(existing) => format!("{};{existing}", dir.display()),
            Err(_) => dir.display().to_string(),
        }
    }

    fn session_command(exe: &Path, spec: &LaunchSpec) -> CommandBuilder {
        let (Some(name), Some(dir)) = (exe.file_name(), exe.parent()) else {
            // Nothing to put on PATH, so there is nothing to wrap: start it
            // directly and leave the console on whatever codepage it opened on.
            // Accented text will not survive that, in either direction, and no
            // decoding on this side can rescue it - see `crate::term::encoding`.
            // Reachable only for a binary with neither a file name nor a
            // parent directory, which is to say never.
            let mut cmd = CommandBuilder::new(exe);
            cmd.arg(&spec.instance);
            return cmd;
        };

        let args = std::iter::once(spec.instance.clone())
            .chain(session_args(spec))
            .map(|arg| escape_for_cmd(&arg))
            .collect::<Vec<_>>()
            .join(" ");
        let mut cmd = CommandBuilder::new("cmd.exe");
        // `/s` keeps cmd from applying its own quoting rules to what follows,
        // and `>nul` keeps `chcp`'s "Active code page" line off the screen.
        cmd.arg("/s");
        cmd.arg("/c");
        cmd.arg(format!(
            "chcp 65001>nul & {} {args}",
            name.to_string_lossy()
        ));
        cmd.env("PATH", prepend_to_path(dir));
        cmd
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

        /// The command line `cmd` will parse, as one string.
        fn command_line(cmd: &CommandBuilder) -> String {
            cmd.get_argv()
                .iter()
                .map(|arg| arg.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join(" ")
        }

        fn path_of(cmd: &CommandBuilder) -> String {
            cmd.get_env("PATH")
                .map(|value| value.to_string_lossy().into_owned())
                .unwrap_or_default()
        }

        /// The bug this pins, and it is worth pinning because the failure is
        /// entirely invisible from this side: the tab opens, `cmd` says
        /// `'C:\Program' is not recognized`, and the shell never runs. A path
        /// with a space in it cannot go on a line `cmd` parses, so the program
        /// is named bare and its directory goes on PATH instead.
        #[test]
        fn a_shell_under_program_files_is_named_bare() {
            let program = Path::new(r"C:\Program Files\Git\bin\bash.exe");
            let cmd = shell_command(program, &["--login".to_string(), "-i".to_string()]);

            let line = command_line(&cmd);
            assert!(
                line.contains("bash.exe --login -i"),
                "the shell is not on the line: {line}"
            );
            assert!(
                !line.contains("Program Files"),
                "the path is on the line, so cmd will split it: {line}"
            );
            assert!(
                path_of(&cmd).starts_with(r"C:\Program Files\Git\bin;"),
                "its directory has to come first on PATH: {}",
                path_of(&cmd)
            );
        }

        /// And the console still gets its codepage, which is the other half of
        /// why a shell is wrapped in `cmd` at all.
        #[test]
        fn a_shell_is_started_with_the_console_in_utf8() {
            let cmd = shell_command(Path::new(r"C:\Windows\System32\cmd.exe"), &[]);
            assert_eq!(
                cmd.get_argv()
                    .first()
                    .map(|a| a.to_string_lossy().into_owned()),
                Some("cmd.exe".to_string())
            );
            assert!(command_line(&cmd).contains("chcp 65001>nul &"));
        }

        /// An IRIS session is named the same way, for the same reason: the
        /// standard install root is under `C:\Program Files\InterSystems`.
        #[test]
        fn an_iris_session_is_named_bare_too() {
            let spec = LaunchSpec {
                instance: "CONSISTEM".into(),
                ..LaunchSpec::default()
            };
            let exe = Path::new(r"C:\Program Files\InterSystems\IRIS\bin\irissession.exe");
            let cmd = session_command(exe, &spec);

            let line = command_line(&cmd);
            assert!(line.contains("irissession.exe CONSISTEM"), "{line}");
            assert!(!line.contains("Program Files"), "{line}");
            assert!(path_of(&cmd).starts_with(r"C:\Program Files\InterSystems\IRIS\bin;"));
        }

        /// A routine name is the reason the escaping exists: `^MYROUTINE`
        /// reaches IRIS whole only if the caret survives cmd's own parsing.
        #[test]
        fn cmd_metacharacters_are_escaped_in_a_session_argument() {
            assert_eq!(escape_for_cmd("^MYROUTINE"), "^^MYROUTINE");
            assert_eq!(escape_for_cmd("^%CSW1GEN"), "^^%CSW1GEN");
            assert_eq!(escape_for_cmd("CONSISTEM"), "CONSISTEM");
            assert_eq!(escape_for_cmd("a&b|c>d"), "a^&b^|c^>d");
        }

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
            for arg in session_args(spec) {
                cmd.arg(arg);
            }
            announce_terminal(&mut cmd);
            Ok(cmd)
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
