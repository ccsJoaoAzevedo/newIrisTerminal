# newIrisTerminal

Terminal emulator for InterSystems IRIS.

Written in Rust, with egui. Runs on Windows, Linux, and macOS.

Sessions are driven through a real pseudo-terminal (ConPTY on Windows,
`openpty` elsewhere), so full-screen routines such as `^%G` render and page
correctly rather than being flattened into line-oriented output.

## Feature-set

* **Done:**
  * Multitab — one IRIS session per tab, independent scrollback and logging.
    `+` (or Ctrl+T) connects straight away on the last-used profile/instance
    with no dialog in the way; right-click `+` for a menu of every configured
    profile and every discovered instance
  * Custom window chrome — the app draws its own titlebar (hand-stroked
    minimize / maximize-restore / close, red on close-hover), the empty part
    of it drags the window and double-click maximizes, and the edges/corners
    resize like a native window. Settings → Window → "Use the system title
    bar" switches back to OS decorations if you'd rather have those
  * Clear-screen that keeps the transcript — `W #` doesn't clear in one shot,
    it erases row by row; the terminal catches that sweep and files the
    pre-clear screen into scrollback instead of destroying it, the way the
    native IrisTerm does. Ctrl+Delete (or right-click → "Clear terminal and
    scrollback") is the separate, deliberate gesture that actually throws the
    history away, asking IRIS for a real clear rather than wiping the grid
    locally — which would otherwise leave IRIS's next prompt painted at a
    stale row
  * ObjectScript syntax colouring in the terminal, prompt-aware so it never
    lights up plain prose: globals, strings, numbers, delimiters/operators,
    commands (full words and their IRIS abbreviations), preprocessor macros
    (`$$$`, `#define`), functions and system variables (`$piece`, `$horolog`),
    class/method/attribute/member references (`##class(...)`, `..Prop`,
    `obj.Method(`), routine and extrinsic calls (`^ROU`, `$$Tag^ROU`) and
    labels. Field names in a theme are named after the semantic scopes of the
    InterSystems VS Code extension, so a colour customisation can be copied
    straight across
  * Autologon — username/password from the OS credential store, with post-login commands
  * Window resize and fit content to window — the grid reflows and the PTY is resized
  * Macros read from XML — `{{param}}` substitution, `confirm="true"` for
    anything that writes, an optional keyboard shortcut per macro
    (`key="Ctrl+Shift+G"`), `hide_command="true"` to keep a password-bearing
    command line out of the panel, and a one-click Run button on every row
  * Theming support — TOML themes, hot-swappable, applied to terminal and
    chrome alike, with an "Open folder" button that jumps straight to the
    themes directory. Also covers font family (from the system's installed
    fonts) and size, cursor style and blink, and solid vs hover-only
    scrollbars
  * Plugin interface — sandboxed WebAssembly, behind the `plugins` feature
  * Logging — per-session transcripts, raw or clean, with password redaction and rotation
  * Easier access and dedicated interfaces for native routines (`^%RD`, `^%RS`, `ZWRITE`, `ZN`)
  * Export output — screen or full scrollback, as text or colour-preserving HTML
  * Global browser — a searchable, paginated grid replacing `^%G`'s paged text
  * Right-click menu for copy / paste / select all / clear terminal and scrollback
  * Custom app icon, embedded both in the `.exe` (Explorer/taskbar) and loaded
    at runtime for the window icon

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
cargo test --test live_session  -- --ignored --nocapture   # session opens, resizes
cargo test --test live_input    -- --ignored --nocapture   # arrow recall, Ctrl+C interrupt
cargo test --test live_globals  -- --ignored --nocapture   # global browser extraction
cargo test --test live_charset  -- --ignored --nocapture   # which encoding is correct here
cargo test --test live_wrap     -- --ignored --nocapture   # where long output gets cut
cargo test --test live_timing   -- --ignored --nocapture   # where session-open time goes
```

Set `IRIS_TEST_INSTANCE` to choose the instance; otherwise the first discovered
one is used. These tests only open a session and read the banner — they never
log in and never write data.

## Configuration

Everything lives under the platform config directory — `%APPDATA%\newIrisTerminal`,
`~/.config/newIrisTerminal`, or `~/Library/Application Support/newIrisTerminal`:

| File | Purpose |
|---|---|
| `settings.toml` | Profiles, theme, font/cursor/scrollbar, window decorations, scrollback, logging |
| `macros.xml` | Your personal macros — editable from the Macros panel |
| `themes/*.toml` | Colour schemes; drop a file in and restart |
| `plugins/*.wasm` | Plugins, with an optional `.toml` manifest beside each |

### Macros

Macros come from two files:

* **Organization** — a shared file, path configured in Settings (UNC share,
  mapped drive, or local copy). Shown with an `org` badge and never written to.
  If it is unreachable you get a notice and your personal macros still load.
* **Personal** — `macros.xml` in the config directory, created on first run and
  editable in the app.

Groups with the same name merge, organisation entries first. Saving only ever
writes personal macros, so a shared macro cannot silently fork into a local copy.

Passwords are **not** stored in `settings.toml`. They go to the OS credential
store (Windows Credential Manager, macOS Keychain, Secret Service) keyed by
profile name.

### Instance discovery

Instances come from `iris list`. Note that the registered instance name is not
always the install directory name, and the keyword in that output varies by
version (`Instance 'NAME'` on standard installs, `Configuration 'NAME'` on
custom ones) — both are handled.

## Global browser

`^%G` is prompt-driven and paginates as free text, which is fine to read and
miserable to search. The **Globals** panel builds the same view from data: a
short read-only ObjectScript walk (`$Query` + `$Get`) emits one tagged line per
node and per `$Piece`, and the result becomes a searchable grid where every
piece is a numbered column — `p1`, `p2`, `p3` map directly onto
`$Piece(value, delim, n)`.

* The delimiter defaults to `^` and is editable per query.
* Splitting happens on the IRIS side, so `$Piece` stays the authority on what
  piece *n* is.
* Pagination is two-level: pages within the fetched rows, and **Fetch more** to
  continue the walk from the last node. Resuming never re-reads or skips a node.
* Nothing in the panel can write. The generated script contains no `Set`,
  `Kill`, or `Merge`, and a test asserts that.

One node per *piece* is written rather than one per node, deliberately: IRIS
truncates output at the terminal's right margin instead of wrapping it, so a
wide node on a single line would silently lose its tail.

## Encoding

Defaults to **UTF-8 double-encoded via CP850 (repair)**, because that is what
the instances here actually need: some IRIS configurations translate output to
UTF-8 and then run the result through CP850 → UTF-8 a second time, so `Nó`
arrives as `├│` and `Configuração` as `Configura├º├úo`.

The repair is safe to leave on. It works per run of non-ASCII characters and
only converts a run whose bytes form valid UTF-8 *and* decode to ordinary Latin
text — so genuine box drawing (`├───┤`) and already-correct accented text pass
through untouched. Plain `UTF-8`, `CP850`, `Windows-1252` and `ISO 8859-1` are
selectable per profile if your instance differs.

Run `cargo test --test live_charset -- --ignored --nocapture` to see which
encoding renders your instance correctly.

## Safety note

Macros and native helpers type into a live session. `RDB*` databases are shared
with the whole team, so any macro that modifies data should carry
`confirm="true"`; the terminal then shows the exact expanded text and requires
an explicit yes before sending. The bundled sample demonstrates this on its
`KILL` example.
