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
