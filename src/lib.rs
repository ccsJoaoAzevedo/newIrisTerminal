//! newIrisTerminal — a terminal emulator for InterSystems IRIS.
//!
//! The crate is a library plus a thin binary so integration tests (and, later,
//! the plugin host) can drive the terminal core without going through the GUI.

pub mod app;
pub mod config;
pub mod features;
pub mod i18n;
pub mod plugins;
pub mod pty;
pub mod term;
pub mod ui;

use eframe::egui;
use std::sync::Arc;

// Função para processar a imagem do ícone
fn load_icon() -> egui::IconData {
    let image = image::load_from_memory(include_bytes!("../assets/icon.ico"))
        .expect("Falha ao carregar a imagem do ícone")
        .into_rgba8();

    let (width, height) = image.dimensions();
    let rgba = image.into_raw();

    egui::IconData {
        rgba,
        width,
        height,
    }
}

/// Inner window size, in egui points, that should hold a terminal of
/// `cols` x `rows` characters.
///
/// A guess, and only a guess: the real cell size comes from the font egui ends
/// up with, which is not known until it has a context to measure in, and the
/// panels above the terminal are laid out at the same time. [`app::App`] takes
/// the measurement on its first frames and corrects the window to the exact
/// geometry; this only decides how far off it starts, so that the correction is
/// a nudge rather than a jump.
fn estimated_inner_size(settings: &config::Settings, cols: u16, rows: u16) -> [f32; 2] {
    let font_size = settings.font_size.max(6.0);
    // Ratios of a typical monospace face: half again as tall as it is set, and
    // a little over half as wide.
    let cell_w = font_size * 0.62;
    let cell_h = font_size * 1.5;
    // The menu bar, the tab strip and the scrollbar the terminal reserves.
    let chrome_h = 78.0;
    let chrome_w = 24.0;
    [
        cols as f32 * cell_w + chrome_w,
        rows as f32 * cell_h + chrome_h,
    ]
}

/// Starts the GUI. The binary is nothing more than a call to this.
pub fn run() -> eframe::Result<()> {
    // Read early, because a window's frame, size and position are all fixed
    // when it is created. `App::new` loads the settings again; the file is
    // small and the alternative is threading it through `run_native`'s
    // callback.
    let settings = config::Settings::load();
    // The copy standing in for the tray's terminal only passes the request on
    // to the real install, so updating that one updates both.
    if features::iris_terminal::running_as_terminal() && features::iris_terminal::forward() {
        return Ok(());
    }
    let mut launch = features::explorer_menu::Launch::parse(std::env::args().skip(1));
    launch.as_terminal |= features::iris_terminal::running_as_terminal();

    // With closing to the tray on, a launch that finds another copy running
    // brings that one forward instead of starting a second, the way
    // Notepad++ does. Otherwise every launch after a close-to-tray left one
    // more hidden copy behind, each holding its sessions open.
    // Not for a launch from Explorer's menu, though: the running copy cannot
    // be told which folder, so a window of its own opens there instead.
    if settings.close_to_tray && launch == Default::default() && ui::tray::wake_existing() {
        return Ok(());
    }

    // Restoring the last size is exact; opening at the default geometry is not,
    // because it is expressed in characters. Either way `App` has the last word
    // once it can measure one.
    let inner_size = settings.restored_window_size().unwrap_or_else(|| {
        let (cols, rows) = settings.default_geometry();
        estimated_inner_size(&settings, cols, rows)
    });
    // Dropped when it no longer lands on a monitor that is attached: a window
    // closed on a second screen would otherwise reopen where that screen used to
    // be, and on a laptop on its own that is a window with no way to reach it -
    // nor, since it is saved the same way, the Settings window. `centered`
    // below then takes over, which is what the setting being off already does.
    let position = settings
        .restored_window_position()
        .filter(|position| ui::monitors::reachable(*position, settings.restored_window_size()));

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size(inner_size)
        .with_min_inner_size([400.0, 240.0])
        // The title still matters with the frame off: it is what the
        // taskbar and the window switcher show.
        .with_title("newIrisTerminal")
        .with_decorations(false)
        .with_resizable(true)
        // Define o ícone da janela e barra de tarefas aqui:
        .with_icon(Arc::new(load_icon()));
    if let Some(position) = position {
        viewport = viewport.with_position(position);
    }
    if settings.restored_maximized() {
        viewport = viewport.with_maximized(true);
    }

    let options = eframe::NativeOptions {
        viewport,
        // eframe applies this after the builder's position, so it is only ever
        // set when there is no saved position to honour.
        centered: position.is_none(),
        ..Default::default()
    };

    eframe::run_native(
        "newIrisTerminal",
        options,
        Box::new(move |cc| Ok(Box::new(app::App::with_launch(cc, launch)))),
    )
}
