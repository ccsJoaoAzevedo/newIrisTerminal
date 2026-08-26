//! A diagnostic that reports what bytes the local IRIS instance actually
//! sends, so the correct default encoding is decided from evidence rather than
//! from how a console happened to render a test log.
//!
//! ```text
//! cargo test --test encoding_probe -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use new_iris_terminal::pty::launcher::{launcher, LaunchSpec};
use new_iris_terminal::pty::PtySession;
use new_iris_terminal::term::Encoding;

#[test]
#[ignore = "needs a local IRIS instance"]
fn report_the_raw_bytes_iris_sends() {
    let instance = std::env::var("IRIS_TEST_INSTANCE")
        .ok()
        .or_else(|| launcher().discover().into_iter().next().map(|i| i.name));
    let Some(instance) = instance else {
        panic!("no IRIS instance found");
    };

    let spec = LaunchSpec {
        instance,
        ..LaunchSpec::default()
    };
    let mut session = PtySession::spawn(launcher().as_ref(), &spec, 80, 24).expect("spawn");

    let mut raw = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(8);
    while Instant::now() < deadline {
        let (bytes, ended) = session.drain();
        raw.extend_from_slice(&bytes);
        if ended {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    session.request_halt();

    let high: Vec<u8> = raw.iter().copied().filter(|b| *b >= 0x80).collect();
    println!(
        "total bytes: {}, non-ASCII bytes: {}",
        raw.len(),
        high.len()
    );
    println!("non-ASCII byte values: {high:02x?}");

    // Show the bytes around each non-ASCII run, which is where the evidence is.
    for (i, &b) in raw.iter().enumerate() {
        if b >= 0x80 {
            let from = i.saturating_sub(6);
            let to = (i + 4).min(raw.len());
            println!(
                "  at {i}: {:02x?}  ascii={:?}",
                &raw[from..to],
                String::from_utf8_lossy(&raw[from..to])
            );
        }
    }

    println!(
        "--- valid UTF-8 as a whole? {} ---",
        std::str::from_utf8(&raw).is_ok()
    );
    for enc in Encoding::ALL {
        let decoded = String::from_utf8_lossy(&enc.decode(&raw)).into_owned();
        let sample: String = decoded
            .lines()
            .filter(|l| !l.trim().is_empty())
            .take(3)
            .collect::<Vec<_>>()
            .join(" | ");
        println!("{:>22}: {sample}", enc.label());
    }
}
