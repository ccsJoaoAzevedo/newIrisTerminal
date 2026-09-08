//! Whether a shell plugin really opens a shell.
//!
//! Ignored by default — it starts a process on this machine. Run with:
//!
//! ```text
//! cargo test --test live_shell -- --ignored --nocapture
//! ```
//!
//! Harmless: it opens each shell in turn, asks it to echo one word, and kills
//! it. Nothing is written anywhere - the shell files themselves are written by
//! the app, not by this.
//!
//! The point of the test is that everything the app does with a shell it
//! already did with IRIS — a pseudo-terminal, a child, a reader thread — and
//! the only new part is the command. That part cannot be checked without
//! starting one: a wrong argument, or a `chcp` wrapper that swallows the
//! program, comes out as a tab that opens and immediately ends.

use std::time::{Duration, Instant};

use new_iris_terminal::plugins::shells;
use new_iris_terminal::pty::Session;
use new_iris_terminal::term::{parser, Grid};

/// Pumps for `window`, feeding everything into `grid`.
fn pump(session: &mut Session, grid: &mut Grid, window: Duration) -> String {
    let mut vte = vte::Parser::new();
    let deadline = Instant::now() + window;
    while Instant::now() < deadline {
        let (bytes, ended) = session.drain();
        if !bytes.is_empty() {
            let replies = parser::advance(&mut vte, grid, &bytes);
            if !replies.is_empty() {
                let _ = session.write(&replies);
            }
        }
        if ended {
            break;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    (0..grid.rows)
        .filter_map(|row| grid.screen.get(row))
        .map(|row| row.cells.iter().map(|cell| cell.ch).collect::<String>())
        .collect::<Vec<_>>()
        .join("\n")
}

/// What this machine offers, and that each entry is really there.
#[test]
#[ignore = "reports what this machine has"]
fn report_the_shells_this_machine_offers() {
    let found = shells::available();
    println!("{} shell(s):", found.len());
    for shell in &found {
        println!(
            "  {}  [{}]  {}",
            shell.name,
            shell.source_label(),
            shell.command_line()
        );
        println!("      from {}", shell.file.display());
    }
    println!("all of them declared in {}", shells::shells_dir().display());
    for shell in &found {
        assert!(
            shell.program.is_file(),
            "{} was offered but is not there",
            shell.program.display()
        );
    }
}

/// The one that matters: every shell opens, and every one of them says
/// something back.
///
/// Every one, not just the first, because that is the difference between
/// catching the bug this exists for and missing it: `cmd.exe` and `wsl.exe`
/// live in `System32` and started fine, while Git Bash lives under
/// `C:\Program Files\...` and did not. The command line inside `cmd /c` is one
/// `cmd` parses, and a path with a space in it is three words to it - so the
/// tab opened on `'C:\Program' is not recognized`. A test that stopped at the
/// first shell said everything was well.
#[test]
#[ignore = "starts a process per shell"]
fn every_shell_opens_and_answers() {
    let shells = shells::available();
    if shells.is_empty() {
        println!("no shells on this machine; nothing to open");
        return;
    }

    let mut failed: Vec<String> = Vec::new();
    for shell in &shells {
        println!("\n=== {} ({}) ===", shell.name, shell.command_line());
        let mut session = match Session::shell(&shell.program, &shell.args, 100, 30) {
            Ok(session) => session,
            Err(e) => {
                failed.push(format!("{}: would not start ({e:#})", shell.name));
                continue;
            }
        };
        let mut grid = Grid::new(100, 30, 200);

        // A word this test invented, so finding it on screen cannot be a
        // banner line or a prompt that happened to contain it. Echoed by every
        // shell there is, in this spelling.
        if let Err(e) = session.write_str("echo nit-shell-probe\r") {
            failed.push(format!("{}: could not be written to ({e:#})", shell.name));
            continue;
        }
        let screen = pump(&mut session, &mut grid, Duration::from_secs(6));
        println!("{}", screen.trim_end());
        session.request_halt();

        // Counted rather than merely found: the echo of what was typed is one,
        // and the shell's own output is the other. One is enough - a shell with
        // echo off is still a working shell - but zero means the line never ran.
        if screen.matches("nit-shell-probe").count() == 0 {
            failed.push(format!("{}: never echoed the probe", shell.name));
        }
    }

    assert!(
        failed.is_empty(),
        "{} of {} shell(s) did not work:\n  {}",
        failed.len(),
        shells.len(),
        failed.join("\n  ")
    );
}
