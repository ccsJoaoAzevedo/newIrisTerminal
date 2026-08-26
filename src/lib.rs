//! newIrisTerminal — a terminal emulator for InterSystems IRIS.
//!
//! The crate is a library plus a thin binary so integration tests (and, later,
//! the plugin host) can drive the terminal core without going through the GUI.

pub mod app;
pub mod config;
pub mod features;
pub mod plugins;
pub mod pty;
pub mod term;
pub mod ui;

/// Starts the GUI. The binary is nothing more than a call to this.
pub fn run() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1000.0, 640.0])
            .with_min_inner_size([400.0, 240.0])
            .with_title("newIrisTerminal"),
        ..Default::default()
    };

    eframe::run_native(
        "newIrisTerminal",
        options,
        Box::new(|cc| Ok(Box::new(app::App::new(cc)))),
    )
}
