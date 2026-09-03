//! End-to-end: the prompt IRIS hands out while the process is stopped at a
//! break is still a command line.
//!
//! ```text
//! cargo test --test live_break_prompt -- --ignored --nocapture --test-threads=1
//! ```
//!
//! A break leaves the program stack level on the prompt — `COMP80 2x0>`,
//! `RDB81-UL 3f2>` — and the space in it used to make the row read as output
//! rather than as a line being typed. Everything the app builds on that
//! reading then stopped: Home and End reached IRIS as `ESC [ H`, which it
//! submits the line on rather than acting on, and Ctrl+A, the Ctrl+arrow
//! motions and clicking to place the cursor did nothing at all.
//!
//! Read-only: `Break`, `Write` and `Quit`, nothing that touches data.

use std::time::{Duration, Instant};

use egui::{Key, Modifiers};
use new_iris_terminal::pty::launcher::{launcher, LaunchSpec};
use new_iris_terminal::pty::PtySession;
use new_iris_terminal::term::{lineedit, parser, Grid, LineEdit};
use new_iris_terminal::ui::input;

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
        let session =
            PtySession::spawn(launcher().as_ref(), &spec, 80, 24).expect("spawning a session");
        Live {
            session,
            grid: Grid::new(80, 24, 500),
            vte: vte::Parser::new(),
        }
    }

    /// Pumps until `predicate` accepts the rendered screen, or the timeout
    /// expires. Returns whether it matched.
    fn wait_for<F: Fn(&Grid) -> bool>(&mut self, timeout: Duration, predicate: F) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let (bytes, ended) = self.session.drain();
            if !bytes.is_empty() {
                let replies = parser::advance(&mut self.vte, &mut self.grid, &bytes);
                if !replies.is_empty() {
                    let _ = self.session.write(&replies);
                }
                if predicate(&self.grid) {
                    return true;
                }
            }
            if ended {
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        predicate(&self.grid)
    }

    fn send(&mut self, bytes: &[u8]) {
        self.session.write(bytes).expect("writing to the PTY");
    }

    fn send_line(&mut self, text: &str) {
        self.send(text.as_bytes());
        self.send(b"\r");
    }

    /// The row the cursor is on, trailing blanks trimmed.
    fn cursor_row(&self) -> String {
        let row: String = self.grid.screen[self.grid.cursor.row]
            .cells
            .iter()
            .map(|c| c.ch)
            .collect();
        row.trim_end().to_string()
    }

    /// Waits for the cursor to be sitting on a bare prompt.
    fn wait_for_prompt(&mut self) -> bool {
        self.wait_for(Duration::from_secs(20), |grid| {
            lineedit::current(grid).is_some_and(|line| line.is_empty())
        })
    }

    /// Sends one key the way the app does, and waits for IRIS's echo of it to
    /// put the cursor where `settled` wants it.
    fn press(&mut self, key: Key, line: Option<LineEdit>, settled: usize) -> bool {
        let bytes = input::key_bytes(key, &Modifiers::NONE, line, self.grid.app_cursor_keys)
            .unwrap_or_else(|| panic!("{key:?} sends nothing"));
        // Home and End at a command line are walks, not sequences: they only
        // ever go out as the arrow keys IRIS acts on.
        assert!(
            !bytes.ends_with(b"H") && !bytes.ends_with(b"F"),
            "{key:?} was sent as a VT sequence ({bytes:?}), which IRIS submits \
             the line on instead of moving the cursor"
        );
        self.send(&bytes);
        self.wait_for(Duration::from_secs(5), |grid| grid.cursor.col == settled)
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.session.request_halt();
    }
}

#[test]
#[ignore = "needs a local IRIS instance"]
fn home_and_end_walk_the_line_at_a_break_prompt() {
    let mut live = Live::start();
    assert!(live.wait_for_prompt(), "never reached a prompt");

    // `Break` with no argument suspends where it stands and hands the prompt
    // back with the stack still on it - the state the screenshot was taken in.
    live.send_line("Xecute \"Break  Write 1\"");
    let at_break = live.wait_for(Duration::from_secs(10), |grid| {
        let row: String = grid.screen[grid.cursor.row]
            .cells
            .iter()
            .map(|c| c.ch)
            .collect();
        row.trim_end().contains(' ') && row.trim_end().ends_with('>')
    });
    assert!(
        at_break,
        "never reached a break prompt; the cursor row was {:?}",
        live.cursor_row()
    );
    let prompt = live.cursor_row();
    println!("break prompt: {prompt:?}");

    // The regression: this row is a line being read, level indicator and all.
    let empty = lineedit::current(&live.grid)
        .unwrap_or_else(|| panic!("{prompt:?} was not read as a command line"));
    assert!(empty.is_empty(), "nothing has been typed yet");
    assert_eq!(empty.start, prompt.chars().count(), "just past the `>`");

    live.send(b"abcdef");
    assert!(
        live.wait_for(Duration::from_secs(5), |grid| {
            lineedit::typed_text(grid).as_deref() == Some("abcdef")
        }),
        "IRIS never echoed the typed line; the cursor row was {:?}",
        live.cursor_row()
    );

    let line = lineedit::current(&live.grid).expect("still a command line");
    assert_eq!(line.start, empty.start);
    assert_eq!(line.end, line.start + 6);
    assert_eq!(line.cursor, line.end);

    // Home walks back over what was typed, and stops on the prompt.
    assert!(
        live.press(Key::Home, Some(line), line.start),
        "Home did not put IRIS's cursor at the start of the line; it is at \
         column {} of {:?}",
        live.grid.cursor.col,
        live.cursor_row()
    );
    // What was typed is still there: a walk moves the cursor and nothing else.
    assert_eq!(lineedit::typed_text(&live.grid).as_deref(), Some("abcdef"));

    // And End walks forward over the same six columns.
    let line = lineedit::current(&live.grid).expect("still a command line");
    assert!(
        live.press(Key::End, Some(line), line.end),
        "End did not put IRIS's cursor back at the end of the line; it is at \
         column {} of {:?}",
        live.grid.cursor.col,
        live.cursor_row()
    );
    assert_eq!(lineedit::typed_text(&live.grid).as_deref(), Some("abcdef"));

    // Leave the process where it was found: rub the line out, then unwind.
    live.send(&[0x7f; 6]);
    live.send_line("Quit");
    assert!(
        live.wait_for_prompt(),
        "the session did not come back to a prompt"
    );
}
