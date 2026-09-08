//! What GitHub actually answers, and whether the download really arrives.
//!
//! Ignored by default — both tests need the network, and the second one moves
//! twelve megabytes over it. Run with:
//!
//! ```text
//! cargo test --test live_update -- --ignored --nocapture
//! ```
//!
//! Read-only against the repository: one API call and one asset download into
//! a temporary directory. Nothing is installed and the running executable is
//! never touched.
//!
//! The point of the tests is the regression behind them: the update checked
//! successfully and then never downloaded. Two separate things were wrong, and
//! only running this on the network in question told them apart.
//!
//! The first was a deadline. `AgentBuilder::timeout` covers "the overall
//! request, including ... reading the response body", so the twenty seconds
//! meant for a small JSON call were also the entire budget for fetching a
//! twelve-megabyte executable — anything slower than about 600 KB/s was cut
//! off partway through.
//!
//! The second is the one this machine actually hits, and it is not the app's
//! fault: GitHub serves release assets from `release-assets.githubusercontent.com`,
//! and the Squid here answers `407` for that host while letting
//! `api.github.com` through anonymously. So the check works and the download
//! cannot, until the proxy is given credentials. Both failures used to be
//! swallowed — the dialog just showed its Download button again, which reads
//! as a button that does nothing.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;

use new_iris_terminal::features::update;

/// Authenticates to the proxy as whatever the app is configured to use, so the
/// tests exercise the path the app takes.
///
/// The username comes from `NIT_PROXY_USER` when it is set, so the download can
/// be tried without opening the settings window; the password always comes from
/// the credential store, because that is the only place it is ever kept.
fn use_configured_proxy() {
    let user = std::env::var("NIT_PROXY_USER").unwrap_or_default();
    update::configure_proxy_user(&user);
    if let Some(proxy) = update::system_proxy() {
        let who = if user.is_empty() {
            "anonymously".to_string()
        } else {
            format!(
                "as {user} (password {})",
                if update::has_proxy_password() {
                    "on file"
                } else {
                    "MISSING"
                }
            )
        };
        println!("through the proxy {proxy}, {who}");
    } else {
        println!("no proxy configured; connecting directly");
    }
}

/// The check: the release is found, the platform's asset is picked out of it,
/// and it says how big it is.
///
/// The size is what the dialog shows progress against, so a release that stops
/// reporting one is worth knowing about even though the download still works.
#[test]
#[ignore = "needs the network"]
fn the_latest_release_names_a_build_for_this_platform() {
    use_configured_proxy();
    let started = Instant::now();
    let release = update::latest().expect("asking GitHub for the latest release");
    println!(
        "{:.1}s: version {} at {}",
        started.elapsed().as_secs_f32(),
        release.version,
        release.download
    );
    println!("  size {} bytes", release.size);

    assert!(!release.version.is_empty(), "the release has no version");
    assert!(
        release.download.starts_with("https://"),
        "not a download URL: {}",
        release.download
    );
    assert!(
        release.size > 1_000_000,
        "an executable this small is not one: {} bytes",
        release.size
    );
    // This build against that one, which is the comparison the app makes at
    // startup. Either answer is correct - it depends which is newer - so what
    // is checked is only that the version parses as something comparable.
    println!(
        "  newer than this build ({}): {}",
        update::CURRENT,
        update::is_newer(&release.version, update::CURRENT)
    );
}

/// The download: every byte arrives, and the progress counter keeps up.
///
/// The one that would have caught the bug. It reports the rate it managed,
/// which is the number the old twenty-second deadline was implicitly demanding
/// be over 600 KB/s.
#[test]
#[ignore = "needs the network and moves ~12 MB"]
fn the_release_asset_downloads_whole() {
    use_configured_proxy();
    let release = update::latest().expect("asking GitHub for the latest release");
    let dir = std::env::temp_dir().join("nit-update-test");
    std::fs::create_dir_all(&dir).expect("a directory to download into");
    let path = dir.join("asset.bin");

    let progress = Arc::new(AtomicU64::new(0));
    let started = Instant::now();
    let copied = match update::fetch(&release, &path, &progress) {
        Ok(copied) => copied,
        Err(e) => {
            // Said out loud rather than left as a bare `407`: this is the
            // failure the whole file exists for, and the message is what tells
            // a blocked host apart from a wrong password.
            let why = format!("{e:#}");
            if why.contains("proxy") || why.contains("407") {
                println!("The proxy would not pass the download.");
                println!("GitHub serves release assets from a host this proxy asks for");
                println!("credentials for. Set the proxy user in Settings and its password");
                println!("in the credential store (NIT_PROXY_USER names the user here), or");
                println!("fetch the release from a browser.");
            }
            panic!("downloading the asset: {why}");
        }
    };
    let took = started.elapsed();

    let written = std::fs::metadata(&path).expect("the downloaded file").len();
    let rate = written as f64 / took.as_secs_f64() / 1024.0;
    println!(
        "{} bytes in {:.1}s ({rate:.0} KB/s) to {}",
        written,
        took.as_secs_f32(),
        path.display()
    );

    assert_eq!(copied, written, "what it reported and what it wrote differ");
    assert_eq!(
        written, release.size,
        "the download is not the size GitHub said it was"
    );
    assert_eq!(
        progress.load(Ordering::Relaxed),
        written,
        "the progress counter and the file disagree"
    );
    // A Windows executable starts `MZ`; the others are ELF or Mach-O. Only the
    // first is checked, and only on Windows, because a redirect page or an
    // error document landing in the file is the failure worth catching and it
    // would start with neither.
    if cfg!(windows) {
        let head = std::fs::read(&path).expect("reading it back");
        assert_eq!(&head[..2], b"MZ", "this is not a Windows executable");
    }
    let _ = std::fs::remove_file(&path);
}
