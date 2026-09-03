//! Accented text through the gestures that read the line back off the screen.
//!
//! ```text
//! cargo test --test live_accents -- --ignored --nocapture --test-threads=1
//! ```
//!
//! IRIS owns the read buffer and never reports it, so recall, End and a rubout
//! are all built by counting the columns on screen and sending IRIS that many
//! keys (see `term::lineedit`). Which makes one column per character the whole
//! contract: if a typed `ó` costs IRIS two characters and the terminal one
//! column, every one of those gestures is out by one per accent, and the errors
//! stack up as the line is walked over.
//!
//! That is the bug these tests pin. The encoding this app shipped in 0.3.0,
//! `cp850-doubled`, sent `├│` for an `ó` and folded it back to one character
//! for display: walking the recall from `w "nó"` to `k` left `COMP80>wk` on
//! the line, and rubbing out a line with an accent in it ran past the prompt
//! and ate the `>`.

use std::time::{Duration, Instant};

use new_iris_terminal::pty::launcher::{launcher, LaunchSpec};
use new_iris_terminal::pty::{PtySession, Session};
use new_iris_terminal::term::{lineedit, parser, Encoding, Grid};

/// A live session, driven the way the app drives one.
struct Prompt {
    session: Session,
    grid: Grid,
    vte: vte::Parser,
}

impl Prompt {
    fn open() -> Self {
        let instance = std::env::var("IRIS_TEST_INSTANCE")
            .ok()
            .filter(|s| !s.is_empty())
            .or_else(|| launcher().discover().into_iter().next().map(|i| i.name))
            .expect("no IRIS instance");
        let spec = LaunchSpec {
            instance,
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

    /// Exactly the wire [`App::recall`] builds: to the end of the line, a
    /// rubout per column, then the replacement.
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
    /// would. Each step is checked, so a drift is caught where it starts
    /// rather than after it has eaten the prompt.
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

/// What IRIS receives for a typed accent, which is where everything else
/// follows from: one character, and the right one. `ó` is 243.
#[test]
#[ignore = "needs a local IRIS instance"]
fn a_typed_accent_reaches_iris_as_one_character() {
    let mut p = Prompt::open();
    p.run(r#"s x="ó" w "len=",$L(x)," code=",$A(x,1),!"#);
    let reported = p
        .grid
        .screen_text()
        .into_iter()
        .rev()
        .find(|l| l.contains("len="))
        .unwrap_or_default();
    assert!(
        reported.contains("len=1") && reported.contains("code=243"),
        "IRIS did not receive one `ó`: {reported:?}"
    );
    p.close();
}

/// The reported bug. Walking the recall back and forth over a line with an
/// accent in it must replace the line exactly, however many times it is walked
/// past - not leave a character of the old line behind each visit.
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
    let forward: Vec<&str> = history.to_vec();
    for round in 1..=3 {
        for command in back.iter().chain(forward.iter()) {
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

    // End, the way the app sends it: one right arrow per column left.
    let line = p.line();
    let mut wire = Vec::new();
    for _ in 0..line.end.saturating_sub(line.cursor) {
        wire.extend_from_slice(b"\x1b[C");
    }
    p.send(&wire, 500);
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
