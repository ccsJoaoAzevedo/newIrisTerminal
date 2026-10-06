//! "Open newIrisTerminal here" in the Explorer right-click menu, with a
//! submenu entry per shell - and the command line both it and the IRIS tray
//! launch the app with.
//!
//! Written under `HKCU\Software\Classes`, so it needs no elevation and touches
//! nobody else's account. Both the folder icon and the empty space inside a
//! folder get the entry. The submenu is a snapshot of the shells found when it
//! was written, so it is rewritten whenever the shell list is reloaded.
//!
//! On Windows 11 a classic entry like this one sits under "Show more options";
//! the compact menu only takes packaged `IExplorerCommand` handlers.

use std::path::{Path, PathBuf};

/// The two places a folder's right-click menu is read from: the folder itself,
/// and the background of an open one.
const ROOTS: [&str; 2] = [
    r"Software\Classes\Directory\shell\newIrisTerminal",
    r"Software\Classes\Directory\Background\shell\newIrisTerminal",
];

/// What the launched copy is asked to open, read off its command line.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Launch {
    /// The shell's id: its file's stem in the shells folder.
    pub shell: Option<String>,
    pub cwd: Option<PathBuf>,
    /// Started by the IRIS tray's Terminal entry - see
    /// [`crate::features::iris_terminal`] - and so owes it a session.
    pub as_terminal: bool,
    /// The server the tray asked for, when it named one.
    pub server: Option<String>,
}

impl Launch {
    /// Reads `--shell <id>` and `--cwd <dir>`. Anything else is ignored, so an
    /// old entry left in the registry never stops the app from starting.
    pub fn parse(args: impl IntoIterator<Item = String>) -> Launch {
        let args: Vec<String> = args.into_iter().collect();
        let mut launch = Launch {
            server: crate::features::iris_terminal::requested_server(&args),
            ..Launch::default()
        };
        let mut args = args.into_iter();
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--shell" => launch.shell = args.next().filter(|s| !s.is_empty()),
                "--cwd" => launch.cwd = args.next().and_then(|d| cwd_from_explorer(&d)),
                crate::features::iris_terminal::AS_TERMINAL => launch.as_terminal = true,
                _ => {}
            }
        }
        launch
    }

    /// The profile of the tab to open, if the shell asked for is still there.
    pub fn profile(&self) -> Option<crate::config::Profile> {
        let id = self.shell.as_deref()?;
        let shell = crate::plugins::shells::available()
            .into_iter()
            .find(|s| shell_id(&s.file) == id)?;
        let mut profile = crate::config::Profile::for_shell(&shell);
        if let Some(shell) = profile.shell.as_mut() {
            shell.cwd = self.cwd.clone().filter(|d| d.is_dir());
        }
        Some(profile)
    }
}

/// Undoes what the command-line rules do to a drive root.
///
/// Explorer substitutes `C:\` for `%V`, so the quoted argument reads `"C:\"`,
/// and the backslash escapes the closing quote: the program receives `C:"`.
/// Every other folder arrives intact.
fn cwd_from_explorer(raw: &str) -> Option<PathBuf> {
    let trimmed = raw.trim_end_matches('"');
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.ends_with(':') {
        return Some(PathBuf::from(format!("{trimmed}\\")));
    }
    Some(PathBuf::from(trimmed))
}

