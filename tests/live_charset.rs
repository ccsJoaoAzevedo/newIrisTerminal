//! Accented text, end to end, against a live instance.
//!
//! ```text
//! cargo test --test live_charset -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Everything about character encoding that can only be settled by measurement
//! lives here, in one test binary on purpose: with `--features plugins` each
//! integration test file links the whole wasmtime dependency graph into an
//! executable of its own, and CI's Linux runner ran out of room for another
//! one - `ld` died with a bus error.
//!
//! Two things are measured, and the design in `term::encoding` rests on both.
//!
//! **What the pseudo-console does.** A local session on Windows runs inside
//! one, and it is not a pipe: it decodes the child's bytes with its own
//! codepage and re-encodes them as UTF-8 for the terminal, then decodes the
//! terminal's UTF-8 and re-encodes it into that codepage for the child. So the
//! wire is UTF-8 whatever codepage the console is on - a raw high byte never
//! arrives, and no decoding on this side could rescue a local session - and
//! only a UTF-8 console carries a typed accent intact.
//!
//! **That one character is one column on both sides.** IRIS owns the read
//! buffer and never reports it, so recall, End and a rubout are all built by
//! counting the columns on screen and sending IRIS that many keys (see
//! `term::lineedit`). If a typed `ó` costs IRIS two characters and the terminal
//! one column, every one of those gestures is out by one per accent, and the
//! errors stack up as the line is walked over. The encoding this app shipped in
//! 0.3.0, `cp850-doubled`, did exactly that: it sent `├│` for an `ó` and folded
//! it back to one character for display, so walking the recall from `w "nó"` to
//! `k` left `COMP80>wk` on the line, and rubbing out a line with an accent in it
//! ran past the prompt and ate the `>`.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use new_iris_terminal::pty::launcher::{launcher, LaunchSpec};
use new_iris_terminal::pty::{PtySession, Session};
use new_iris_terminal::term::{lineedit, parser, Encoding, Grid};
use portable_pty::{CommandBuilder, PtySize};

fn instance_name() -> String {
    std::env::var("IRIS_TEST_INSTANCE")
        .ok()
        .filter(|s| !s.is_empty())
        .or_else(|| launcher().discover().into_iter().next().map(|i| i.name))
        .expect("no IRIS instance")
}

// ---------------------------------------------------------------------------
// Which decode renders this instance's output
// ---------------------------------------------------------------------------

