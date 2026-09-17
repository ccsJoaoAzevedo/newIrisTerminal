//! What dragging the window edge costs in memory.
//!
//! Not a correctness test: a scale, run by hand, for a report that widening the
//! window grows the process and maximizing settles it again. It counts live
//! heap bytes rather than asking the OS for the resident set, so the number is
//! this program's own allocation and not the allocator's opinion about when to
//! give pages back.
//!
//! ```text
//! cargo test --release --test resize_memory -- --ignored --nocapture
//! ```

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicIsize, Ordering};

use eframe::egui;
use new_iris_terminal::config::theme::Theme;
use new_iris_terminal::term::Grid;
use new_iris_terminal::ui::terminal_view::{self, PaneRole, RenderOpts, ViewState, TERMINAL_COLS};

/// Every allocation, less every deallocation. The default `realloc` and
/// `alloc_zeroed` are built on these two, so both are counted.
static LIVE: AtomicIsize = AtomicIsize::new(0);

struct Counting;

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        LIVE.fetch_add(layout.size() as isize, Ordering::Relaxed);
        System.alloc(layout)
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size() as isize, Ordering::Relaxed);
        System.dealloc(ptr, layout);
    }
}

#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn live_kb() -> f64 {
    LIVE.load(Ordering::Relaxed) as f64 / 1024.0
}

/// A session that has been used: a screenful of output, and scrollback behind
/// it, which is the state the report is about.
fn used_grid(rows: usize) -> Grid {
    let mut grid = Grid::new(TERMINAL_COLS, rows, 5000);
    let mut vte = vte::Parser::new();
    let line = "USER>do ^%CSW1A write \"x\",! set a=1 ; a line of output\r\n";
    for _ in 0..600 {
        new_iris_terminal::term::parser::advance(&mut vte, &mut grid, line.as_bytes());
    }
    grid
}

/// One frame at a given window width, exactly as the shell draws one: the pane
/// is drawn, and the session is then resized to what the pane measured.
fn frame(ctx: &egui::Context, grid: &mut Grid, state: &mut ViewState, width: f32, height: f32) {
    let theme = Theme::default();
    let opts = RenderOpts::default();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(width, height),
        )),
        ..Default::default()
    };
    let mut measured = None;
    let _ = ctx.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            let result = terminal_view::show(
                ui,
                grid,
                state,
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
            measured = Some((result.cols, result.rows));
        });
    });
    if let Some((cols, rows)) = measured {
        if cols >= 2 && rows >= 2 {
            grid.resize(cols, rows);
        }
    }
}

#[test]
#[ignore = "a scale, not an assertion"]
fn dragging_the_window_wider_and_settling() {
    let ctx = egui::Context::default();
    let mut grid = used_grid(24);
    let mut state = ViewState::default();

    // Settle at the size a window opens at, and let every cache fill.
    for _ in 0..40 {
        frame(&ctx, &mut grid, &mut state, 700.0, 460.0);
    }
    let opened = live_kb();
    println!("opened, settled at ~80 columns : {opened:>10.0} KiB");

    // The drag: a resize event per few pixels, all the way out.
    let mut peak: f64 = 0.0;
    let mut width = 700.0_f32;
    while width < 2600.0 {
        width += 4.0;
        frame(&ctx, &mut grid, &mut state, width, 460.0);
        peak = peak.max(live_kb());
    }
    let dragged = live_kb();
    println!("after dragging out to ~300     : {dragged:>10.0} KiB   (peak {peak:.0} KiB)");

    // Maximized: one size, held, which is what the report says settles it.
    for _ in 0..40 {
        frame(&ctx, &mut grid, &mut state, 2600.0, 460.0);
    }
    let held = live_kb();
    println!("held at that size for 40 frames: {held:>10.0} KiB");

    // And back, to see whether anything is kept that the narrow window needs.
    for _ in 0..40 {
        frame(&ctx, &mut grid, &mut state, 700.0, 460.0);
    }
    println!("back at ~80 columns            : {:>10.0} KiB", live_kb());
    println!();
    println!(
        "grid: {} columns x {} rows, {} lines in all",
        grid.cols,
        grid.rows,
        grid.total_lines()
    );
}
