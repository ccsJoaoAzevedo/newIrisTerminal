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
//!
//! What the system cannot tell us is the *credentials*, and a proxy can want
//! them for the download while letting the check through: GitHub serves release
//! assets from a host of their own, and a Squid with per-host rules will answer
//! `407` for that one and pass `api.github.com` anonymously. That is why there
//! is a proxy user in the settings and a password in the credential store —
//! see [`proxy_credentials`] — and why a refusal now says which of the two it
//! was. Only Basic authentication: neither `ureq` nor the `curl.exe` Windows
//! ships can do NTLM, so a proxy that insists on it is one the app cannot get
//! past, and the dialog offers the browser instead.
//!
//! `tests/live_update.rs` is where all of this is checked against the real
//! repository, and it is the only way any of it can be.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

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

/// How long to wait on the check before giving up. A failed check is a
/// non-event — it must never be something the user waits for.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// How long a stalled *download* is given before it counts as dead.
///
/// Per read rather than for the whole transfer, which is the bug this replaced:
/// `AgentBuilder::timeout` is documented as covering "the overall request,
/// including ... reading the response body", so the check's twenty seconds were
/// also the entire budget for fetching a twelve-megabyte executable. Anything
/// slower than about 600 KB/s - a proxy, a virus scanner reading the stream,
/// a busy morning - was cut off partway through and reported as a failure, so
/// the update never downloaded and therefore never installed. A download is
/// allowed to be slow; it is only not allowed to be silent.
const DOWNLOAD_STALL: std::time::Duration = std::time::Duration::from_secs(60);

/// The suffix a replaced executable is parked under until the next start.
const OLD_SUFFIX: &str = ".old";

/// A published release, reduced to what the app does something with.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Release {
    /// As published, `v` stripped: `0.2.0`.
    pub version: String,
    /// Direct download for this platform's executable.
    pub download: String,
    /// How big that download is, as GitHub reports it. `0` when the release
    /// does not say, which is what the dialog checks before showing a share of
    /// it rather than a byte count.
    pub size: u64,
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

/// Where the proxy username and password are kept.
///
/// The name is the account under [`crate::config::profile::KEYRING_SERVICE`],
/// beside the profile passwords: a proxy password is a credential, and this app
/// already has one place for those. Nothing about it goes into settings.toml.
pub const PROXY_KEYRING_ACCOUNT: &str = "http-proxy";

/// The proxy username, if one has been set, and the password stored for it.
///
/// Needed because a Squid answering `407` will not let the download past
/// without it. It is the failure this updater was actually stuck on: the
/// release asset is served from a host the proxy demands authentication for,
/// so the check - to an allowed host - succeeded and the download never did,
/// which looked exactly like a Download button that did nothing.
fn proxy_credentials(user: &str) -> Option<(String, String)> {
    let user = user.trim();
    if user.is_empty() {
        return None;
    }
    let password = keyring::Entry::new(
        crate::config::profile::KEYRING_SERVICE,
        PROXY_KEYRING_ACCOUNT,
    )
    .ok()?
    .get_password()
    .ok()?;
    Some((user.to_string(), password))
}

/// Stores the proxy password, or forgets it when `password` is empty.
pub fn set_proxy_password(password: &str) -> Result<()> {
    let entry = keyring::Entry::new(
        crate::config::profile::KEYRING_SERVICE,
        PROXY_KEYRING_ACCOUNT,
    )?;
    if password.is_empty() {
        // An empty value means "forget this", so clearing the field in the
        // settings window really removes the secret rather than storing "".
        let _ = entry.delete_password();
        Ok(())
    } else {
        entry.set_password(password)?;
        Ok(())
    }
}

/// Whether a proxy password is on file, without reading it out.
pub fn has_proxy_password() -> bool {
    keyring::Entry::new(
        crate::config::profile::KEYRING_SERVICE,
        PROXY_KEYRING_ACCOUNT,
    )
    .ok()
    .and_then(|entry| entry.get_password().ok())
    .is_some_and(|password| !password.is_empty())
}

/// The username the agents authenticate to the proxy as, set by
/// [`configure_proxy_user`] from the settings.
///
/// A global because the two agents are built from functions the background
/// threads call with nothing else to hand, and threading the settings into
/// them would mean carrying a copy of the whole `Settings` into every check.
/// Written once, at startup and whenever the setting changes.
static PROXY_USER: std::sync::RwLock<String> = std::sync::RwLock::new(String::new());

/// Tells the updater which user to authenticate to the proxy as.
pub fn configure_proxy_user(user: &str) {
    if let Ok(mut current) = PROXY_USER.write() {
        *current = user.trim().to_string();
    }
}

fn proxy_user() -> String {
    PROXY_USER
        .read()
        .map(|user| user.clone())
        .unwrap_or_default()
}

