//! Integration tests that talk to a real IRIS instance.
//!
//! Ignored by default — they need an installed, running instance, so they must
//! not break `cargo test` on a machine (or CI runner) without one. Run with:
//!
//! ```text
//! cargo test --test live_session -- --ignored --nocapture
//! ```
//!
//! Set `IRIS_TEST_INSTANCE` to choose the instance; otherwise the first one
//! discovered is used.
//!
//! These tests only open a session and read the login banner. They never log
//! in and never write data — every `RDB*` database is shared with the team.

use std::time::{Duration, Instant};

use new_iris_terminal::pty::launcher::{launcher, LaunchSpec};
use new_iris_terminal::pty::PtySession;
use new_iris_terminal::term::{parser, Grid};

/// Pumps the session until `predicate` accepts the rendered screen, or the
/// timeout expires. Returns the final screen text either way.
fn read_until<F>(
    session: &mut PtySession,
    grid: &mut Grid,
    timeout: Duration,
    predicate: F,
) -> String
where
    F: Fn(&str) -> bool,
{
    let mut vte = vte::Parser::new();
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        let (bytes, ended) = session.drain();
        if !bytes.is_empty() {
            let replies = parser::advance(&mut vte, grid, &bytes);
            if !replies.is_empty() {
                let _ = session.write(&replies);
            }
            let screen = grid.screen_text().join("\n");
            if predicate(&screen) {
                return screen;
            }
        }
        if ended {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    grid.screen_text().join("\n")
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
fn discovery_finds_at_least_one_instance() {
    let found = launcher().discover();
    println!("discovered: {found:#?}");
    assert!(
        !found.is_empty(),
        "no IRIS instances discovered on this machine"
    );
}

#[test]
#[ignore = "needs a local IRIS instance"]
fn a_session_starts_and_produces_output() {
    let instance = test_instance().expect("no instance to test against");
    println!("using instance: {instance}");

    let spec = LaunchSpec {
        instance: instance.clone(),
        ..LaunchSpec::default()
    };

    let mut session = PtySession::spawn(launcher().as_ref(), &spec, 80, 24)
        .unwrap_or_else(|e| panic!("could not start a session for {instance}: {e:#}"));

    let mut grid = Grid::new(80, 24, 1000);
    // A login banner or a prompt both count as "IRIS is talking to us".
    let screen = read_until(&mut session, &mut grid, Duration::from_secs(20), |s| {
        !s.trim().is_empty()
    });

    println!("--- screen ---\n{screen}\n--------------");
    session.request_halt();

    assert!(
        !screen.trim().is_empty(),
        "session produced no output within 20s"
    );
}

#[test]
#[ignore = "needs a local IRIS instance"]
fn the_pty_reports_the_size_we_asked_for() {
    let instance = test_instance().expect("no instance to test against");
    let spec = LaunchSpec {
        instance,
        ..LaunchSpec::default()
    };

    let mut session =
        PtySession::spawn(launcher().as_ref(), &spec, 100, 30).expect("starting a session");

    assert_eq!(session.size(), (100, 30));
    session.resize(120, 40).expect("resizing");
    assert_eq!(session.size(), (120, 40));

    session.request_halt();
}
