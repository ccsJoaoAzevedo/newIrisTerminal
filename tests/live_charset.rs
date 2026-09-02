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

/// What IRIS actually receives when the terminal types an accented character.
///
/// The output half of this is understood - the banner arrives double-encoded,
/// because the pseudo-console converts the instance's UTF-8 to the console
/// codepage and back. The input half has to travel the same road in reverse,
/// and this is the probe that says which spelling comes out the other end as
/// the character the user pressed.
///
/// `$L` and `$A` are the evidence: for a correctly received `ó`, IRIS reports a
/// length of 1 and character code 243. Two characters, or 195, means it got the
/// raw UTF-8 bytes instead.
#[test]
#[ignore = "needs a local IRIS instance"]
fn report_what_iris_receives_for_a_typed_accent() {
    use new_iris_terminal::pty::Session;

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
    let pump = |session: &mut Session, grid: &mut Grid, vte: &mut vte::Parser, ms: u64| {
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            let (bytes, _) = session.drain();
            if !bytes.is_empty() {
                let replies = parser::advance(vte, grid, &Encoding::Cp850Doubled.decode(&bytes));
                if !replies.is_empty() {
                    let _ = session.write(&replies);
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    };
    pump(&mut session, &mut grid, &mut vte, 4000);

    // `ó` on its own: one character IRIS can report the code of.
    let text = "ó";
    let candidates: [(&str, Vec<u8>); 4] = [
        // What the app itself now sends.
        ("app encode", Encoding::Cp850Doubled.encode(text)),
        ("plain UTF-8", text.as_bytes().to_vec()),
        // Each byte IRIS should receive, carried as the CP850 glyph the
        // pseudo-console will turn back into that byte.
        ("CP850-doubled", Encoding::Cp850.decode(text.as_bytes())),
        ("raw CP850", Encoding::Cp850.encode(text)),
    ];
    assert_eq!(
        Encoding::Cp850Doubled.encode(text),
        Encoding::Cp850.decode(text.as_bytes()),
        "the app's encode is the mirror of the repair"
    );

    for (name, bytes) in candidates {
        // Read-only: a length and a character code, nothing written anywhere.
        let _ = session.write(b"s x=\"");
        let _ = session.write(&bytes);
        let _ = session.write(b"\" w \"len=\",$L(x),\" code=\",$A(x,1),!\r");
        pump(&mut session, &mut grid, &mut vte, 1500);
        let line = grid
            .screen_text()
            .into_iter()
            .rev()
            .find(|l| l.contains("len="))
            .unwrap_or_default();
        println!("{name:>14}: {}", line.trim());
    }

    // A whole word, through the same path the keyboard and a paste take: what
    // IRIS writes back has to read as what was typed.
    let word = "Configuração não";
    let _ = session.write(b"w \"");
    let _ = session.write(&Encoding::Cp850Doubled.encode(word));
    let _ = session.write(b"\",!\r");
    pump(&mut session, &mut grid, &mut vte, 1500);
    let echoed = grid
        .screen_text()
        .into_iter()
        .rev()
        .find(|l| l.trim().starts_with(word))
        .unwrap_or_default();
    println!("{:>14}: {}", "written back", echoed.trim());

    println!("\n--- screen ---");
    for line in grid.screen_text() {
        if !line.trim().is_empty() {
            println!("{}", line.trim_end());
        }
    }
    session.request_halt();
}
