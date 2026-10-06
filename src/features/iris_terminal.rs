//! Standing in for `Iristerm.exe`, so the IRIS tray's Terminal entry opens
//! this app.
//!
//! The launcher has no setting for which terminal it starts: it runs
//! `Iristerm.exe` out of the instance's `bin` folder. So installing means
//! putting a copy of this program there under that name, with the original
//! kept beside it as `Iristerm.exe.original`, and uninstalling means putting
//! the original back. Nothing else in the folder is touched.
//!
//! The copy does not run itself. It hands its arguments to the program it was
//! installed from - see [`forward`] - so an update to that one is an update to
//! what the tray opens, and the copy never goes stale.

use std::path::{Path, PathBuf};

/// The launcher's terminal, inside an instance's `bin` folder.
const TERMINAL: &str = "Iristerm.exe";
/// Where the original goes while this app stands in for it.
const ORIGINAL: &str = "Iristerm.exe.original";
/// A copy displaced while running: Windows renames a running program but will
/// not delete or overwrite one.
const DISPLACED: &str = "Iristerm.exe.removed";
/// Present in every build of this program and in no `Iristerm.exe`: how a file
/// in `bin` is told to be ours.
const MARK: &[u8] = b"newIrisTerminal stands in for Iristerm.exe";

/// Passed by the copy in `bin` to the program it forwards to, so that one
/// knows it was asked for a terminal rather than started by hand.
pub const AS_TERMINAL: &str = "--as-iris-terminal";

/// Where this app stands with one instance's terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// The launcher's own terminal, untouched.
    Original,
    /// This app, with the original kept aside.
    Installed,
    /// An original is kept aside, but `Iristerm.exe` is not ours: an IRIS
    /// upgrade wrote a new one. Installing again keeps the new one instead.
    Overwritten,
    /// No `Iristerm.exe` at all.
    Missing,
}

pub fn state(bin: &Path) -> State {
    let terminal = bin.join(TERMINAL);
    if !terminal.is_file() {
        return State::Missing;
    }
    match (bin.join(ORIGINAL).is_file(), is_ours(&terminal)) {
        (_, true) => State::Installed,
        (true, false) => State::Overwritten,
        (false, false) => State::Original,
    }
}

fn is_ours(path: &Path) -> bool {
    std::fs::read(path)
        .map(|bytes| bytes.windows(MARK.len()).any(|w| w == MARK))
        .unwrap_or(false)
}

/// Puts this program in place of the instance's terminal.
pub fn install(bin: &Path) -> std::io::Result<()> {
    let exe = std::env::current_exe()?;
    install_from(&exe, bin)?;
    remember_home(&exe)
}

fn install_from(exe: &Path, bin: &Path) -> std::io::Result<()> {
    let terminal = bin.join(TERMINAL);
    let original = bin.join(ORIGINAL);
    if terminal.is_file() {
        if is_ours(&terminal) {
            displace(bin)?;
        } else {
            // A first install, or one after an IRIS upgrade: either way what is
            // there now is the launcher's, and the newest one is the one to keep.
            std::fs::rename(&terminal, &original)?;
        }
    }
    std::fs::copy(exe, &terminal).map(|_| ())
}

/// Puts the launcher's own terminal back.
pub fn uninstall(bin: &Path) -> std::io::Result<()> {
    let terminal = bin.join(TERMINAL);
    let original = bin.join(ORIGINAL);
    if !original.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("{} not found", original.display()),
        ));
    }
    if terminal.is_file() {
        if !is_ours(&terminal) {
            // IRIS replaced it since: that one is newer than what was kept.
            return std::fs::remove_file(&original);
        }
        displace(bin)?;
    }
    std::fs::rename(&original, &terminal)
}

/// Moves our copy out of the way. Renamed rather than deleted, so it works
/// while the tray's terminal is the very copy doing the uninstalling.
fn displace(bin: &Path) -> std::io::Result<()> {
    let displaced = bin.join(DISPLACED);
    let _ = std::fs::remove_file(&displaced);
    std::fs::rename(bin.join(TERMINAL), &displaced)?;
    let _ = std::fs::remove_file(&displaced);
    Ok(())
}

