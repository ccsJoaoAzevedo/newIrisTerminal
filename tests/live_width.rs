//! How long a line the terminal can carry, end to end against a real IRIS.
//!
//! ```text
//! cargo test --test live_width -- --ignored --nocapture --test-threads=1
//! ```
//!
//! IRIS truncates a `Write` at the device right margin, so the width the app
//! reports is the longest line a session can ever produce - and the longest
//! command it can ever echo. This is the test that says what that number is
//! worth in practice: it asks for a line far longer than the window, and
//! checks that all of it arrived and that the grid is holding it.
//!
//! Nothing here writes data: the command is a `Write` of a computed string.

use std::time::{Duration, Instant};

use new_iris_terminal::pty::launcher::{launcher, LaunchSpec};
use new_iris_terminal::pty::PtySession;
use new_iris_terminal::term::{parser, Grid};
use new_iris_terminal::ui::terminal_view::TERMINAL_COLS;

/// Characters to ask for on one line. Past any window, past the 512-column
/// margin the app used to claim, and a length the ERP really does produce -
/// a `zwrite` of one node of a wide global.
const LINE: usize = 3000;

struct Live {
    session: PtySession,
    grid: Grid,
    vte: vte::Parser,
    raw: Vec<u8>,
}

impl Live {
    /// A session at the geometry the app itself opens one at: the wide grid,
    /// and a window-sized number of rows.
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
        let cols = TERMINAL_COLS as u16;
        let session =
            PtySession::spawn(launcher().as_ref(), &spec, cols, 24).expect("spawning a session");
        Live {
            session,
            grid: Grid::new(TERMINAL_COLS, 24, 500),
            vte: vte::Parser::new(),
            raw: Vec::new(),
        }
    }

    /// Pumps until the raw stream contains `needle`, parsing as it goes.
    fn wait_for(&mut self, needle: &str, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let (bytes, ended) = self.session.drain();
            if !bytes.is_empty() {
                self.raw.extend_from_slice(&bytes);
                let replies = parser::advance(&mut self.vte, &mut self.grid, &bytes);
                if !replies.is_empty() {
                    let _ = self.session.write(&replies);
                }
                if String::from_utf8_lossy(&self.raw).contains(needle) {
                    return true;
                }
            }
            if ended {
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn send_line(&mut self, text: &str) {
        self.session.write(text.as_bytes()).expect("write");
        self.session.write(b"\r").expect("write");
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.session.request_halt();
    }
}

/// The marker is spelt as two pieces IRIS concatenates, so the echo of the
/// command never contains the assembled form and cannot be mistaken for the
/// answer.
const MARK: &str = "\"[[E\"_\"ND]]\"";

#[test]
#[ignore = "needs a local IRIS instance"]
fn a_line_far_wider_than_the_window_arrives_whole() {
    let mut live = Live::start();
    assert!(
        live.wait_for(">", Duration::from_secs(20)),
        "never reached a prompt"
    );

    live.send_line(&format!(
        "write $TRANSLATE($JUSTIFY(\"\",{LINE}),\" \",\"X\"),{MARK}"
    ));
    assert!(
        live.wait_for("[[END]]", Duration::from_secs(20)),
        "the line never finished arriving"
    );

    let received = String::from_utf8_lossy(&live.raw)
        .chars()
        .filter(|c| *c == 'X')
        .count();
    // One extra: the "X" in the echoed command itself.
    assert!(
        received >= LINE,
        "IRIS truncated the line: {received} X's of {LINE}"
    );

    // And the grid is holding it on one row, which is what the view then wraps.
    let longest = (0..live.grid.total_lines())
        .filter_map(|line| live.grid.line(line))
        .map(|row| row.to_text().chars().filter(|c| *c == 'X').count())
        .max()
        .unwrap_or(0);
    assert!(
        longest >= LINE,
        "the grid kept only {longest} of {LINE} characters on one line"
    );
}

/// The other half of the same margin: what is typed is echoed by IRIS, and the
/// echo is truncated at the margin too. That is what made a long command look
/// like a terminal that had stopped taking keys.
#[test]
#[ignore = "needs a local IRIS instance"]
fn a_command_far_longer_than_the_window_is_accepted_and_echoed_whole() {
    let mut live = Live::start();
    assert!(
        live.wait_for(">", Duration::from_secs(20)),
        "never reached a prompt"
    );

    let typed = "X".repeat(LINE);
    live.send_line(&format!("write \"L=\",$LENGTH(\"{typed}\"),{MARK}"));
    assert!(
        live.wait_for("[[END]]", Duration::from_secs(20)),
        "the command never ran"
    );

    let out = String::from_utf8_lossy(&live.raw).to_string();
    let answer: String = out
        .rsplit_once("L=")
        .map(|(_, tail)| tail.chars().take_while(|c| c.is_ascii_digit()).collect())
        .unwrap_or_default();
    assert_eq!(
        answer,
        LINE.to_string(),
        "IRIS received a different number of characters than were typed"
    );

    // The echo is on the grid too, all of it, on the row the command was typed
    // on: a line the app could not hold would be the same bug seen from the
    // other side.
    let longest = (0..live.grid.total_lines())
        .filter_map(|line| live.grid.line(line))
        .map(|row| row.to_text().chars().filter(|c| *c == 'X').count())
        .max()
        .unwrap_or(0);
    assert!(
        longest >= LINE,
        "the echo was cut to {longest} of {LINE} characters"
    );
}

/// The app does not open a session at its final size: a tab is spawned at a
/// fallback geometry and resized to the real one on the first frame. A width a
/// session can be *spawned* at therefore says nothing about a width it can be
/// *grown* into - and that is the difference this test exists for.
///
/// Resized to exactly 32767 columns - `SHRT_MAX` - the pseudoconsole stops
/// answering: the session comes up as a black screen that ignores every key.
/// [`TERMINAL_COLS`] is half of that for this reason, and this is the test that
/// says so, so that raising it is a decision someone makes on purpose.
#[test]
#[ignore = "needs a local IRIS instance"]
fn a_session_survives_being_grown_into_the_margin_we_claim() {
    let instance = std::env::var("IRIS_TEST_INSTANCE")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| launcher().discover().into_iter().next().map(|i| i.name))
        .expect("no IRIS instance found");
    let spec = LaunchSpec {
        instance,
        ..LaunchSpec::default()
    };
    // A window-shaped number of rows, and the fallback width the app opens at.
    let mut live = Live {
        session: PtySession::spawn(launcher().as_ref(), &spec, 80, 24).expect("spawn at 80x24"),
        grid: Grid::new(80, 47, 500),
        vte: vte::Parser::new(),
        raw: Vec::new(),
    };
    assert!(
        live.wait_for(">", Duration::from_secs(20)),
        "never reached a prompt at the fallback size"
    );

    live.session
        .resize(TERMINAL_COLS as u16, 47)
        .expect("resizing to the margin we claim");
    live.grid.resize(TERMINAL_COLS, 47);
    live.raw.clear();

    live.send_line(&format!("write \"still here\",{MARK}"));
    assert!(
        live.wait_for("[[END]]", Duration::from_secs(15)),
        "the session stopped answering after being resized to {TERMINAL_COLS} columns"
    );
}
