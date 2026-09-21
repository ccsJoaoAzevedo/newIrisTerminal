//! The application shell: owns the tabs, drains their PTYs each frame, and
//! draws the chrome around the terminal view.

use egui::{Context, Key, Modifiers};

use std::sync::Mutex;
use std::time::{Duration, Instant};

use crate::config::{self, ensure_config_tree, load_themes, LogMode, Profile, Settings, Theme};
use crate::features::analyze;
use crate::features::autologon::{Autologon, State as AutoState};
use crate::features::doc_lookup::{self, DocLookup, Lookup};
use crate::features::export::{self, Range};
use crate::features::history::{self, History};
use crate::features::logging::{self, SessionLog};
use crate::features::macros::{self, MacroGroup};
use crate::features::update;
use crate::i18n::{tr, tr1, tr2};
use crate::plugins::PluginHost;
use crate::pty::launcher::launcher;
use crate::pty::Session;
use crate::term::{lineedit, Grid, Motion};
use crate::ui::chrome::{self, WindowAction};
use crate::ui::macro_manager;
use crate::ui::panels::{self, PanelState, PendingMacro, UiRequest};
use crate::ui::terminal_view::{self, RenderOpts, Selection, ViewState};
use crate::ui::theme_manager::{self, ThemeAction};
use crate::ui::{fonts, icons, input, shortcut};

// The shell is split by what each part is responsible for, and every one of
// these holds part of `impl App`. Behaviour lives beside the state it acts on;
// the state itself, and the frame loop that drives all of it, stay here.
mod edit;
mod frame;
mod layout;
mod menu;
mod pane;
mod tab;
mod tabs;
mod updates;
mod window;

// Back into one scope, so that every part of the shell sees the same
// names it did when all of this was one file - including through the
// `use super::*` each of those modules starts with.
// The shell's own API, unchanged by the split: these were reachable as
// `crate::app::*` when this was one file, and the rest of the crate still
// names them that way.
pub use layout::{Pane, Split, SplitDir};
pub use tab::Tab;
pub use updates::UpdateState;

use layout::{
    draw_pane, fitted_inner_size, split_divider, split_extent, At, LayoutAction, PaneMeasure,
    WindowFit, FALLBACK_COLS, FALLBACK_ROWS, FIT_ATTEMPTS, SPLIT_DIVIDER, TITLE_CONTROL_SIDE,
    TITLE_FREE_STRIP,
};
use tab::{close_down, cursor_bytes, one_char, selection_in_line};
use tabs::Renaming;

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

/// How wide the find bar is, so it can be floated against the right edge of the
/// terminal the way an editor puts one.
const FIND_BAR_WIDTH: f32 = 460.0;

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
    /// The rectangles the terminal panes drew into this frame.
    ///
    /// Collected so the window's resize grips can keep off them: a grip is in a
    /// foreground layer and outranks whatever is under it, and a terminal
    /// reaching the window edge would lose its first column to one. Cleared and
    /// refilled every frame - a pane that has gone must not still be claiming
    /// the space it used to be in.
    pane_rects: Vec<egui::Rect>,
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
    /// Whether the window is minimized right now.
    ///
    /// Read every frame and used to hold every session at the size it had: a
    /// minimized window still draws, and the pane it draws measures as almost
    /// nothing, so obeying that measurement reflowed the grid and told the far
    /// side to redraw into a sliver. Coming back up then showed a session with
    /// its output gone.
    minimized: bool,
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
    /// Size and position of the Settings window: restored from the settings
    /// file at startup, and written back on exit for whichever of the two
    /// window switches is on.
    settings_placement: crate::ui::detach::Placement,
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
    /// A split, unsplit or close asked for from a pane's right-click menu,
    /// waiting for the panes to have finished drawing. See [`LayoutAction`].
    pending_layout: Option<LayoutAction>,
}

