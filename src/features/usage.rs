//! The usage report: which settings differ from a fresh install, and how much
//! of the rest is in use, sent by e-mail to someone the user names.
//!
//! What it is for is deciding what to keep. An option nobody changes is a
//! candidate to remove, one everybody changes the same way is a candidate to
//! become the default, and the only way to know which is which is to ask the
//! people using it. So the app asks - once per new version, and never sends
//! anything by itself: the report is put into an e-mail in the user's own mail
//! program, addressed to whoever the user typed, and the user sends it.
//!
//! Only what says how the app is used goes in. The settings file itself is
//! not sent: it holds the profiles, and a profile is a server's address, a
//! user name and a namespace - nothing anybody deciding about features needs,
//! and not anyone else's to read. So the report is a diff against the
//! defaults, with every value that names a person, a place or a path either
//! left out or reduced to "changed", and the profiles, macros and themes
//! counted rather than listed. The command history is never touched.

use std::collections::BTreeMap;

use anyhow::{bail, Result};

use crate::config::Settings;

/// Settings that are not about using the app at all: where its windows were,
/// what the report itself is, and the profiles, which are counted instead.
const LEFT_OUT: &[&str] = &[
    "profiles",
    "window_size",
    "window_position",
    "window_maximized",
    "settings_window_size",
    "settings_window_position",
    "usage_report_email",
    "last_version",
];

/// Settings whose being changed says something and whose value says too much:
/// a profile's name, a proxy user, a folder, the text a screen saver shows.
const VALUE_HIDDEN: &[&str] = &[
    "default_profile",
    "proxy_user",
    "log_dir",
    "org_macros_path",
    "screensaver.logo_text",
    "screensaver.logo_image",
    "screensaver.dvd_text",
];

/// What the rest of the app knows and the settings do not.
#[derive(Clone, Debug, Default)]
pub struct Inventory {
    pub personal_macros: usize,
    /// The organisation's macro file's macros, when there is one.
    pub organisation_macros: Option<usize>,
    /// Themes of the user's own, beside the built-in ones.
    pub own_themes: usize,
}

/// The report, as plain text: the body of the e-mail.
pub fn report(settings: &Settings, inventory: &Inventory) -> String {
    let mut out = format!(
        "newIrisTerminal {} - usage report\n\
         Only what differs from a fresh install. Paths, names, addresses and \
         window positions are left out.\n\n",
        crate::features::update::CURRENT
    );

    out.push_str("[settings changed from the default]\n");
    let changed = changed(settings);
    if changed.is_empty() {
        out.push_str("(none)\n");
    }
    for (key, now, default) in changed {
        out.push_str(&format!("{key} = {now}   (default {default})\n"));
    }

    let profiles = &settings.profiles;
    let count = |keep: &dyn Fn(&crate::config::Profile) -> bool| {
        profiles.iter().filter(|p| keep(p)).count()
    };
    out.push_str("\n[profiles]\n");
    out.push_str(&format!(
        "local IRIS: {}\n",
        count(&|p| p.remote.is_none() && !p.is_shell())
    ));
    out.push_str(&format!(
        "remote (Telnet): {}\n",
        count(&|p| p.remote.is_some())
    ));
    out.push_str(&format!("shell: {}\n", count(&|p| p.is_shell())));
    out.push_str(&format!("with autologon: {}\n", count(&|p| p.autologon)));
    out.push_str(&format!(
        "with commands after login: {}\n",
        count(&|p| !p.post_login.is_empty())
    ));
    out.push_str(&format!(
        "with a transcript of their own: {}\n",
        count(&|p| p.logging != crate::config::LogMode::Off)
    ));
    out.push_str(&format!(
        "with a macro file of their own: {}\n",
        count(&|p| p.macro_file.is_some())
    ));

    out.push_str("\n[macros]\n");
    out.push_str(&format!("personal: {}\n", inventory.personal_macros));
    match inventory.organisation_macros {
        Some(n) => out.push_str(&format!("organisation file: {n}\n")),
        None => out.push_str("organisation file: none\n"),
    }

    out.push_str("\n[themes]\n");
    out.push_str(&format!("own themes: {}\n", inventory.own_themes));
    out
}

