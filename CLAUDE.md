# newIrisTerminal

A terminal emulator for InterSystems IRIS, written in Rust with `egui`/`eframe`.
It opens sessions against local IRIS instances, local shells, and remote servers
over Telnet, and adds the things a DBA or developer working in IRIS wants that a
generic terminal has no idea about: command recall that survives restarts,
ObjectScript syntax colouring, autologon, transcripts, and macros.

This file is for an agent working in this repository. It says where things are,
what must not be broken, and how to check that you have not broken it.

---

## Build and test

Neither `cargo` nor `windres` is on `PATH` in this project's shell. Export both
in the same command as `cargo`, because shell state does not persist between
calls:

```bash
export PATH="/c/Program Files/Rust stable GNU 1.98/bin:/c/Users/lucas.arent/AppData/Local/Microsoft/WinGet/Packages/BrechtSanders.WinLibs.POSIX.UCRT_Microsoft.Winget.Source_8wekyb3d8bbwe/mingw64/bin:$PATH"
```

`windres` is needed by `build.rs`, which embeds `assets/icon.ico`; without it
the build panics rather than failing cleanly. The toolchain is GNU
(`x86_64-pc-windows-gnu`), not MSVC, which is why there is no `.pdb` and why a
profiler cannot symbolize a build (see **Profiling**).

Everything CI runs, and what you must run before claiming a change works:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo clippy --all-targets --features plugins -- -D warnings
cargo test --all-targets
cargo test --features plugins --all-targets
```

`plugins` is behind a feature flag and would otherwise rot, so it is tested
both ways. Warnings are errors; do not silence one with `#[allow]` without
saying in a comment why the lint is wrong here.

Docs are part of the build's cleanliness. `cargo doc --no-deps` must emit **no
warnings** — a link to a private item does not resolve for a reader, so write
the name in backticks instead of `[brackets]` when the target is private.

### Tests that need a live IRIS

`tests/live_*.rs` are `#[ignore]`d by default: they need an installed, running
instance and must never break `cargo test` on a machine without one. Run one
with:

```bash
cargo test --test live_session -- --ignored --nocapture
```

**These tests must never log in and never write data.** Every `RDB*` database is
shared with the whole team.

### The paint benchmark

`tests/paint_cost.rs` is a stopwatch, not an assertion, and is `#[ignore]`d for
that reason. It measures what one frame of the terminal grid costs on the CPU,
split into building the shapes and tessellating them:

```bash
cargo test --release --test paint_cost -- --ignored --nocapture
```

Run it before and after anything that touches drawing. The machine is noisy, so
compare a stashed baseline against the change **in the same run**, not against a
number from an hour ago.

---

## Layout

```
src/
  main.rs, lib.rs   the binary is one call to lib::run
  app/              the application shell - state, chrome, frame loop
  term/             the terminal core - bytes in, a screen of characters out
  pty/              sessions: local IRIS, local shell, remote Telnet
  ui/               everything egui
  config/           settings, profiles, themes, server list, all on disk
  features/         what the terminal does besides being a terminal
  plugins/          the WASM plugin host, behind the `plugins` feature
  i18n.rs           `tr`, `tr1`, `tr2` - every user-facing string goes through one
```

`term/` knows nothing about egui, the window or the user, and is drivable
headlessly — which is what the integration tests do. Keep it that way: if you
find yourself wanting a `Context` in `term/`, the logic belongs in `ui/`.

### `app/` — the shell

One type, `App`, whose `impl` is spread across modules by responsibility. Every
one of these holds part of `impl App`; the state itself lives in `mod.rs`.

| file | what it answers for |
| --- | --- |
| `mod.rs` | the `App` struct, `App::new`, and the re-exports the rest of the crate uses |
| `frame.rs` | `impl eframe::App` — the frame loop, and the repaint pacing at the end of it |
| `tab.rs` | `Tab`: one session, its grid, its log, its scrollback view |
| `tabs.rs` | opening, closing, splitting, choosing tabs and panes |
| `layout.rs` | `At`, `Pane`, `Split`, and fitting the window to a character grid |
| `pane.rs` | drawing one terminal pane — the hot path |
| `edit.rs` | acting on the line being typed, and on the selection |
| `menu.rs` | the menu bar, the shortcuts, and the `UiRequest`s they raise |
| `window.rs` | style, font, geometry, the close confirmation |
| `updates.rs` | checking for and installing a newer release |

### `ui/terminal_view/` — the grid widget

| file | what it answers for |
| --- | --- |
| `mod.rs` | the public types and `show`, the widget's entry point |
| `glyphs.rs` | the glyph cache and the per-row mesh — **the hot path** |
| `paint.rs` | one display row of cells into shapes |
| `scroll.rs` | both scrollbars, the wheel, drag-scrolling |
| `mouse.rs` | clicks, drags, double-clicks, in grid coordinates |
| `menu.rs` | the right-click menu |

---

## How it fits together