/// Shortest gap between two frames asked for by session output.
///
/// 30 a second. Output is the one thing this paces - a keystroke's echo is the
/// first chunk after a quiet moment and is never held back, and everything the
/// mouse and keyboard drive comes through the window's own events - so what it
/// costs is half the intermediate states of text scrolling past. At the rate
/// where the limit even applies, those were never readable. What it buys is
/// half of every frame's work: laying the grid out, tessellating it, and the
/// graphics driver's share, which together are most of what the app spends.
///
/// It also means the same output reaches the screen in fewer, larger batches,
/// which is strictly less work for the same pixels: parsing what arrives costs
/// almost nothing next to drawing it.
const OUTPUT_FRAME: Duration = Duration::from_millis(33);

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        // So a session's reader thread can ask for the frame that will show
        // what it just read, instead of the UI redrawing forever on the chance
        // that something arrived.
        //
        // Rate-limited, because a session pouring out a long listing hands over
        // a chunk every few hundred microseconds and each one would otherwise
        // ask for a frame: the screen cannot show more than it refreshes, and
        // on a 144 Hz display that is more than twice the work for pixels
        // nobody sees. The first chunk after a quiet moment is never delayed -
        // that one is the echo of a keystroke, and it has to feel immediate -
        // so only a burst is thinned, and even then only to the rate below.
        let waker_ctx = cc.egui_ctx.clone();
        let last_wake = Mutex::new(None::<Instant>);
        crate::pty::set_waker(move || {
            let mut last = match last_wake.lock() {
                Ok(last) => last,
                // Another thread panicked mid-wake; a redraw is the safe answer.
                Err(poisoned) => poisoned.into_inner(),
            };
            let now = Instant::now();
            match *last {
                Some(at) if now.duration_since(at) < OUTPUT_FRAME => {
                    waker_ctx.request_repaint_after(OUTPUT_FRAME - now.duration_since(at));
                }
                _ => {
                    *last = Some(now);
                    waker_ctx.request_repaint();
                }
            }
        });
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

        // Read before `settings` is moved into the app.
        let mut restore = settings.restored_settings_placement();
        // Same rule as the main window in `crate::run`: a position saved on a
        // monitor that is no longer attached is dropped, and the window opens
        // in the middle of the main one instead. The Settings window is the
        // worse half of that bug - there is no taskbar entry to drag it back
        // by, so an unreachable one cannot be closed either.
        restore.position = restore
            .position
            .filter(|position| crate::ui::monitors::reachable(*position, restore.size));
        let settings_placement = crate::ui::detach::Placement {
            restore,
            seen: crate::ui::detach::Geometry::default(),
        };

        let mut app = App {
            new_tab_profile: default_profile,
            macro_groups: load_macros(&settings).groups,
            pane_rects: Vec::new(),
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
            minimized: false,
            view_size: (FALLBACK_COLS as usize, FALLBACK_ROWS as usize),
            status: None,
            status_at: None,
            updates: UpdateState::default(),
            font_family: String::new(),
            font_request: String::new(),
            renaming: None,
            pending_layout: None,
            settings_placement,
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
        // Before the check: it is what the check authenticates to the proxy
        // with, and a check that starts without it gets a 407 instead.
        update::configure_proxy_user(&app.settings.proxy_user);
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
}

/// A square button carrying one of the app's own marks.
///
/// The alternative was a letter: the new-tab button was a `+` and the tab's
/// close button a lowercase `x`, which at this size is a small letter next to a
/// slightly larger one. A painted mark can be designed against its neighbours -
/// see [`icons`] - and it cannot come out as a tofu box in a font that has no
/// glyph for it.
/// `tint` is the theme's colour for this mark, when it names one. It wins over
/// the widget colours whether or not the pointer is on the button: a theme
/// picking out the `+` means it picked it out, not "unless you hover it".
fn icon_button(
    ui: &mut egui::Ui,
    glyph: icons::Glyph,
    tint: Option<egui::Color32>,
) -> egui::Response {
    let side = ui.spacing().interact_size.y;
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(side), egui::Sense::click());
    let visuals = ui.style().interact(&response);
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, egui::Rounding::same(3.0), visuals.bg_fill);
    }
    icons::draw(
        ui.painter(),
        rect,
        glyph,
        tint.unwrap_or(visuals.fg_stroke.color),
        visuals.bg_fill,
    );
    response
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