/// Every setting that differs from the default, as `(key, now, default)`,
/// in the order of the keys. A nested table's keys are dotted:
/// `screensaver.kind`.
pub fn changed(settings: &Settings) -> Vec<(String, String, String)> {
    let (Ok(now), Ok(default)) = (
        toml::Value::try_from(settings),
        toml::Value::try_from(Settings::default()),
    ) else {
        return Vec::new();
    };
    let mut now_flat = BTreeMap::new();
    let mut default_flat = BTreeMap::new();
    flatten("", &now, &mut now_flat);
    flatten("", &default, &mut default_flat);

    let mut out = Vec::new();
    for (key, value) in &now_flat {
        let top = key.split('.').next().unwrap_or_default();
        if LEFT_OUT.contains(&top) {
            continue;
        }
        let before = default_flat.get(key);
        if before == Some(value) {
            continue;
        }
        let hidden = VALUE_HIDDEN.contains(&key.as_str()) || looks_personal(value);
        let shown = |v: Option<&toml::Value>| match v {
            None => "(none)".to_string(),
            Some(_) if hidden => "(changed)".to_string(),
            Some(v) => display(v),
        };
        let default = match before {
            Some(_) if hidden => "(other)".to_string(),
            other => shown(other),
        };
        out.push((key.clone(), shown(Some(value)), default));
    }
    // A setting the default has and this file does not - an `Option` left
    // empty - is a change as well.
    for (key, value) in &default_flat {
        let top = key.split('.').next().unwrap_or_default();
        if !LEFT_OUT.contains(&top) && !now_flat.contains_key(key) {
            out.push((key.clone(), "(none)".to_string(), display(value)));
        }
    }
    out.sort();
    out
}

fn flatten(prefix: &str, value: &toml::Value, out: &mut BTreeMap<String, toml::Value>) {
    match value {
        toml::Value::Table(table) => {
            for (key, value) in table {
                let key = if prefix.is_empty() {
                    key.clone()
                } else {
                    format!("{prefix}.{key}")
                };
                flatten(&key, value, out);
            }
        }
        other => {
            out.insert(prefix.to_string(), other.clone());
        }
    }
}

/// A value that names somewhere or someone, whatever setting it is in: a
/// path, or an address.
fn looks_personal(value: &toml::Value) -> bool {
    match value {
        toml::Value::String(s) => s.contains(['\\', '/', '@']),
        toml::Value::Array(items) => items.iter().any(looks_personal),
        _ => false,
    }
}

/// A value as it reads in the report. A float is rounded: the settings hold
/// `f32`s, and `1.25` read back as an `f64` is `1.25`, but `1.1` is
/// `1.100000023841858`.
fn display(value: &toml::Value) -> String {
    match value {
        toml::Value::Float(f) => {
            let text = format!("{f:.3}");
            text.trim_end_matches('0').trim_end_matches('.').to_string()
        }
        other => other.to_string(),
    }
}

/// Whether `address` could be an e-mail address. Only enough to keep a
/// half-typed one from opening a mail program addressed to nobody.
pub fn plausible_address(address: &str) -> bool {
    let address = address.trim();
    let Some((user, domain)) = address.split_once('@') else {
        return false;
    };
    !user.is_empty()
        && domain.contains('.')
        && !domain.starts_with('.')
        && !domain.ends_with('.')
        && !address.contains(char::is_whitespace)
        && !domain.contains('@')
}

/// How long a `mailto:` link may get before a mail program is likely to cut
/// it off. Outlook's limit is about two thousand characters; the body of a
/// longer one goes by the clipboard instead.
pub const MAILTO_LIMIT: usize = 1_900;

/// A `mailto:` link with the address, subject and body filled in.
pub fn mailto(to: &str, subject: &str, body: &str) -> String {
    format!(
        "mailto:{}?subject={}&body={}",
        percent(to.trim()),
        percent(subject),
        percent(&body.replace("\r\n", "\n").replace('\n', "\r\n"))
    )
    // An address keeps its `@`: some mail programs do not decode the part
    // before the `?`.
    .replacen("%40", "@", 1)
}

