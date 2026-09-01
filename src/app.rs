//! The application shell: owns the tabs, drains their PTYs each frame, and
//! draws the chrome around the terminal view.

use egui::{Context, Key, Modifiers};

use crate::config::{self, ensure_config_tree, load_themes, LogMode, Profile, Settings, Theme};
use crate::features::analyze;
use crate::features::autologon::{Autologon, State as AutoState};
use crate::features::export::{self, Range};
use crate::features::history::History;
use crate::features::logging::{self, SessionLog};
use crate::features::macros::{self, MacroGroup};
use crate::features::update;
use crate::i18n::{tr, tr1, tr2};
use crate::plugins::PluginHost;
use crate::pty::launcher::launcher;
use crate::pty::Session;
use crate::term::{lineedit, Grid, Motion};
use crate::ui::chrome::{self, WindowAction};
use crate::ui::panels::{self, PanelState, PendingMacro, UiRequest};
use crate::ui::terminal_view::{self, RenderOpts, Selection, ViewState};
use crate::ui::theme_manager::{self, ThemeAction};
use crate::ui::{fonts, input, shortcut};

/// What the app knows about a newer version.
///
/// The check runs on a thread, so this is the state machine between "asked" and
/// "the user has decided": nothing here blocks a frame.
#[derive(Default)]
pub struct UpdateState {
    events: Option<crossbeam_channel::Receiver<update::Event>>,
    /// A release newer than this build, once one has been found.
    pub available: Option<update::Release>,
    /// The downloaded executable, waiting to be put in place.
    pub staged: Option<std::path::PathBuf>,
    pub downloading: bool,
    pub error: Option<String>,
    /// The dialog has been shown for this release and dismissed. Kept so a
    /// "later" is not undone by the next frame.
    pub asked: bool,
    /// The user asked for this check, so its answer is worth a status line even
    /// when the answer is "nothing new".
    announce: bool,
}

impl UpdateState {
    /// Starts a check on a thread of its own.
    fn start_check(&mut self) {
        let (tx, rx) = crossbeam_channel::unbounded();
        self.events = Some(rx);
        self.error = None;
        update::check_in_background(tx);
    }

    /// Starts a check the user asked for, whose answer is always reported.
    pub fn check_now(&mut self) {
        self.announce = true;
        self.start_check();
    }

    /// Starts downloading the release that was found.
    pub fn start_download(&mut self) {
        let Some(release) = self.available.clone() else {
            return;
        };
        let (tx, rx) = crossbeam_channel::unbounded();
        self.events = Some(rx);
        self.downloading = true;
        self.error = None;
        update::download_in_background(release, tx);
    }

    fn drain(&mut self) -> Vec<update::Event> {
        let Some(rx) = self.events.as_ref() else {
            return Vec::new();
        };
        rx.try_iter().collect()
    }
}

/// Fallback PTY size, used only before the first frame has measured the
/// window. After that, new sessions open at the size the terminal is actually
/// being drawn at.
///
/// This matters more than it looks: IRIS truncates output at the device right
/// margin rather than wrapping it, so a session opened at 80 columns loses
/// everything past column 80 until it is resized. Opening at the real size
/// means nothing is cut in the first place.
const FALLBACK_COLS: u16 = 80;
const FALLBACK_ROWS: u16 = 24;

/// How many frames the opening size may take to settle. A correction normally
/// lands on the first one; the budget is there so a window manager that refuses
/// the size it is asked for cannot leave the app resizing itself forever.
const FIT_ATTEMPTS: u8 = 8;

/// A window that has yet to be sized to the terminal geometry it was asked to
/// open at.
///
/// The size in the settings file is a character geometry, not a pixel one, and
/// the pixels it works out to depend on the font egui ends up with and on how
/// tall the panels above the terminal are laid out. Neither is known before the
/// first frame, so the window opens at an estimate and is corrected here once
/// there is a measurement to correct it with.
struct WindowFit {
    cols: u16,
    rows: u16,
    /// Frames left to get there.
    attempts: u8,
    /// Whether the window is being centred rather than opened at a saved
    /// position. Resizing anchors the top-left corner, which would walk a
    /// centred window off-centre, so its centre is held instead.
    recentre: bool,
    /// The centre to hold, taken from the first frame that had a window rect to
    /// take it from.
    centre: Option<egui::Pos2>,
}

/// Takes one key press out of this frame's events, matching the modifiers
/// exactly, and reports whether it was there.
///
/// Consuming matters: without it a chord is handled here *and* translated into
/// a control code for IRIS, so Ctrl+T opened a tab and typed `0x14` into the
/// session. egui's own `consume_key` is not usable for this because it matches
/// leniently - `Ctrl+Shift+T` satisfies a pattern of `Ctrl` - which would let
/// an app shortcut swallow a macro bound to the same key plus Shift.
fn consume_exact(ctx: &Context, modifiers: Modifiers, key: Key) -> bool {
    ctx.input_mut(|i| {
        let mut hit = false;
        i.events.retain(|event| {
            let is_match = matches!(
                event,
                egui::Event::Key {
                    key: event_key,
                    modifiers: event_modifiers,
                    pressed: true,
                    ..
                } if *event_key == key && event_modifiers.matches_exact(modifiers)
            );
            hit |= is_match;
            !is_match
        });
        hit
    })
}

/// A tab name being edited.
struct Renaming {
    tab: usize,
    draft: String,
    /// Cleared after the field has been given focus once. Without this the
    /// terminal claims the keyboard back - it grabs focus whenever nothing else
    /// holds it - and the new name would be typed into IRIS.
    focus: bool,
}

/// Rotate a transcript once it passes this size.
const LOG_ROTATE_BYTES: u64 = 64 * 1024 * 1024;

/// One open session and everything that hangs off it.
pub struct Tab {
    /// Stable for this tab's lifetime and unique across tabs, so the terminal
    /// widget keeps one identity even as tabs are opened, closed and reordered.
    pub uid: u64,
    pub profile: Profile,
    pub session: Option<Session>,
    pub grid: Grid,
    pub parser: vte::Parser,
    pub view: ViewState,
    pub autologon: Autologon,
    pub log: Option<SessionLog>,
    /// User-set name; falls back to the OSC title, then the profile name.
    pub custom_title: Option<String>,
    /// How far back through the command history this tab has walked, if at
    /// all. Per tab, so recalling in one does not move another.
    pub recall_index: Option<usize>,
    /// When a clear-screen was asked of IRIS, so a purge that never arrives can
    /// be called off. See [`Tab::request_clear`].
    clear_asked: Option<std::time::Instant>,
    /// Set when the child exits, so the tab explains itself instead of freezing.
    pub ended: bool,
    pub error: Option<String>,
}

/// Source of [`Tab::uid`]. Never reused, so a closed tab's id cannot collide
/// with a later one and resurrect its focus or selection state.
static NEXT_TAB_UID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

impl Tab {
    pub fn new(profile: Profile, settings: &Settings, cols: u16, rows: u16) -> Self {
        let log = open_log(&profile, settings);
        let mut tab = Tab {
            uid: NEXT_TAB_UID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            autologon: Autologon::new(&profile),
            grid: Grid::new(cols as usize, rows as usize, settings.scrollback_limit),
            profile,
            session: None,
            parser: vte::Parser::new(),
            view: ViewState::default(),
            log,
            custom_title: None,
            recall_index: None,
            clear_asked: None,
            ended: false,
            error: None,
        };
        tab.start();
        tab
    }

    pub fn start(&mut self) {
        self.ended = false;
        self.error = None;
        self.autologon = Autologon::new(&self.profile);

        let cols = self.grid.cols as u16;
        let rows = self.grid.rows as u16;

        // A remote server has no process here to start: it is reached by
        // logging in to its Telnet service, the same way the launcher's own
        // Terminal reaches it. Everything above this point is identical.
        let opened = match self.profile.remote.as_ref() {
            Some(remote) => Session::telnet(&remote.address, remote.port, cols, rows),
            None => {
                let launcher = launcher();
                Session::local(launcher.as_ref(), &self.profile.launch_spec(), cols, rows)
            }
        };

        match opened {
            Ok(session) => {
                self.session = Some(session);
                let endpoint = self.profile.endpoint();
                self.note(&crate::i18n::tr1("session started ({})", &endpoint));
            }
            Err(e) => {
                // Surface the failure in the tab rather than a popup — the user
                // may have several tabs and needs to know which one broke.
                self.error = Some(format!("{e:#}"));
                self.ended = true;
                self.session = None;
            }
        }
    }

    fn note(&mut self, text: &str) {
        if let Some(log) = self.log.as_mut() {
            let _ = log.write_note(text);
        }
    }

    pub fn title(&self) -> String {
        self.custom_title
            .clone()
            // IRIS sets the window title to the executable path, which is
            // useless as a tab name.
            .or_else(|| self.grid.title.clone().filter(|t| !t.contains(".exe")))
            .unwrap_or_else(|| {
                // The instance first: it is what tells two tabs apart, whereas
                // the profile name is often left at its default and the same on
                // every tab.
                if self.profile.instance.is_empty() {
                    self.profile.name.clone()
                } else {
                    self.profile.instance.clone()
                }
            })
    }

