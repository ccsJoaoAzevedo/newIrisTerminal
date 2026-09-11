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
        let mut session = match Session::shell(&shell.program, &shell.args, None, 100, 30) {
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

/// What a typed `clear` or `cls` does to the transcript, and what the app's own
/// clear gesture does to the screen.
///
/// Both halves are the bug this exists for. A Windows pseudoconsole ends every
/// clear-screen it relays with `ESC [ 3 J`, and obeying that deleted the whole
/// session's history the moment anyone typed `cls`. The gesture, meanwhile,
/// used to send IRIS's `W #` at a shell, which is not a command any shell has.
///
/// The bytes the gesture sends are spelled out here rather than imported -
/// `clear_gesture` is private to the app - so they have to be kept in step with
/// [`new_iris_terminal::app`]. That is the point: this is the test that says
/// whether they still work on the shells this machine actually has.
#[test]
#[ignore = "starts a process per shell"]
fn clearing_a_shell_keeps_the_transcript_and_empties_the_screen() {
    let shells = shells::available();
    if shells.is_empty() {
        println!("no shells on this machine; nothing to clear");
        return;
    }

    let mut failed: Vec<String> = Vec::new();
    for shell in &shells {
        println!("\n=== {} ===", shell.name);
        let is_cmd = shell
            .program
            .file_stem()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case("cmd"));
        let Ok(mut session) = Session::shell(&shell.program, &shell.args, None, 80, 12) else {
            failed.push(format!("{}: would not start", shell.name));
            continue;
        };
        let mut grid = Grid::new(80, 12, 500);
        pump(&mut session, &mut grid, Duration::from_secs(4));

        // Something to lose: more lines than the screen holds, so the top of it
        // is already in the scrollback before anything is cleared.
        for n in 0..20 {
            let _ = session.write_str(&format!("echo nit-line-{n}\r"));
        }
        pump(&mut session, &mut grid, Duration::from_secs(6));
        let before = grid.scrollback.len();
        if before == 0 {
            // Nothing echoed twenty lines back, so this entry is not a command
            // interpreter sitting at a prompt - a plugin pointing at a
            // full-screen program is still a shell the menu can open, and
            // clearing it means nothing. `every_shell_opens_and_answers` is
            // what holds a real shell that has gone quiet to account.
            println!("  not a shell at a prompt; nothing to clear");
            session.request_halt();
            continue;
        }

        // Half one: the shell's own clear. The screen goes, the history stays.
        let _ = session.write_str(if is_cmd { "cls\r" } else { "clear\r" });
        pump(&mut session, &mut grid, Duration::from_secs(6));
        if grid.scrollback.len() < before {
            failed.push(format!(
                "{}: the clear ate the transcript ({} lines of history left of {before})",
                shell.name,
                grid.scrollback.len()
            ));
        }

        // Half two: the app's gesture, with something half typed to make sure
        // what it sends cannot be glued onto the end of it.
        let _ = session.write_str("echo half-typed");
        pump(&mut session, &mut grid, Duration::from_secs(3));
        grid.purge_history_on_next_clear();
        if is_cmd {
            // The Escape is a write of its own or the pseudoconsole reads it as
            // the start of a sequence and swallows it.
            let _ = session.write(b"\x1b");
            let _ = session.write(b"cls\r");
        } else {
            let _ = session.write(b"\x0c");
        }
        pump(&mut session, &mut grid, Duration::from_secs(6));
        session.request_halt();

        let screen: Vec<String> = grid
            .screen
            .iter()
            .map(|row| row.cells.iter().map(|cell| cell.ch).collect())
            .collect();
        println!("{}", screen.join("\n").trim_end());
        if screen.iter().any(|row| row.contains("nit-line-")) {
            failed.push(format!(
                "{}: the gesture left the old screen up",
                shell.name
            ));
        }
        // The prompt is painted near the top by the far side itself, which is
        // the whole reason the clear is asked of it rather than done here.
        if screen[..4].iter().all(|row| row.trim().is_empty()) {
            failed.push(format!(
                "{}: nothing came back after the gesture",
                shell.name
            ));
        }
    }

    assert!(
        failed.is_empty(),
        "{} of {} shell(s) cleared wrongly:\n  {}",
        failed.len(),
        shells.len(),
        failed.join("\n  ")
    );
}
