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
    history away. At an idle prompt it asks IRIS for the clear (`W #`) rather
    than wiping the grid, because the far side repaints by absolute cursor
    position and would otherwise put its next prompt back at the row it had
    reached with blank rows above it; the echoed command and the old screen
    are dropped rather than filed, so nothing of it stays behind. Mid-line or
    mid-routine, where the command could be swallowed as input, the grid is
    cleared locally instead
  * ObjectScript syntax colouring in the terminal, prompt-aware so it never
    lights up plain prose: globals, strings, numbers, delimiters/operators,
    commands (full words and their IRIS abbreviations), preprocessor macros
    (`$$$`, `#define`), functions and system variables (`$piece`, `$horolog`),
    class/method/attribute/member references (`##class(...)`, `..Prop`,
    `obj.Method(`), routine and extrinsic calls (`^ROU`, `$$Tag^ROU`) and
    labels. Field names in a theme are named after the semantic scopes of the
    InterSystems VS Code extension, so a colour customisation can be copied
    straight across
  * Line editing at the prompt, as close to a text field as a terminal gets —
    Home and End walk to the ends of the command being typed, Ctrl+Left/Right
    walk it a word at a time, clicking inside it puts the cursor there, Ctrl+A
    selects it, Shift plus Left/Right/Home/End (or Ctrl+Shift+Left/Right, by
    word) drags a selection out of it, and Backspace/Delete rub that selection
    — or the mouse's — out, as does typing or pasting over it. An unshifted
    movement key drops the selection again. Word boundaries are an editor's,
    counting a run of punctuation as a stop of its own, so Ctrl+Right walks
    `do ^%CSW1GEN("X")` a piece at a time. IRIS owns the read buffer, so all
    of it is built from the arrow keys and rubouts it does act on — and
    extending a selection sends nothing at all; off a command line (in `^%G`,
    say) the keys reach IRIS untouched
  * Command history that outlives the session — every command typed at an IRIS
    prompt is remembered, and Up/Down walk it. Shared by every tab, and kept in
    `history.txt` unless Settings → Session → "Remember commands from earlier
    sessions" is off
  * Copy on select — a finished selection goes to the clipboard without waiting
    for Ctrl+C. Settings → Session
  * Autologon — username/password from the OS credential store, with post-login commands
  * Window resize and fit content to window — the grid reflows and the PTY is resized
  * Macros read from XML — `{{param}}` substitution, `confirm="true"` for
    anything that writes, an optional keyboard shortcut per macro
    (`key="Ctrl+Shift+G"`), `hide_command="true"` to keep a password-bearing
    command line out of the panel, and a one-click Run button on every row.
    Ships with one: **Developer Tools (Exec)**
  * Theming support — TOML themes, hot-swappable, applied to terminal and
    chrome alike, with an "Open folder" button that jumps straight to the
    themes directory. Also covers font family (from the system's installed
    fonts) and size, cursor style and blink, and solid vs hover-only
    scrollbars
  * Plugin interface — sandboxed WebAssembly, behind the `plugins` feature
  * Logging — per-session transcripts, raw or clean, with password redaction and rotation
  * IRIS utilities panel — fill in the fields and the exact line is composed
    and sent: **Compile classes**
    (`do $SYSTEM.OBJ.CompilePackage("<package>","<flag>")`, flag defaulting to
    `bkf1`) and **Generate interface** (`do ^%CSW1GEN("<routine/group>")`).
    Clicking the open one again folds it away and keeps what you typed
  * Export output — screen or full scrollback, as text or colour-preserving HTML
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
| `settings.toml` | Profiles, theme, font/cursor/scrollbar, window decorations, scrollback, copy-on-select, command history, logging |
| `macros.xml` | Your personal macros — editable from the Macros panel |
| `history.txt` | Commands typed at an IRIS prompt, for Up/Down recall |
| `themes/*.toml` | Colour schemes; drop a file in and restart |
| `plugins/*.wasm` | Plugins, with an optional `.toml` manifest beside each |

### Macros

Macros come from two files:

* **Organization** — a shared file, path configured in Settings (UNC share,
  mapped drive, or local copy). Shown with an `org` badge and never written to.
  If it is unreachable you get a notice and your personal macros still load.
* **Personal** — `macros.xml` in the config directory, created on first run and
  editable in the app. While it is still exactly as shipped it is refreshed
  when the bundled set changes; the first edit claims the file for good.

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