    /// Pulls output, parses it, answers device reports, and runs autologon.
    pub fn pump(&mut self, plugins: &mut PluginHost) {
        let Some(session) = self.session.as_mut() else {
            return;
        };

        let (bytes, ended) = session.drain();
        if !bytes.is_empty() {
            // Plugins see the raw stream first, so they can rewrite before it
            // is interpreted.
            let bytes = plugins.on_output(&bytes);

            // Mute logging across the password step so a credential never
            // reaches disk.
            let muting = self.autologon.state() == AutoState::WaitPassword;
            if let Some(log) = self.log.as_mut() {
                if muting {
                    log.mute();
                }
                let _ = log.write_raw(&bytes);
            }

            // IRIS speaks a configurable codepage; transcode before the parser,
            // which assumes UTF-8. Escape sequences are ASCII either way.
            let decoded = self.profile.encoding.decode(&bytes);
            let replies = crate::term::parser::advance(&mut self.parser, &mut self.grid, &decoded);
            if !replies.is_empty() {
                let _ = session.write(&replies);
            }

            // Autologon reads the decoded screen rather than raw bytes, so a
            // prompt split across two reads is still recognised.
            if let Some(to_send) = self.autologon.observe(&self.grid) {
                let _ = session.write(&self.profile.encoding.encode(&to_send));
            }

            let still_on_password = self.autologon.state() == AutoState::WaitPassword;
            let settled = self.grid.scrollback.len();
            if let Some(log) = self.log.as_mut() {
                if !still_on_password {
                    log.unmute();
                }
                // Only transcribed once the log says it will read it: this runs
                // on every chunk of output, and the transcript is a `String`
                // per line of the whole history.
                if log.wants_settled() {
                    let lines = self.grid.all_text();
                    let _ = log.write_settled(&lines, settled);
                }
            }
        }

        // A clear that never came - the session was sitting in a `read`, say,
        // and swallowed the command as input. The purge is called off rather
        // than left armed to surprise the next clear-screen.
        if let Some(asked) = self.clear_asked {
            if asked.elapsed() > std::time::Duration::from_secs(2) {
                self.grid.cancel_purge();
                self.clear_asked = None;
            }
        }

        if ended {
            self.ended = true;
            self.session = None;
            self.note("session ended");
        }
    }

    /// Sends raw bytes (key sequences, already in wire form).
    pub fn send(&self, bytes: &[u8]) {
        if let Some(session) = self.session.as_ref() {
            let _ = session.write(bytes);
        }
    }

    /// Sends typed or pasted text, encoded into the instance's codepage.
    pub fn send_text(&self, text: &str) {
        if let Some(session) = self.session.as_ref() {
            let _ = session.write(&self.profile.encoding.encode(text));
        }
    }

    /// Sends whole lines, each terminated the way Enter would terminate it.
    pub fn send_lines(&self, lines: &[String]) {
        for line in lines {
            self.send_text(line);
            self.send(b"\r");
        }
    }

    /// Asks IRIS to clear its own screen, the way typing `W #` would, and
    /// arranges for the clear that comes back to drop the transcript instead of
    /// filing it.
    ///
    /// The terminal cannot do this on its own. The far side keeps its own idea
    /// of where the cursor is and repaints by absolute position, so a grid
    /// cleared behind its back leaves the next prompt painted back down at the
    /// row it had reached, with blank rows above it. `W #` is what resets that
    /// idea. Nothing of it stays on screen: the echoed command and the pre-clear
    /// screen are both dropped by the purge.
    ///
    /// Only ever called at an idle prompt - see
    /// [`App::clear_active_terminal`], which is what keeps the command from
    /// being swallowed as input by a `read` or appended to a half-typed line.
    pub fn request_clear(&mut self) {
        if self.session.is_none() {
            return;
        }
        self.grid.purge_history_on_next_clear();
        self.clear_asked = Some(std::time::Instant::now());
        self.send_lines(&["W #".to_string()]);
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        if cols == self.grid.cols && rows == self.grid.rows {
            return;
        }
        self.grid.resize(cols, rows);
        if let Some(session) = self.session.as_mut() {
            let _ = session.resize(cols as u16, rows as u16);
        }
    }
}

/// Arrow keys that walk IRIS's cursor `columns` to the right, or to the left
/// when negative. Empty for no movement.
fn cursor_bytes(columns: i64) -> Vec<u8> {
    if columns == 0 {
        return Vec::new();
    }
    let sequence: &[u8] = if columns < 0 { b"\x1b[D" } else { b"\x1b[C" };
    sequence.repeat(columns.unsigned_abs() as usize)
}

/// Columns of the command line covered by the tab's selection.
///
/// `None` unless the whole selection sits inside the line being typed: a
/// selection that reaches into the scrollback is highlighted text, and there is
/// nothing in IRIS's read buffer that corresponds to it.
fn selection_in_line(tab: &Tab, line: crate::term::LineEdit) -> Option<(usize, usize)> {
    let at = tab.grid.scrollback.len() + tab.grid.cursor.row;
    let (from, to) = tab.view.selection?.span_on(at)?;
    let from = from.max(line.start);
    let to = to.min(line.end);
    (from < to).then_some((from, to))
}

fn open_log(profile: &Profile, settings: &Settings) -> Option<SessionLog> {
    let mode = settings.log_mode_for(profile);
    if mode == LogMode::Off {
        return None;
    }
    match SessionLog::open(&settings.log_dir, &profile.name, mode, LOG_ROTATE_BYTES) {
        Ok(log) => Some(log),
        Err(e) => {
            log::error!("logging disabled for {}: {e:#}", profile.name);
            None
        }
    }
}

pub struct App {
    pub settings: Settings,
    pub themes: Vec<Theme>,
    pub tabs: Vec<Tab>,
    pub active: usize,
    /// Instances found on this machine, for the new-tab dialog.
    pub instances: Vec<String>,
    /// Servers the InterSystems launcher knows about, and which one it treats
    /// as preferred. Read once at startup: the Server Manager is a separate
    /// program, and a list that changed under the app mid-session would only
    /// ever surprise the user. Restarting picks up an edit.
    pub servers: crate::config::ServerList,
    pub macro_groups: Vec<MacroGroup>,
    /// Commands typed at an IRIS prompt, shared by every tab so a new one opens
    /// knowing what was run in the last.
    history: History,
    plugins: PluginHost,
    panels: PanelState,
    /// The tab whose terminal last took keyboard focus, so a tab switch can
    /// hand focus over exactly once instead of fighting dialogs every frame.
    focused_tab: Option<u64>,
    /// Size the terminal was last drawn at. New sessions open at this size so
    /// their output is never truncated at a stale width.
    terminal_size: (u16, u16),
    /// Size of the window in character cells, as last drawn. Distinct from
    /// `terminal_size`, which is the wider grid IRIS is told about; this is the
    /// geometry the user is actually looking at and the one worth reporting.
    view_size: (usize, usize),
    /// Profile a new tab opens with. There is no dialog in front of it: the
    /// `+` button and Ctrl+T connect straight away, and this is what they
    /// connect to. Right-clicking `+` picks a different one.
    new_tab_profile: Profile,
    status: Option<String>,
    /// When the message in the footer went up, so it can be taken down again
    /// once `status_timeout_secs` has passed.
    status_at: Option<std::time::Instant>,
    /// What the update thread has told us so far, and what the dialog is
    /// showing. Every field is `None` on a build that has never checked.
    updates: UpdateState,
    /// Font family egui has actually been given, which is not always the one
    /// in `settings`: a family that is no longer installed has to degrade to
    /// the bundled monospace, because naming an unregistered family panics
    /// inside egui's glyph measurement.
    font_family: String,
    /// Family last handed to [`fonts::install`], successfully or not. Settings
    /// changes arrive every frame while a slider is dragged, and reinstalling
    /// rebuilds the glyph atlas, so the work is skipped unless the name moved.
    font_request: String,
    /// Tab whose name is being edited, if any.
    renaming: Option<Renaming>,
    /// Set when a close was intercepted to ask about live sessions.
    confirm_close: bool,
    /// Set once the user has said to close anyway, so the confirmation cannot
    /// cancel the very close it just approved.
    close_confirmed: bool,
    /// The opening size still to be applied, while there is one.
    fit: Option<WindowFit>,
    /// Window geometry as last seen by [`App::track_window_geometry`], which is
    /// what gets written back on exit for the next launch to open at. Kept
    /// frame by frame because a window that is closing no longer has a rect to
    /// ask for.
    window_size: Option<[f32; 2]>,
    window_position: Option<[f32; 2]>,
    window_maximized: bool,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let _ = ensure_config_tree();
        let settings = Settings::load();
        // Before anything is drawn: every label the first frame asks for goes
        // through `tr`, which reads this.
        crate::i18n::set_language(settings.language);
        let themes = load_themes();

        // Housekeeping that would otherwise never happen.
        let _ = logging::prune(&settings.log_dir, settings.log_retention_days);

        let l = launcher();
        let instances = crate::pty::launcher::instances(l.as_ref())
            .into_iter()
            .map(|i| i.name)
            .collect::<Vec<_>>();

        // The launcher's own list, so the servers offered here are the ones the
        // rest of the toolchain already has configured.
        let servers = crate::config::servers::discover();

        let plugins = if settings.enable_plugins {
            PluginHost::load_from(&config::plugins_dir())
        } else {
            PluginHost::disabled()
        };