/// The raw bytes of the login banner, through the app's own launch path.
fn banner() -> Vec<u8> {
    let spec = LaunchSpec {
        instance: instance_name(),
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
    assert_eq!(
        correct,
        vec![Encoding::Utf8],
        "a local session's wire must be UTF-8 and nothing else"
    );
}

// ---------------------------------------------------------------------------
// What each console codepage does to the bytes on the pipe
// ---------------------------------------------------------------------------

/// What one console codepage did to the session.
struct Measured {
    /// The banner line naming the configuration, as the terminal rendered it.
    banner: String,
    /// Whether the bytes on the pipe were valid UTF-8 from end to end.
    utf8_wire: bool,
    /// Whether any byte on the pipe could not have belonged to a UTF-8
    /// sequence - a codepage byte that escaped the console's re-encoding.
    raw_high_byte: bool,
    /// What `$ASCII` reported for a `ó` written to the pipe as UTF-8.
    typed_accent: String,
}

fn measure(codepage: &str) -> Measured {
    let instance = launcher()
        .discover()
        .into_iter()
        .next()
        .expect("no IRIS instance");
    let bin = instance.bin_dir.expect("no bin directory for the instance");

    let pair = portable_pty::native_pty_system()
        .openpty(PtySize {
            rows: 24,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("openpty");

    // The app's own launch line, with the codepage as the variable: see
    // `launcher::windows::session_command`.
    let mut cmd = CommandBuilder::new("cmd.exe");
    cmd.arg("/s");
    cmd.arg("/c");
    cmd.arg(format!(
        "chcp {codepage}>nul & irissession.exe {}",
        instance.name
    ));
    cmd.env(
        "PATH",
        match std::env::var("PATH") {
            Ok(existing) => format!("{};{existing}", bin.display()),
            Err(_) => bin.display().to_string(),
        },
    );
    cmd.env("TERM", "vt100");
    let mut child = pair.slave.spawn_command(cmd).expect("spawn");
    drop(pair.slave);

    let mut reader = pair.master.try_clone_reader().expect("reader");
    let (tx, rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                break;
            }
        }
    });
    let mut writer = pair.master.take_writer().expect("writer");

    let mut grid = Grid::new(80, 24, 500);
    let mut vte = vte::Parser::new();
    let mut raw: Vec<u8> = Vec::new();
    let pump = |ms: u64, grid: &mut Grid, vte: &mut vte::Parser, raw: &mut Vec<u8>| {
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            while let Ok(bytes) = rx.try_recv() {
                raw.extend_from_slice(&bytes);
                // No transcoding: the point is what the pipe carries.
                parser::advance(vte, grid, &bytes);
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    };

    pump(9000, &mut grid, &mut vte, &mut raw);
    let banner = grid
        .screen_text()
        .into_iter()
        .find(|l| l.contains("onfigura"))
        .unwrap_or_default()
        .trim()
        .to_string();

    // `ó` as UTF-8, which is what the terminal writes.
    let mut wire = b"s x=\"".to_vec();
    wire.extend_from_slice("ó".as_bytes());
    wire.extend_from_slice(b"\" w \"len=\",$L(x),\" code=\",$A(x,1),!\r");
    let _ = writer.write_all(&wire);
    let _ = writer.flush();
    pump(1800, &mut grid, &mut vte, &mut raw);
    let typed_accent = grid
        .screen_text()
        .into_iter()
        .rev()
        .find(|l| l.contains("len="))
        .unwrap_or_default()
        .trim()
        .to_string();

    let _ = writer.write_all(b"h\r");
    let _ = writer.flush();
    pump(600, &mut grid, &mut vte, &mut raw);
    let _ = child.kill();

    Measured {
        banner,
        utf8_wire: std::str::from_utf8(&raw).is_ok(),
        raw_high_byte: raw.iter().any(|b| *b >= 0x80 && !is_utf8_lead_or_cont(*b)),
        typed_accent,
    }
}

/// A byte that could belong to a UTF-8 sequence. Anything outside this and
/// ASCII is a codepage byte that reached the pipe untranslated.
fn is_utf8_lead_or_cont(byte: u8) -> bool {
    (0x80..=0xbf).contains(&byte) || (0xc2..=0xf4).contains(&byte)
}

/// The instance is assumed to speak UTF-8, which every current build does. So a
/// console on any other codepage mangles its output, and the two flavours are
/// worth seeing side by side: CP850 gives `N├│`, which is what this app once
/// tried to repair from this side, and Windows-1252 gives `NÃ³`.
#[test]
#[ignore = "needs a local IRIS instance"]
fn the_console_hands_the_terminal_utf8_whatever_codepage_it_is_on() {
    let utf8 = measure("65001");
    println!("[chcp 65001] banner        {:?}", utf8.banner);
    println!("[chcp 65001] typed accent  {:?}", utf8.typed_accent);

    // The one configuration that works, in both directions.
    assert!(
        utf8.banner.contains("Configuração"),
        "a UTF-8 console did not render the banner: {:?}",
        utf8.banner
    );
    assert!(
        utf8.utf8_wire,
        "a UTF-8 console put invalid UTF-8 on the pipe"
    );
    assert!(
        utf8.typed_accent.contains("len=1") && utf8.typed_accent.contains("code=243"),
        "a typed `ó` did not reach IRIS whole: {:?}",
        utf8.typed_accent
    );

    // And the two that do not, which is where the reported bug came from. The
    // console mangles the instance's output a different way each time - but in
    // both, what arrives is still UTF-8, and a typed accent is still lost
    // before IRIS sees it. Neither is something this side could decode its way
    // out of.
    for codepage in ["850", "1252"] {
        let m = measure(codepage);
        println!("[chcp {codepage}] banner        {:?}", m.banner);
        println!("[chcp {codepage}] typed accent  {:?}", m.typed_accent);

        assert!(m.utf8_wire, "chcp {codepage}: the pipe was not UTF-8");
        assert!(
            !m.raw_high_byte,
            "chcp {codepage}: a codepage byte reached the pipe untranslated"
        );
        assert!(
            !m.banner.contains("Configuração"),
            "chcp {codepage}: the banner came through clean, so the console \
             no longer re-encodes and this test has nothing to say: {:?}",
            m.banner
        );
        assert!(
            !m.typed_accent.contains("code=243"),
            "chcp {codepage}: a typed accent survived, so `chcp 65001` may no \
             longer be what makes typing work: {:?}",
            m.typed_accent
        );
    }
}

// ---------------------------------------------------------------------------
// The gestures that read the line back off the screen
// ---------------------------------------------------------------------------

/// A live session, driven the way the app drives one.
struct Prompt {
    session: Session,
    grid: Grid,
    vte: vte::Parser,
}

impl Prompt {
    fn open() -> Self {
        let spec = LaunchSpec {
            instance: instance_name(),
            ..LaunchSpec::default()
        };
        let mut prompt = Prompt {
            session: Session::Pty(
                PtySession::spawn(launcher().as_ref(), &spec, 80, 24).expect("spawn"),
            ),
            grid: Grid::new(80, 24, 500),
            vte: vte::Parser::new(),
        };
        prompt.pump(9000);
        assert!(
            lineedit::current(&prompt.grid).is_some(),
            "no prompt after the banner: {:?}",
            prompt.row()
        );
        prompt
    }

    fn pump(&mut self, ms: u64) {
        let deadline = Instant::now() + Duration::from_millis(ms);
        while Instant::now() < deadline {
            let (bytes, _) = self.session.drain();
            if !bytes.is_empty() {
                // The wire a local session speaks, which is the one the app
                // reads it through: `Profile::wire_encoding`.
                let decoded = Encoding::Utf8.decode(&bytes);
                let replies = parser::advance(&mut self.vte, &mut self.grid, &decoded);
                if !replies.is_empty() {
                    let _ = self.session.write(&replies);
                }
            }
            std::thread::sleep(Duration::from_millis(15));
        }
    }

    fn send(&mut self, bytes: &[u8], ms: u64) {
        let _ = self.session.write(bytes);
        self.pump(ms);
    }

    fn type_text(&mut self, text: &str, ms: u64) {
        let bytes = Encoding::Utf8.encode(text);
        self.send(&bytes, ms);
    }

    /// Runs a command and waits for the prompt it comes back to.
    fn run(&mut self, command: &str) {
        self.type_text(command, 300);
        self.send(b"\r", 900);
    }

    fn row(&self) -> String {
        self.grid.screen_text()[self.grid.cursor.row]
            .trim_end()
            .to_string()
    }

    fn line(&self) -> lineedit::LineEdit {
        lineedit::current(&self.grid).unwrap_or_else(|| {
            panic!(
                "the cursor is no longer on a command line: row {:?} col {}",
                self.row(),
                self.grid.cursor.col
            )
        })
    }

    /// The text typed at the prompt, as the app reads it back.
    fn typed(&self) -> String {
        lineedit::typed_text(&self.grid).unwrap_or_default()
    }

    /// The last line of output containing `needle`.
    fn reported(&self, needle: &str) -> String {
        self.grid
            .screen_text()
            .into_iter()
            .rev()
            .find(|l| l.contains(needle))
            .unwrap_or_default()
            .trim()
            .to_string()
    }

    /// One right arrow per column left, which is how the app sends End.
    fn press_end(&mut self) {
        let line = self.line();
        let mut wire = Vec::new();
        for _ in 0..line.end.saturating_sub(line.cursor) {
            wire.extend_from_slice(b"\x1b[C");
        }
        self.send(&wire, 500);
    }

    /// Exactly the wire `App::recall` builds: to the end of the line, a rubout
    /// per column, then the replacement.
    fn recall(&mut self, text: &str) {
        let line = self.line();
        let mut wire = Vec::new();
        for _ in 0..line.end.saturating_sub(line.cursor) {
            wire.extend_from_slice(b"\x1b[C");
        }
        wire.extend(std::iter::repeat_n(0x7f, line.len()));
        wire.extend_from_slice(&Encoding::Utf8.encode(text));
        self.send(&wire, 900);
    }

    /// Rubs out the whole line, a column at a time, as Backspace held down
    /// would. Each step is checked, so a drift is caught where it starts rather
    /// than after it has eaten the prompt.
    fn rub_out_line(&mut self) {
        let start = self.line().start;
        let mut expected = self.typed();
        while !expected.is_empty() {
            expected.pop();
            self.send(&[0x7f], 400);
            assert_eq!(
                self.line().start,
                start,
                "a rubout moved the prompt: row {:?}",
                self.row()
            );
            assert_eq!(
                self.typed().trim_end(),
                expected.trim_end(),
                "a rubout erased the wrong number of columns: row {:?}",
                self.row()
            );
        }
    }

    fn close(mut self) {
        for _ in 0..80 {
            let _ = self.session.write(&[0x7f]);
        }
        self.pump(400);
        self.session.request_halt();
    }
}

/// Everything a session has to get right about accented text, through the app's
/// own launch path: the banner arrives as Portuguese with no box-drawing debris,
/// the process reported is the session rather than the shell that set the
/// codepage, and a typed accent reaches IRIS as one character - `ó` is 243.
#[test]
#[ignore = "needs a local IRIS instance"]
fn a_session_speaks_utf8_end_to_end() {
    let mut p = Prompt::open();

    let screen = p.grid.screen_text().join("\n");
    assert!(
        screen.contains("Nó:") && screen.contains("Configuração:"),
        "the banner did not arrive as Portuguese:\n{screen}"
    );
    assert!(
        !screen.contains('├'),
        "the console is still re-encoding: the banner carries box-drawing debris"
    );
    if let Session::Pty(pty) = &p.session {
        assert!(pty.process_id().expect("a process id") > 0);
    }

    p.run(r#"s x="ó" w "len=",$L(x)," code=",$A(x,1),!"#);
    let reported = p.reported("len=");
    assert!(
        reported.contains("len=1") && reported.contains("code=243"),
        "IRIS did not receive one `ó`: {reported:?}"
    );
    p.close();
}

/// The reported bug. Walking the recall back and forth over a line with an
/// accent in it must replace the line exactly, however many times it is walked
/// past - not leave a character of the old line behind on each visit.
#[test]
#[ignore = "needs a local IRIS instance"]
fn walking_the_recall_over_an_accent_replaces_the_line_exactly() {
    let mut p = Prompt::open();

    // The history from the report, oldest first.
    let history = ["q", "k", r#"w "nó""#];
    for command in history {
        p.run(command);
    }

    // Up walks back through it, Down walks forward again.
    let back: Vec<&str> = history.iter().rev().copied().collect();
    for round in 1..=3 {
        for command in back.iter().chain(history.iter()) {
            p.recall(command);
            assert_eq!(
                p.typed(),
                *command,
                "round {round}: recalling {command:?} left {:?}",
                p.row()
            );
        }
    }

    p.rub_out_line();
    p.close();
}

/// The second half of the report: type into the middle of a line, walk to the
/// end, then hold Backspace. Every rubout has to erase exactly one column, or
/// the run walks off the front of the line and starts eating the prompt.
#[test]
#[ignore = "needs a local IRIS instance"]
fn rubbing_out_an_accent_typed_mid_line_stops_at_the_prompt() {
    let mut p = Prompt::open();

    // `w ""`, then back between the quotes and type the word there.
    p.type_text(r#"w """#, 500);
    p.send(b"\x1b[D", 400);
    p.type_text("nó", 600);
    assert_eq!(
        p.typed(),
        r#"w "nó""#,
        "the insert did not land: {:?}",
        p.row()
    );

    p.press_end();
    let line = p.line();
    assert!(
        line.at_end(),
        "End did not reach the end of the line: {line:?}"
    );

    p.rub_out_line();

    // The prompt itself is untouched, and one more rubout does not move it.
    let before = p.row();
    p.send(&[0x7f], 400);
    assert_eq!(p.row(), before, "a rubout on an empty line ate the prompt");
    p.close();
}
