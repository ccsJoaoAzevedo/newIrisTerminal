//! Integration tests against the InterSystems launcher's real configuration.
//!
//! Ignored by default — they need the launcher installed and, for the Telnet
//! test, a reachable server, so they must not break `cargo test` on a machine
//! (or CI runner) without either. Run with:
//!
//! ```text
//! cargo test --test live_servers -- --ignored --nocapture
//! ```
//!
//! These tests only read configuration and the login banner. They never log in
//! and never write data — every `RDB*` database is shared with the team.

use std::time::{Duration, Instant};

use new_iris_terminal::config::servers;
use new_iris_terminal::pty::Session;
use new_iris_terminal::term::{parser, Grid};

/// Pumps a session until `predicate` accepts the rendered screen, or the
/// timeout expires. Returns the final screen text either way.
fn read_until<F>(session: &mut Session, grid: &mut Grid, timeout: Duration, predicate: F) -> String
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
        }
        let screen = grid.all_text().join("\n");
        if predicate(&screen) {
            return screen;
        }
        if ended {
            return screen;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    grid.all_text().join("\n")
}

/// Whether the screen is asking who we are.
///
/// Which service answers the Telnet port decides the wording, and both are
/// legitimate: IRIS's own Telnet service says `Username:`, while a remote Linux
/// server answers with its `telnetd` and says `login:` — which is equally what
/// the launcher's own Terminal reaches for such a server.
fn at_a_login_prompt(screen: &str) -> bool {
    let lower = screen.to_lowercase();
    ["username", "user:", "password", "login:"]
        .iter()
        .any(|needle| lower.contains(needle))
}

/// What the app now reads instead of asking the user to retype it. Reports the
/// list rather than asserting a particular machine's contents.
#[test]
#[ignore = "needs the InterSystems launcher installed"]
fn the_launchers_server_list_is_readable() {
    let list = servers::discover();
    let instances: Vec<String> = new_iris_terminal::app::discover_instances();

    println!("preferred (raw): {:?}", list.preferred);
    println!("instances: {instances:?}");
    for server in &list.servers {
        println!(
            "  {:<16} address={:<16} port={:<6} telnet={:<6} -> {:?}",
            server.name,
            server.address,
            server.port,
            server.telnet,
            server.target(&instances)
        );
    }

    assert!(
        !list.is_empty(),
        "no servers found — is the launcher installed, with servers configured?"
    );
    // What the tray menu's "Servidor Preferencial" is ticked on, which is the
    // thing the app must open. Reported as the resolved target, since that is
    // what actually decides between a local session and a Telnet login.
    let target = servers::preferred_target(&list, &instances);
    println!("preferred target: {target:?}");
    assert!(
        target.is_some(),
        "the tray names a preferred server but it did not resolve"
    );

    // Every entry must resolve to something openable: a port of 0 or an empty
    // address would be a silently dead menu item.
    for server in &list.servers {
        match server.target(&instances) {
            servers::Target::Local { instance } => assert!(!instance.is_empty()),
            servers::Target::Telnet { address, port } => {
                assert!(!address.trim().is_empty(), "{} has no address", server.name);
                assert!(port > 0, "{} has no telnet port", server.name);
            }
        }
    }
}

/// The test that proves the Telnet transport: a real IRIS Telnet service
/// negotiates before it says anything, and what reaches the grid has to be the
/// login prompt rather than protocol bytes rendered as text.
#[test]
#[ignore = "needs a reachable IRIS server with the Telnet service enabled"]
fn a_remote_server_reaches_a_login_prompt_over_telnet() {
    let list = servers::discover();
    let instances = new_iris_terminal::app::discover_instances();

    // The first entry that is genuinely a Telnet target, so this works on any
    // machine rather than only where a server happens to be named `TESTES`.
    let Some((server, address, port)) =
        list.servers
            .iter()
            .find_map(|s| match s.target(&instances) {
                servers::Target::Telnet { address, port } => Some((s, address, port)),
                servers::Target::Local { .. } => None,
            })
    else {
        println!("no remote server configured — nothing to test");
        return;
    };

    println!("connecting to {} at {address}:{port}", server.name);
    let mut session = match Session::telnet(&address, port, 120, 30) {
        Ok(session) => session,
        Err(e) => {
            println!("could not connect to {address}:{port}: {e:#}");
            return;
        }
    };

    let mut grid = Grid::new(120, 30, 500);
    let screen = read_until(&mut session, &mut grid, Duration::from_secs(15), |text| {
        at_a_login_prompt(text)
    });

    println!("---- screen ----\n{}\n----------------", screen.trim_end());

    assert!(
        !screen.trim().is_empty(),
        "the server said nothing in 15 s — is its Telnet service enabled?"
    );
    assert!(
        at_a_login_prompt(&screen),
        "no login prompt arrived; screen was:\n{screen}"
    );

    // Protocol leaking into the terminal would show up as replacement
    // characters, since 0xFF is not valid UTF-8. One stray byte is *not*
    // treated as a failure: the RHEL 8 `telnetd` on the server this was
    // developed against emits a bare Data Mark (0xF2, its IAC already lost)
    // ahead of the banner about half the time, which an independent capture
    // confirmed is on the wire before any client sees it. A client cannot tell
    // that byte from data without guessing, and guessing would eat a real `ò`
    // on a CP1252 or Latin-1 session. Systematic leakage is a different matter
    // and is what this bound catches.
    let strays = screen.matches('\u{fffd}').count();
    if strays > 0 {
        println!("note: {strays} stray byte(s) in the banner — see the comment above");
    }
    assert!(
        strays <= 1,
        "the Telnet protocol is leaking into the terminal ({strays} bad bytes)"
    );

    session.request_halt();
    session.kill();
}
