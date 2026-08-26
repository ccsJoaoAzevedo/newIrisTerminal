//! The contract between the terminal and a plugin.
//!
//! Kept in its own module, free of wasmtime types, so it compiles in every
//! build and can be documented as the stable surface plugin authors target.
//!
//! ## Guest exports
//!
//! A plugin module may export any of these; all are optional.
//!
//! | Export | Signature | Meaning |
//! |---|---|---|
//! | `nit_on_output` | `(ptr: i32, len: i32) -> i64` | Observe/rewrite output. Returns packed ptr+len of the replacement, or 0 to pass through. |
//! | `nit_on_input`  | `(ptr: i32, len: i32) -> i64` | Same, for user input heading to IRIS. |
//! | `nit_command`   | `(ptr: i32, len: i32)`        | Run a registered command by name. |
//! | `nit_init`      | `()`                          | Called once after load. |
//! | `memory`        | —                             | Required so the host can pass buffers. |
//! | `nit_alloc`     | `(len: i32) -> i32`           | Required if the plugin wants the host to pass it data. |
//!
//! ## Host imports (module `nit`)
//!
//! | Import | Signature | Meaning |
//! |---|---|---|
//! | `send_text`  | `(ptr: i32, len: i32)` | Queue text to be typed into the session. |
//! | `set_status` | `(ptr: i32, len: i32)` | Show a message in the status bar. |
//! | `register_command` | `(ptr: i32, len: i32)` | Offer a command in the palette. |
//!
//! `send_text` is the sharpest edge here: a plugin can type into a live session
//! against a shared database. Requests are queued and applied by the app rather
//! than executed inside the sandbox call, so the UI stays in control of when —
//! and whether — they run.

use serde::{Deserialize, Serialize};

/// Something a plugin asked the host to do. Returned to the app, which decides
/// whether to honour it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Hook {
    /// Type this text into the active session.
    SendText(String),
    /// Show this in the status bar.
    SetStatus(String),
    /// Offer this command in the palette.
    RegisterCommand(String),
}

/// A plugin's manifest, read from `<name>.toml` beside the `.wasm`.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Manifest {
    pub name: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub description: String,
    /// Author-declared hooks. Informational: the host calls whatever the
    /// module actually exports.
    #[serde(default)]
    pub hooks: Vec<String>,
}

/// What the UI shows about a loaded plugin.
#[derive(Clone, Debug, Default)]
pub struct PluginInfo {
    pub name: String,
    pub version: String,
    pub description: String,
    pub commands: Vec<String>,
    /// Set when the module failed to load or trapped; it is then disabled.
    pub error: Option<String>,
}

/// Packs a pointer and length into the `i64` return convention used by the
/// transform hooks: high 32 bits are the pointer, low 32 the length.
pub fn pack(ptr: u32, len: u32) -> i64 {
    ((ptr as i64) << 32) | (len as i64)
}

/// Inverse of [`pack`]. Returns `None` for 0, which means "no replacement".
pub fn unpack(value: i64) -> Option<(u32, u32)> {
    if value == 0 {
        return None;
    }
    Some(((value >> 32) as u32, (value & 0xffff_ffff) as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pack_and_unpack_round_trip() {
        assert_eq!(unpack(pack(0x1234, 56)), Some((0x1234, 56)));
        assert_eq!(unpack(pack(u32::MAX, u32::MAX)), Some((u32::MAX, u32::MAX)));
    }

    /// Zero must mean "pass through unchanged", never a real buffer, or a
    /// plugin that declines to transform would blank the terminal.
    #[test]
    fn zero_means_no_replacement() {
        assert_eq!(unpack(0), None);
    }

    #[test]
    fn a_zero_length_buffer_is_still_a_replacement() {
        // Deliberately distinct from "no replacement": a plugin may want to
        // swallow output entirely.
        assert_eq!(unpack(pack(8, 0)), Some((8, 0)));
    }

    #[test]
    fn manifest_parses_with_only_a_name() {
        let m: Manifest = toml::from_str("name = \"highlight\"").expect("parse");
        assert_eq!(m.name, "highlight");
        assert!(m.version.is_empty());
    }
}