        // What a new tab connects to, in order of how deliberate the choice
        // was: a startup profile the user configured here, then whatever the
        // launcher's tray menu is set to open, then whatever instance was found
        // first.
        let default_profile = settings
            .startup_profile()
            .cloned()
            .or_else(|| preferred_profile(&servers, &instances))
            .unwrap_or_else(|| Profile {
                instance: instances.first().cloned().unwrap_or_default(),
                ..Profile::default()
            });

        // Nothing to fit when a saved pixel size is being restored: that size
        // already produces the geometry it was recorded at. A window opening
        // maximized has no say in its size either.
        let fit = (settings.restored_window_size().is_none() && !settings.restored_maximized())
            .then(|| WindowFit {
                cols: settings.default_geometry().0,
                rows: settings.default_geometry().1,
                attempts: FIT_ATTEMPTS,
                recentre: settings.restored_window_position().is_none(),
                centre: None,
            });

        let mut app = App {
            new_tab_profile: default_profile,
            macro_groups: load_macros(&settings).groups,
            history: History::load(
                &config::command_history_path(),
                settings.save_command_history,
            ),
            settings,
            themes,
            tabs: Vec::new(),
            active: 0,
            instances,
            servers,
            plugins,
            panels: PanelState::default(),
            focused_tab: None,
            terminal_size: (FALLBACK_COLS, FALLBACK_ROWS),
            view_size: (FALLBACK_COLS as usize, FALLBACK_ROWS as usize),
            status: None,
            status_at: None,
            updates: UpdateState::default(),
            font_family: String::new(),
            font_request: String::new(),
            renaming: None,
            confirm_close: false,
            close_confirmed: false,
            fit,
            window_size: None,
            window_position: None,
            window_maximized: false,
        };

        // Whatever the last update left behind is no longer running and can
        // go, and the check for the next one starts now: it is a request over
        // a corporate proxy, so it is not going to answer this frame.
        update::clean_up();
        if app.settings.check_for_updates {
            app.updates.start_check();
        }

        App::apply_style(&cc.egui_ctx, &app.theme(), &app.settings);
        app.apply_font(&cc.egui_ctx);