fn home_file() -> PathBuf {
    crate::config::config_dir().join("iris-terminal-home.txt")
}

fn remember_home(exe: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(crate::config::config_dir())?;
    std::fs::write(home_file(), exe.display().to_string())
}

/// Whether this process is the copy in an instance's `bin` folder.
pub fn running_as_terminal() -> bool {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.file_name().map(|n| n.to_string_lossy().into_owned()))
        .is_some_and(|name| name.eq_ignore_ascii_case(TERMINAL))
}

/// Hands this launch to the program the copy was installed from, if that is
/// still where it was. `false` means run here instead.
pub fn forward() -> bool {
    let Ok(home) = std::fs::read_to_string(home_file()) else {
        return false;
    };
    let home = PathBuf::from(home.trim());
    let this = std::env::current_exe().ok();
    if !home.is_file() || this.as_deref() == Some(home.as_path()) {
        return false;
    }
    std::process::Command::new(home)
        .arg(AS_TERMINAL)
        .args(std::env::args_os().skip(1))
        .spawn()
        .is_ok()
}

/// The server the launcher asked for, from the arguments it passes its
/// terminal (`/server=NAME`). `None` opens what the tray has as preferred.
pub fn requested_server(args: &[String]) -> Option<String> {
    args.iter().find_map(|arg| {
        let (key, value) = arg.trim_start_matches(['/', '-']).split_once('=')?;
        key.eq_ignore_ascii_case("server")
            .then(|| value.trim_matches('"').to_string())
            .filter(|v| !v.is_empty())
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bin_with(tag: &str, terminal: &[u8]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nit-iristerm-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(TERMINAL), terminal).unwrap();
        dir
    }

    fn ours(dir: &Path) -> PathBuf {
        let exe = dir.join("ours.exe");
        std::fs::write(&exe, [b"x".as_slice(), MARK].concat()).unwrap();
        exe
    }

    #[test]
    fn installing_then_uninstalling_gives_back_the_original_byte_for_byte() {
        let bin = bin_with("roundtrip", b"launcher's terminal");
        let exe = ours(bin.as_path());
        install_from(&exe, bin.as_path()).unwrap();
        assert_eq!(state(bin.as_path()), State::Installed);
        uninstall(bin.as_path()).unwrap();
        assert_eq!(state(bin.as_path()), State::Original);
        assert_eq!(
            std::fs::read(bin.as_path().join(TERMINAL)).unwrap(),
            b"launcher's terminal"
        );
        assert!(!bin.as_path().join(ORIGINAL).exists());
    }

    #[test]
    fn installing_twice_never_backs_up_our_own_copy() {
        let bin = bin_with("twice", b"launcher's terminal");
        let exe = ours(bin.as_path());
        install_from(&exe, bin.as_path()).unwrap();
        install_from(&exe, bin.as_path()).unwrap();
        assert_eq!(
            std::fs::read(bin.as_path().join(ORIGINAL)).unwrap(),
            b"launcher's terminal"
        );
    }

    #[test]
    fn an_iris_upgrade_after_installing_is_kept_rather_than_rolled_back() {
        let bin = bin_with("upgrade", b"old terminal");
        let exe = ours(bin.as_path());
        install_from(&exe, bin.as_path()).unwrap();
        std::fs::write(bin.as_path().join(TERMINAL), b"new terminal").unwrap();
        assert_eq!(state(bin.as_path()), State::Overwritten);
        uninstall(bin.as_path()).unwrap();
        assert_eq!(
            std::fs::read(bin.as_path().join(TERMINAL)).unwrap(),
            b"new terminal"
        );
        assert!(!bin.as_path().join(ORIGINAL).exists());
    }

    #[test]
    fn the_server_is_read_off_the_launchers_arguments() {
        let args = |a: &[&str]| a.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(
            requested_server(&args(&["/server=PROD"])).as_deref(),
            Some("PROD")
        );
        assert_eq!(requested_server(&args(&["/console=cn_ap:X"])), None);
    }
}
