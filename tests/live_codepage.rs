//! What a pseudo-console's codepage does to the bytes on the pipe.
//!
//! ```text
//! cargo test --test live_codepage -- --ignored --nocapture --test-threads=1
//! ```
//!
//! This is the measurement the whole encoding design rests on, so it is worth
//! being able to run again rather than taking on trust. It opens the same
//! session three times with the console on three codepages and reports what
//! reaches the terminal and what reaches IRIS.
//!
//! Two facts come out of it, and together they say a local session's charset is
//! not a choice this app gets to make:
//!
//! * **The wire is always UTF-8.** Whatever codepage the console is on, it
//!   decodes the instance's bytes with that codepage and re-encodes them as
//!   UTF-8 for the terminal. A raw high byte never arrives. So no decoding on
//!   this side can rescue a local session - which is why `Encoding`'s
//!   single-byte codepages are a Telnet setting, where the socket really does
//!   carry the instance's own bytes.
//! * **Only a UTF-8 console carries a typed accent.** On the OEM codepage a
//!   typed `ó` reaches IRIS as `?`, character 63, whatever bytes are written
//!   for it. So `chcp 65001` is not a nicety in front of the session; it is the
//!   only configuration in which typing accented text works at all.
//!
//! The instance is assumed to speak UTF-8, which every current build does. A
//! console on any other codepage therefore mangles its output here, and the two
//! flavours of mangling are worth seeing side by side: CP850 gives `N├│`, which
//! is the `├│` this app once tried to repair from this side, and Windows-1252
//! gives `NÃ³`.

use std::io::{Read, Write};
use std::time::{Duration, Instant};

use new_iris_terminal::pty::launcher::launcher;
use new_iris_terminal::term::{parser, Grid};
use portable_pty::{CommandBuilder, PtySize};

/// What one console codepage did to the session.
struct Measured {
    /// The banner line naming the configuration, as the terminal rendered it.
    banner: String,
    /// Whether the bytes on the pipe were valid UTF-8 from end to end.
    utf8_wire: bool,
    /// Whether any byte on the pipe was a lone high byte - a codepage byte that
    /// escaped the console's re-encoding.
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
    // console mangles the instance's output in a different way each time - but
    // in both, what arrives is still UTF-8, and a typed accent is still lost
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