        // Straight into the instance. Anyone with something to change has
        // Settings; everyone else was only ever going to press Connect.
        if app.settings.open_on_start {
            app.open_new_tab();
        }
        app
    }

    /// Applies theme colours *and* the style bits that are not part of
    /// `Visuals`. `set_visuals` replaces only the colours, so the scrollbar
    /// spacing has to be set separately or the switch would do nothing.
    fn apply_style(ctx: &Context, theme: &Theme, settings: &Settings) {
        ctx.set_visuals(theme.visuals());
        ctx.style_mut(|style| {
            // Floating bars are egui's default and are the reason the
            // scrollbars read as absent: they stay a hairline until hovered.
            style.spacing.scroll.floating = !settings.show_scrollbars;
            if settings.show_scrollbars {
                style.spacing.scroll.bar_width = 10.0;
            }
        });
    }

    /// Hands the configured font to egui and records what it actually got.
    ///
    /// The setting wins over the theme's suggestion; the theme's own
    /// `font_family` is the default for anyone who has not chosen one. A name
    /// that will not load leaves the bundled monospace in place and says so,
    /// rather than being passed on to panic later.
    fn apply_font(&mut self, ctx: &Context) {
        let wanted = if self.settings.font_family.is_empty() {
            self.theme().font_family
        } else {
            self.settings.font_family.clone()
        };

        if wanted == self.font_request {
            return;
        }
        self.font_request = wanted.clone();

        if fonts::install(ctx, &wanted) {
            self.font_family = wanted;
        } else {
            self.font_family = String::new();
            self.set_status(tr1(
                "Font {} is not installed; using the built-in monospace.",
                &format!("{wanted:?}"),
            ));
        }
    }

    /// How the terminal should be drawn, from the current settings.
    fn render_opts(&self) -> RenderOpts {
        RenderOpts {
            font_size: self.settings.font_size,
            font_family: self.font_family.clone(),
            cursor_style: self.settings.cursor_style,
            cursor_blink: self.settings.cursor_blink,
            scrollbar: self.settings.show_scrollbars,
            syntax: self.settings.terminal_syntax_highlight,
            wrap: self.settings.wrap_lines,
            copy_on_select: self.settings.copy_on_select,
        }
    }

    pub fn theme(&self) -> Theme {
        self.themes
            .iter()
            .find(|t| t.name == self.settings.theme)
            .cloned()
            .unwrap_or_default()
    }

    /// Clears the active terminal and forgets its history.
    ///
    /// The deliberate version of what `W #` only looks like: IRIS's own
    /// clear-screen files the screen into the scrollback, and this is the
    /// gesture for when the history really is meant to go. Also on the
    /// right-click menu, because a shortcut nobody can see is a shortcut nobody
    /// uses.
    ///
    /// Clearing the grid on its own is not enough, and this is the whole
    /// subtlety of the gesture: the far side keeps its own idea of where the
    /// cursor is and repaints by absolute position, so a screen wiped behind
    /// its back gets the next prompt painted back down at the row it had
    /// reached, with the cleared rows blank above it. The only thing that
    /// resets that idea is a clear-screen from IRIS, so at an idle prompt the
    /// clear is asked of IRIS - and the echoed command and the pre-clear screen
    /// are both dropped, so nothing of it is left on screen or in the
    /// scrollback.
    ///
    /// Anywhere else - mid-line, mid-routine, or on a session that has ended -
    /// the command could be swallowed as input or appended to what is being
    /// typed, so the grid is cleared locally instead, and the prompt stays
    /// where IRIS left it.
    ///
    /// Nothing is said about any of this in the status line. The gesture is a
    /// frequent one, and a note that appears under every clear costs a row of
    /// the terminal to say what the screen has already shown; what it does is
    /// on the right-click entry's tooltip instead.
    fn clear_active_terminal(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        tab.view.clear_selection();
        tab.view.scroll_to_bottom();

        // An idle prompt: IRIS is reading a command line and nothing has been
        // typed on it yet.
        let idle_prompt = lineedit::current(&tab.grid).is_some_and(|line| line.is_empty());
        if tab.session.is_some() && idle_prompt {
            tab.request_clear();
        } else {
            tab.grid.hard_reset();
        }
    }

    /// Connects a new tab to whatever `new_tab_profile` currently names.
    pub fn open_new_tab(&mut self) {
        let profile = self.new_tab_profile.clone();
        self.open_tab(profile);
    }

    pub fn open_tab(&mut self, profile: Profile) {
        let (cols, rows) = self.terminal_size;
        self.tabs
            .push(Tab::new(profile, &self.settings, cols, rows));
        self.active = self.tabs.len() - 1;
    }

    pub fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        // Ask IRIS to halt so it releases locks; Drop kills anything that
        // ignores the request.
        if let Some(session) = self.tabs[index].session.as_ref() {
            session.request_halt();
        }
        self.tabs.remove(index);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.tabs.get(self.active)
    }

    fn set_status(&mut self, text: impl Into<String>) {
        self.status = Some(text.into());
        self.status_at = Some(std::time::Instant::now());
    }

    /// Takes the footer message down once it has had its time, and asks for the
    /// frame that will do it.
    ///
    /// Without the repaint request the message would sit there until something
    /// else happened to draw a frame - which, at an idle prompt, is nothing at
    /// all.
    fn expire_status(&mut self, ctx: &Context) {
        let seconds = self.settings.status_timeout_secs;
        if seconds == 0 || self.status.is_none() {
            return;
        }
        let life = std::time::Duration::from_secs(u64::from(seconds));
        let Some(at) = self.status_at else {
            return;
        };
        match life.checked_sub(at.elapsed()) {
            Some(left) if !left.is_zero() => ctx.request_repaint_after(left),
            _ => {
                self.status = None;
                self.status_at = None;
            }
        }
    }

    /// Whether the active terminal currently owns the keyboard.
    ///
    /// Macro shortcuts are gated on this. `Ctrl+Shift+G` is a shortcut when
    /// the terminal has focus and an ordinary editing gesture when the cursor
    /// is in a text field, and only the widget with focus can say which.
    fn terminal_has_focus(&self, ctx: &Context) -> bool {
        let Some(tab) = self.active_tab() else {
            return false;
        };
        let id = egui::Id::new(("nit-terminal", tab.uid));
        ctx.memory(|m| m.focused() == Some(id))
    }

    fn handle_shortcuts(&mut self, ctx: &Context) {
        let cmd = Modifiers::COMMAND;
        let new_tab = consume_exact(ctx, cmd, Key::T);
        let close_tab = consume_exact(ctx, cmd, Key::W);
        let next_tab = consume_exact(ctx, cmd, Key::Tab);
        let zoom_in = consume_exact(ctx, cmd, Key::Plus) | consume_exact(ctx, cmd, Key::Equals);
        let zoom_out = consume_exact(ctx, cmd, Key::Minus);

        let mut jump = None;
        for (n, key) in [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ]
        .iter()
        .enumerate()
        {
            if consume_exact(ctx, cmd, *key) {
                jump = Some(n);
            }
        }

        if new_tab {
            self.open_new_tab();
        }
        if close_tab && !self.tabs.is_empty() {
            self.close_tab(self.active);
        }
        if next_tab && !self.tabs.is_empty() {
            self.active = (self.active + 1) % self.tabs.len();
        }
        if let Some(n) = jump {
            if n < self.tabs.len() {
                self.active = n;
            }
        }
        // Font size drives cell size, which drives grid dimensions, so zooming
        // reflows and resizes the PTY through the ordinary resize path.
        if zoom_in || zoom_out {
            let delta = if zoom_in { 1.0 } else { -1.0 };
            self.settings.font_size = (self.settings.font_size + delta).clamp(8.0, 28.0);
            let _ = self.settings.save();
        }

        // Ctrl+Delete: reset the terminal and drop the history, the one
        // gesture that is meant to destroy the transcript. Gated on the
        // terminal having focus so it stays "delete word" in a text field.
        let terminal_focus = self.terminal_has_focus(ctx);
        if terminal_focus && consume_exact(ctx, Modifiers::CTRL, Key::Delete) {
            self.clear_active_terminal();
        }

        // Macro shortcuts come after the app's own, which is what the editor's
        // "the app already uses this" warning promises. A macro whose `key` is
        // missing or unparseable simply never fires; the text is shared and
        // hand-edited, so it cannot be trusted to mean anything.
        if terminal_focus {
            let mut fire = None;
            for group in &self.macro_groups {
                for m in &group.macros {
                    let Some((modifiers, key)) = m.key.as_deref().and_then(shortcut::parse) else {
                        continue;
                    };
                    if consume_exact(ctx, modifiers, key) {
                        fire = Some(m.clone());
                    }
                }
            }
            // Routed through the ordinary request path, so `confirm` and
            // parameter prompting apply exactly as they do to a click.
            if let Some(m) = fire {
                self.handle_request(ctx, UiRequest::RunMacro(m));
            }
        }
    }

    /// The top row: app controls on the left, and - when the app is drawing its
    /// own frame - the window buttons and the draggable area on the right.
    fn menu_bar(
        &mut self,
        ui: &mut egui::Ui,
        buttons: &crate::config::theme::WindowButtons,
    ) -> Option<WindowAction> {
        let mut action = None;
        // Opening a tab has to happen after the closure: it borrows `self`
        // mutably, and the bar is already holding it.
        let mut open_default = false;
        let mut pick: Option<Profile> = None;
        // The system bar brings its own controls, and the setting can turn the
        // app's off entirely.
        let own_buttons = !self.settings.native_decorations && self.settings.show_window_buttons;
        ui.horizontal(|ui| {
            // Drawn before anything else when the theme puts them on the left,
            // which is where Aqua has them.
            if own_buttons && buttons.left {
                if let Some(asked) = chrome::leading_window_buttons(ui, buttons) {
                    action = Some(asked);
                }
                ui.add_space(6.0);
            }
            let endpoint = self.new_tab_profile.endpoint();
            let new_tab = ui.button("+").on_hover_text(tr1(
                "New session on {} (Ctrl+T).\nRight-click to connect somewhere else.",
                &endpoint,
            ));
            if new_tab.clicked() {
                open_default = true;
            }
            // The escape hatch that replaces the dialog: everything it used to
            // offer - the profiles and the instances found on this machine -
            // one click away instead of in front of every new session.
            new_tab.context_menu(|ui| {
                // The launcher's servers first: they are the whole reason this
                // menu is worth opening, and the preferred one is already what
                // the button does.
                if !self.servers.is_empty() {
                    ui.weak(tr("IRIS servers"));
                    let preferred = self.new_tab_profile.name.clone();
                    for server in self.servers.others(Some(&preferred)) {
                        let mut button = ui.button(server.menu_label());
                        // What the entry actually does, since a local server
                        // opens an instance and a remote one asks for a login.
                        let hint = match server.target(&self.instances) {
                            crate::config::servers::Target::Local { instance } => {
                                tr1("Local session on instance {}", &instance)
                            }
                            crate::config::servers::Target::Telnet { address, port } => {
                                tr1("Telnet login to {}", &format!("{address}:{port}"))
                            }
                        };
                        let hint = if server.comment.trim().is_empty() {
                            hint
                        } else {
                            format!("{hint}\n{}", server.comment.trim())
                        };
                        button = button.on_hover_text(hint);
                        if button.clicked() {
                            pick = Some(Profile::for_server(
                                server,
                                &self.instances,
                                &self.new_tab_profile,
                            ));
                            ui.close_menu();
                        }
                    }
                    let loose_instances = self
                        .instances
                        .iter()
                        .any(|name| !self.servers.covers_instance(&self.instances, name));
                    if !self.settings.profiles.is_empty() || loose_instances {
                        ui.separator();
                    }
                }
                for profile in &self.settings.profiles {
                    let label = if profile.instance.is_empty() {
                        profile.name.clone()
                    } else {
                        format!("{}  ({})", profile.name, profile.instance)
                    };
                    if ui.button(label).clicked() {
                        pick = Some(profile.clone());
                        ui.close_menu();
                    }
                }
                if !self.settings.profiles.is_empty() && !self.instances.is_empty() {
                    ui.separator();
                }
                for name in &self.instances {
                    // Skipped when a server entry already opens it: the same
                    // session under two names is not a choice.
                    if self.servers.covers_instance(&self.instances, name) {
                        continue;
                    }
                    if ui.button(name).clicked() {
                        // Built through the same path as a server, which is what
                        // guarantees a local instance opens locally. Inheriting
                        // the current profile wholesale used to carry its
                        // `remote` across, so picking the instance `CONSISTEM`
                        // while a Telnet tab was current opened Telnet again.
                        pick = Some(Profile::for_server(
                            &crate::config::servers::Server::for_instance(name),
                            &self.instances,
                            &self.new_tab_profile,
                        ));
                        ui.close_menu();
                    }
                }
                if self.settings.profiles.is_empty()
                    && self.instances.is_empty()
                    && self.servers.is_empty()
                {
                    ui.weak(tr("No servers, profiles or instances found."));
                }
            });
            ui.separator();
            ui.toggle_value(&mut self.panels.show_macros, tr("Macros"));
            // Toggles, not plain buttons: clicking the button that opened a
            // dialog is how everyone expects to close it again, and it shows
            // which dialogs are open the way Macros already does.
            ui.toggle_value(&mut self.panels.show_export, tr("Export"));
            ui.toggle_value(&mut self.panels.show_settings, tr("Settings"));
            ui.separator();
            if let Some(tab) = self.active_tab() {
                let (cols, rows) = self.view_size;
                ui.weak(format!("{}  {cols}x{rows}", tab.profile.endpoint()));
            }

            // Last, so the leftover space it claims for dragging is whatever
            // the items above did not take. Claimed even when the controls are
            // hidden or already drawn on the left: without it there is nothing
            // to drag the window by.
            if !self.settings.native_decorations {
                let trailing = own_buttons && !buttons.left;
                if let Some(asked) = chrome::title_bar_controls(ui, buttons, trailing, "nit-main") {
                    action = Some(asked);
                }
            }
        });

        // A pick opens that session and leaves the default alone. Making the
        // choice stick was worse than it sounds: after one Telnet server, the
        // plain "+" kept reconnecting to it, and there was no longer any way to
        // get back to the preferred server except through the menu.
        if let Some(profile) = pick {
            self.open_tab(profile);
        } else if open_default {
            self.open_new_tab();
        }
        action
    }

    /// Notes where the window is and how big it is, so that the values are at
    /// hand when the app exits and the window has already gone.
    ///
    /// A minimized window is skipped rather than recorded: Windows parks one
    /// off-screen at a nonsense position, and reopening there would put the app
    /// somewhere the user cannot reach it. A maximized one keeps whatever
    /// restored geometry was recorded before it was maximized, which is exactly
    /// what its own restore button would give back.
    fn track_window_geometry(&mut self, ctx: &Context) {
        ctx.input(|i| {
            let viewport = i.viewport();
            if viewport.minimized.unwrap_or(false) {
                return;
            }
            self.window_maximized = viewport.maximized.unwrap_or(false);
            if self.window_maximized || viewport.fullscreen.unwrap_or(false) {
                return;
            }
            if let Some(rect) = viewport.inner_rect {
                if rect.width() >= 1.0 && rect.height() >= 1.0 {
                    self.window_size = Some([rect.width(), rect.height()]);
                }
            }
            if let Some(rect) = viewport.outer_rect {
                // The off-screen parking spot again, for the platforms that do
                // not report `minimized` at all.
                if rect.min.x.is_finite() && rect.min.y.is_finite() && rect.min.x > -30_000.0 {
                    self.window_position = Some([rect.min.x, rect.min.y]);
                }
            }
        });
    }

    /// Writes the geometry back to the settings file, for whichever of the two
    /// switches is on. Called as the app exits.
    fn persist_window_geometry(&mut self) {
        let mut changed = false;
        if self.settings.save_terminal_size {
            if self.window_size.is_some() && self.settings.window_size != self.window_size {
                self.settings.window_size = self.window_size;
                changed = true;
            }
            if self.settings.window_maximized != self.window_maximized {
                self.settings.window_maximized = self.window_maximized;
                changed = true;
            }
        }
        if self.settings.save_window_position
            && self.window_position.is_some()
            && self.settings.window_position != self.window_position
        {
            self.settings.window_position = self.window_position;
            changed = true;
        }
        if changed {
            if let Err(e) = self.settings.save() {
                log::warn!("could not save the window geometry: {e}");
            }
        }
    }

    /// Nudges the window until the terminal measures exactly the geometry it
    /// was asked to open at, then stops touching it for the rest of the run.
    ///
    /// `view_cols` and `view_rows` are what the last frame actually drew, so
    /// the difference from the target converts straight into points through
    /// `cell`. The half point of slack is there because the view floors the
    /// space it is given: a window one rounding error short of the target would
    /// otherwise come out a column narrower.
    fn fit_window(&mut self, ctx: &Context, view_cols: usize, view_rows: usize, cell: egui::Vec2) {
        let Some(fit) = self.fit.as_mut() else {
            return;
        };
        let target = (fit.cols as usize, fit.rows as usize);
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        if maximized || fit.attempts == 0 || (view_cols, view_rows) == target {
            let (want_cols, want_rows) = target;
            log::debug!(
                "window opened at {view_cols}x{view_rows} characters, asked for {want_cols}x{want_rows}"
            );
            self.fit = None;
            return;
        }
        let Some(inner) = ctx.input(|i| i.viewport().inner_rect) else {
            return;
        };
        if fit.recentre && fit.centre.is_none() {
            fit.centre = ctx.input(|i| i.viewport().outer_rect).map(|r| r.center());
        }
        fit.attempts -= 1;

        let size = fitted_inner_size(inner.size(), (view_cols, view_rows), target, cell);
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        if let Some(centre) = fit.centre {
            // The position is the frame's, the size asked for is the content's,
            // so a system title bar has to be added back before the two can be
            // put on top of each other. Zero with the app's own chrome.
            let border = ctx
                .input(|i| i.viewport().outer_rect)
                .map(|outer| outer.size() - inner.size())
                .unwrap_or_default();
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(
                centre - (size + border) / 2.0,
            ));
        }
        // The measurement that says whether this worked only exists on the next
        // frame, and an idle terminal has no other reason to draw one.
        ctx.request_repaint();
    }

    /// Whether closing should stop and ask first.
    fn should_confirm_close(&self) -> bool {
        !self.close_confirmed
            && self.settings.confirm_close_with_live_session
            && self.tabs.iter().any(|t| t.session.is_some())
    }

    /// Asks before dropping live sessions.
    ///
    /// Honours `confirm_close_with_live_session`, which until now was a setting
    /// nothing read. It covers both routes in: the app's own close button and
    /// the window manager's, since with the system frame gone the second is
    /// often the only one left.
    fn close_confirm_dialog(&mut self, ctx: &Context) {
        if !self.confirm_close {
            return;
        }

        let live = self.tabs.iter().filter(|t| t.session.is_some()).count();
        let mut close_anyway = false;
        let mut cancel = false;
        let mut open = true;

        egui::Window::new(tr("Close newIrisTerminal?"))
            .id(egui::Id::new("nit-close-confirm"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(match live {
                    1 => tr("1 session is still connected.").to_string(),
                    n => tr1("{} sessions are still connected.", &n.to_string()),
                });
                ui.small(tr("Closing sends HALT to each of them."));
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(tr("Close anyway")).clicked() {
                        close_anyway = true;
                    }
                    if ui.button(tr("Keep working")).clicked() {
                        cancel = true;
                    }
                });
            });

        if close_anyway {
            self.confirm_close = false;
            self.close_confirmed = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if cancel || !open {
            self.confirm_close = false;
        }
    }

    fn tab_strip(&mut self, ui: &mut egui::Ui) {
        let mut to_close = None;
        let mut to_rename = None;
        egui::ScrollArea::horizontal().show(ui, |ui| {
            ui.horizontal(|ui| {
                for index in 0..self.tabs.len() {
                    let selected = index == self.active;
                    let mut label = self.tabs[index].title();
                    if self.tabs[index].ended {
                        label.push_str(" (ended)");
                    }
                    let response = ui
                        .selectable_label(selected, label)
                        .on_hover_text(tr("Double-click to rename."));
                    if response.clicked() {
                        self.active = index;
                    }
                    if response.double_clicked() {
                        to_rename = Some(index);
                    }
                    response.context_menu(|ui| {
                        if ui.button(tr("Rename...")).clicked() {
                            to_rename = Some(index);
                            ui.close_menu();
                        }
                        if ui.button(tr("Close")).clicked() {
                            to_close = Some(index);
                            ui.close_menu();
                        }
                    });
                    if ui.small_button("x").clicked() {
                        to_close = Some(index);
                    }
                    ui.separator();
                }
            });
        });

        if let Some(index) = to_rename {
            // Seeded with the name on screen, so renaming is an edit rather
            // than starting from nothing.
            self.renaming = Some(Renaming {
                tab: index,
                draft: self.tabs[index].title(),
                focus: true,
            });
        }
        // Applied after the loop: closing a tab shifts every index after it.
        if let Some(index) = to_close {
            self.close_tab(index);
        }
    }

    /// Names one tab.
    ///
    /// A window rather than an editable label in the strip: the terminal claims
    /// the keyboard whenever nothing else holds it, and a field that has to win
    /// that fight every frame is a worse trade than one dialog.
    fn rename_tab_dialog(&mut self, ctx: &Context) {
        let Some(mut renaming) = self.renaming.take() else {
            return;
        };
        if renaming.tab >= self.tabs.len() {
            return;
        }

        let mut open = true;
        let mut commit = false;
        let mut clear = false;
        let mut cancel = false;

        egui::Window::new(tr("Rename tab"))
            .id(egui::Id::new("nit-rename-tab"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                let response =
                    ui.add(egui::TextEdit::singleline(&mut renaming.draft).desired_width(220.0));
                if renaming.focus {
                    response.request_focus();
                    renaming.focus = false;
                }
                if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    commit = true;
                }

                ui.small(tr("Empty goes back to the automatic name."));
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(tr("Rename")).clicked() {
                        commit = true;
                    }
                    if ui.button(tr("Automatic")).clicked() {
                        clear = true;
                    }
                    if ui.button(tr("Cancel")).clicked() {
                        cancel = true;
                    }
                });
            });

        if commit {
            let name = renaming.draft.trim().to_string();
            self.tabs[renaming.tab].custom_title = (!name.is_empty()).then_some(name);
        } else if clear {
            self.tabs[renaming.tab].custom_title = None;
        } else if !(cancel || !open) {
            // Still open, so the draft survives to the next frame.
            self.renaming = Some(renaming);
        }
    }

    /// Carries out whatever a panel asked for.
    fn handle_request(&mut self, ctx: &Context, request: UiRequest) {
        match request {
            UiRequest::RunMacro(m) => {
                // Parameters or a confirmation flag both mean "ask first".
                if m.needs_input() || m.confirm {
                    self.panels.pending = Some(PendingMacro::new(m));
                } else {
                    let lines = m.expand(&[]);
                    self.send_lines_to_active(&lines);
                }
            }
            UiRequest::RunNative(native, values) => {
                let invocation = native.build(&values);
                self.send_lines_to_active(&invocation.lines);
                self.set_status(invocation.summary);
            }
            UiRequest::SendLines(lines) => self.send_lines_to_active(&lines),

            UiRequest::ExportText(range) => self.export(range, export::Format::Text),
            UiRequest::ExportHtml(range) => self.export(range, export::Format::Html),
            UiRequest::CopyRange(range) => {
                if let Some(tab) = self.active_tab() {
                    let text = export::to_text(&tab.grid, range);
                    ctx.copy_text(text);
                    self.set_status(tr("Copied to the clipboard."));
                }
            }

            UiRequest::SettingsChanged => {
                // The style is reapplied either way: a failed write still has
                // to be reflected on screen, or the UI would disagree with the
                // settings the user just changed.
                App::apply_style(ctx, &self.theme(), &self.settings);
                self.apply_font(ctx);
                self.history.set_persist(
                    &config::command_history_path(),
                    self.settings.save_command_history,
                );
                if let Err(e) = self.settings.save() {
                    self.set_status(tr1("Could not save settings: {}", &format!("{e:#}")));
                }
            }
            UiRequest::ReloadMacros => {
                let report = load_macros(&self.settings);
                self.macro_groups = report.groups;
                // Selection and draft are indices into the list that was just
                // replaced, so they would now address a different macro.
                self.panels.forget_macro_selection();
                let count: usize = self.macro_groups.iter().map(|g| g.macros.len()).sum();
                let reloaded = tr1("Reloaded {} macros.", &count.to_string());
                if report.problems.is_empty() {
                    self.set_status(reloaded);
                } else {
                    self.set_status(format!("{reloaded} {}", report.problems.join(" ")));
                }
            }
            UiRequest::OpenFolder(path) => {
                if let Err(e) = config::open_in_file_manager(&path) {
                    self.set_status(tr2(
                        "Could not open {}: {}",
                        &path.display().to_string(),
                        &format!("{e:#}"),
                    ));
                }
            }
            UiRequest::CheckForUpdates => self.updates.check_now(),
            UiRequest::SavePersonalMacros => {
                let path = config::personal_macros_path();
                let xml = macros::to_xml(&self.macro_groups);
                match std::fs::write(&path, xml) {
                    Ok(()) => self.set_status(tr1(
                        "Saved personal macros to {}",
                        &path.display().to_string(),
                    )),
                    Err(e) => self.set_status(tr1("Could not save macros: {}", &format!("{e:#}"))),
                }
            }
        }
    }

    /// Takes whatever the update thread has said since the last frame.
    fn poll_updates(&mut self) {
        for event in self.updates.drain() {
            match event {
                update::Event::Available(release) => {
                    self.updates.available = Some(release);
                    self.updates.asked = true;
                }
                update::Event::UpToDate => {
                    // Only worth saying when the user asked the question.
                    if self.updates.announce {
                        self.set_status(tr1("This is the newest version ({}).", update::CURRENT));
                    }
                }
                update::Event::Downloaded(path) => {
                    self.updates.downloading = false;
                    self.updates.staged = Some(path);
                }
                update::Event::Failed(why) => {
                    self.updates.downloading = false;
                    // A failed check at startup is not the user's problem: it
                    // goes to the log. One they asked for is answered.
                    if self.updates.announce {
                        self.set_status(tr1("Could not check for updates: {}", &why));
                    } else {
                        log::warn!("update check failed: {why}");
                    }
                }
            }
            self.updates.announce = false;
        }
    }

    /// Puts the downloaded build in place and closes, so the copy that is
    /// starting takes over.
    fn apply_update(&mut self, ctx: &Context) {
        let Some(staged) = self.updates.staged.clone() else {
            return;
        };
        match update::install(&staged) {
            Ok(()) => {
                // The new copy is already starting; this one has to go, and it
                // goes the way Alt+F4 does so a live session is still asked
                // about.
                self.updates.available = None;
                self.updates.staged = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => {
                self.updates.error = Some(format!("{e:#}"));
                self.set_status(tr1("Could not apply the update: {}", &format!("{e:#}")));
            }
        }
    }

    /// Writes the active tab's output to a file and opens a Claude Code
    /// session on it.
    ///
    /// Two steps rather than one because they fail for different reasons and
    /// the user can act on each: nothing to analyze yet, or no `claude` on the
    /// PATH.
    fn analyze_with_claude(&mut self, scope: analyze::Scope) {
        let Some(tab) = self.active_tab() else {
            self.set_status(tr("No active session."));
            return;
        };
        let endpoint = tab.profile.endpoint();
        let selection = tab.view.selected_text(&tab.grid);
        let path = match analyze::write_context(&tab.grid, scope, &endpoint, selection.as_deref()) {
            Ok(path) => path,
            Err(e) => {
                self.set_status(tr1("Could not prepare the output: {}", &format!("{e:#}")));
                return;
            }
        };
        match analyze::launch(&path) {
            Ok(()) => self.set_status(tr1(
                "Claude is opening with this output in context ({}). Ask it whatever you like.",
                &path.display().to_string(),
            )),
            Err(e) => self.set_status(tr2(
                "Could not start claude: {}. The output is in {}",
                &format!("{e:#}"),
                &path.display().to_string(),
            )),
        }
    }

    /// Carries out what the theme manager asked for.
    ///
    /// The manager edits the themes in place so the window behind it repaints
    /// as a colour is dragged; this is the half that reaches the disk, which a
    /// paint pass has no business doing.
    fn apply_theme_action(&mut self, ctx: &Context, action: ThemeAction) {
        match action {
            ThemeAction::Activate(name) => {
                self.settings.theme = name;
                if let Err(e) = self.settings.save() {
                    self.set_status(tr1("Could not save settings: {}", &format!("{e:#}")));
                }
                App::apply_style(ctx, &self.theme(), &self.settings);
                self.apply_font(ctx);
            }
            ThemeAction::Save(name) => {
                let Some(index) = self.themes.iter().position(|t| t.name == name) else {
                    return;
                };
                // A built-in lives in the binary. Nothing here can edit one, so
                // nothing here writes one out either.
                if self.themes[index].builtin {
                    return;
                }
                let path = match self.themes[index].path.clone() {
                    Some(path) => path,
                    None => {
                        let path = config::theme_path_for(&name);
                        self.themes[index].path = Some(path.clone());
                        path
                    }
                };
                if let Err(e) = config::save_theme(&self.themes[index], &path) {
                    self.set_status(tr1("Could not save theme: {}", &format!("{e:#}")));
                }
            }
            ThemeAction::Delete(name) => {
                let Some(index) = self.themes.iter().position(|t| t.name == name) else {
                    return;
                };
                if self.themes[index].builtin {
                    return;
                }
                let removed = self.themes.remove(index);
                if let Some(path) = removed.path.as_ref() {
                    if let Err(e) = std::fs::remove_file(path) {
                        self.set_status(tr2(
                            "Could not delete {}: {}",
                            &path.display().to_string(),
                            &e.to_string(),
                        ));
                    }
                }
                // Deleting the theme in use has to leave something on screen.
                if self.settings.theme == name {
                    let fallback = self
                        .themes
                        .first()
                        .map(|t| t.name.clone())
                        .unwrap_or_default();
                    self.apply_theme_action(ctx, ThemeAction::Activate(fallback));
                } else {
                    self.set_status(tr1("Deleted theme {}.", &name));
                }
            }
        }
    }

    /// Remembers the command sitting on the active tab's prompt line.
    ///
    /// Read off the screen rather than accumulated from keystrokes, so a line
    /// that arrived by paste, or that IRIS recalled itself, counts exactly like
    /// a typed one. A row with no IRIS prompt on it is not a command line and
    /// is not recorded - which is also what keeps a password out of the file,
    /// since a credential prompt carries no `>` and is never echoed.
    /// `unechoed` is text typed in the same frame as the Enter, which IRIS has
    /// not sent back yet and which the screen therefore does not show.
    fn record_command(&mut self, tab: usize, unechoed: &str) {
        let Some(tab) = self.tabs.get_mut(tab) else {
            return;
        };
        tab.recall_index = None;
        if tab.autologon.state() == AutoState::WaitPassword {
            return;
        }
        let Some(text) = lineedit::typed_text(&tab.grid) else {
            return;
        };
        self.history.record(&format!("{text}{unechoed}"));
    }

    /// Replaces the line being typed with the next command from the history.
    ///
    /// IRIS keeps its own recall, but only for as long as the process lives and
    /// only for what was typed into it; this is the app's, shared by every tab
    /// and - when the setting allows - by every session that came before.
    ///
    /// Done by rubbing the line out and typing the replacement, because IRIS
    /// owns the read buffer and the only way to change it is to send the keys
    /// that would have changed it.
    fn recall(&mut self, index: usize, direction: input::Recall) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        // Only ever called at the end of a command line, but the state could
        // have moved on between the key press and here.
        let Some(line) = lineedit::current(&tab.grid).filter(|l| l.at_end()) else {
            return;
        };

        let next = match direction {
            input::Recall::Back => match self.history.back(tab.recall_index) {
                Some(at) => Some(at),
                // Already at the oldest command: leave the line alone rather
                // than clearing it.
                None => return,
            },
            input::Recall::Forward => match self.history.forward(tab.recall_index) {
                Some(next) => next,
                None => return,
            },
        };

        let text = next
            .and_then(|at| self.history.get(at))
            .unwrap_or_default()
            .to_string();

        let mut wire = vec![0x7f; line.len()];
        wire.extend_from_slice(&self.tabs[index].profile.encoding.encode(&text));
        let wire = self.plugins.on_input(&wire);
        self.tabs[index].send(&wire);
        self.tabs[index].recall_index = next;
        self.tabs[index].view.scroll_to_bottom();
    }

    /// Selects the command being typed, so it can be copied or rubbed out in
    /// one gesture. Ctrl+A.
    fn select_typed_line(&mut self, index: usize) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let Some(line) = lineedit::current(&tab.grid).filter(|l| !l.is_empty()) else {
            return;
        };
        let at = tab.grid.scrollback.len() + tab.grid.cursor.row;
        tab.view.selection = Some(Selection::across(at, line.start, line.end));
    }

    /// Drags the loose end of the selection over the command line, the way
    /// Shift plus a movement key does in a text field.
    ///
    /// The anchor is where the selection was started from - IRIS's cursor, the
    /// first time - and only the other end moves, so shift-left and then
    /// shift-right walks back over what was just selected. Nothing is sent:
    /// IRIS's cursor stays put and only the highlight moves.
    fn extend_selection(&mut self, index: usize, motion: Motion) {
        let Some(tab) = self.tabs.get_mut(index) else {
            return;
        };
        let Some(line) = lineedit::current(&tab.grid) else {
            return;
        };
        let at = tab.grid.scrollback.len() + tab.grid.cursor.row;

        // An existing selection on this line continues; anything else - none at
        // all, or one left over in the scrollback - starts again from the
        // cursor.
        let (anchor, focus) = tab
            .view
            .selection
            .filter(|s| s.span_on(at).is_some())
            .map(|s| (s.start.1, s.end.1))
            .unwrap_or((line.cursor, line.cursor));
        let anchor = anchor.clamp(line.start, line.end);
        let focus = lineedit::target(&tab.grid, line, focus, motion);

        // Walking the loose end back onto the anchor selects nothing, which is
        // no selection at all rather than an empty one - an empty selection
        // would go on quietly claiming Ctrl+C.
        tab.view.selection = (focus != anchor).then_some(Selection {
            start: (at, anchor),
            end: (at, focus),
        });
    }

    /// Walks IRIS's cursor over the command line by a whole motion - a word at
    /// a time, for Ctrl plus an arrow.
    ///
    /// Only the app knows where the word boundaries are, since only the app can
    /// see the line; IRIS is then told in the one language it acts on, which is
    /// arrow keys. The selection goes, exactly as it would in a text field when
    /// the cursor walks away from it.
    fn move_by(&mut self, index: usize, motion: Motion) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let Some(line) = lineedit::current(&tab.grid) else {
            return;
        };
        let to = lineedit::target(&tab.grid, line, line.cursor, motion);
        self.tabs[index].view.clear_selection();
        self.move_cursor(index, to as i64 - line.cursor as i64);
    }

    /// Rubs the selected part of the command line out of IRIS's read buffer.
    ///
    /// Rubout erases the character *before* the cursor, so the cursor is walked
    /// to the end of the selection first and the whole thing goes out as one
    /// write - a half-applied erase would leave the line in a state neither
    /// side agrees on.
    fn erase_selection(&mut self, index: usize) {
        let Some(tab) = self.tabs.get(index) else {
            return;
        };
        let Some(line) = lineedit::current(&tab.grid) else {
            return;
        };
        let Some((from, to)) = selection_in_line(tab, line) else {
            return;
        };

        let mut wire = cursor_bytes(to as i64 - line.cursor as i64);
        wire.extend(std::iter::repeat_n(0x7f, to - from));
        let wire = self.plugins.on_input(&wire);
        self.tabs[index].send(&wire);
        self.tabs[index].view.clear_selection();
        self.tabs[index].recall_index = None;
    }

    /// Walks IRIS's cursor `columns` to the right, or to the left when
    /// negative, with the arrow keys it does act on.
    fn move_cursor(&mut self, index: usize, columns: i64) {
        let wire = cursor_bytes(columns);
        if wire.is_empty() {
            return;
        }
        let wire = self.plugins.on_input(&wire);
        if let Some(tab) = self.tabs.get(index) {
            tab.send(&wire);
        }
    }

    fn send_lines_to_active(&mut self, lines: &[String]) {
        let Some(tab) = self.tabs.get(self.active) else {
            self.set_status(tr("No active session."));
            return;
        };
        if tab.session.is_none() {
            self.set_status(tr("That session has ended."));
            return;
        }
        tab.send_lines(lines);
    }

    fn export(&mut self, range: Range, format: export::Format) {
        let Some(tab) = self.active_tab() else {
            self.set_status(tr("No active session to export."));
            return;
        };
        let theme = self.theme();
        let contents = match format {
            export::Format::Text => export::to_text(&tab.grid, range),
            export::Format::Html => export::to_html(&tab.grid, range, &theme),
        };
        let name = export::suggested_name(&tab.profile.name, format);
        let path = self.settings.log_dir.join(name);

        match export::write_file(&path, &contents) {
            Ok(()) => self.set_status(tr1("Exported to {}", &path.display().to_string())),
            Err(e) => self.set_status(tr1("Export failed: {}", &format!("{e:#}"))),
        }
    }
}

