//! The wasmtime-backed plugin host.
//!
//! Compiled only with `--features plugins`.
//!
//! Safety posture: each module is instantiated with **no** WASI, so it has no
//! filesystem, network, clock, or environment access — only the three host
//! functions in [`super::api`]. A plugin that traps or misbehaves is disabled
//! and reported rather than taking the terminal down with it.

use std::path::Path;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use wasmtime::{Caller, Engine, Extern, Linker, Memory, Module, Store, TypedFunc};

use super::api::{unpack, Hook, Manifest, PluginInfo};

/// Upper bound on a transform's replacement buffer. A plugin returning a
/// nonsensical length must not make the host allocate without limit.
const MAX_BUFFER: u32 = 8 * 1024 * 1024;

/// Shared between the host functions and the loop that drains them.
#[derive(Default)]
struct HostState {
    requests: Vec<Hook>,
}

struct LoadedPlugin {
    info: PluginInfo,
    store: Store<Arc<Mutex<HostState>>>,
    memory: Memory,
    alloc: Option<TypedFunc<i32, i32>>,
    on_output: Option<TypedFunc<(i32, i32), i64>>,
    on_input: Option<TypedFunc<(i32, i32), i64>>,
    command: Option<TypedFunc<(i32, i32), ()>>,
    state: Arc<Mutex<HostState>>,
    /// Set once the plugin traps; it is then skipped.
    disabled: bool,
}

#[derive(Default)]
pub struct PluginHost {
    plugins: Vec<LoadedPlugin>,
    infos: Vec<PluginInfo>,
}

impl PluginHost {
    /// A host with no plugins loaded, used when the feature is switched off in
    /// settings even though it is compiled in.
    pub fn disabled() -> Self {
        PluginHost::default()
    }

    pub fn is_enabled() -> bool {
        true
    }

    /// Loads every `.wasm` in `dir`. A directory that does not exist simply
    /// yields no plugins — not an error.
    pub fn load_from(dir: &Path) -> Self {
        let mut host = PluginHost::default();
        let Ok(entries) = std::fs::read_dir(dir) else {
            return host;
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("wasm") {
                continue;
            }
            match host.load_one(&path) {
                Ok(plugin) => {
                    host.infos.push(plugin.info.clone());
                    host.plugins.push(plugin);
                }
                Err(e) => {
                    log::warn!("plugin {} failed to load: {e:#}", path.display());
                    host.infos.push(PluginInfo {
                        name: path
                            .file_stem()
                            .map(|s| s.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        error: Some(format!("{e:#}")),
                        ..PluginInfo::default()
                    });
                }
            }
        }
        host
    }

    fn load_one(&mut self, path: &Path) -> Result<LoadedPlugin> {
        let engine = Engine::default();
        let module = Module::from_file(&engine, path)
            .with_context(|| format!("compiling {}", path.display()))?;

        let state = Arc::new(Mutex::new(HostState::default()));
        let mut store = Store::new(&engine, Arc::clone(&state));
        // No WASI: the sandbox is the point.
        let mut linker: Linker<Arc<Mutex<HostState>>> = Linker::new(&engine);

        register_host_fn(&mut linker, "send_text", Hook::SendText)?;
        register_host_fn(&mut linker, "set_status", Hook::SetStatus)?;
        register_host_fn(&mut linker, "register_command", Hook::RegisterCommand)?;

        let instance = linker
            .instantiate(&mut store, &module)
            .with_context(|| format!("instantiating {}", path.display()))?;

        let memory = match instance.get_export(&mut store, "memory") {
            Some(Extern::Memory(m)) => m,
            _ => anyhow::bail!("plugin does not export `memory`"),
        };

        let manifest = read_manifest(path);
        let mut info = PluginInfo {
            name: if manifest.name.is_empty() {
                path.file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default()
            } else {
                manifest.name.clone()
            },
            version: manifest.version.clone(),
            description: manifest.description.clone(),
            ..PluginInfo::default()
        };

        let alloc = instance
            .get_typed_func::<i32, i32>(&mut store, "nit_alloc")
            .ok();
        let on_output = instance
            .get_typed_func::<(i32, i32), i64>(&mut store, "nit_on_output")
            .ok();
        let on_input = instance
            .get_typed_func::<(i32, i32), i64>(&mut store, "nit_on_input")
            .ok();
        let command = instance
            .get_typed_func::<(i32, i32), ()>(&mut store, "nit_command")
            .ok();

        // `nit_init` is where a plugin registers its commands.
        if let Ok(init) = instance.get_typed_func::<(), ()>(&mut store, "nit_init") {
            if let Err(e) = init.call(&mut store, ()) {
                anyhow::bail!("nit_init trapped: {e}");
            }
        }
        for hook in state.lock().unwrap().requests.drain(..) {
            if let Hook::RegisterCommand(name) = hook {
                info.commands.push(name);
            }
        }

        Ok(LoadedPlugin {
            info,
            store,
            memory,
            alloc,
            on_output,
            on_input,
            command,
            state,
            disabled: false,
        })
    }

    pub fn loaded(&self) -> &[PluginInfo] {
        &self.infos
    }

    pub fn on_output(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.transform(bytes, true)
    }

    pub fn on_input(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.transform(bytes, false)
    }

    /// Runs every plugin's transform in turn, chaining the output of one into
    /// the next. A plugin that traps is disabled and its result discarded, so
    /// one bad plugin cannot corrupt the stream for the others.
    fn transform(&mut self, bytes: &[u8], output: bool) -> Vec<u8> {
        let mut current = bytes.to_vec();

        for plugin in self.plugins.iter_mut().filter(|p| !p.disabled) {
            let func = if output {
                plugin.on_output.clone()
            } else {
                plugin.on_input.clone()
            };
            let Some(func) = func else { continue };

            match plugin.call_transform(func, &current) {
                Ok(Some(replacement)) => current = replacement,
                Ok(None) => {}
                Err(e) => {
                    log::warn!("plugin {} disabled after a trap: {e:#}", plugin.info.name);
                    plugin.info.error = Some(format!("{e:#}"));
                    plugin.disabled = true;
                }
            }
        }
        current
    }

    pub fn run_command(&mut self, name: &str) -> Vec<Hook> {
        for plugin in self.plugins.iter_mut().filter(|p| !p.disabled) {
            if !plugin.info.commands.iter().any(|c| c == name) {
                continue;
            }
            let Some(func) = plugin.command.clone() else {
                continue;
            };
            if let Err(e) = plugin.call_command(func, name) {
                log::warn!("plugin {} disabled after a trap: {e:#}", plugin.info.name);
                plugin.info.error = Some(format!("{e:#}"));
                plugin.disabled = true;
            }
        }
        self.take_requests()
    }

    /// Drains everything plugins have asked for since the last call.
    pub fn take_requests(&mut self) -> Vec<Hook> {
        let mut all = Vec::new();
        for plugin in &mut self.plugins {
            if let Ok(mut state) = plugin.state.lock() {
                all.append(&mut state.requests);
            }
        }
        all
    }
}

impl LoadedPlugin {
    /// Copies `bytes` into the guest, calls `func`, and reads back any
    /// replacement.
    fn call_transform(
        &mut self,
        func: TypedFunc<(i32, i32), i64>,
        bytes: &[u8],
    ) -> Result<Option<Vec<u8>>> {
        let ptr = self.write_guest(bytes)?;
        let packed = func
            .call(&mut self.store, (ptr as i32, bytes.len() as i32))
            .map_err(|e| anyhow::anyhow!("{e}"))?;

        let Some((out_ptr, out_len)) = unpack(packed) else {
            return Ok(None);
        };
        if out_len > MAX_BUFFER {
            anyhow::bail!("returned buffer of {out_len} bytes exceeds the limit");
        }

        let data = self.memory.data(&self.store);
        let start = out_ptr as usize;
        let end = start
            .checked_add(out_len as usize)
            .filter(|end| *end <= data.len())
            .context("returned buffer is out of bounds")?;
        Ok(Some(data[start..end].to_vec()))
    }