/// How the command line names a shell. The file stem rather than the display
/// name, because two files may share a name and a file may be renamed inside.
pub fn shell_id(file: &Path) -> String {
    file.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// One value to write: the key under `HKCU`, the value name (empty for the
/// default value), and the data.
type Entry = (String, &'static str, String);

/// Everything the menu consists of, for one executable and these shells as
/// `(id, name, program)`. Pure, so the shape can be tested without a registry.
fn entries(exe: &str, title: &str, shells: &[(String, String, String)]) -> Vec<Entry> {
    let mut out = Vec::new();
    for root in ROOTS {
        out.push((root.to_string(), "MUIVerb", title.to_string()));
        out.push((root.to_string(), "Icon", format!("\"{exe}\",0")));
        // Present and empty is what makes the entry a cascade read from `shell`.
        out.push((root.to_string(), "SubCommands", String::new()));
        for (n, (id, name, program)) in shells.iter().enumerate() {
            // Explorer orders the submenu by key name; the number keeps the
            // order the shells were found in.
            let key = format!(r"{root}\shell\{n:02}");
            out.push((key.clone(), "MUIVerb", name.clone()));
            out.push((key.clone(), "Icon", format!("\"{program}\",0")));
            out.push((
                format!(r"{key}\command"),
                "",
                format!("\"{exe}\" --shell \"{id}\" --cwd \"%V\""),
            ));
        }
    }
    out
}

/// Whether the entry is in the menu now.
#[cfg(windows)]
pub fn is_registered() -> bool {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    RegKey::predef(HKEY_CURRENT_USER)
        .open_subkey(ROOTS[1])
        .is_ok()
}

/// Writes the entry afresh, so a shell that has gone leaves the submenu.
#[cfg(windows)]
pub fn register() -> std::io::Result<()> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let exe = std::env::current_exe()?;
    let shells: Vec<_> = crate::plugins::shells::available()
        .iter()
        .map(|s| {
            (
                shell_id(&s.file),
                s.name.clone(),
                s.program.display().to_string(),
            )
        })
        .collect();
    unregister()?;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    let title = crate::i18n::tr("Open newIrisTerminal here");
    for (key, name, value) in entries(&exe.display().to_string(), title, &shells) {
        let (key, _) = hkcu.create_subkey(&key)?;
        key.set_value(name, &value)?;
    }
    Ok(())
}

/// Takes the entry out of the menu. Not there already is not an error.
#[cfg(windows)]
pub fn unregister() -> std::io::Result<()> {
    use winreg::{enums::HKEY_CURRENT_USER, RegKey};
    let hkcu = RegKey::predef(HKEY_CURRENT_USER);
    for root in ROOTS {
        match hkcu.delete_subkey_all(root) {
            Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn is_registered() -> bool {
    false
}

#[cfg(not(windows))]
pub fn register() -> std::io::Result<()> {
    Err(std::io::Error::other("only on Windows"))
}

#[cfg(not(windows))]
pub fn unregister() -> std::io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Launch {
        Launch::parse(args.iter().map(|s| s.to_string()))
    }

    #[test]
    fn a_folder_with_spaces_arrives_whole() {
        let launch = parse(&["--shell", "Git Bash", "--cwd", r"C:\My Projects\x"]);
        assert_eq!(launch.shell.as_deref(), Some("Git Bash"));
        assert_eq!(launch.cwd, Some(PathBuf::from(r"C:\My Projects\x")));
    }

    #[test]
    fn a_drive_root_survives_its_escaped_quote() {
        assert_eq!(parse(&["--cwd", "C:\""]).cwd, Some(PathBuf::from(r"C:\")));
    }

    #[test]
    fn a_flag_with_no_value_is_ignored() {
        assert_eq!(parse(&["--shell"]), Launch::default());
        assert_eq!(parse(&["--whatever", "x"]), Launch::default());
    }

    #[test]
    fn every_shell_gets_a_submenu_entry_in_both_menus() {
        let shells = vec![
            ("cmd".into(), "Command Prompt".into(), r"C:\cmd.exe".into()),
            ("bash".into(), "Git Bash".into(), r"C:\bash.exe".into()),
        ];
        let entries = entries(r"C:\t.exe", "Open here", &shells);
        for root in ROOTS {
            let command = entries
                .iter()
                .find(|(k, _, _)| *k == format!(r"{root}\shell\01\command"))
                .expect("second shell's command");
            assert_eq!(command.2, r#""C:\t.exe" --shell "bash" --cwd "%V""#);
            assert!(entries
                .iter()
                .any(|(k, n, v)| k == root && *n == "SubCommands" && v.is_empty()));
        }
    }
}
