//! Where the time goes when opening a session.
//!
//! ```text
//! cargo test --test live_timing -- --ignored --nocapture --test-threads=1
//! ```

use std::time::{Duration, Instant};

use new_iris_terminal::pty::launcher::{launcher, IrisLauncher, LaunchSpec};
use new_iris_terminal::pty::PtySession;
use new_iris_terminal::term::{parser, Grid};

fn instance() -> String {
    std::env::var("IRIS_TEST_INSTANCE")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| launcher().discover().into_iter().next().map(|i| i.name))
        .expect("no IRIS instance")
}

/// Spawns a session and returns (build-command, spawn, first-byte, to-prompt).
fn timed_open(name: &str, l: &dyn IrisLauncher) -> (u128, u128, u128, u128) {
    let spec = LaunchSpec {
        instance: name.to_string(),
        ..LaunchSpec::default()
    };

    let t0 = Instant::now();
    let _cmd = l.command(&spec).expect("build command");
    let build = t0.elapsed().as_millis();

    let t1 = Instant::now();
    let mut session = PtySession::spawn(l, &spec, 80, 24).expect("spawn");
    let spawn = t1.elapsed().as_millis();

    let mut grid = Grid::new(80, 24, 200);
    let mut vte = vte::Parser::new();
    let t2 = Instant::now();
    let mut first_byte = 0;

    let deadline = Instant::now() + Duration::from_secs(25);
    let mut to_prompt = 0;
    while Instant::now() < deadline {
        let (bytes, ended) = session.drain();
        if !bytes.is_empty() {
            if first_byte == 0 {
                first_byte = t2.elapsed().as_millis();
            }
            parser::advance(&mut vte, &mut grid, &bytes);
            let screen = grid.screen_text().join("\n");
            if screen
                .lines()
                .rfind(|l| !l.trim().is_empty())
                .map(|l| l.trim_end().ends_with('>'))
                .unwrap_or(false)
            {
                to_prompt = t2.elapsed().as_millis();
                break;
            }
        }
        if ended {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }

    session.request_halt();
    (build, spawn, first_byte, to_prompt)
}

#[test]
#[ignore = "needs a local IRIS instance"]
fn report_session_open_timings() {
    let name = instance();

    let t = Instant::now();
    let l = launcher();
    println!("launcher() construction: {} ms", t.elapsed().as_millis());

    let t = Instant::now();
    let found = l.discover();
    println!(
        "discover() [runs `iris list`]: {} ms -> {:?}",
        t.elapsed().as_millis(),
        found.iter().map(|i| &i.name).collect::<Vec<_>>()
    );

    println!(
        "\n{:<10} {:>8} {:>8} {:>11} {:>10}",
        "session", "build", "spawn", "first-byte", "to-prompt"
    );
    for n in 1..=3 {
        // A fresh launcher each time, exactly as `Tab::start` does today.
        let l = launcher();
        let (build, spawn, first, prompt) = timed_open(&name, l.as_ref());
        println!("{n:<10} {build:>6} ms {spawn:>6} ms {first:>9} ms {prompt:>8} ms");
    }
}