/// The proxy to reach the internet through, as `host:port`.
///
/// Read rather than configured: the machines this runs on are handed their
/// proxy by policy, and a second place to configure it is a second place for it
/// to be wrong. The *credentials* are configured, because there is nowhere to
/// read those from - see [`proxy_credentials`].
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

/// The agent for the check: one small JSON GET, with a deadline on the whole
/// thing because the user may be waiting on the answer.
fn agent() -> ureq::Agent {
    build_agent(ureq::AgentBuilder::new().timeout(TIMEOUT))
}

/// The agent for the download: no overall deadline, and a per-read one instead.
///
/// The distinction is the whole of [`DOWNLOAD_STALL`]: a twelve-megabyte
/// transfer over a corporate proxy takes as long as it takes, and capping the
/// total time is how the updater ended up never getting a file at all. What is
/// still capped is a connection that has stopped saying anything, which is the
/// failure worth giving up on.
fn download_agent() -> ureq::Agent {
    build_agent(
        ureq::AgentBuilder::new()
            .timeout_connect(TIMEOUT)
            .timeout_read(DOWNLOAD_STALL)
            .timeout_write(DOWNLOAD_STALL),
    )
}

/// The proxy as ureq wants to be told about it: `user:password@host:port`.
///
/// The shape matters more than it looks. ureq splits the credentials off at the
/// *last* `@` and the username off at the *first* `:`, so a password holding
/// either character survives - which real ones do. A username holding one
/// would not, and there is nothing this can do about that: the format has no
/// escaping.
fn proxy_spec(host: &str, credentials: Option<(String, String)>) -> String {
    match credentials {
        Some((user, password)) => format!("{}:{}@{}", user.trim(), password, host.trim()),
        None => host.trim().to_string(),
    }
}

