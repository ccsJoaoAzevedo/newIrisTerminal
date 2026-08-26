//! Plugin interface.
//!
//! Plugins are WebAssembly modules, chosen over native `dylib`s for two
//! reasons: a sandbox (a plugin gets no filesystem, network, or process access
//! it is not handed) and no ABI to break across compiler versions.
//!
//! The host API is deliberately tiny. Plugins observe and transform the byte
//! streams, and can ask to send text or set a status message. They cannot
//! reach the PTY, the config, or the credential store.
//!
//! Compiled only with `--features plugins`, so the default build carries none
//! of the wasmtime dependency. [`PluginHost`] has a no-op stand-in otherwise,
//! which keeps `app.rs` free of `#[cfg]` noise.

pub mod api;

#[cfg(feature = "plugins")]
pub mod host;

#[cfg(feature = "plugins")]
pub use host::PluginHost;

#[cfg(not(feature = "plugins"))]
pub use stub::PluginHost;

/// The build without plugin support. Every method matches the real host's
/// signature and does nothing, so callers need no conditional compilation.
#[cfg(not(feature = "plugins"))]
mod stub {
    use super::api::{Hook, PluginInfo};
    use std::path::Path;

    #[derive(Default)]
    pub struct PluginHost;

    impl PluginHost {
        /// A host that runs nothing. Named the same in both builds so callers
        /// need no `#[cfg]`.
        pub fn disabled() -> Self {
            PluginHost
        }

        pub fn load_from(_dir: &Path) -> Self {
            PluginHost
        }

        pub fn is_enabled() -> bool {
            false
        }

        pub fn loaded(&self) -> &[PluginInfo] {
            &[]
        }

        pub fn on_output(&mut self, bytes: &[u8]) -> Vec<u8> {
            bytes.to_vec()
        }

        pub fn on_input(&mut self, bytes: &[u8]) -> Vec<u8> {
            bytes.to_vec()
        }

        pub fn run_command(&mut self, _name: &str) -> Vec<Hook> {
            Vec::new()
        }

        pub fn take_requests(&mut self) -> Vec<Hook> {
            Vec::new()
        }
    }
}