/// Percent-encoding for a `mailto:` link: everything but the unreserved
/// characters, as UTF-8.
fn percent(text: &str) -> String {
    let mut out = String::with_capacity(text.len() * 3);
    for byte in text.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'~') {
            out.push(byte as char);
        } else {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// Hands a link to whatever the system opens that kind of link with - for a
/// `mailto:`, the default mail program.
#[cfg(windows)]
pub fn open(url: &str) -> Result<()> {
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (verb, target) = (wide("open"), wide(url));
    // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call,
    // and every other argument is the documented "none".
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    // Anything above 32 is success; below it, the reason - most often that no
    // program is set to open `mailto:` links at all.
    if result as isize <= 32 {
        bail!("no program is set to open {} links", scheme(url));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn open(url: &str) -> Result<()> {
    let program = if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    match std::process::Command::new(program).arg(url).spawn() {
        Ok(_) => Ok(()),
        Err(e) => bail!("could not open {} links: {e}", scheme(url)),
    }
}

fn scheme(url: &str) -> &str {
    url.split(':').next().unwrap_or(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fresh_install_has_nothing_changed() {
        assert!(changed(&Settings::default()).is_empty());
    }

    #[test]
    fn a_changed_setting_is_reported_with_its_default() {
        let settings = Settings {
            autocomplete_mode: crate::config::AutocompleteMode::DataOnly,
            font_size: 16.0,
            ..Settings::default()
        };
        let changed = changed(&settings);
        assert!(
            changed.contains(&(
                "autocomplete_mode".into(),
                "\"data\"".into(),
                "\"full\"".into()
            )),
            "{changed:?}"
        );
        assert!(
            changed.contains(&("font_size".into(), "16".into(), "14".into())),
            "{changed:?}"
        );
    }

    /// The report is read by someone else: nothing in it may say where the
    /// user's servers are, who they log in as, or where their files live.
    #[test]
    fn nothing_that_names_a_person_or_a_place_is_reported() {
        let mut settings = Settings {
            proxy_user: "jsilva".into(),
            default_profile: "Cliente X".into(),
            usage_report_email: "someone@example.com".into(),
            window_position: Some([10.0, 20.0]),
            org_macros_path: "\\\\server\\share\\macros.xml".into(),
            ..Settings::default()
        };
        settings.profiles.push(crate::config::Profile {
            name: "Cliente X".into(),
            username: "jsilva".into(),
            ..crate::config::Profile::default()
        });
        settings.screensaver.logo_text = "Joana".into();
        let text = report(&settings, &Inventory::default());
        for secret in ["jsilva", "Cliente X", "someone@", "server", "Joana", "10.0"] {
            assert!(!text.contains(secret), "{secret:?} in:\n{text}");
        }
        // Still said to have been changed, which is what the report is for.
        assert!(text.contains("proxy_user = (changed)"), "{text}");
        assert!(text.contains("local IRIS: 1"), "{text}");
    }

    #[test]
    fn a_float_reads_as_it_was_typed() {
        let settings = Settings {
            ui_scale: 1.1,
            ..Settings::default()
        };
        let changed = changed(&settings);
        assert!(
            changed.contains(&("ui_scale".into(), "1.1".into(), "1".into())),
            "{changed:?}"
        );
    }

    #[test]
    fn only_something_shaped_like_an_address_is_sent_to() {
        assert!(plausible_address("lucas@consistem.com.br"));
        assert!(plausible_address("  a@b.co "));
        for bad in [
            "", "lucas", "lucas@", "@b.co", "a@b", "a b@c.d", "a@.co", "a@b@c.d",
        ] {
            assert!(!plausible_address(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_mailto_link_encodes_its_body_and_keeps_its_lines() {
        let link = mailto("a@b.co", "Usage report", "x = 1 & y\nnext");
        assert_eq!(
            link,
            "mailto:a@b.co?subject=Usage%20report&body=x%20%3D%201%20%26%20y%0D%0Anext"
        );
    }
}