/// The half both agents share: the user agent GitHub insists on, and the
/// machine's own proxy.
fn build_agent(builder: ureq::AgentBuilder) -> ureq::Agent {
    let mut builder = builder.user_agent(USER_AGENT);
    if let Some(host) = system_proxy() {
        // Credentials go in the URL, which is how ureq is told about them; they
        // reach the proxy as a `Proxy-Authorization: Basic` header. Only Basic
        // - ureq cannot do NTLM, and neither can the `curl.exe` Windows ships
        // - so a proxy offering only NTLM is one this cannot get past, and the
        // dialog says so rather than failing quietly.
        let spec = proxy_spec(&host, proxy_credentials(&proxy_user()));
        match ureq::Proxy::new(&spec) {
            Ok(proxy) => builder = builder.proxy(proxy),
            // A malformed proxy setting is worth a line in the log, not a
            // failed update check: the connection may well work without one.
            // Logged without the spec, which would carry the password.
            Err(e) => log::warn!("ignoring the system proxy {host:?}: {e}"),
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

    let (download, size) = body
        .get("assets")
        .and_then(|a| a.as_array())
        .into_iter()
        .flatten()
        .find_map(|asset| {
            let name = asset.get("name")?.as_str()?;
            if !name.contains(asset_marker()) {
                return None;
            }
            let url = asset.get("browser_download_url")?.as_str()?.to_string();
            // Absent rather than fatal: the size is only used to show how far
            // the download has got, and a release without one is still a
            // release worth installing.
            let size = asset.get("size").and_then(|v| v.as_u64()).unwrap_or(0);
            Some((url, size))
        })
        .ok_or_else(|| anyhow!("release {version} has no build for this platform"))?;

    let notes = body
        .get("body")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .trim()
        .to_string();

    Ok(Release {
        version,
        download,
        size,
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
///
/// `progress` is written to as the bytes arrive, so the dialog can say how far
/// it has got. A twelve-megabyte transfer through a proxy is long enough that
/// a spinner alone leaves no way to tell working from hung - which is exactly
/// the state this updater was reported to be stuck in.
pub fn download_in_background(release: Release, progress: Arc<AtomicU64>, tx: Sender<Event>) {
    std::thread::spawn(move || {
        let event = match download(&release, &progress) {
            Ok(path) => Event::Downloaded(path),
            Err(e) => {
                log::warn!("downloading {} failed: {e:#}", release.download);
                Event::Failed(format!("{e:#}"))
            }
        };
        let _ = tx.send(event);
    });
}

fn download(release: &Release, progress: &AtomicU64) -> Result<PathBuf> {
    let exe = std::env::current_exe().context("finding the running executable")?;
    let staged = staged_path(&exe);
    fetch(release, &staged, progress)?;
    Ok(staged)
}

/// Downloads the release's asset to `into`, reporting progress as it goes.
///
/// Public so `tests/live_update.rs` can do exactly what the app does without
/// touching the running executable: the download is the half of the updater
/// that goes wrong, and it is the half that cannot be checked without the
/// network.
///
/// The file is removed again if what arrived is too small to be an executable.
/// A truncated download that still parsed as a file would be swapped in and
/// then refuse to start, which is the one failure with no way back.
pub fn fetch(release: &Release, into: &Path, progress: &AtomicU64) -> Result<u64> {
    let response = download_agent()
        .get(&release.download)
        .call()
        .map_err(proxy_hint)
        .with_context(|| format!("downloading {}", release.download))?;
    let mut reader = response.into_reader();
    let mut file =
        std::fs::File::create(into).with_context(|| format!("creating {}", into.display()))?;
    let copied = copy_reporting(&mut reader, &mut file, progress)
        .with_context(|| format!("writing {}", into.display()))?;
    drop(file);

    if copied < 1_000_000 {
        let _ = std::fs::remove_file(into);
        bail!("the download stopped after {copied} bytes");
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(into, std::fs::Permissions::from_mode(0o755))
            .context("making the download executable")?;
    }
    Ok(copied)
}

/// Turns a proxy refusal into something that says what to do about it.
///
/// ureq reports a `407` as "Provided proxy credentials are incorrect", which is
/// actively misleading when none were provided at all - and that is the usual
/// case here, because the release asset is served from a host a proxy may ask
/// for credentials for even when the check's host is allowed anonymously. This
/// is the message that turned a mystery into a setting.
fn proxy_hint(error: ureq::Error) -> anyhow::Error {
    if error.kind() != ureq::ErrorKind::ProxyUnauthorized {
        return error.into();
    }
    let proxy = system_proxy().unwrap_or_else(|| "the proxy".to_string());
    let user = proxy_user();
    if user.is_empty() {
        anyhow!(
            "{proxy} wants credentials for this host. Set the proxy user and password \
             under Updates in Settings, or fetch the release from a browser."
        )
    } else {
        anyhow!(
            "{proxy} rejected the credentials for {user}. Check the user and password \
             under Updates in Settings; only Basic authentication is supported, not NTLM."
        )
    }
}

/// `std::io::copy`, but saying how far it has got as it goes.
///
/// The reason it is not `std::io::copy`: that one reports the total once, at
/// the end, which is no use to a progress bar and no use at all when the thing
/// being diagnosed is a transfer that never finishes.
fn copy_reporting(
    reader: &mut impl std::io::Read,
    writer: &mut impl std::io::Write,
    progress: &AtomicU64,
) -> std::io::Result<u64> {
    // Big enough that the syscalls are not the cost, small enough that the
    // number on screen moves.
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut total = 0_u64;
    loop {
        let read = match reader.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            // A signal, not an error: the read is worth trying again.
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        writer.write_all(&buffer[..read])?;
        total += read as u64;
        progress.store(total, Ordering::Relaxed);
    }
    writer.flush()?;
    Ok(total)
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

    /// The proxy spec, and the two characters a real password is likely to
    /// hold. ureq splits at the last `@` and the first `:`, so both survive -
    /// and this is the test that says so, because getting it wrong would send
    /// a truncated password to the proxy and report a plain `407`.
    #[test]
    fn proxy_credentials_survive_the_characters_passwords_have_in_them() {
        assert_eq!(proxy_spec("host:3128", None), "host:3128");

        let spec = proxy_spec(" host:3128 ", Some(("someone".into(), "p@ss:w0rd".into())));
        assert_eq!(spec, "someone:p@ss:w0rd@host:3128");
        let parsed = ureq::Proxy::new(&spec).expect("ureq should accept this");
        // Reading the parsed fields back is the only way to be sure ureq took
        // it the way it was meant: what reaches the proxy is built from these.
        assert!(format!("{parsed:?}").contains("p@ss:w0rd"));
    }

    /// The copy that replaced `std::io::copy` has to report as it goes, not
    /// once at the end: what it feeds is a progress bar, and a transfer that
    /// only reports when it finishes is exactly the one nobody could tell from
    /// a hung download.
    #[test]
    fn a_download_says_how_far_it_has_got_while_it_is_going() {
        // Two buffers' worth and a bit, so there is more than one report.
        let source = vec![7_u8; 64 * 1024 * 2 + 11];
        let progress = AtomicU64::new(0);
        let mut out: Vec<u8> = Vec::new();
        let copied = copy_reporting(&mut source.as_slice(), &mut out, &progress).expect("copy");

        assert_eq!(copied, source.len() as u64);
        assert_eq!(out, source, "and every byte has to arrive");
        assert_eq!(
            progress.load(Ordering::Relaxed),
            source.len() as u64,
            "the last report is the total"
        );
    }

    /// An empty body is a failed download, not a file: it must report zero
    /// rather than succeeding at nothing.
    #[test]
    fn an_empty_body_copies_nothing() {
        let progress = AtomicU64::new(0);
        let mut out: Vec<u8> = Vec::new();
        let copied = copy_reporting(&mut [].as_slice(), &mut out, &progress).expect("copy");
        assert_eq!(copied, 0);
        assert_eq!(progress.load(Ordering::Relaxed), 0);
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
