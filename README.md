# newIrisTerminal

Terminal emulator for InterSystems IRIS.

Written in Rust, with egui. Runs on Windows, Linux, and macOS.

Sessions are driven through a real pseudo-terminal (ConPTY on Windows,
`openpty` elsewhere), so full-screen routines such as `^%G` render and page
correctly rather than being flattened into line-oriented output.

## Feature-set

* **Done:**
  * Multitab — one IRIS session per tab, independent scrollback and logging
  * Autologon — username/password from the OS credential store, with post-login commands
  * Window resize and fit content to window — the grid reflows and the PTY is resized
  * Macros read from XML — `{{param}}` substitution, and `confirm="true"` for anything that writes
  * Theming support — TOML themes, hot-swappable, applied to terminal and chrome alike
  * Plugin interface — sandboxed WebAssembly, behind the `plugins` feature
  * Logging — per-session transcripts, raw or clean, with password redaction and rotation
  * Easier access and dedicated interfaces for native routines (`^%G`, `^%RD`, `^%RS`, `ZWRITE`, `ZN`)
  * Export output — screen or full scrollback, as text or colour-preserving HTML

## Building

Requires a Rust toolchain with a working linker.

```sh
cargo build --release
cargo test
```

The plugin host is optional and off by default, because it pulls in wasmtime:

```sh
cargo build --release --features plugins
```

### Windows without Visual Studio

The GNU toolchain avoids the Visual Studio Build Tools dependency, but the
minimal MinGW that ships inside the Rust MSI lacks the assembler `dlltool`
needs. Install a full MinGW-w64 alongside it:

```powershell
winget install Rustlang.Rust.GNU
winget install BrechtSanders.WinLibs.POSIX.MSVCRT
```

## Testing

Unit tests cover the VT parser, grid, encodings, macro XML, autologon, logging,
export, and input mapping. None of them need IRIS:

```sh
cargo test
```

Integration tests that talk to a real instance are ignored by default:

```sh
cargo test --test live_session -- --ignored --nocapture
cargo test --test encoding_probe -- --ignored --nocapture   # reports what your instance actually sends
```

Set `IRIS_TEST_INSTANCE` to choose the instance; otherwise the first discovered
one is used. These tests only open a session and read the banner — they never
log in and never write data.

## Configuration

Everything lives under the platform config directory — `%APPDATA%\newIrisTerminal`,
`~/.config/newIrisTerminal`, or `~/Library/Application Support/newIrisTerminal`:

| File | Purpose |
|---|---|
| `settings.toml` | Profiles, theme, font size, scrollback, logging |
| `macros.xml` | Macro definitions (a documented sample is written on first run) |
| `themes/*.toml` | Colour schemes; drop a file in and restart |
| `plugins/*.wasm` | Plugins, with an optional `.toml` manifest beside each |

Passwords are **not** stored in `settings.toml`. They go to the OS credential
store (Windows Credential Manager, macOS Keychain, Secret Service) keyed by
profile name.

### Instance discovery

Instances come from `iris list`. Note that the registered instance name is not
always the install directory name, and the keyword in that output varies by
version (`Instance 'NAME'` on standard installs, `Configuration 'NAME'` on
custom ones) — both are handled.

### Encoding

Defaults to UTF-8, which current IRIS builds emit. Older Caché/IRIS instances,
or ones with a non-UTF-8 I/O translation configured, may need CP850 or
Windows-1252 — set it per profile in Settings. Run the `encoding_probe` test to
see what your instance actually sends.

## Safety note

Macros and native helpers type into a live session. `RDB*` databases are shared
with the whole team, so any macro that modifies data should carry
`confirm="true"`; the terminal then shows the exact expanded text and requires
an explicit yes before sending. The bundled sample demonstrates this on its
`KILL` example.