    fn call_command(&mut self, func: TypedFunc<(i32, i32), ()>, name: &str) -> Result<()> {
        let ptr = self.write_guest(name.as_bytes())?;
        func.call(&mut self.store, (ptr as i32, name.len() as i32))
            .map_err(|e| anyhow::anyhow!("{e}"))
    }

    /// Asks the guest to allocate, then copies into that allocation.
    fn write_guest(&mut self, bytes: &[u8]) -> Result<u32> {
        let alloc = self
            .alloc
            .clone()
            .context("plugin exports a hook but not `nit_alloc`")?;
        let ptr = alloc
            .call(&mut self.store, bytes.len() as i32)
            .map_err(|e| anyhow::anyhow!("nit_alloc trapped: {e}"))? as u32;

        self.memory
            .write(&mut self.store, ptr as usize, bytes)
            .context("writing into plugin memory")?;
        Ok(ptr)
    }
}

/// Wires one host function that takes a (ptr, len) string and queues a [`Hook`].
fn register_host_fn(
    linker: &mut Linker<Arc<Mutex<HostState>>>,
    name: &'static str,
    make: fn(String) -> Hook,
) -> Result<()> {
    linker.func_wrap(
        "nit",
        name,
        move |mut caller: Caller<'_, Arc<Mutex<HostState>>>, ptr: i32, len: i32| {
            let Some(Extern::Memory(memory)) = caller.get_export("memory") else {
                return;
            };
            if len < 0 || len as u32 > MAX_BUFFER {
                return;
            }
            let data = memory.data(&caller);
            let start = ptr.max(0) as usize;
            let Some(end) = start
                .checked_add(len as usize)
                .filter(|end| *end <= data.len())
            else {
                return;
            };
            let text = String::from_utf8_lossy(&data[start..end]).into_owned();

            if let Ok(mut state) = caller.data().lock() {
                state.requests.push(make(text));
            }
        },
    )?;
    Ok(())
}

fn read_manifest(wasm_path: &Path) -> Manifest {
    let toml_path = wasm_path.with_extension("toml");
    std::fs::read_to_string(toml_path)
        .ok()
        .and_then(|text| toml::from_str(&text).ok())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_plugin_directory_yields_no_plugins() {
        let host = PluginHost::load_from(Path::new("does-not-exist"));
        assert!(host.loaded().is_empty());
    }

    #[test]
    fn a_non_wasm_file_is_ignored() {
        let dir = std::env::temp_dir().join(format!("nit-plugins-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("readme.txt"), b"not a plugin").unwrap();

        let host = PluginHost::load_from(&dir);
        assert!(host.loaded().is_empty());
    }

    /// A corrupt module must be reported and skipped, never fatal.
    #[test]
    fn an_invalid_wasm_file_is_reported_not_fatal() {
        let dir = std::env::temp_dir().join(format!("nit-plugins-bad-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("broken.wasm"), b"definitely not wasm").unwrap();

        let host = PluginHost::load_from(&dir);
        assert_eq!(host.loaded().len(), 1);
        assert!(host.loaded()[0].error.is_some());
    }

    #[test]
    fn with_no_plugins_the_streams_pass_through_untouched() {
        let mut host = PluginHost::disabled();
        assert_eq!(host.on_output(b"USER>"), b"USER>".to_vec());
        assert_eq!(host.on_input(b"halt\r"), b"halt\r".to_vec());
        assert!(host.take_requests().is_empty());
    }
}
