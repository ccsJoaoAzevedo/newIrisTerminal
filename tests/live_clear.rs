//! What IRIS actually sends for `W #`, and what the grid does with it.
//!
//! Ignored by default — it needs an installed, running instance. Run with:
//!
//! ```text
//! cargo test --test live_clear -- --ignored --nocapture
//! ```
//!
//! Read-only: it writes `W #` to the device and nothing else. No login, no
//! data, no globals touched.
//!
//! The point of the test is the regression behind it. `W #` clears the screen
//! in place, and blanking those rows where they stood lost the transcript with
//! them: there was nothing to scroll back to, unlike the native IrisTerm.

use std::time::{Duration, Instant};

use new_iris_terminal::pty::launcher::{launcher, LaunchSpec};
use new_iris_terminal::pty::PtySession;
use new_iris_terminal::term::{parser, Grid};

/// Pumps for `window`, feeding everything into `grid` and keeping the raw
/// bytes so the escape sequence itself can be reported.
fn pump(session: &mut PtySession, grid: &mut Grid, window: Duration) -> Vec<u8> {
    let mut vte = vte::Parser::new();
    let mut raw = Vec::new();
    let deadline = Instant::now() + window;

    while Instant::now() < deadline {
        let (bytes, ended) = session.drain();
        if !bytes.is_empty() {
            raw.extend_from_slice(&bytes);
            let replies = parser::advance(&mut vte, grid, &bytes);
            if !replies.is_empty() {
                let _ = session.write(&replies);
            }
        }
        if ended {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    raw
}

/// Opens a session and pumps until it has settled.
fn start(instance: &str) -> (PtySession, Grid) {
    let spec = LaunchSpec {
        instance: instance.to_string(),
        ..LaunchSpec::default()
    };
    let mut session = PtySession::spawn(launcher().as_ref(), &spec, 80, 24)
        .unwrap_or_else(|e| panic!("could not start a session for {instance}: {e:#}"));
    let mut grid = Grid::new(80, 24, 1000);
    pump(&mut session, &mut grid, Duration::from_secs(5));
    (session, grid)
}

fn test_instance() -> Option<String> {
    if let Ok(name) = std::env::var("IRIS_TEST_INSTANCE") {
        if !name.is_empty() {
            return Some(name);
        }
    }
    launcher().discover().into_iter().next().map(|i| i.name)
}

#[test]
#[ignore = "needs a local IRIS instance"]
fn clearing_the_screen_leaves_the_transcript_in_the_scrollback() {
    let instance = test_instance().expect("no instance to test against");
    let (mut session, mut grid) = start(&instance);

    // Something recognisable to look for afterwards.
    let _ = session.write(b"W \"NIT-MARKER\"\r");
    pump(&mut session, &mut grid, Duration::from_secs(3));
    assert!(
        grid.all_text().iter().any(|l| l.contains("NIT-MARKER")),
        "the marker never reached the screen"
    );

    let _ = session.write(b"W #\r");
    let raw = pump(&mut session, &mut grid, Duration::from_secs(3));
    println!("bytes for `W #`: {}", raw.escape_ascii());

    let screen = grid.screen_text().join("\n");
    let history = grid
        .scrollback
        .iter()
        .map(|r| r.to_text())
        .collect::<Vec<_>>();
    println!(
        "--- screen ---\n{screen}\n--- history ---\n{}",
        history.join("\n")
    );

    session.request_halt();

    assert!(
        !screen.contains("NIT-MARKER"),
        "`W #` did not clear the screen at all"
    );
    assert!(
        history.iter().any(|l| l.contains("NIT-MARKER")),
        "`W #` cleared the screen without filing it into the scrollback"
    );
}

/// The Ctrl+Delete gesture, end to end: wipe the grid, ask IRIS to clear, and
/// end up with an empty scrollback and a prompt IRIS agrees is at the top.
///
/// Doing it locally is what did not work: IRIS positions the cursor absolutely,
/// so a grid cleared behind its back left the next prompt painted back down at
/// the row it had reached.
#[test]
#[ignore = "needs a local IRIS instance"]
fn a_requested_clear_empties_the_terminal_and_moves_the_prompt_to_the_top() {
    let instance = test_instance().expect("no instance to test against");
    let (mut session, mut grid) = start(&instance);

    // Push the prompt well down the screen first.
    for _ in 0..8 {
        let _ = session.write(b"W \"NIT-FILLER\"\r");
        pump(&mut session, &mut grid, Duration::from_millis(400));
    }
    assert!(
        grid.cursor.row > 2,
        "the prompt never got far enough down the screen to be worth clearing"
    );

    // The gesture: ask IRIS for the clear, and drop the history when it comes.
    grid.purge_history_on_next_clear();
    let _ = session.write(b"W #\r");
    pump(&mut session, &mut grid, Duration::from_secs(3));

    // And a keystroke afterwards, which is where the old behaviour showed:
    // IRIS put the next prompt back at the row it still believed it was on.
    let _ = session.write(b"\r");
    pump(&mut session, &mut grid, Duration::from_secs(2));

    let screen = grid.screen_text().join("\n");
    println!("--- screen ---\n{screen}\n--------------");
    session.request_halt();

    assert!(
        grid.scrollback.is_empty(),
        "the clear left history behind: {:?}",
        grid.scrollback
            .iter()
            .map(|r| r.to_text())
            .collect::<Vec<_>>()
    );
    assert!(!screen.contains("NIT-FILLER"), "the screen was not cleared");
    // Two rows for the cleared prompt and the one the Enter produced under it -
    // not the row eight lines down that IRIS used to come back to.
    assert!(
        grid.cursor.row < 5,
        "the prompt came back at row {} instead of near the top",
        grid.cursor.row
    );
}
