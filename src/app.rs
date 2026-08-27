//! The application shell: owns the tabs, drains their PTYs each frame, and
//! draws the chrome around the terminal view.

use egui::{Context, Key};

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
use crate::ui::input;
use crate::ui::panels::{self, PanelState, PendingMacro, UiRequest};
use crate::ui::terminal_view::{self, ViewState};

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
                if self.profile.name.is_empty() {
                    self.profile.instance.clone()
                } else {
                    self.profile.name.clone()
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
    show_new_tab: bool,
    new_tab_profile: Profile,
    status: Option<String>,
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
            show_new_tab: false,
            status: None,
        };

        cc.egui_ctx.set_visuals(app.theme().visuals());

        if app.settings.open_on_start {
            if let Some(profile) = app.settings.startup_profile().cloned() {
                app.open_tab(profile);
            } else {
                app.show_new_tab = true;
            }
        }
        app
    }

    pub fn theme(&self) -> Theme {
        self.themes
            .iter()
            .find(|t| t.name == self.settings.theme)
            .cloned()
            .unwrap_or_default()
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

    fn handle_shortcuts(&mut self, ctx: &Context) {
        let (new_tab, close_tab, next_tab, jump, zoom_in, zoom_out) = ctx.input(|i| {
            let cmd = |key| i.modifiers.command && i.key_pressed(key);
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
                if i.modifiers.command && i.key_pressed(*key) {
                    jump = Some(n);
                }
            }
            (
                cmd(Key::T),
                cmd(Key::W),
                cmd(Key::Tab),
                jump,
                cmd(Key::Plus) || cmd(Key::Equals),
                cmd(Key::Minus),
            )
        });

        if new_tab {
            self.show_new_tab = true;
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
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .button("+")
                .on_hover_text("New session (Ctrl+T)")
                .clicked()
            {
                self.show_new_tab = true;
            }
            ui.separator();
            ui.toggle_value(&mut self.panels.show_macros, "Macros");
            ui.toggle_value(&mut self.panels.show_globals, "Globals");
            if ui.button("Export").clicked() {
                self.panels.show_export = true;
            }
            if ui.button("Settings").clicked() {
                self.panels.show_settings = true;
            }
            ui.separator();
            if let Some(tab) = self.active_tab() {
                ui.weak(format!(
                    "{}  {}x{}",
                    tab.profile.instance, tab.grid.cols, tab.grid.rows
                ));
            }
        });
    }

    fn tab_strip(&mut self, ui: &mut egui::Ui) {
        let mut to_close = None;
        egui::ScrollArea::horizontal().show(ui, |ui| {
            ui.horizontal(|ui| {
                for index in 0..self.tabs.len() {
                    let selected = index == self.active;
                    let mut label = self.tabs[index].title();
                    if self.tabs[index].ended {
                        label.push_str(" (ended)");
                    }
                    if ui.selectable_label(selected, label).clicked() {
                        self.active = index;
                    }
                    if ui.small_button("x").clicked() {
                        to_close = Some(index);
                    }
                    ui.separator();
                }
            });
        });
        if let Some(index) = to_close {
            self.close_tab(index);
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

            UiRequest::SettingsChanged => match self.settings.save() {
                Ok(()) => ctx.set_visuals(self.theme().visuals()),
                Err(e) => self.set_status(format!("Could not save settings: {e:#}")),
            },
            UiRequest::ReloadMacros => {
                let report = load_macros(&self.settings);
                self.macro_groups = report.groups;
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

    fn new_tab_dialog(&mut self, ctx: &Context) {
        if !self.show_new_tab {
            return;
        }
        let mut open = true;
        let mut launch = false;

        egui::Window::new("New session")
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                if !self.settings.profiles.is_empty() {
                    ui.label("Profile");
                    egui::ComboBox::from_id_source("profile-picker")
                        .selected_text(self.new_tab_profile.name.clone())
                        .show_ui(ui, |ui| {
                            for profile in &self.settings.profiles {
                                if ui
                                    .selectable_label(
                                        profile.name == self.new_tab_profile.name,
                                        &profile.name,
                                    )
                                    .clicked()
                                {
                                    self.new_tab_profile = profile.clone();
                                }
                            }
                        });
                    ui.separator();
                }

                ui.label("Instance");
                if self.instances.is_empty() {
                    ui.text_edit_singleline(&mut self.new_tab_profile.instance);
                    ui.small("No IRIS instances discovered - type the name.");
                } else {
                    egui::ComboBox::from_id_source("instance-picker")
                        .selected_text(self.new_tab_profile.instance.clone())
                        .show_ui(ui, |ui| {
                            for name in &self.instances {
                                if ui
                                    .selectable_label(*name == self.new_tab_profile.instance, name)
                                    .clicked()
                                {
                                    self.new_tab_profile.instance = name.clone();
                                }
                            }
                        });
                }

                ui.label("Namespace");
                ui.text_edit_singleline(&mut self.new_tab_profile.namespace);

                ui.separator();
                ui.horizontal(|ui| {
                    let ready = !self.new_tab_profile.instance.is_empty();
                    if ui
                        .add_enabled(ready, egui::Button::new("Connect"))
                        .clicked()
                    {
                        launch = true;
                    }
                    if ui.button("Cancel").clicked() {
                        self.show_new_tab = false;
                    }
                });
            });

        if launch {
            self.open_tab(self.new_tab_profile.clone());
            self.show_new_tab = false;
        }
        if !open {
            self.show_new_tab = false;
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

        egui::TopBottomPanel::top("menu").show(ctx, |ui| self.menu_bar(ui));
        egui::TopBottomPanel::top("tabs").show(ctx, |ui| self.tab_strip(ui));

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
                    ui.centered_and_justified(|ui| {
                        if ui.button("Open a session (Ctrl+T)").clicked() {
                            self.show_new_tab = true;
                        }
                    });
                    return;
                }

                let active = self.active.min(self.tabs.len() - 1);
                let font_size = self.settings.font_size;

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
                        font_size,
                        uid,
                        take_focus,
                    )
                };

                // Every tab, not just the visible one: a background session
                // left at the old width would keep truncating its output at
                // that width until it was next looked at.
                self.terminal_size = (result.cols as u16, result.rows as u16);
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

        self.new_tab_dialog(ctx);

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
        // while any tab is connected.
        if self.tabs.iter().any(|t| t.session.is_some()) {
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
