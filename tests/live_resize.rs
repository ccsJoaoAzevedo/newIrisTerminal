//! Resizing the window against a real IRIS.
//!
//! ```text
//! cargo test --test live_resize -- --ignored --nocapture
//! ```
//!
//! A Windows pseudoconsole answers every resize by repainting the whole screen,
//! and its repaint has exactly the shape of IRIS's own `W #`: home the cursor,
//! then erase each row on the way down, writing the content back as it goes.
//! Read as a clear, the screen it hands back is filed into the transcript and
//! the repaint lands underneath the copy - so dragging the window taller
//! duplicated everything on screen, once per resize. This is the test that says
//! it does not.
//!
//! Nothing here writes data: the commands are `Write`s of literals.

use std::time::{Duration, Instant};

use new_iris_terminal::pty::launcher::{launcher, LaunchSpec};
use new_iris_terminal::pty::PtySession;
use new_iris_terminal::term::{parser, Grid};
use new_iris_terminal::ui::terminal_view::TERMINAL_COLS;

struct Live {
    session: PtySession,
    grid: Grid,
    vte: vte::Parser,
}

impl Live {
    fn start() -> Self {
        let instance = std::env::var("IRIS_TEST_INSTANCE")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| launcher().discover().into_iter().next().map(|i| i.name))
            .expect("no IRIS instance found");
        let spec = LaunchSpec {
            instance,
            ..LaunchSpec::default()
        };
        Live {
            // The shape the app opens a tab in: a fallback size, resized to the
            // real geometry once there is a frame to measure it from.
            session: PtySession::spawn(launcher().as_ref(), &spec, 80, 24).expect("spawn"),
            grid: Grid::new(80, 24, 500),
            vte: vte::Parser::new(),
        }
    }

    /// Reads and parses for `ms`, the way a run of frames would.
    fn settle(&mut self, ms: u64) {
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            let (bytes, _) = self.session.drain();
            if !bytes.is_empty() {
                let replies = parser::advance(&mut self.vte, &mut self.grid, &bytes);
                if !replies.is_empty() {
                    let _ = self.session.write(&replies);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// Both halves of what the app does when the window changes size.
    fn resize(&mut self, cols: usize, rows: usize) {
        self.session
            .resize(cols as u16, rows as u16)
            .expect("resizing the pty");
        self.grid.resize(cols, rows);
        self.settle(600);
    }

    /// Every non-blank line the session holds, history included.
    fn lines(&self) -> Vec<String> {
        (0..self.grid.total_lines())
            .filter_map(|line| self.grid.line(line))
            .map(|row| row.to_text().trim_end().to_string())
            .filter(|text| !text.is_empty())
            .collect()
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.session.request_halt();
    }
}

#[test]
#[ignore = "needs a local IRIS instance"]
fn resizing_the_window_does_not_duplicate_the_screen() {
    let mut live = Live::start();
    live.settle(1500);
    live.resize(TERMINAL_COLS, 24);

    live.session.write(b"write \"one\"\r").expect("write");
    live.settle(700);
    live.session.write(b"write \"two\"\r").expect("write");
    live.settle(700);

    let before = live.lines();
    assert!(
        before.iter().any(|l| l.contains("write \"two\"")),
        "the session never ran the commands: {before:?}"
    );

    // Taller, then shorter, then back: a window drag is dozens of these.
    for (cols, rows) in [
        (TERMINAL_COLS, 40),
        (TERMINAL_COLS, 20),
        (TERMINAL_COLS, 47),
        (TERMINAL_COLS, 24),
    ] {
        live.resize(cols, rows);
        let after = live.lines();
        assert_eq!(
            after, before,
            "resizing to {cols}x{rows} changed what the session holds"
        );
    }

    // And the count itself, said plainly: one copy of each command, not four.
    let echoes = live
        .lines()
        .iter()
        .filter(|l| l.contains("write \"one\""))
        .count();
    assert_eq!(echoes, 1, "the echoed command is on screen {echoes} times");
}
