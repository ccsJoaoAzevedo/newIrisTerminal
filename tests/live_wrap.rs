//! Reproduces the "long lines get cut off" report and shows where the loss is.
//!
//! ```text
//! cargo test --test live_wrap -- --ignored --nocapture --test-threads=1
//! ```
//!
//! The question this answers: when IRIS writes a line wider than the terminal,
//! does IRIS wrap it, does it truncate it, or does our grid drop the overflow?

use std::time::{Duration, Instant};

use new_iris_terminal::pty::launcher::{launcher, LaunchSpec};
use new_iris_terminal::pty::PtySession;
use new_iris_terminal::term::{parser, Encoding, Grid};

/// Runs one command at a given PTY size and returns (raw bytes, rendered grid).
fn run(cols: u16, rows: u16, command: &str) -> (Vec<u8>, Grid) {
    let instance = std::env::var("IRIS_TEST_INSTANCE")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| launcher().discover().into_iter().next().map(|i| i.name))
        .expect("no IRIS instance");

    let spec = LaunchSpec {
        instance,
        ..LaunchSpec::default()
    };
    let mut session = PtySession::spawn(launcher().as_ref(), &spec, cols, rows).expect("spawn");

    let mut grid = Grid::new(cols as usize, rows as usize, 2000);
    let mut vte = vte::Parser::new();
    let mut raw = Vec::new();

    let pump = |session: &mut PtySession,
                grid: &mut Grid,
                raw: &mut Vec<u8>,
                vte: &mut vte::Parser,
                until: Duration,
                want: &dyn Fn(&Grid) -> bool| {
        let deadline = Instant::now() + until;
        while Instant::now() < deadline {
            let (bytes, ended) = session.drain();
            if !bytes.is_empty() {
                raw.extend_from_slice(&bytes);
                let decoded = Encoding::Utf8.decode(&bytes);
                parser::advance(vte, grid, &decoded);
                if want(grid) {
                    return true;
                }
            }
            if ended {
                return false;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    };

    let at_prompt = |g: &Grid| {
        g.screen_text()
            .iter()
            .rfind(|l| !l.trim().is_empty())
            .map(|l| l.trim_end().ends_with('>'))
            .unwrap_or(false)
    };
    pump(
        &mut session,
        &mut grid,
        &mut raw,
        &mut vte,
        Duration::from_secs(20),
        &at_prompt,
    );

    raw.clear();
    session.write(command.as_bytes()).unwrap();
    session.write(b"\r").unwrap();
    pump(
        &mut session,
        &mut grid,
        &mut raw,
        &mut vte,
        Duration::from_secs(10),
        &|_| false,
    );

    session.request_halt();
    (raw, grid)
}

#[test]
#[ignore = "needs a local IRIS instance"]
fn a_line_wider_than_the_terminal_is_not_lost() {
    const WIDTH: u16 = 80;
    const LEN: usize = 300;

    // A repeating ruler so any truncation point is obvious.
    let command = format!("Write $Justify(\"\",{LEN})_\"END\",!");
    let (raw, grid) = run(WIDTH, 24, &command);

    // How many payload characters actually arrived, ignoring escapes.
    let text = String::from_utf8_lossy(&raw);
    println!("raw bytes: {}", raw.len());
    println!(
        "CR count: {}, LF count: {}",
        text.matches('\r').count(),
        text.matches('\n').count()
    );

    let all = grid.all_text().join("");
    println!("rendered non-blank chars: {}", all.trim().len());
    println!("grid contains END marker: {}", all.contains("END"));

    for (i, line) in grid.all_text().iter().enumerate() {
        if !line.trim().is_empty() {
            println!("{i:>3}: [{}] {}", line.len(), line);
        }
    }

    assert!(
        all.contains("END"),
        "the end of a {LEN}-char line never reached the grid at {WIDTH} columns"
    );
}

/// Is the terminal width the lever? If the same command yields more columns
/// at a wider PTY, then the cut is the device margin and the fix is to open
/// the session at the real window size instead of a hardcoded 80.
#[test]
#[ignore = "needs a local IRIS instance"]
fn wider_terminal_yields_more_output() {
    let command = "Set s=\"\" For i=1:1:52 { Set s=s_$Justify(i,4)_\" \" } Write s,!".to_string();

    let mut seen = Vec::new();
    for width in [80u16, 120, 200] {
        let (_, grid) = run(width, 24, &command);
        let joined = grid.all_text().join("");
        let last = (1..=52)
            .rev()
            .find(|i| joined.contains(&format!("{i:>4}")))
            .unwrap_or(0);
        let longest = grid.all_text().iter().map(|l| l.len()).max().unwrap_or(0);
        println!("width {width:>3}: last column = {last:>2}, longest rendered line = {longest}");
        seen.push((width, last));
    }

    let at80 = seen[0].1;
    let at200 = seen[2].1;
    assert!(
        at200 > at80,
        "widening the terminal changed nothing ({at80} -> {at200}); the cut is not the margin"
    );
}

#[test]
#[ignore = "needs a local IRIS instance"]
fn report_where_iris_breaks_a_long_line() {
    const WIDTH: u16 = 80;

    // 260 numbered columns: "0001 0002 ..." so we can see exactly which
    // column IRIS chose to break at, if it breaks at all.
    let command = "Set s=\"\" For i=1:1:52 { Set s=s_$Justify(i,4)_\" \" } Write s,!".to_string();
    let (raw, grid) = run(WIDTH, 24, &command);

    let text = String::from_utf8_lossy(&raw);
    // Where do line endings fall relative to the payload?
    let mut col = 0usize;
    let mut breaks = Vec::new();
    for ch in text.chars() {
        match ch {
            '\n' => {
                breaks.push(col);
                col = 0;
            }
            '\r' => col = 0,
            c if !c.is_control() => col += 1,
            _ => {}
        }
    }
    println!("payload column at each LF: {breaks:?}");
    println!("(a break at {WIDTH} means IRIS wrapped; none means it relies on the terminal)");

    let joined = grid.all_text().join("");
    println!(
        "last numbered column present in the grid: {}",
        (1..=52)
            .rev()
            .find(|i| joined.contains(&format!("{i:>4}")))
            .unwrap_or(0)
    );

    for line in grid.all_text().iter().filter(|l| !l.trim().is_empty()) {
        println!("[{}] {line}", line.len());
    }
}

/// Does a resize after login actually reach IRIS?
///
/// This decides the fix. If it does, opening at the window size plus normal
/// resize handling is enough. If it does not, the size at spawn is the only
/// one that ever matters and the session must be opened at the right size.
#[test]
#[ignore = "needs a local IRIS instance"]
fn resize_after_login_reaches_iris() {
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
    let mut grid = Grid::new(80, 24, 2000);
    let mut vte = vte::Parser::new();

    let pump = |session: &mut PtySession, grid: &mut Grid, vte: &mut vte::Parser, ms: u64| {
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            let (bytes, _) = session.drain();
            if !bytes.is_empty() {
                let decoded = Encoding::Utf8.decode(&bytes);
                parser::advance(vte, grid, &decoded);
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    };

    pump(&mut session, &mut grid, &mut vte, 4000);

    // Grow the terminal the way dragging the window would.
    session.resize(200, 40).expect("resize");
    grid.resize(200, 40);
    pump(&mut session, &mut grid, &mut vte, 1000);

    let command = "Set s=\"\" For i=1:1:52 { Set s=s_$Justify(i,4)_\" \" } Write s,!";
    session.write(command.as_bytes()).unwrap();
    session.write(b"\r").unwrap();
    pump(&mut session, &mut grid, &mut vte, 4000);

    let joined = grid.all_text().join("");
    let last = (1..=52)
        .rev()
        .find(|i| joined.contains(&format!("{i:>4}")))
        .unwrap_or(0);
    println!("after resizing 80 -> 200, last column reached: {last}");
    println!("(16 means the resize never reached IRIS; ~40 means it did)");

    session.request_halt();
    assert!(
        last > 20,
        "resize did not reach IRIS: still truncating at the original 80 columns"
    );
}
