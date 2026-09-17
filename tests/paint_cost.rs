//! What one frame of the terminal actually costs on the CPU.
//!
//! Not a correctness test: a stopwatch, run by hand, for the two halves of a
//! frame that the app pays for on every redraw - building the shapes, and
//! turning them into triangles. Ignored by default because a timing number is
//! not a thing to fail a build on; run it with
//! `cargo test --release --test paint_cost -- --ignored --nocapture`.

use eframe::egui;
use new_iris_terminal::config::theme::Theme;
use new_iris_terminal::term::Grid;
use new_iris_terminal::ui::terminal_view::{self, PaneRole, RenderOpts, ViewState};

/// A screen with something on every row, which is what an IRIS session
/// scrolling output looks like and the case the cost has to be paid for.
fn filled_grid(cols: usize, rows: usize, width: usize) -> Grid {
    let mut grid = Grid::new(cols, rows, 1000);
    let mut vte = vte::Parser::new();
    let line: String = std::iter::repeat_n("do ^%CSW1A write \"x\",! set a=1 ", 40)
        .collect::<String>()
        .chars()
        .take(width.min(cols))
        .collect();
    for _ in 0..rows {
        new_iris_terminal::term::parser::advance(
            &mut vte,
            &mut grid,
            format!("{line}\r\n").as_bytes(),
        );
    }
    grid
}

fn measure(cols: usize, rows: usize, size: egui::Vec2) {
    measure_inner(cols, rows, size, Fill::Full);
    measure_inner(cols, rows, size, Fill::Session);
    measure_inner(cols, rows, size, Fill::Blank);
}

/// How much of the screen has something on it.
#[derive(Clone, Copy, PartialEq)]
enum Fill {
    /// Every column of every row, which is the worst case and the one a
    /// full-screen program produces.
    Full,
    /// What an IRIS prompt actually looks like: short lines with most of the
    /// width left blank.
    Session,
    /// Nothing at all - the floor, and what egui's own frame costs.
    Blank,
}

fn measure_inner(cols: usize, rows: usize, size: egui::Vec2, fill: Fill) {
    let ctx = egui::Context::default();
    let grid = match fill {
        Fill::Full => filled_grid(cols, rows, cols),
        Fill::Session => filled_grid(cols, rows, 46),
        Fill::Blank => Grid::new(cols, rows, 1000),
    };
    let theme = Theme::default();
    let opts = RenderOpts::default();
    let mut state = ViewState::default();

    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
        ..Default::default()
    };

    // The first frames build the font atlas and warm every cache; timing them
    // would measure the warm-up, not the steady state the app runs in.
    let mut build = std::time::Duration::ZERO;
    let mut tessellate = std::time::Duration::ZERO;
    let mut shapes = 0usize;
    let mut vertices = 0usize;
    const FRAMES: u32 = 30;
    for frame in 0..FRAMES + 5 {
        let started = std::time::Instant::now();
        let output = ctx.run(input.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                terminal_view::show(
                    ui,
                    &grid,
                    &mut state,
                    &theme,
                    &opts,
                    1,
                    PaneRole {
                        focused: true,
                        take_focus: false,
                        split: false,
                    },
                    &[],
                );
            });
        });
        let built = started.elapsed();
        let count = output.shapes.len();
        let started = std::time::Instant::now();
        let meshes = ctx.tessellate(output.shapes, output.pixels_per_point);
        let tess = started.elapsed();
        if frame >= 5 {
            build += built;
            tessellate += tess;
            shapes = count;
            vertices = meshes
                .iter()
                .map(|p| match &p.primitive {
                    egui::epaint::Primitive::Mesh(m) => m.vertices.len(),
                    _ => 0,
                })
                .sum();
        }
    }
    let build = build.as_secs_f64() * 1000.0 / f64::from(FRAMES);
    let tessellate = tessellate.as_secs_f64() * 1000.0 / f64::from(FRAMES);
    println!(
        "{} {cols}x{rows} @ {}x{}: build {build:.2} ms + tessellate {tessellate:.2} ms = {:.2} ms/frame  ({shapes} shapes, {vertices} vertices)",
        match fill {
            Fill::Full => "full   ",
            Fill::Session => "session",
            Fill::Blank => "blank  ",
        },
        size.x as u32,
        size.y as u32,
        build + tessellate,
    );
    println!(
        "   at 60 fps that is {:.0}% of one core",
        (build + tessellate) * 60.0 / 10.0
    );
}

#[test]
#[ignore = "a stopwatch, not an assertion"]
fn one_frame_of_a_full_screen() {
    measure(80, 24, egui::vec2(800.0, 500.0));
    measure(120, 40, egui::vec2(1200.0, 800.0));
    measure(200, 60, egui::vec2(1920.0, 1080.0));
}

#[test]
#[ignore = "a stopwatch, not an assertion"]
fn how_far_the_lattice_is_from_the_font() {
    let ctx = egui::Context::default();
    let _ = ctx.run(egui::RawInput::default(), |_| {});
    for size in [10.0_f32, 11.0, 12.0, 13.0, 14.0, 16.0, 18.0, 20.0, 24.0] {
        let font = egui::FontId::monospace(size);
        let natural = ctx.fonts(|f| f.glyph_width(&font, 'M'));
        println!(
            "size {size:>4}: natural {natural:.4}  lattice {:.4}  drift/col {:.4}",
            natural.ceil(),
            natural.ceil() - natural
        );
    }
}
