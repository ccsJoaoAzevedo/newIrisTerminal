//! Checking GitHub for a newer build, and swapping this one for it.
//!
//! The check is one HTTPS GET against the releases API, run on a thread of its
//! own at startup so a slow proxy cannot hold the first frame. Nothing is
//! downloaded and nothing is replaced without being asked: the app says a
//! version is available, and the user decides.
//!
//! Behind a corporate proxy this only works if it goes through it, so the
//! system's own proxy settings are read and used — on Windows from the same
//! registry keys Internet Options writes, elsewhere from the `HTTPS_PROXY` /
//! `HTTP_PROXY` environment variables.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, bail, Context, Result};
use crossbeam_channel::Sender;

/// The version this build calls itself, from `Cargo.toml`.
pub const CURRENT: &str = env!("CARGO_PKG_VERSION");

/// The repository, as `Cargo.toml` gives it.
const REPOSITORY: &str = env!("CARGO_PKG_REPOSITORY");

/// Where releases are published, worked out from [`REPOSITORY`] rather than
/// written down a second time: the two had already drifted apart once, and an
/// updater pointed at somebody else's repository is worse than none.
fn releases_url() -> Option<String> {
    let slug = REPOSITORY
        .trim_end_matches('/')
        .trim_end_matches(".git")
        .split("github.com/")
        .nth(1)?;
    // owner/name and nothing else: a URL with a path after the repository
    // would otherwise be pasted into the API call as-is.
    let mut parts = slug.split('/');
    let owner = parts.next().filter(|s| !s.is_empty())?;
    let name = parts.next().filter(|s| !s.is_empty())?;
    Some(format!(
        "https://api.github.com/repos/{owner}/{name}/releases/latest"
    ))
}

/// GitHub refuses a request without one.
const USER_AGENT: &str = concat!("newIrisTerminal/", env!("CARGO_PKG_VERSION"));

/// How long to wait on the network before giving up. A failed check is a
/// non-event — it must never be something the user waits for.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// The suffix a replaced executable is parked under until the next start.
const OLD_SUFFIX: &str = ".old";

/// A published release, reduced to what the app does something with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// As published, `v` stripped: `0.2.0`.
    pub version: String,
    /// Direct download for this platform's executable.
    pub download: String,
    /// What the release says about itself, for the dialog.
    pub notes: String,
}

/// What the background thread has to say.
#[derive(Clone, Debug)]
pub enum Event {
    /// A newer release exists.
    Available(Release),
    /// The check ran and this is already the newest build.
    UpToDate,
    /// The new executable has been downloaded to this path.
    Downloaded(PathBuf),
    /// The check or the download failed. Never fatal: the app carries on.
    Failed(String),
}

/// Splits a version into numbers, ignoring anything that is not one.
///
/// `v1.2.3-rc1` and `1.2.3` compare as the same three numbers: a pre-release
/// suffix is not something this project publishes, and guessing at an ordering
/// for one would be worse than ignoring it.
fn parts(version: &str) -> Vec<u64> {
    version
        .trim()
        .trim_start_matches(['v', 'V'])
        .split(['.', '-', '+'])
        .map(|piece| {
            piece
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
        })
        .filter(|piece| !piece.is_empty())
        .filter_map(|piece| piece.parse().ok())
        .collect()
}

/// Whether `candidate` is a later version than `current`.
///
/// Compared number by number, with a missing number counting as zero, so `0.2`
/// is newer than `0.1.9` and the same as `0.2.0`.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    let (a, b) = (parts(candidate), parts(current));
    let len = a.len().max(b.len());
    for i in 0..len {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    false
}

/// The proxy to reach the internet through, as `host:port`.
///
/// Read rather than configured: the machines this runs on are handed their
/// proxy by policy, and a second place to configure it is a second place for it
/// to be wrong.
pub fn system_proxy() -> Option<String> {
    for key in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
        if let Ok(value) = std::env::var(key) {
            if !value.trim().is_empty() {
                return Some(value.trim().to_string());
            }
        }
    }
    #[cfg(windows)]
    {
        // The same values Internet Options shows, which is what the rest of the
        // machine's software goes through.
        use winreg::enums::HKEY_CURRENT_USER;
        use winreg::RegKey;
        let settings = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
            .ok()?;
        let enabled: u32 = settings.get_value("ProxyEnable").ok()?;
        if enabled == 0 {
            return None;
        }
        let server: String = settings.get_value("ProxyServer").ok()?;
        // Either `host:port` or a per-protocol list: `http=a:1;https=b:2`.
        if !server.contains('=') {
            return Some(server);
        }
        for entry in server.split(';') {
            if let Some(rest) = entry.trim().strip_prefix("https=") {
                return Some(rest.to_string());
            }
        }
        for entry in server.split(';') {
            if let Some(rest) = entry.trim().strip_prefix("http=") {
                return Some(rest.to_string());
            }
        }
        None
    }
    #[cfg(not(windows))]
    None
}

