//! End-to-end input tests against a real IRIS instance.
//!
//! ```text
//! cargo test --test live_input -- --ignored --nocapture --test-threads=1
//! ```
//!
//! These drive the same code paths the GUI uses, but bypass egui: they push
//! bytes into the PTY and assert on the rendered grid. That is enough to prove
//! the two things unit tests cannot — that Up recalls a command, and that
//! Ctrl+C interrupts a running routine.
//!
//! Nothing here writes data. The commands used are `Write`, `Hang`, and
//! `$NAMESPACE` reads, because `RDB*` databases are shared.

use std::time::{Duration, Instant};

use new_iris_terminal::pty::launcher::{launcher, LaunchSpec};
use new_iris_terminal::pty::PtySession;
use new_iris_terminal::term::{parser, Grid};

struct Live {
    session: PtySession,
    grid: Grid,
    vte: vte::Parser,
    /// Every byte received, for sequence-level assertions.
    raw: Vec<u8>,
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
            raw: Vec::new(),
        }
    }

    /// Pumps until `predicate` accepts the rendered screen, or the timeout
    /// expires. Returns whether it matched.
    fn wait_for<F: Fn(&str) -> bool>(&mut self, timeout: Duration, predicate: F) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            let (bytes, ended) = self.session.drain();
            if !bytes.is_empty() {
                self.raw.extend_from_slice(&bytes);
                let replies = parser::advance(&mut self.vte, &mut self.grid, &bytes);
                if !replies.is_empty() {
                    let _ = self.session.write(&replies);
                }
                if predicate(&self.screen()) {
                    return true;
                }
            }
            if ended {
                break;
            }
            std::thread::sleep(Duration::from_millis(30));
        }
        predicate(&self.screen())
    }

    fn screen(&self) -> String {
        self.grid.screen_text().join("\n")
    }

    fn send(&mut self, bytes: &[u8]) {
        self.session.write(bytes).expect("writing to the PTY");
    }

    fn send_line(&mut self, text: &str) {
        self.send(text.as_bytes());
        self.send(b"\r");
    }

    /// Waits for an IRIS prompt (`NAMESPACE>`) to be the last thing on screen.
    fn wait_for_prompt(&mut self) -> bool {
        self.wait_for(Duration::from_secs(20), |screen| {
            screen
                .lines()
                .rfind(|l| !l.trim().is_empty())
                .map(|l| l.trim_end().ends_with('>'))
                .unwrap_or(false)
        })
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        self.session.request_halt();
    }
}

/// Whether IRIS turned on DECCKM (application cursor keys). If it did, arrows
/// must be sent as `ESC O A` rather than `ESC [ A`, and sending the wrong form
/// means Up never recalls anything.
fn wants_application_cursor_keys(raw: &[u8]) -> bool {
    let on = raw
        .windows(6)
        .rposition(|w| w == b"\x1b[?1h")
        .or_else(|| raw.windows(5).rposition(|w| w == b"\x1b[?1h"));
    let off = raw.windows(5).rposition(|w| w == b"\x1b[?1l");
    match (on, off) {
        (Some(on), Some(off)) => on > off,
        (Some(_), None) => true,
        _ => false,
    }
}

#[test]
#[ignore = "needs a local IRIS instance"]
fn report_cursor_key_mode() {
    let mut live = Live::start();
    assert!(live.wait_for_prompt(), "never reached a prompt");

    let app_mode = wants_application_cursor_keys(&live.raw);
    println!("DECCKM (application cursor keys) requested: {app_mode}");
    println!(
        "arrows should therefore be sent as: {}",
        if app_mode { "ESC O A" } else { "ESC [ A" }
    );
    // Informational: the assertions live in the recall test below.
}

#[test]
#[ignore = "needs a local IRIS instance"]
fn up_arrow_recalls_the_previous_command() {
    let mut live = Live::start();
    assert!(live.wait_for_prompt(), "never reached a prompt");

    // A command with an unmistakable result, so we know it ran.
    live.send_line("Write 6*7");
    assert!(
        live.wait_for(Duration::from_secs(10), |s| s.contains("42")),
        "the probe command did not run; screen was:\n{}",
        live.screen()
    );

    let before = live.screen();

    // Send whichever arrow encoding this session actually asked for.
    let arrow: &[u8] = if wants_application_cursor_keys(&live.raw) {
        b"\x1bOA"
    } else {
        b"\x1b[A"
    };
    live.send(arrow);

    // Recall echoes the previous command onto the current prompt line without
    // executing it.
    let recalled = live.wait_for(Duration::from_secs(6), |s| {
        s.lines()
            .rfind(|l| !l.trim().is_empty())
            .map(|l| l.contains("Write 6*7"))
            .unwrap_or(false)
    });

    println!(
        "--- before ---\n{before}\n--- after Up ---\n{}",
        live.screen()
    );
    assert!(
        recalled,
        "Up did not recall the previous command; last line was {:?}",
        live.screen()
            .lines()
            .rfind(|l| !l.trim().is_empty())
            .unwrap_or("")
            .to_string()
    );

    // Leave the recalled line unexecuted.
    live.send(b"\x03");
}

#[test]
#[ignore = "needs a local IRIS instance"]
fn ctrl_c_interrupts_a_running_routine() {
    let mut live = Live::start();
    assert!(live.wait_for_prompt(), "never reached a prompt");

    // `Hang` blocks without burning CPU, which is the polite way to occupy a
    // shared instance for a second or two.
    live.send_line("For i=1:1:60 { Hang 1 }");

    // Wait for the echo, pumping as we go — sleeping without draining would
    // leave the echoed bytes sitting unread in the channel.
    assert!(
        live.wait_for(Duration::from_secs(10), |s| s.contains("Hang 1")),
        "the loop command was never echoed; screen:\n{}",
        live.screen()
    );

    // Keep pumping for a moment so the prompt has a chance to disappear.
    live.wait_for(Duration::from_secs(2), |_| false);
    let during = live.screen();
    assert!(
        !during.trim_end().ends_with('>'),
        "the loop did not start; screen:\n{during}"
    );

    // 0x03 is what the GUI sends for Ctrl+C when nothing is selected.
    live.send(b"\x03");

    let interrupted = live.wait_for(Duration::from_secs(10), |s| {
        s.contains("INTERRUPT")
            || s.lines()
                .rfind(|l| !l.trim().is_empty())
                .map(|l| l.trim_end().ends_with('>'))
                .unwrap_or(false)
    });

    println!("--- after Ctrl+C ---\n{}", live.screen());
    assert!(
        interrupted,
        "Ctrl+C did not interrupt; screen was:\n{}",
        live.screen()
    );
}