/// Loads the organisation file (if configured) and the personal one, and
/// reports anything that went wrong so a missing share is visible rather than
/// silently halving the macro list.
fn load_macros(settings: &Settings) -> macros::LoadReport {
    let personal = config::personal_macros_path();
    // Ships the sample on first run, and refreshes it while it is still
    // exactly as shipped, so a change to the bundled set reaches an install
    // that has never edited the file.
    macros::ensure_personal_file(&personal);
    macros::load_all(settings.org_macros(), &personal)
}

/// Inner window size that turns a terminal of `view` characters into one of
/// `target` characters.
///
/// Only the difference is worked out, so everything around the terminal - the
/// menu bar, the tab strip, the scrollbar it reserves - drops out of the sum
/// without having to be known. The half point of slack covers the view flooring
/// the space it is given: a window a rounding error short of a whole column
/// would otherwise come out one column narrower than asked for.
fn fitted_inner_size(
    inner: egui::Vec2,
    view: (usize, usize),
    target: (usize, usize),
    cell: egui::Vec2,
) -> egui::Vec2 {
    egui::Vec2::new(
        inner.x + (target.0 as f32 - view.0 as f32) * cell.x + 0.5,
        inner.y + (target.1 as f32 - view.1 as f32) * cell.y + 0.5,
    )
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        let theme = self.theme();
        let mut requests: Vec<UiRequest> = Vec::new();
        // What the terminal measured this frame, filled in once it has drawn.
        let mut fit: Option<(usize, usize, egui::Vec2)> = None;

        self.track_window_geometry(ctx);

        for tab in &mut self.tabs {
            tab.pump(&mut self.plugins);
        }

        // Plugins may have asked for things while transforming output.
        for hook in self.plugins.take_requests() {
            match hook {
                crate::plugins::api::Hook::SendText(text) => {
                    requests.push(UiRequest::SendLines(vec![text]))
                }
                crate::plugins::api::Hook::SetStatus(text) => self.set_status(text),
                crate::plugins::api::Hook::RegisterCommand(_) => {}
            }
        }

        self.poll_updates();
        self.expire_status(ctx);
        self.handle_shortcuts(ctx);

        // A close asked for by the window manager - Alt+F4, or the taskbar -
        // arrives as a flag rather than an event, and has to be caught before
        // anything else gets a chance to draw over the question.
        if ctx.input(|i| i.viewport().close_requested()) && self.should_confirm_close() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_close = true;
        }

        let mut window_action = None;
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            window_action = self.menu_bar(ui, &theme.window_buttons);
        });
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| self.tab_strip(ui));

        if let Some(action) = window_action {
            // Close is the one action that may be refused; the rest are
            // immediate.
            if action == WindowAction::Close && self.should_confirm_close() {
                self.confirm_close = true;
            } else {
                chrome::apply(ctx, action);
            }
        }

        if let Some(status) = self.status.clone() {
            egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(status);
                    if ui.small_button(tr("dismiss")).clicked() {
                        self.status = None;
                        self.status_at = None;
                    }
                });
            });
        }

        if self.panels.show_macros {
            egui::SidePanel::right("macros")
                .default_width(280.0)
                .show(ctx, |ui| {
                    // One scroll area around the whole panel rather than one
                    // per section: the macro list, the details under it and the
                    // IRIS utilities below that are one column of content, and
                    // giving each a share of the height leaves every one of
                    // them too short on a small window.
                    egui::ScrollArea::vertical()
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if let Some(request) =
                                panels::macros_panel(ui, &mut self.macro_groups, &mut self.panels)
                            {
                                requests.push(request);
                            }
                            ui.separator();
                            if let Some(request) = panels::natives_panel(ui, &mut self.panels) {
                                requests.push(request);
                            }
                        });
                });
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(theme.background))
            .show(ctx, |ui| {
                if self.tabs.is_empty() {
                    let mut open = false;
                    ui.centered_and_justified(|ui| {
                        if ui.button(tr("Open a session (Ctrl+T)")).clicked() {
                            open = true;
                        }
                    });
                    if open {
                        self.open_new_tab();
                    }
                    return;
                }

                let active = self.active.min(self.tabs.len() - 1);
                let opts = self.render_opts();

                // Autologon status belongs next to the terminal it applies to.
                if let Some(note) = self.tabs[active].autologon.status_note() {
                    ui.label(note);
                }
                if let Some(error) = self.tabs[active].error.clone() {
                    ui.colored_label(theme.ansi[9], error);
                }
                if self.tabs[active].ended && self.tabs[active].error.is_none() {
                    ui.horizontal(|ui| {
                        ui.label(tr("Session ended."));
                        if ui.button(tr("Reconnect")).clicked() {
                            self.tabs[active].start();
                        }
                    });
                }

                let has_selection = self.tabs[active]
                    .view
                    .selection
                    .map(|s| !s.is_empty())
                    .unwrap_or(false);

                let uid = self.tabs[active].uid;
                let take_focus = self.focused_tab != Some(uid);
                self.focused_tab = Some(uid);

                let result = {
                    let tab = &mut self.tabs[active];
                    terminal_view::show(
                        ui,
                        &tab.grid,
                        &mut tab.view,
                        &theme,
                        &opts,
                        uid,
                        take_focus,
                    )
                };

                // Every tab, not just the visible one: a background session
                // left at the old width would keep truncating its output at
                // that width until it was next looked at.
                self.terminal_size = (result.cols as u16, result.rows as u16);
                self.view_size = (result.view_cols, result.view_rows);
                fit = Some((result.view_cols, result.view_rows, result.cell));
                for tab in &mut self.tabs {
                    tab.resize(result.cols, result.rows);
                }

                // Right-click menu actions reuse the same paths as the
                // keyboard shortcuts and the Export dialog.
                // Copy-on-select: a finished drag goes straight to the
                // clipboard, without waiting for Ctrl+C.
                if result.copy_selection {
                    let tab = &self.tabs[active];
                    if let Some(text) = tab.view.selected_text(&tab.grid) {
                        ctx.copy_text(text);
                    }
                }
                if let Some(columns) = result.cursor_move {
                    self.move_cursor(active, columns);
                }

                if let Some(action) = result.context_action {
                    use terminal_view::ContextAction;
                    match action {
                        ContextAction::CopySelection => {
                            let tab = &self.tabs[active];
                            if let Some(text) = tab.view.selected_text(&tab.grid) {
                                ctx.copy_text(text);
                            }
                        }
                        ContextAction::Paste => {
                            // The clipboard is only readable through egui's
                            // paste event, so ask for one rather than reaching
                            // for the OS clipboard behind egui's back.
                            ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                        }
                        ContextAction::SelectAll => {
                            let tab = &mut self.tabs[active];
                            let grid = &tab.grid;
                            tab.view.select_all(grid);
                        }
                        ContextAction::ClearSelection => {
                            self.tabs[active].view.clear_selection();
                        }
                        ContextAction::ExportScreen => self.panels.show_export = true,
                        ContextAction::Analyze(scope) => self.analyze_with_claude(scope),
                        ContextAction::ClearTerminal => self.clear_active_terminal(),
                    }
                }

                // Only the focused terminal consumes keystrokes.
                if result.response.clicked() {
                    result.response.request_focus();
                }
                if result.response.has_focus() {
                    let events = ui.input(|i| i.events.clone());
                    let (insert_down, delete_down) = input::chord_keys_down();
                    let line = lineedit::current(&self.tabs[active].grid);
                    let input_ctx = input::InputContext {
                        has_selection,
                        line,
                        can_recall: !self.history.is_empty(),
                        selected_span: line
                            .and_then(|line| selection_in_line(&self.tabs[active], line)),
                        insert_down,
                        delete_down,
                    };
                    let mut action = input::translate(&events, &input_ctx);

                    if action.select_line {
                        self.select_typed_line(active);
                    }
                    if let Some(motion) = action.extend_selection {
                        self.extend_selection(active, motion);
                    }
                    if let Some(motion) = action.move_cursor {
                        self.move_by(active, motion);
                    }
                    if action.collapse_selection {
                        self.tabs[active].view.clear_selection();
                    }
                    // A selection inside the command line behaves the way one
                    // in a text field does: an erase key takes it out, and so
                    // does typing or pasting over it, before the new text goes
                    // in behind it.
                    let replaced = input_ctx.selected_span.is_some() && !action.select_line && {
                        !action.text.is_empty() || action.paste.is_some()
                    };
                    if action.erase_selection || replaced {
                        self.erase_selection(active);
                    }

                    // Recorded before the Enter reaches IRIS, while the line is
                    // still on screen to be read.
                    if let Some(unechoed) = action.submitted.as_deref() {
                        // Submitting ends the selection with the line it was
                        // on, rather than leaving it highlighted in the
                        // scrollback.
                        self.tabs[active].view.clear_selection();
                        self.record_command(active, unechoed);
                    }
                    if let Some(direction) = action.recall {
                        self.recall(active, direction);
                    }
                    // Typing abandons wherever the recall had walked to.
                    if !action.text.is_empty() {
                        self.tabs[active].recall_index = None;
                    }

                    if action.copy {
                        let tab = &self.tabs[active];
                        if let Some(text) = tab.view.selected_text(&tab.grid) {
                            ctx.copy_text(text);
                        }
                    }
                    if let Some(text) = action.paste.take() {
                        // A paste rewrites the line, so wherever the recall had
                        // walked to is no longer where the line came from.
                        self.tabs[active].recall_index = None;
                        let text = input::sanitize_paste(&text);
                        let encoded = self.tabs[active].profile.encoding.encode(&text);
                        let encoded = self.plugins.on_input(&encoded);
                        self.tabs[active].send(&encoded);
                    }
                    // IRIS never reports its insert/replace state, so the
                    // keystroke is the only signal there is. Display only - see
                    // `Grid::insert_mode`.
                    if action.toggle_insert {
                        let grid = &mut self.tabs[active].grid;
                        grid.insert_mode = !grid.insert_mode;
                    }
                    if !action.is_empty() {
                        // Typing always returns the view to the live output.
                        self.tabs[active].view.scroll_to_bottom();

                        let mut wire = self.tabs[active].profile.encoding.encode(&action.text);
                        wire.extend_from_slice(&action.bytes);
                        let wire = self.plugins.on_input(&wire);
                        self.tabs[active].send(&wire);
                    }
                }
            });

        self.rename_tab_dialog(ctx);
        self.close_confirm_dialog(ctx);
        if panels::update_dialog(ctx, &mut self.updates) {
            self.apply_update(ctx);
        }

        // Last, and in a foreground layer: the panels and the terminal reach
        // the window edge, and the terminal senses drags of its own.
        if !self.settings.native_decorations {
            chrome::resize_grips(ctx, "nit-main");
        }

        if let Some(request) =
            panels::macro_editor_dialog(ctx, &mut self.macro_groups, &mut self.panels)
        {
            requests.push(request);
        }
        if let Some(request) = panels::pending_macro_dialog(ctx, &mut self.panels) {
            requests.push(request);
        }
        if let Some(request) = panels::export_dialog(ctx, &mut self.panels) {
            requests.push(request);
        }
        if let Some(request) = panels::settings_dialog(
            ctx,
            &mut self.settings,
            &self.themes,
            &mut self.panels,
            &self.instances,
            &theme.window_buttons,
        ) {
            requests.push(request);
        }
        // After the settings window, which is where it is opened from, and
        // before the requests are carried out, so a colour changed this frame is
        // on screen in the next one.
        let active_theme = self.settings.theme.clone();
        let native_decorations = self.settings.native_decorations;
        let theme_actions = theme_manager::theme_manager(
            ctx,
            &mut self.panels.themes,
            &mut self.themes,
            &active_theme,
            &theme.window_buttons,
            native_decorations,
        );
        // An edit to the theme in use has to show at once, which is the whole
        // point of editing it with the terminal behind the window.
        if !theme_actions.is_empty() || self.panels.themes.open {
            App::apply_style(ctx, &self.theme(), &self.settings);
        }
        for action in theme_actions {
            self.apply_theme_action(ctx, action);
        }

        for request in requests {
            self.handle_request(ctx, request);
        }

        if let Some((view_cols, view_rows, cell)) = fit {
            self.fit_window(ctx, view_cols, view_rows, cell);
        }

        // A live session can produce output at any moment, so keep animating
        // while any tab is connected. A blinking cursor needs the same, because
        // an idle prompt gives the frame no other reason to be redrawn.
        if self.settings.cursor_blink || self.tabs.iter().any(|t| t.session.is_some()) {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        for tab in &mut self.tabs {
            if let Some(session) = tab.session.as_ref() {
                session.request_halt();
            }
            if let Some(log) = tab.log.as_mut() {
                let _ = log.write_note("application closed");
                let _ = log.flush();
            }
        }
        self.persist_window_geometry();
    }
}