fn agent() -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new()
        .timeout(TIMEOUT)
        .user_agent(USER_AGENT);
    if let Some(proxy) = system_proxy() {
        match ureq::Proxy::new(&proxy) {
            Ok(proxy) => builder = builder.proxy(proxy),
            // A malformed proxy setting is worth a line in the log, not a
            // failed update check: the connection may well work without one.
            Err(e) => log::warn!("ignoring the system proxy {proxy:?}: {e}"),
        }
    }
    builder.build()
}

/// The file name fragment that marks an asset as this platform's build.
fn asset_marker() -> &'static str {
    if cfg!(windows) {
        ".exe"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    }
}

/// Asks GitHub what the latest release is.
pub fn latest() -> Result<Release> {
    let url = releases_url()
        .ok_or_else(|| anyhow!("{REPOSITORY} is not a GitHub repository this can ask about"))?;
    let body: serde_json::Value = agent()
        .get(&url)
        .call()
        .context("asking GitHub for the latest release")?
        .into_json()
        .context("reading GitHub's answer")?;

    let version = body
        .get("tag_name")
        .and_then(|v| v.as_str())
        .ok_or_else(|| anyhow!("the release has no tag"))?
        .trim_start_matches(['v', 'V'])
        .to_string();

    let download = body
        .get("assets")
        .and_then(|a| a.as_array())
        .into_iter()
        .flatten()
        .find_map(|asset| {
            let name = asset.get("name")?.as_str()?;
            name.contains(asset_marker())
                .then(|| asset.get("browser_download_url")?.as_str())
                .flatten()
        })
        .ok_or_else(|| anyhow!("release {version} has no build for this platform"))?
        .to_string();

    let notes = body
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();

    Ok(Release {
        version,
        download,
        notes,
    })
}

/// Runs the check on a thread and reports back.
///
/// Never returns an error: a failed check is reported as an [`Event`] and the
/// app carries on. It is a convenience, not a dependency.
pub fn check_in_background(tx: Sender<Event>) {
    std::thread::spawn(move || {
        let event = match latest() {
            Ok(release) if is_newer(&release.version, CURRENT) => Event::Available(release),
            Ok(_) => Event::UpToDate,
            Err(e) => Event::Failed(format!("{e:#}")),
        };
        let _ = tx.send(event);
    });
}

/// Downloads the release's executable beside the running one, on a thread.
///
/// Beside it rather than in a temp folder: the swap is a rename, and a rename
/// only works within one filesystem.
pub fn download_in_background(release: Release, tx: Sender<Event>) {
    std::thread::spawn(move || {
        let event = match download(&release) {
            Ok(path) => Event::Downloaded(path),
            Err(e) => Event::Failed(format!("{e:#}")),
        };
        let _ = tx.send(event);
    });
}

fn download(release: &Release) -> Result<PathBuf> {
    let exe = std::env::current_exe().context("finding the running executable")?;
    let staged = staged_path(&exe);

    let response = agent()
        .get(&release.download)
        .call()
        .with_context(|| format!("downloading {}", release.download))?;
    let mut reader = response.into_reader();
    let mut file =
        std::fs::File::create(&staged).with_context(|| format!("creating {}", staged.display()))?;
    let copied = std::io::copy(&mut reader, &mut file)
        .with_context(|| format!("writing {}", staged.display()))?;
    drop(file);

    // A truncated download that still parsed as a file would be swapped in and
    // then refuse to start, which is the one failure with no way back.
    if copied < 1_000_000 {
        let _ = std::fs::remove_file(&staged);
        bail!("the download stopped after {copied} bytes");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755))
            .context("making the download executable")?;
    }
    Ok(staged)
}

fn staged_path(exe: &Path) -> PathBuf {
    let mut name = exe.as_os_str().to_os_string();
    name.push(".new");
    PathBuf::from(name)
}