**The far side owns the line being typed.** This is the single most important
thing to understand before changing anything in `app/edit.rs`. There is no local
input buffer: IRIS (or the shell) holds the line and the cursor in it. So every
editing gesture — recall, word-motion, selection replacement, surrounding a
selection with quotes — is implemented by *sending the keystrokes that would
have produced it*, and it has to know where the cursor is to do that. This is
why those functions all end in a `send`, and why `term/lineedit.rs` exists: it
reads the prompt and the typed line back off the screen.

**Nothing is polled.** The UI is a repaint loop. A session's reader thread wakes
it when it has bytes (`pty::set_waker`), and the frame loop asks for the next
frame only when something is actually moving. An idle terminal must cost
nothing; redrawing on the chance that something arrived used to cost a core.
See the pacing at the end of `app/frame.rs::update`.

**Menu items and shortcuts do not act.** Both raise a `UiRequest`, and
`App::handle_request` carries it out. One path means a gesture behaves the same
however it was reached, and that a confirmation or a parameter prompt is only
written once. Add new gestures the same way.

**Settings, profiles, themes and macros all live on disk** under
`config::config_dir()` (`%APPDATA%\newIrisTerminal`). Never hardcode that path;
call the function.

---

## Invariants

Break one of these and the bug will be subtle, so they are listed rather than
left to be rediscovered.

- **A password must never reach disk or history.** `Tab::pump` mutes the log
  across the autologon password step, and `App::record_command` refuses to
  record when `autologon.state()` is `WaitPassword` or when `lineedit` finds no
  prompt on the row. Anything new that persists what the user typed has to
  answer to both.
- **Glyphs sit on a lattice.** `cell_size` rounds the cell width *up* to whole
  pixels, and everything — glyphs, background rects, the cursor — is positioned
  through `glyph_x`. Laying a run out as one string instead lets the text
  renderer accumulate its own advances, which drifts from the lattice by up to
  0.8 px per column. Do not "optimize" the per-glyph positioning away; there is
  a test (`a_row_drawn_as_one_shape_lands_exactly_where_one_shape_per_glyph_did`)
  that compares tessellated vertices at four display scales and will catch it.
- **A row is drawn as one shape, assembled as a `Galley`, never as a raw
  `Mesh`.** A galley's texture coordinates stay in texels and are scaled by the
  tessellator against the font atlas at the moment of drawing. Building a mesh
  directly means baking that scale in early, and drawing a frame of garbage
  every time the atlas grows.
- **Both panes of a split tab must be drained every frame.** A session that is
  not read will eventually block IRIS.
- **The grid's width is what IRIS was told.** `TERMINAL_COLS` is deliberately
  huge for IRIS, which truncates a `Write` at the margin it was given. A shell
  gets the window's real width instead, because a shell *draws* to the width it
  is given. See `RenderOpts::wide_grid`.
- **Every user-facing string goes through `tr`/`tr1`/`tr2`.** A bare string
  literal in a widget is a bug.

---

## Conventions

The comment style here is unusual and worth matching. Comments explain **why**,
not what — the reason a thing is done the strange way it is done, and the bug
that would come back if it were done the obvious way. If a comment would only
restate the code, leave it out. Doc comments on public items say what the thing
is for, not how it is spelled.

Test names are sentences describing the behaviour, not the function under test:
`a_dragged_split_never_squeezes_a_pane_below_the_minimum`, not `test_drag`.
Tests live beside the code they exercise, in a `#[cfg(test)] mod tests` at the
bottom of the file.

Portuguese appears in a few older comments and in one user-visible string in
`lib.rs`. New code is written in English.

---

## Profiling

`cargo flamegraph` works here only from an **elevated** shell (blondie traces
through ETW), and even then the capture comes back unsymbolized: the GNU
toolchain emits DWARF and no `.pdb`, and blondie resolves through `dbghelp`,
which reads only PDB. The addresses are recoverable offline — the binary still
carries its symbol table, and the ASLR load base can be pinned by scoring
candidates against it and confirming against the PE entry point.

For a number rather than a breakdown, sample `TotalProcessorTime` over a window:

```powershell
$p = Start-Process .\target\release\new-iris-terminal.exe -PassThru
Start-Sleep 8; $q = Get-Process -Id $p.Id; $t1 = $q.TotalProcessorTime
Start-Sleep 15; $q.Refresh()
($q.TotalProcessorTime - $t1).TotalSeconds / 15 * 100   # % of one core
```

Idle, with no session open, that number should be **under 1%**. If it is not,
something is asking for frames that nothing is drawing.

---

## Traps

- **The proxy blocks GitHub release assets.** `api.github.com` and `github.com`
  are allowed; `release-assets.githubusercontent.com` answers 407. Any "download
  failed" against a release URL here is the proxy, not the code.
- **There is no release CI.** A release is a hand-built binary uploaded through
  the REST API.
- **`cargo test --features plugins` can fail with `only metadata stub found for
  rlib dependency std`** after mixing build configurations in `target/`. It is a
  stale artifact, not your change; a clean rebuild fixes it.
- Do not write to `%APPDATA%\newIrisTerminal` from a test. Tests that need
  config use a temporary directory.
