# newIrisTerminal

A terminal emulator for InterSystems IRIS — Rust, egui, and a real
pseudo-terminal, so full-screen routines such as `^%G` page correctly instead of
being flattened into line-oriented output.

Runs on Windows, Linux and macOS. Interface in English and Brazilian
Portuguese.

## Highlights

* **Tabs** — one IRIS session per tab, each with its own scrollback and log.
  `+` (Ctrl+T) connects on the last-used server; right-click it for every
  profile, server and discovered instance. A tab is named after its instance
  and, if you like, the namespace the session is in — `CONSISTEM | RDB76-TR`,
  following every `ZN`.
* **Two sessions in one tab** — right-click a tab, or the terminal itself, and
  *Split to right* or *Split to bottom*: a second session opens in the pane it
  makes. Drag the divider between them to give one pane more room, or
  double-click it to go back to half each. The strip entry names whichever pane
  the keyboard is in — `1: CONSISTEM | COMP80`, `2: ...` — and renaming the tab
  asks for both names. Only the pane you are typing in shows a cursor. Each
  pane sizes its own session, so neither is truncated at a width it is not
  drawn at. *Remove split* gives the second session a tab of its own without
  closing anything; *Close pane* closes just the session you right-clicked and
  leaves the other one in the tab.
* **Its own window frame** — the tab strip sits where the title bar would be.
  Drag it from anywhere that is not a button — the session line it shows is
  reading matter, not a widget — double-click to maximize, resize from any
  edge. The buttons come from the theme, can be moved to either end, hidden one
  at a time, or switched off altogether.
* **Lines far longer than the window** — the terminal reports a right margin of
  16384 columns, so a `zwrite` of a wide global arrives whole and a command
  longer than the window is echoed whole; the window is a view onto the line,
  wrapped or scrolled sideways. IRIS truncates a `Write` at the margin it is
  told, which is why the old 512 looked like a terminal that stopped accepting
  keys at 503 characters. Claiming the width costs nothing — a row only holds
  the columns something has been written to — but it is not free to claim more:
  a pseudoconsole resized to exactly 32767 columns stops answering altogether,
  which `live_width` measures and pins.
* **A clear-screen that keeps the transcript** — `W #` files the old screen into
  scrollback instead of destroying it, the way the native IrisTerm does.
  Ctrl+Delete is the separate, deliberate gesture that really throws it away.
* **ObjectScript colouring in the terminal** — globals, strings, macros,
  class/method references, routine and extrinsic calls, commands and their IRIS
  abbreviations. Prompt-aware, so plain prose never lights up.
* **A real text input area** — the terminal tells the system where its caret
  is, which is what lets anything that composes text elsewhere type into it: an
  input method, the emoji panel, the touch keyboard. egui reports that only for
  a text field, so a window without one says it accepts no text at all — and a
  terminal is nothing but a text field with a caret in it.
* **Copy and paste in one step** — right-click → *Copy and paste* puts the
  selection on the clipboard and types it at the prompt, which is what picking a
  global name or a routine label off the screen is usually for.
* **Line editing at the prompt** — Home/End, Ctrl+Left/Right by word, click to
  place the cursor, Ctrl+A to select the line, Shift to drag a selection out of
  it, double-click to take a word and triple-click a whole line. Built entirely
  from keys IRIS acts on, since IRIS owns the read buffer — in whichever of the
  two spellings of an arrow key the session asks for, which is what IRIS 2023
  changed. Dragging a selection past the top or bottom edge scrolls, the way it
  does in an editor.
* **Command history that outlives the session** — Up offers a tab the commands
  typed at *its* own prompt first, and only then the ones earlier runs left
  behind; what another tab has typed while this one was open stays with that
  tab. Lines a macro or an IRIS helper sent are never offered back, and IRIS's
  own recall — which is full of them — is never reachable.
* **Themes you can edit in the app** — ten built-in and immutable; duplicate one
  and every colour is yours, including the sixteen ANSI slots and the sixteen
  ObjectScript ones, with a live sample beside the editor.
* **Analyze with Claude** — right-click → *Analyze with Claude* opens a Claude
  Code session in a window of its own with the terminal output already in its
  context, then waits for your question rather than asking one for you. Four
  scopes: all output, the last 10 commands, the last 5, or just the selection.
  In a split tab it asks first whether to hand over the pane you clicked in or
  both of them, each under a heading of its own. Needs `claude` on the PATH.
* **Macros from XML** — `{{param}}` substitution, `confirm="true"` for anything
  that writes, a keyboard shortcut per macro — typed out or recorded by pressing
  it — and `hide_command="true"` for a command line carrying a password.
  Whatever is half-typed at the prompt is rubbed out first, so a macro is the
  command it says it is.
* **IRIS utilities** — fill in the fields and the exact line is composed and
  sent: compile a package, compile a routine group, generate an interface.
* **Export** — screen or full scrollback, as text or colour-preserving HTML.
* **Logging** — per-session transcripts, raw or clean, with password redaction
  and rotation. Written through to the file as the session runs, so a
  transcript can be read while the session that is producing it is still open.
* **Autologon** — credentials from the OS credential store, never from
  `settings.toml`.
* **Auto-update** — checks GitHub for a newer release at startup, through the
  machine's own proxy. Nothing is downloaded or replaced without being asked.
* **Plugins** — sandboxed WebAssembly, behind the optional `plugins` feature.