fn old_path(exe: &Path) -> PathBuf {
    let mut name = exe.as_os_str().to_os_string();
    name.push(OLD_SUFFIX);
    PathBuf::from(name)
}

/// Puts the downloaded build in place and starts it.
///
/// A running executable cannot be overwritten on Windows, but it *can* be
/// renamed out of the way - so the running one is moved aside, the new one
/// takes its name, and the copy left behind is deleted by [`clean_up`] at the
/// next start. Every step is undone if the next one fails: a half-applied
/// update would leave the user with no working executable at all.
///
/// The caller closes the app; this does not, because only the caller knows
/// whether a session is still connected.
pub fn install(staged: &Path) -> Result<()> {
    let exe = std::env::current_exe().context("finding the running executable")?;
    let old = old_path(&exe);
    let _ = std::fs::remove_file(&old);

    std::fs::rename(&exe, &old).with_context(|| format!("moving {} aside", exe.display()))?;
    if let Err(e) = std::fs::rename(staged, &exe) {
        // Put it back: without this the app has just deleted itself.
        let _ = std::fs::rename(&old, &exe);
        return Err(e).with_context(|| format!("putting {} in place", exe.display()));
    }
    if let Err(e) = std::process::Command::new(&exe).spawn() {
        let _ = std::fs::rename(&exe, staged);
        let _ = std::fs::rename(&old, &exe);
        return Err(e).context("starting the new version");
    }
    Ok(())
}

/// Deletes what the last update left behind. Called at startup, when the file
/// is no longer running and can go.
pub fn clean_up() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let _ = std::fs::remove_file(old_path(&exe));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The one that had already gone wrong: the updater was asking a
    /// repository that is not the one this is published from.
    #[test]
    fn the_releases_url_follows_the_repository() {
        let url = releases_url().expect("a GitHub repository");
        assert!(
            url.starts_with("https://api.github.com/repos/"),
            "unexpected: {url}"
        );
        assert!(url.ends_with("/releases/latest"), "unexpected: {url}");
        // Whatever `Cargo.toml` says, it has to be what the URL asks for.
        let slug = REPOSITORY.trim_end_matches('/').split("github.com/").nth(1);
        assert_eq!(
            url,
            format!(
                "https://api.github.com/repos/{}/releases/latest",
                slug.expect("a github.com URL")
            )
        );
    }

    #[test]
    fn a_later_version_is_newer() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.1.1", "0.1.0"));
        assert!(is_newer("1.0.0", "0.9.9"));
        assert!(is_newer("v0.2.0", "0.1.0"), "the tag's v must not matter");
    }

    /// The check runs at every start, so "the same version" has to be the
    /// quiet case rather than an update offered forever.
    #[test]
    fn the_same_version_is_not_newer() {
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("0.1", "0.1.0"), "a missing number is a zero");
    }

    /// A tag nobody can parse must not read as an update: it would offer a
    /// download on every start and never stop.
    #[test]
    fn nonsense_is_not_newer() {
        assert!(!is_newer("", "0.1.0"));
        assert!(!is_newer("latest", "0.1.0"));
        assert!(!is_newer("not.a.version", "0.1.0"));
    }

    #[test]
    fn a_release_tag_is_read_as_numbers() {
        assert_eq!(parts("v1.2.3"), vec![1, 2, 3]);
        // A pre-release suffix reduces to the version it is a candidate for,
        // which is what the comparison above is documented to do with one.
        assert_eq!(parts("0.1.0-rc1"), vec![0, 1, 0]);
        assert_eq!(parts("2024.10"), vec![2024, 10]);
    }

    /// The staged download and the file the running build is parked in have to
    /// be beside the executable, or the rename that swaps them cannot work.
    #[test]
    fn the_swap_happens_in_one_directory() {
        let exe = Path::new("C:/apps/new-iris-terminal.exe");
        assert_eq!(staged_path(exe).parent(), exe.parent());
        assert_eq!(old_path(exe).parent(), exe.parent());
        assert_ne!(staged_path(exe), old_path(exe));
        assert_ne!(staged_path(exe), exe.to_path_buf());
    }

    /// This build's own version must be one the comparison understands, or
    /// every release would look older than it.
    #[test]
    fn this_build_has_a_readable_version() {
        assert!(!parts(CURRENT).is_empty(), "unreadable version: {CURRENT}");
        assert!(is_newer("999.0.0", CURRENT));
    }
}
