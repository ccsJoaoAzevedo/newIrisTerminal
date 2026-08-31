//! The application shell: owns the tabs, drains their PTYs each frame, and
//! draws the chrome around the terminal view.

use egui::{Context, Key, Modifiers};

use crate::config::{self, ensure_config_tree, load_themes, LogMode, Profile, Settings, Theme};
use crate::features::autologon::{Autologon, State as AutoState};
use crate::features::export::{self, Range};
use crate::features::global_browser;
use crate::features::logging::{self, SessionLog};
use crate::features::macros::{self, MacroGroup};
use crate::plugins::PluginHost;
use crate::pty::launcher::launcher;
use crate::pty::PtySession;
use crate::term::Grid;
use crate::ui::chrome::{self, WindowAction};
use crate::ui::panels::{self, PanelState, PendingMacro, UiRequest};
use crate::ui::terminal_view::{self, RenderOpts, ViewState};
use crate::ui::{fonts, input, shortcut};

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

/// An in-flight structured query against the session.
///
/// The output still scrolls through the terminal — nothing is hidden from the
/// user — but a copy is accumulated here so it can be parsed into a grid.
pub struct Capture {
    pub buffer: String,
    pub started: std::time::Instant,
    /// Substrings that mean the walk finished, one way or the other.
    pub terminators: Vec<String>,
}

impl Capture {
    pub fn new(terminators: Vec<String>) -> Self {
        Capture {
            buffer: String::new(),
            started: std::time::Instant::now(),
            terminators,
        }
    }

    pub fn is_done(&self) -> bool {
        self.terminators.iter().any(|t| self.buffer.contains(t))
    }

    /// A query that never terminates must not leave the panel spinning
    /// forever — a global can be slow, so this is generous.
    pub fn is_expired(&self) -> bool {
        self.started.elapsed() > std::time::Duration::from_secs(60)
    }
}

/// One open session and everything that hangs off it.
pub struct Tab {
    /// Stable for this tab's lifetime and unique across tabs, so the terminal
    /// widget keeps one identity even as tabs are opened, closed and reordered.
    pub uid: u64,
    pub profile: Profile,
    pub session: Option<PtySession>,
    pub grid: Grid,
    pub parser: vte::Parser,
    pub view: ViewState,
    pub autologon: Autologon,
    pub log: Option<SessionLog>,
    /// Set while a structured query is running.
    pub capture: Option<Capture>,
    /// User-set name; falls back to the OSC title, then the profile name.
    pub custom_title: Option<String>,
    /// When a clear-screen was asked of IRIS, so a purge that never arrives
    /// can be called off. See [`Tab::request_clear`].
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
            capture: None,
            custom_title: None,
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
        let launcher = launcher();