## Themes

Ten built-ins, none of which can be edited or deleted:

| Theme | |
|---|---|
| IRIS Dark | the default |
| IRIS Classic Green | phosphor, with a syntax palette of its own so the colours do not fight the hue |
| Tokyo | ported from the author's VS Code theme |
| Light | for a bright room |
| Tiger Aqua / Tiger Graphite | Mac OS X 10.4, with glass traffic lights and the Aqua scroll handle |
| Windows XP | Luna's blue title bar and its gradient buttons |
| KDE Plastik | KDE 3's grey-blue widgets around a white Konsole |
| Hello Kitty / Hello Kitty Dark | pink, and pink after dark |

Duplicate one and the theme manager gives you every colour it carries —
terminal, chrome, window buttons, the sixteen ANSI slots and the sixteen
ObjectScript scopes — with a sample beside the editor that repaints as you drag
a swatch. Themes you make are TOML files in the config directory; a file dropped
in there by hand is picked up at the next start.

Settings and the theme manager open as windows of their own, framed by the app
the way the main window is, so they are not covering the terminal you are
choosing colours against.

## Install

Grab a build from the
[releases page](https://github.com/ccsJoaoAzevedo/newIrisTerminal/releases), or
build it yourself:

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

## Configuration

Everything lives under the platform config directory —
`%APPDATA%\newIrisTerminal`, `~/.config/newIrisTerminal`, or
`~/Library/Application Support/newIrisTerminal`:

| File | Purpose |
|---|---|
| `settings.toml` | Language, theme, font, window, session and logging settings, and the profiles |
| `macros.xml` | Your personal macros — editable in the app |
| `history.txt` | Commands typed at an IRIS prompt, for recall |
| `themes/*.toml` | Your own themes, written by the theme manager |
| `analysis/*.md` | Output handed to Claude Code by *Analyze with Claude* |
| `plugins/*.wasm` | Plugins, with an optional `.toml` manifest beside each |

Passwords are never in `settings.toml`: they go to the OS credential store
(Windows Credential Manager, macOS Keychain, Secret Service), keyed by profile
name.

Macros come from two files — a shared organisation file (read-only, shown with
an `org` badge) and your personal `macros.xml`. Groups of the same name merge,
organisation entries first, and saving only ever writes the personal file.

### Encoding

A session has one character set, used to decode what arrives and to encode what
is typed, and nothing translates a second time in between — the same rule PuTTY
and IRISTerm keep. It has to hold in both directions: the terminal builds
recall, Home, End and rubbing out a selection by counting the columns on screen
and sending IRIS that many keys, so a character that costs IRIS two and the
screen one puts every one of those gestures out by one per accent.

**A local session is UTF-8, and there is nothing to choose.** It runs inside a
Windows pseudo-console, which decodes the instance's bytes with its own codepage
and re-encodes them as UTF-8 for the terminal — so the wire is UTF-8 whatever
codepage the console is on, and nothing this side decodes could change that.
What the codepage does decide is whether the bytes survive: on the machine's
OEM codepage `Configuração` arrived as `Configura├º├úo` and a typed `ó` reached
IRIS as `?`. Sessions are opened with `chcp 65001` in front of them, which is
the only configuration in which accented text works in both directions.

**A Telnet session is where the codepages are real**, because no console stands
in the path and the socket carries the instance's own bytes. UTF-8, CP850,
Windows-1252 and ISO 8859-1 are selectable per remote profile; an older Caché or
IRIS, or one with a non-UTF-8 I/O translation table configured, needs one of the
others. The setting only appears for a profile that opens a server.

To see what your instance sends, and what the console does to it:

```sh
cargo test --test live_charset  -- --ignored --nocapture
cargo test --test live_codepage -- --ignored --nocapture
cargo test --test live_accents  -- --ignored --nocapture
```

## Versioning

[ZeroVer](https://0ver.org): the major version stays at zero. A release bumps
the minor, and the updater compares the numbers rather than the string, so
`0.2.0` is newer than `0.1.9`.

## Tests

The unit tests cover the VT parser, grid, encodings, macro XML, autologon,
logging, export, themes, translations, the updater and input mapping, and need
no IRIS:

```sh
cargo test
```

The tests that talk to a real instance are ignored by default; they open a
session and read the banner, never logging in and never writing data:

```sh
cargo test --test live_session -- --ignored --nocapture
cargo test --test live_width   -- --ignored --nocapture
cargo test --test live_resize  -- --ignored --nocapture
```

`live_width` is the one that says what the reported margin is worth: it asks the
instance for a 3000-character line and checks that all of it arrived and that
the grid is holding it on one row. `live_resize` covers the other side of the
same conversation — a Windows pseudoconsole answers every resize by repainting
the whole screen, in a sequence that is character for character IRIS's own
`W #`, and reading that as a clear-screen used to duplicate everything on screen
once per resize.

`IRIS_TEST_INSTANCE` picks the instance; otherwise the first discovered one is
used. Instances come from `iris list`, whose keyword varies by version
(`Instance 'NAME'`, `Configuration 'NAME'`) — both are handled.

## Safety note

Macros and the IRIS utilities type into a live session, and `RDB*` databases are
shared with the whole team. Any macro that modifies data should carry
`confirm="true"`: the terminal then shows the exact expanded text and requires
an explicit yes before sending.
