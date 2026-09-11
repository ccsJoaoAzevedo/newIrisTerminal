# newIrisTerminal 🖥️

A terminal emulator for InterSystems IRIS — Windows, Linux and macOS.

## 📖 Overview

**newIrisTerminal** drives IRIS sessions through a real pseudo-terminal, so
full-screen routines such as `^%G` page correctly instead of being flattened
into line-oriented output.

Built in Rust with egui. Interface in English and Brazilian Portuguese.

## ✨ Highlights

* **Tabs:** one IRIS session per tab, each with its own scrollback and log.
  Ctrl+T connects on the last-used server; right-click `+` for every profile,
  server and discovered instance. A tab is named after its instance and, if you
  like, the namespace the session is in — `CONSISTEM | RDB76-TR` — following
  every `ZN`.
* **Split panes:** *Split to right* or *Split to bottom* opens a second session
  in the same tab. Drag the divider to resize, double-click it to even them out.
  Each pane sizes its own session, so neither is truncated at a width it is not
  drawn at.
* **Lines far longer than the window:** the terminal reports a right margin of
  16384 columns, so a `zwrite` of a wide global arrives whole and a long command
  is echoed whole. The window is a view onto the line, wrapped or scrolled
  sideways.
* **ObjectScript colouring:** globals, strings, macros, class and method
  references, routine and extrinsic calls, commands and their IRIS
  abbreviations. Prompt-aware, so plain prose never lights up.
* **Line editing at the prompt:** Home/End, Ctrl+Left/Right by word, click to
  place the cursor, Ctrl+A to select, double-click for a word, triple-click for
  a line. Built entirely from keys IRIS acts on, since IRIS owns the read
  buffer. Typing `"`, `'`, `(`, `[` or `{` over a selection wraps it in the pair
  instead of replacing it, the way an editor does — so a global name picked off
  the screen is quoted in one keystroke.
* **Find in the transcript:** Ctrl+F searches everything the session has
  printed, scrollback included. Enter and Shift+Enter — or F3 — walk the hits,
  every one is highlighted, and the view scrolls to the one being looked at.
* **Command history that outlives the session:** Up offers a tab the commands
  typed at its own prompt first, then the ones earlier runs left behind. Lines a
  macro sent are never offered back.
* **Macros from XML:** `{{param}}` substitution, `confirm="true"` for anything
  that writes, a keyboard shortcut per macro, and `hide_command="true"` for a
  command line carrying a password. Written and edited in the built-in macro
  manager.
* **IRIS utilities:** compile a package, compile a routine group, generate an
  interface — each with a field per argument and the exact line it will send
  shown underneath.
* **Other shells:** Command Prompt, Windows PowerShell, PowerShell 7, Git Bash,
  WSL, or whatever `/etc/shells` lists. Each is a `.toml` in `plugins/shells/`,
  written there the first time the app finds it installed and yours to edit or
  delete after that.
* **A clear-screen that keeps the transcript:** `W #` files the old screen into
  scrollback instead of destroying it, the way the native IrisTerm does.
  Ctrl+Delete is the deliberate gesture that really throws it away.
* **Themes you can edit in the app:** ten built-in and immutable — IRIS Dark,
  IRIS Classic Green, Tokyo, Light, Tiger Aqua, Tiger Graphite, Windows XP, KDE
  Plastik, Hello Kitty and Hello Kitty Dark. Duplicate one and every colour is
  yours, including the sixteen ANSI slots and the sixteen ObjectScript ones,
  with a live sample beside the editor.
* **Export and logging:** screen or full scrollback, as text or
  colour-preserving HTML, to a file or the clipboard. Per-session transcripts
  are written through as the session runs, with password redaction and rotation.
* **Analyze with Claude:** opens a Claude Code session with the terminal output
  already in its context, then waits for your question. Four scopes: all
  output, the last 10 commands, the last 5, or just the selection. Needs
  `claude` on the PATH.
* **Autologon:** credentials from the OS credential store, never from
  `settings.toml`.
* **Auto-update:** checks GitHub for a newer release at startup, through the
  machine's own proxy. Nothing is downloaded or replaced without being asked.

## 🧾 Requirements

- **OS:** Windows 10 or later, Linux, or macOS
- **Disk:** ~30 MB free space
- An InterSystems IRIS or Caché instance to connect to — local instances are
  discovered automatically via `iris list`

## Setup

- [ ] Grab the latest build from the [releases page](https://github.com/ccsJoaoAzevedo/newIrisTerminal/releases).
- [ ] Run the executable — there is no installer and nothing is written outside
      the config directory.
- [ ] Pick a theme and a font in Settings, and add a profile for any server you
      connect to that is not a local instance.

## 📥 Downloads & Links

➡️ **[Get the latest build](https://github.com/ccsJoaoAzevedo/newIrisTerminal/releases/latest)**

Repository: https://github.com/ccsJoaoAzevedo/newIrisTerminal

## Configuration

Everything lives under the platform config directory —
`%APPDATA%\newIrisTerminal`, `~/.config/newIrisTerminal`, or
`~/Library/Application Support/newIrisTerminal`:

| File | Purpose |
|---|---|
| `settings.toml` | Language, theme, font, window, session and logging settings, and the profiles |
| `macros.xml` | Your personal macros — written by the macro manager |
| `history.txt` | Commands typed at an IRIS prompt, for recall |
| `themes/*.toml` | Your own themes, written by the theme manager |
| `analysis/*.md` | Output handed to Claude Code by *Analyze with Claude* |
| `plugins/shells/*.toml` | One per shell, found or declared |

Passwords are never in `settings.toml`: they go to the OS credential store
(Windows Credential Manager, macOS Keychain, Secret Service), keyed by profile
name — and the HTTP proxy's password with them, under `http-proxy`.

### Encoding

A session has one character set, used to decode what arrives and to encode what
is typed, and nothing translates a second time in between.

**A local session is UTF-8, and there is nothing to choose.** Sessions are
opened with `chcp 65001` in front of them, which is the only configuration in
which accented text works in both directions.

**A Telnet session is where the codepages are real**, because no console stands
in the path and the socket carries the instance's own bytes. UTF-8, CP850,
Windows-1252 and ISO 8859-1 are selectable per remote profile.

## Build from source

```sh
cargo build --release
cargo test
```

The plugin host pulls in wasmtime and is off by default:

```sh
cargo build --release --features plugins
```

On Windows without Visual Studio, the GNU toolchain needs a full MinGW-w64
beside it (the one inside the Rust MSI lacks the assembler `dlltool` wants):

```powershell
winget install Rustlang.Rust.GNU
winget install BrechtSanders.WinLibs.POSIX.MSVCRT
```

The tests that talk to a real instance are ignored by default; they open a
session and read the banner, never logging in and never writing data:

```sh
cargo test --test live_session -- --ignored --nocapture
```

`IRIS_TEST_INSTANCE` picks the instance; otherwise the first discovered one is
used.

## ⚠️ Safety note

Macros and the IRIS utilities type into a live session, and `RDB*` databases are
shared with the whole team. Any macro that modifies data should carry
`confirm="true"`: the terminal then shows the exact expanded text and requires
an explicit yes before sending.

## Versioning

[ZeroVer](https://0ver.org): the major version stays at zero. The updater
compares the numbers rather than the string, so `0.2.0` is newer than `0.1.9`.

## License

MIT