        match PtySession::spawn(launcher.as_ref(), &self.profile.launch_spec(), cols, rows) {
            Ok(session) => {
                self.session = Some(session);
                let instance = self.profile.instance.clone();
                self.note(&format!("session started ({instance})"));
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

            // Feed any in-flight structured query. This runs before the
            // password check so a capture never picks up a credential prompt.
            if let Some(capture) = self.capture.as_mut() {
                capture.buffer.push_str(&String::from_utf8_lossy(&decoded));
            }

            let still_on_password = self.autologon.state() == AutoState::WaitPassword;
            let lines = self.grid.all_text();
            let settled = self.grid.scrollback.len();
            if let Some(log) = self.log.as_mut() {
                if !still_on_password {
                    log.unmute();
                }
                let _ = log.write_settled(&lines, settled);
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

    /// Asks IRIS to clear the screen, the way typing `W #` would.
    ///
    /// The terminal cannot do this on its own: IRIS tracks the cursor itself
    /// and positions absolutely, so a grid cleared locally leaves the next
    /// prompt painted back down at the row IRIS still believes it is on. The
    /// grid is told to drop its history when that clear arrives, so the gesture
    /// ends with an empty terminal rather than one holding the echoed command.
    pub fn request_clear(&mut self) {
        if self.session.is_none() {
            return;
        }
        self.grid.purge_history_on_next_clear();
        self.clear_asked = Some(std::time::Instant::now());
        self.send_lines(&["W #".to_string()]);
    }

    /// Sends whole lines, each terminated the way Enter would terminate it.
    pub fn send_lines(&self, lines: &[String]) {
        for line in lines {
            self.send_text(line);
            self.send(b"\r");
        }
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
    pub macro_groups: Vec<MacroGroup>,
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
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let _ = ensure_config_tree();
        let settings = Settings::load();
        let themes = load_themes();

        // Housekeeping that would otherwise never happen.
        let _ = logging::prune(&settings.log_dir, settings.log_retention_days);

        let l = launcher();
        let instances = crate::pty::launcher::instances(l.as_ref())
            .into_iter()
            .map(|i| i.name)
            .collect::<Vec<_>>();

        let plugins = if settings.enable_plugins {
            PluginHost::load_from(&config::plugins_dir())
        } else {
            PluginHost::disabled()
        };

        let mut app = App {
            new_tab_profile: settings
                .startup_profile()
                .cloned()
                .unwrap_or_else(|| Profile {
                    instance: instances.first().cloned().unwrap_or_default(),
                    ..Profile::default()
                }),
            macro_groups: load_macros(&settings).groups,
            settings,
            themes,
            tabs: Vec::new(),
            active: 0,
            instances,
            plugins,
            panels: PanelState::default(),
            focused_tab: None,
            terminal_size: (FALLBACK_COLS, FALLBACK_ROWS),
            view_size: (FALLBACK_COLS as usize, FALLBACK_ROWS as usize),
            status: None,
            font_family: String::new(),
            font_request: String::new(),
            renaming: None,
            confirm_close: false,
            close_confirmed: false,
        };

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
            self.set_status(format!(
                "Font {wanted:?} is not installed; using the built-in monospace."
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
    /// clear-screen now files the screen into the scrollback, and this is the
    /// gesture for when the history really is meant to go. Also on the
    /// right-click menu, because a shortcut nobody can see is a shortcut nobody
    /// uses.
    ///
    /// The clear is asked of IRIS rather than done locally, so its idea of the
    /// cursor is cleared along with the screen. Wiping the grid here instead
    /// was the bug: IRIS positions the cursor absolutely, so it went on
    /// painting from the row it had reached and the next prompt appeared back
    /// down the screen with the cleared rows blank above it. Only a session
    /// that cannot be asked - one that has ended - is cleared locally.
    fn clear_active_terminal(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        tab.view.clear_selection();
        tab.view.scroll_to_bottom();
        if tab.session.is_some() {
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
    fn menu_bar(&mut self, ui: &mut egui::Ui) -> Option<WindowAction> {
        let mut action = None;
        // Opening a tab has to happen after the closure: it borrows `self`
        // mutably, and the bar is already holding it.
        let mut open = false;
        let mut pick: Option<Profile> = None;
        ui.horizontal(|ui| {
            let instance = if self.new_tab_profile.instance.is_empty() {
                "no instance configured".to_string()
            } else {
                self.new_tab_profile.instance.clone()
            };
            let new_tab = ui.button("+").on_hover_text(format!(
                "New session on {instance} (Ctrl+T).\nRight-click to connect somewhere else."
            ));
            if new_tab.clicked() {
                open = true;
            }
            // The escape hatch that replaces the dialog: everything it used to
            // offer - the profiles and the instances found on this machine -
            // one click away instead of in front of every new session.
            new_tab.context_menu(|ui| {
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
                    if ui.button(name).clicked() {
                        pick = Some(Profile {
                            instance: name.clone(),
                            ..self.new_tab_profile.clone()
                        });
                        ui.close_menu();
                    }
                }
                if self.settings.profiles.is_empty() && self.instances.is_empty() {
                    ui.weak("No profiles or instances found.");
                }
            });
            ui.separator();
            ui.toggle_value(&mut self.panels.show_macros, "Macros");
            ui.toggle_value(&mut self.panels.show_globals, "Globals");
            // Toggles, not plain buttons: clicking the button that opened a
            // dialog is how everyone expects to close it again, and it shows
            // which dialogs are open the way Macros/Globals already do.
            ui.toggle_value(&mut self.panels.show_export, "Export");
            ui.toggle_value(&mut self.panels.show_settings, "Settings");
            ui.separator();
            if let Some(tab) = self.active_tab() {
                let (cols, rows) = self.view_size;
                ui.weak(format!("{}  {cols}x{rows}", tab.profile.instance));
            }

            // Last, so the leftover space it claims for dragging is whatever
            // the items above did not take.
            if !self.settings.native_decorations {
                action = chrome::title_bar_controls(ui);
            }
        });

        if let Some(profile) = pick {
            self.new_tab_profile = profile;
            open = true;
        }
        if open {
            self.open_new_tab();
        }
        action
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

        egui::Window::new("Close newIrisTerminal?")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(match live {
                    1 => "1 session is still connected.".to_string(),
                    n => format!("{n} sessions are still connected."),
                });
                ui.small("Closing sends HALT to each of them.");
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Close anyway").clicked() {
                        close_anyway = true;
                    }
                    if ui.button("Keep working").clicked() {
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
                        .on_hover_text("Double-click to rename.");
                    if response.clicked() {
                        self.active = index;
                    }
                    if response.double_clicked() {
                        to_rename = Some(index);
                    }
                    response.context_menu(|ui| {
                        if ui.button("Rename...").clicked() {
                            to_rename = Some(index);
                            ui.close_menu();
                        }
                        if ui.button("Close").clicked() {
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

        egui::Window::new("Rename tab")
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

                ui.small("Empty goes back to the automatic name.");
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Rename").clicked() {
                        commit = true;
                    }
                    if ui.button("Automatic").clicked() {
                        clear = true;
                    }
                    if ui.button("Cancel").clicked() {
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
            UiRequest::RunNative(native, arg) => {
                let invocation = native.build(&arg);
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
                    self.set_status("Copied to the clipboard.");
                }
            }

            UiRequest::SettingsChanged => {
                // The style is reapplied either way: a failed write still has
                // to be reflected on screen, or the UI would disagree with the
                // settings the user just changed.
                App::apply_style(ctx, &self.theme(), &self.settings);
                self.apply_font(ctx);
                if let Err(e) = self.settings.save() {
                    self.set_status(format!("Could not save settings: {e:#}"));
                }
            }
            UiRequest::ReloadMacros => {
                let report = load_macros(&self.settings);
                self.macro_groups = report.groups;
                // Selection and draft are indices into the list that was just
                // replaced, so they would now address a different macro.
                self.panels.forget_macro_selection();
                let count: usize = self.macro_groups.iter().map(|g| g.macros.len()).sum();
                if report.problems.is_empty() {
                    self.set_status(format!("Reloaded {count} macros."));
                } else {
                    self.set_status(format!(
                        "Reloaded {count} macros. {}",
                        report.problems.join(" ")
                    ));
                }
            }
            UiRequest::RunGlobalQuery => self.run_global_query(),
            UiRequest::CopyGlobalPage => {
                let text = panels::page_as_text(&self.panels.globals);
                ctx.copy_text(text);
                self.set_status("Copied the visible rows.");
            }
            UiRequest::OpenFolder(path) => {
                if let Err(e) = config::open_in_file_manager(&path) {
                    self.set_status(format!("Could not open {}: {e:#}", path.display()));
                }
            }
            UiRequest::SavePersonalMacros => {
                let path = config::personal_macros_path();
                let xml = macros::to_xml(&self.macro_groups);
                match std::fs::write(&path, xml) {
                    Ok(()) => {
                        self.set_status(format!("Saved personal macros to {}", path.display()))
                    }
                    Err(e) => self.set_status(format!("Could not save macros: {e:#}")),
                }
            }
        }
    }

    /// Sends the global-browser query and starts capturing its output.
    fn run_global_query(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            self.panels.globals.message = Some("No active session.".into());
            return;
        };
        if tab.session.is_none() {
            self.panels.globals.message = Some("That session has ended.".into());
            return;
        }
        if tab.capture.is_some() {
            self.panels.globals.message = Some("A query is already running.".into());
            return;
        }

        let script = global_browser::build_script(&self.panels.globals.query);
        // Both terminators, so an IRIS error ends the wait as promptly as a
        // successful walk does.
        tab.capture = Some(Capture::new(vec!["@E@".into(), "@X@".into()]));
        tab.send_lines(&[script]);

        self.panels.globals.running = true;
        self.panels.globals.message = None;
        self.panels.globals.page = global_browser::Page::default();
        self.panels.globals.page_index = 0;
    }

    /// Turns a finished capture into a page. Called once per frame.
    fn collect_global_query(&mut self) {
        let Some(tab) = self.tabs.get_mut(self.active) else {
            return;
        };
        let Some(capture) = tab.capture.as_ref() else {
            return;
        };

        if capture.is_done() {
            let text = std::mem::take(&mut tab.capture).unwrap().buffer;
            let page = global_browser::parse(&text, self.panels.globals.query.limit);
            let count = page.nodes.len();
            self.panels.globals.page = page;
            self.panels.globals.running = false;
            self.panels.globals.page_index = 0;
            self.panels.globals.message = Some(format!("{count} nodes."));
        } else if capture.is_expired() {
            tab.capture = None;
            self.panels.globals.running = false;
            self.panels.globals.message =
                Some("Timed out waiting for IRIS. The session is still usable.".into());
        }
    }

    fn send_lines_to_active(&mut self, lines: &[String]) {
        let Some(tab) = self.tabs.get(self.active) else {
            self.set_status("No active session.");
            return;
        };
        if tab.session.is_none() {
            self.set_status("That session has ended.");
            return;
        }
        tab.send_lines(lines);
    }

    fn export(&mut self, range: Range, format: export::Format) {
        let Some(tab) = self.active_tab() else {
            self.set_status("No active session to export.");
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
            Ok(()) => self.set_status(format!("Exported to {}", path.display())),
            Err(e) => self.set_status(format!("Export failed: {e:#}")),
        }
    }
}

/// Loads the organisation file (if configured) and the personal one, and
/// reports anything that went wrong so a missing share is visible rather than
/// silently halving the macro list.
fn load_macros(settings: &Settings) -> macros::LoadReport {
    let personal = config::personal_macros_path();
    if !personal.exists() {
        // Ship the sample on first run so the format is self-documenting.
        let _ = std::fs::write(&personal, macros::SAMPLE);
    }
    macros::load_all(settings.org_macros(), &personal)
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        let theme = self.theme();
        let mut requests: Vec<UiRequest> = Vec::new();

        for tab in &mut self.tabs {
            tab.pump(&mut self.plugins);
        }

        // Plugins may have asked for things while transforming output.
        for hook in self.plugins.take_requests() {
            match hook {
                crate::plugins::api::Hook::SendText(text) => {
                    requests.push(UiRequest::SendLines(vec![text]))
                }
                crate::plugins::api::Hook::SetStatus(text) => self.status = Some(text),
                crate::plugins::api::Hook::RegisterCommand(_) => {}
            }
        }

        self.collect_global_query();
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
            window_action = self.menu_bar(ui);
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
                    if ui.small_button("dismiss").clicked() {
                        self.status = None;
                    }
                });
            });
        }

        if self.panels.show_macros {
            egui::SidePanel::right("macros")
                .default_width(280.0)
                .show(ctx, |ui| {
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
        }

        if self.panels.show_globals {
            egui::TopBottomPanel::bottom("globals")
                .resizable(true)
                .default_height(280.0)
                .show(ctx, |ui| {
                    if let Some(request) =
                        panels::global_browser_panel(ui, &mut self.panels.globals, &theme)
                    {
                        requests.push(request);
                    }
                });
        }

        egui::CentralPanel::default()
            .frame(egui::Frame::none().fill(theme.background))
            .show(ctx, |ui| {
                if self.tabs.is_empty() {
                    let mut open = false;
                    ui.centered_and_justified(|ui| {
                        if ui.button("Open a session (Ctrl+T)").clicked() {
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
                        ui.label("Session ended.");
                        if ui.button("Reconnect").clicked() {
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
                for tab in &mut self.tabs {
                    tab.resize(result.cols, result.rows);
                }

                // Right-click menu actions reuse the same paths as the
                // keyboard shortcuts and the Export dialog.
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
                        ContextAction::ClearTerminal => self.clear_active_terminal(),
                    }
                }

                // Only the focused terminal consumes keystrokes.
                if result.response.clicked() {
                    result.response.request_focus();
                }
                if result.response.has_focus() {
                    let events = ui.input(|i| i.events.clone());
                    let mut action = input::translate(&events, has_selection);

                    if action.copy {
                        let tab = &self.tabs[active];
                        if let Some(text) = tab.view.selected_text(&tab.grid) {
                            ctx.copy_text(text);
                        }
                    }
                    if let Some(text) = action.paste.take() {
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

        // Last, and in a foreground layer: the panels and the terminal reach
        // the window edge, and the terminal senses drags of its own.
        if !self.settings.native_decorations {
            chrome::resize_grips(ctx);
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
        ) {
            requests.push(request);
        }

        for request in requests {
            self.handle_request(ctx, request);
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