/// The profile for whatever the launcher's tray menu is set to open.
///
/// Named for the target rather than built from it directly, because the tray's
/// "Este Servidor" entry names an instance and the other entries name servers;
/// [`crate::config::servers::preferred_target`] is what tells the two apart.
fn preferred_profile(servers: &crate::config::ServerList, instances: &[String]) -> Option<Profile> {
    use crate::config::servers::{Server, Target};
    let base = Profile::default();
    match crate::config::servers::preferred_target(servers, instances)? {
        Target::Local { instance } => Some(Profile::for_server(
            &Server::for_instance(&instance),
            instances,
            &base,
        )),
        Target::Telnet { .. } => {
            // The entry itself carries the address, the port and the name the
            // tab should show, so the server is looked up rather than rebuilt
            // from the bare target.
            let server = servers.preferred()?;
            Some(Profile::for_server(server, instances, &base))
        }
    }
}

/// Convenience for callers that want the discovered instances without an app.
pub fn discover_instances() -> Vec<String> {
    let l = launcher();
    crate::pty::launcher::instances(l.as_ref())
        .into_iter()
        .map(|i| i.name)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The window is corrected from a measurement, so the check that matters is
    /// that measuring the corrected window gives the geometry that was asked
    /// for - in one step, at any font size and whatever the chrome around the
    /// terminal happens to take up.
    #[test]
    fn one_correction_lands_on_the_geometry_that_was_asked_for() {
        let target = (
            config::DEFAULT_TERMINAL_COLS as usize,
            config::DEFAULT_TERMINAL_ROWS as usize,
        );

        for cell in [
            egui::Vec2::new(8.0, 16.0),
            egui::Vec2::new(9.0, 21.0),
            egui::Vec2::new(13.0, 30.0),
        ] {
            for chrome in [egui::Vec2::new(24.0, 78.0), egui::Vec2::new(12.0, 61.0)] {
                for inner in [
                    egui::Vec2::new(1000.0, 640.0),
                    egui::Vec2::new(400.0, 240.0),
                    egui::Vec2::new(1913.0, 1027.0),
                ] {
                    // What `terminal_view::show` would measure in that window.
                    let measure = |inner: egui::Vec2| {
                        (
                            (((inner.x - chrome.x) / cell.x).floor() as usize).max(1),
                            (((inner.y - chrome.y) / cell.y).floor() as usize).max(1),
                        )
                    };

                    let fitted = fitted_inner_size(inner, measure(inner), target, cell);
                    assert_eq!(
                        measure(fitted),
                        target,
                        "cell {cell:?}, chrome {chrome:?}, from {inner:?}"
                    );
                }
            }
        }
    }

    /// A window already at the right size is left exactly where it is, so the
    /// fit cannot drift the window it was meant to leave alone.
    #[test]
    fn a_window_that_already_fits_is_not_moved() {
        let inner = egui::Vec2::new(1000.0, 640.0);
        let size = fitted_inner_size(inner, (100, 30), (100, 30), egui::Vec2::new(8.0, 16.0));
        assert!((size.x - inner.x).abs() <= 0.5 && (size.y - inner.y).abs() <= 0.5);
    }
}
