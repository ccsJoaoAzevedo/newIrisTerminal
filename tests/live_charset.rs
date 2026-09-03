//! Establishes, from real bytes, how this instance mangles accented text and
//! which decode repairs it.
//!
//! ```text
//! cargo test --test live_charset -- --ignored --nocapture --test-threads=1
//! ```

use std::time::{Duration, Instant};

use new_iris_terminal::pty::launcher::{launcher, LaunchSpec};
use new_iris_terminal::pty::PtySession;
use new_iris_terminal::term::{parser, Encoding, Grid};

fn banner() -> Vec<u8> {
    let instance = std::env::var("IRIS_TEST_INSTANCE")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| launcher().discover().into_iter().next().map(|i| i.name))
        .expect("no IRIS instance");

    let spec = LaunchSpec {
        instance,
        ..LaunchSpec::default()
    };
    let mut session = PtySession::spawn(launcher().as_ref(), &spec, 80, 24).expect("spawn");

    let mut raw = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let (bytes, ended) = session.drain();
        raw.extend_from_slice(&bytes);
        if ended {
            break;
        }
        if raw.windows(1).any(|w| w == b">") && !bytes.is_empty() {
            // Give it a beat to finish the banner.
            std::thread::sleep(Duration::from_millis(300));
            let (more, _) = session.drain();
            raw.extend_from_slice(&more);
            break;
        }
        std::thread::sleep(Duration::from_millis(30));
    }
    session.request_halt();
    raw
}

fn render(raw: &[u8], encoding: Encoding) -> String {
    let mut grid = Grid::new(80, 24, 100);
    let mut vte = vte::Parser::new();
    parser::advance(&mut vte, &mut grid, &encoding.decode(raw));
    grid.screen_text().join("\n")
}

#[test]
#[ignore = "needs a local IRIS instance"]
fn identify_the_encoding_that_renders_portuguese_correctly() {
    let raw = banner();
    assert!(!raw.is_empty(), "no banner captured");

    println!(
        "valid UTF-8 as a whole: {}",
        std::str::from_utf8(&raw).is_ok()
    );

    for enc in Encoding::ALL {
        let text = render(&raw, enc);
        let line = text
            .lines()
            .find(|l| l.contains("CCDESNOT") || l.contains(':'))
            .unwrap_or("")
            .trim()
            .to_string();
        println!("{:>26} : {line}", enc.label());
    }

    // The banner reads "Nó: <node>, Configuração: <instance>". Exactly one
    // decode should produce real Portuguese rather than box-drawing debris.
    let correct: Vec<_> = Encoding::ALL
        .into_iter()
        .filter(|enc| {
            let text = render(&raw, *enc);
            text.contains("Nó") && text.contains("Configuração")
        })
        .collect();

    println!("\nencodings that render it correctly: {correct:?}");
    assert!(
        !correct.is_empty(),
        "no available encoding renders the banner correctly"
    );
}

/// Everything a session has to get right about accented text, end to end,
/// through the app's own launch path.
///
/// This is the test that would have caught all three faces of one bug. A
/// Windows pseudo-console starts on the machine's OEM codepage and re-encodes
/// whatever crosses it: the banner arrived as `N├│`, a typed `ó` reached IRIS
/// as `?`, and the accent cost a column that only IRIS counted - so every
/// repaint of a recalled line landed one column to the right and left a
/// character of the old line behind, stacking up an `s` per visit. Opening the
/// session with the console in UTF-8 settles all three at once.
#[test]
#[ignore = "needs a local IRIS instance"]
fn a_session_speaks_utf8_end_to_end() {
    use new_iris_terminal::pty::Session;
    use new_iris_terminal::term::lineedit;

    let instance = std::env::var("IRIS_TEST_INSTANCE")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| launcher().discover().into_iter().next().map(|i| i.name))
        .expect("no IRIS instance");
    let spec = LaunchSpec {
        instance,
        ..LaunchSpec::default()
    };
    let mut session =
        Session::Pty(PtySession::spawn(launcher().as_ref(), &spec, 80, 24).expect("spawn"));

    let mut grid = Grid::new(80, 24, 500);
    let mut vte = vte::Parser::new();
    let mut raw: Vec<u8> = Vec::new();

    macro_rules! pump {
        ($ms:expr) => {{
            let deadline = Instant::now() + Duration::from_millis($ms);
            while Instant::now() < deadline {
                let (bytes, _) = session.drain();
                if !bytes.is_empty() {
                    raw.extend_from_slice(&bytes);
                    // The default encoding, which is no transcoding at all.
                    let decoded = Encoding::default().decode(&bytes);
                    let replies = parser::advance(&mut vte, &mut grid, &decoded);
                    if !replies.is_empty() {
                        let _ = session.write(&replies);
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }};
    }
    let row = |grid: &Grid| grid.screen_text()[grid.cursor.row].trim_end().to_string();
    let send = |session: &Session, text: &str| {
        let _ = session.write(&Encoding::default().encode(text));
    };

    pump!(9000);
    let screen = grid.screen_text().join("\n");
    assert!(
        screen.contains("Nó:") && screen.contains("Configuração:"),
        "the banner did not arrive as Portuguese:\n{screen}"
    );
    assert!(
        !raw.windows(3).any(|w| w == [0xe2, 0x94, 0x9c]),
        "the console is still re-encoding: the banner carries box-drawing debris"
    );

    // The process reported is the session, not the shell that set the codepage.
    if let Session::Pty(pty) = &session {
        let pid = pty.process_id().expect("a process id");
        assert!(pid > 0);
    }

    // A typed accent reaches IRIS as one character, and as the right one.
    send(
        &session,
        "s x=\"ó\" w \"len=\",$L(x),\" code=\",$A(x,1),!\r",
    );
    pump!(1500);
    let reported = grid
        .screen_text()
        .into_iter()
        .rev()
        .find(|l| l.contains("len="))
        .unwrap_or_default();
    assert!(
        reported.contains("len=1") && reported.contains("code=243"),
        "IRIS did not receive the accent: {reported:?}"
    );

    // And a recall over an accented line replaces it exactly, however many
    // times it is walked past. The wire is the one `App::recall` builds.
    let accented = r#"set a="nó""#;
    send(&session, accented);
    send(&session, "\r");
    pump!(900);
    send(&session, "set b=1");
    send(&session, "\r");
    pump!(900);

    for round in 1..=3 {
        for command in [accented, "set b=1"] {
            let line = lineedit::current(&grid).expect("a command line");
            let mut wire: Vec<u8> = Vec::new();
            wire.extend(std::iter::repeat_n(0x7f, line.len()));
            wire.extend_from_slice(&Encoding::default().encode(command));
            let _ = session.write(&wire);
            pump!(700);
            let shown = row(&grid);
            assert!(
                shown.ends_with(command) && !shown.contains(&format!("s{command}")),
                "round {round}: recalling {command:?} left {shown:?}"
            );
        }
    }

    // Leave the prompt as it was found.
    for _ in 0..60 {
        let _ = session.write(&[0x7f]);
    }
    pump!(400);
    session.request_halt();
}
