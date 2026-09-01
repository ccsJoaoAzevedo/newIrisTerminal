//! Reaching a remote IRIS server over Telnet.
//!
//! A local instance is started as a child process on a pseudo-terminal; a
//! remote server has no process here to start, and the launcher's own Terminal
//! reaches it exactly the way this does — by opening the instance's Telnet
//! service, which is the port the Server Manager records beside the address.
//!
//! IRIS runs a real Telnet server, not a raw socket, so the option protocol has
//! to be spoken: both instances tested opened with negotiation before a single
//! byte of the login prompt (`IAC WILL SGA` from one, four `IAC DO`s from the
//! other). [`Codec`] is that layer and nothing more — it answers what it must,
//! refuses what it does not implement, and hands the terminal a clean byte
//! stream indistinguishable from a PTY's.
//!
//! Deliberately split in two: [`Codec`] is pure and testable against captured
//! bytes, and [`TelnetSession`] is the socket and thread around it.

use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use crossbeam_channel::Receiver;

use super::session::SessionEvent;

// ---- the protocol ------------------------------------------------------

const IAC: u8 = 255;
const SE: u8 = 240;
const SB: u8 = 250;
const WILL: u8 = 251;
const WONT: u8 = 252;
const DO: u8 = 253;
const DONT: u8 = 254;

const OPT_BINARY: u8 = 0;
const OPT_ECHO: u8 = 1;
const OPT_SGA: u8 = 3;
const OPT_TTYPE: u8 = 24;
const OPT_NAWS: u8 = 31;

/// Sub-negotiation command: the server asking for the terminal type.
const TTYPE_SEND: u8 = 1;
/// Sub-negotiation command: our answer.
const TTYPE_IS: u8 = 0;

/// What we tell the server we are.
///
/// The same claim the PTY path makes through `TERM`, and for the same reason:
/// the grid implements a VT102-compatible subset, and claiming `xterm` would
/// invite mouse and alt-screen sequences it does not model.
const TERMINAL_TYPE: &[u8] = b"vt100";

/// Options we will turn on when asked.
///
/// `BINARY` and `SGA` keep the stream 8-bit clean and character-at-a-time,
/// which is what an interactive terminal needs; `TTYPE` and `NAWS` are how the
/// far side learns what it is talking to and how big it is. Everything else —
/// terminal speed, X display location, the environment — is refused: we have no
/// answer for it, and a refusal ends the exchange where a silence would leave
/// the server waiting.
const SUPPORTED: [u8; 4] = [OPT_BINARY, OPT_SGA, OPT_TTYPE, OPT_NAWS];

#[derive(Clone, Copy, PartialEq, Eq)]
enum State {
    Data,
    Iac,
    /// After IAC WILL/WONT/DO/DONT, holding which one.
    Negotiating(u8),
    /// Inside IAC SB ... IAC SE.
    Subnegotiation,
    /// An IAC inside a sub-negotiation, which may be the closing IAC SE.
    SubnegotiationIac,
}

/// The Telnet option layer: strips the protocol out of the incoming stream,
/// produces the replies it owes, and escapes outgoing data.
pub struct Codec {
    state: State,
    /// Bytes of the sub-negotiation currently being collected, option first.
    sb: Vec<u8>,
    /// Whether the server agreed to 8-bit data in our direction. Until it does,
    /// NVT rules apply and a bare CR has to be sent as CR NUL.
    binary_out: bool,
    /// Whether the server asked for window sizes. Sending them unasked is a
    /// protocol error, so a resize is silent until this is set.
    naws: bool,
    /// Set when the previous data byte was CR, so the NUL that NVT requires
    /// after it can be dropped rather than printed into the grid.
    after_cr: bool,
    /// Last size we were told to advertise, so agreeing to NAWS mid-session
    /// still reports the real window rather than waiting for a resize.
    size: (u16, u16),
    /// Options we have told the server we are doing, and options we have told
    /// it to do. Indexed by option code.
    ///
    /// RFC 854 requires a negotiation that would not change anything to go
    /// unanswered: acknowledging a state already held is how two well-behaved
    /// implementations talk past each other forever. It also matters in
    /// practice — a Linux `telnetd` answers a redundant request by flushing its
    /// output with a SYNCH, and a SYNCH's Data Mark lands in the middle of the
    /// login banner.
    us: [bool; 256],
    them: [bool; 256],
}

impl Default for Codec {
    fn default() -> Self {
        Codec {
            state: State::Data,
            sb: Vec::new(),
            binary_out: false,
            naws: false,
            after_cr: false,
            size: (80, 24),
            us: [false; 256],
            them: [false; 256],
        }
    }
}

/// What [`Codec::feed`] pulled out of one chunk from the socket.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Decoded {
    /// Terminal data, protocol removed. Goes to the parser.
    pub data: Vec<u8>,
    /// Protocol bytes owed back to the server. Must be written before the next
    /// chunk is processed, or the far side sits waiting for an answer.
    pub replies: Vec<u8>,
}

impl Codec {
    /// The opening offer, sent before anything is read.
    ///
    /// Announcing rather than waiting: one of the two instances tested says
    /// nothing about the terminal type at all, and a session that never
    /// advertises `vt100` gets IRIS's dumb-terminal behaviour.
    pub fn greeting(&mut self) -> Vec<u8> {
        let mut out = Vec::new();
        for option in [OPT_TTYPE, OPT_NAWS] {
            self.us[option as usize] = true;
            out.extend_from_slice(&[IAC, WILL, option]);
        }
        // IRIS and a Unix login both echo what is typed; the terminal must not
        // do it a second time.
        for option in [OPT_SGA, OPT_ECHO] {
            self.them[option as usize] = true;
            out.extend_from_slice(&[IAC, DO, option]);
        }
        out
    }

    /// Remembers the window size and, once the server has asked for sizes,
    /// produces the sub-negotiation reporting it.
    pub fn set_size(&mut self, cols: u16, rows: u16) -> Vec<u8> {
        self.size = (cols, rows);
        self.naws_report()
    }

    fn naws_report(&self) -> Vec<u8> {
        if !self.naws {
            return Vec::new();
        }
        let (cols, rows) = self.size;
        let mut out = vec![IAC, SB, OPT_NAWS];
        // Each byte of the size is data, so a 255 in it needs escaping like any
        // other — a 255-column window is unlikely but it is not impossible.
        for byte in [
            (cols >> 8) as u8,
            (cols & 0xff) as u8,
            (rows >> 8) as u8,
            (rows & 0xff) as u8,
        ] {
            out.push(byte);
            if byte == IAC {
                out.push(IAC);
            }
        }
        out.extend_from_slice(&[IAC, SE]);
        out
    }

    /// Escapes terminal data for the wire.
    pub fn encode(&self, data: &[u8]) -> Vec<u8> {
        let mut out = Vec::with_capacity(data.len());
        for (i, &byte) in data.iter().enumerate() {
            match byte {
                // A literal 255 is always doubled, or the server reads it as
                // the start of a command.
                IAC => out.extend_from_slice(&[IAC, IAC]),
                // Outside binary mode, NVT says a CR that does not begin a
                // CR LF must be followed by NUL. IRIS's Enter is a bare CR, so
                // without this every line ends ambiguously.
                b'\r' if !self.binary_out => {
                    out.push(b'\r');
                    if data.get(i + 1) != Some(&b'\n') {
                        out.push(0);
                    }
                }
                _ => out.push(byte),
            }
        }
        out
    }

    /// Runs one chunk from the socket through the protocol.
    pub fn feed(&mut self, incoming: &[u8]) -> Decoded {
        let mut out = Decoded {
            data: Vec::with_capacity(incoming.len()),
            replies: Vec::new(),
        };

        for &byte in incoming {
            match self.state {
                State::Data => {
                    if byte == IAC {
                        self.state = State::Iac;
                    } else {
                        self.push_data(byte, &mut out.data);
                    }
                }
                State::Iac => match byte {
                    // A doubled IAC is one literal 255 of data.
                    IAC => {
                        self.state = State::Data;
                        self.push_data(IAC, &mut out.data);
                    }
                    WILL | WONT | DO | DONT => self.state = State::Negotiating(byte),
                    SB => {
                        self.sb.clear();
                        self.state = State::Subnegotiation;
                    }
                    // Every other command is a no-op for us: there is nothing
                    // a terminal owes in reply to a Are-You-There or a
                    // Data-Mark, and ignoring one is better than dying on it.
                    _ => self.state = State::Data,
                },
                State::Negotiating(verb) => {
                    self.negotiate(verb, byte, &mut out.replies);
                    self.state = State::Data;
                }
                State::Subnegotiation => {
                    if byte == IAC {
                        self.state = State::SubnegotiationIac;
                    } else {
                        self.collect_sb(byte);
                    }
                }
                State::SubnegotiationIac => match byte {
                    // Escaped 255 inside the payload.
                    IAC => {
                        self.collect_sb(IAC);
                        self.state = State::Subnegotiation;
                    }
                    SE => {
                        self.finish_sb(&mut out.replies);
                        self.state = State::Data;
                    }
                    // A stray command inside a sub-negotiation: abandon it
                    // rather than accumulating the rest of the session into it.
                    _ => {
                        self.sb.clear();
                        self.state = State::Data;
                    }
                },
            }
        }

        out
    }

    /// One byte of terminal data, with the NVT CR NUL rule applied.
    fn push_data(&mut self, byte: u8, data: &mut Vec<u8>) {
        // NVT sends a lone CR as CR NUL. The NUL is padding, and printing it
        // would advance the cursor over a cell that holds nothing.
        if self.after_cr && byte == 0 {
            self.after_cr = false;
            return;
        }
        self.after_cr = byte == b'\r';
        data.push(byte);
    }

    /// Caps a sub-negotiation payload.
    ///
    /// A server that opens `IAC SB` and never closes it would otherwise grow
    /// this without limit. Nothing we understand is longer than a handful of
    /// bytes, so anything past the cap is already malformed.
    fn collect_sb(&mut self, byte: u8) {
        if self.sb.len() < 64 {
            self.sb.push(byte);
        }
    }

    fn negotiate(&mut self, verb: u8, option: u8, replies: &mut Vec<u8>) {
        let index = option as usize;
        match verb {
            // The server wants *us* to enable an option.
            DO => {
                if SUPPORTED.contains(&option) {
                    // An acknowledgement is only sent when it changes
                    // something: confirming a state already held is what makes
                    // two implementations talk past each other forever.
                    if !self.us[index] {
                        self.us[index] = true;
                        replies.extend_from_slice(&[IAC, WILL, option]);
                    }
                    // The side effects follow the server's *request*, not our
                    // answer. A `DO NAWS` for something the greeting already
                    // offered still means "and now tell me the size", and
                    // staying silent leaves the server without one.
                    match option {
                        OPT_BINARY => self.binary_out = true,
                        OPT_NAWS => {
                            self.naws = true;
                            replies.extend_from_slice(&self.naws_report());
                        }
                        _ => {}
                    }
                } else {
                    // A refusal is idempotent and always owed. This is not
                    // pedantry: a Linux `telnetd` left waiting for an answer
                    // eventually flushes its output with a SYNCH, whose Data
                    // Mark lands in the middle of the login banner.
                    self.us[index] = false;
                    replies.extend_from_slice(&[IAC, WONT, option]);
                }
            }
            DONT => {
                match option {
                    OPT_BINARY => self.binary_out = false,
                    OPT_NAWS => self.naws = false,
                    _ => {}
                }
                if self.us[index] {
                    self.us[index] = false;
                    replies.extend_from_slice(&[IAC, WONT, option]);
                }
            }
            // The server offers to enable an option on its side. ECHO is the
            // one we actively want: the far side echoing what is typed is why
            // the terminal must not echo it too.
            WILL => {
                if SUPPORTED.contains(&option) || option == OPT_ECHO {
                    if !self.them[index] {
                        self.them[index] = true;
                        replies.extend_from_slice(&[IAC, DO, option]);
                    }
                } else {
                    self.them[index] = false;
                    replies.extend_from_slice(&[IAC, DONT, option]);
                }
            }
            // The server withdraws an option. Confirmed only if it was on:
            // answering a withdrawal of something never enabled is the other
            // half of the loop rule.
            WONT if self.them[index] => {
                self.them[index] = false;
                replies.extend_from_slice(&[IAC, DONT, option]);
            }
            _ => {}
        }
    }

    fn finish_sb(&mut self, replies: &mut Vec<u8>) {
        let payload = std::mem::take(&mut self.sb);
        // The only sub-negotiation we answer: "send your terminal type".
        if payload.first() == Some(&OPT_TTYPE) && payload.get(1) == Some(&TTYPE_SEND) {
            replies.extend_from_slice(&[IAC, SB, OPT_TTYPE, TTYPE_IS]);
            replies.extend_from_slice(TERMINAL_TYPE);
            replies.extend_from_slice(&[IAC, SE]);
        }
    }
}

// ---- the session -------------------------------------------------------

/// A live Telnet session, shaped to match [`super::PtySession`] so a tab does
/// not care which one it holds.
pub struct TelnetSession {
    /// Write half. The reader thread shares it, because the protocol replies it
    /// produces have to go out between chunks.
    socket: Arc<Mutex<TcpStream>>,
    codec: Arc<Mutex<Codec>>,
    events: Receiver<SessionEvent>,
    closed: Arc<AtomicBool>,
    cols: u16,
    rows: u16,
    /// Where we connected, for error messages and the tab's tooltip.
    pub endpoint: String,
}

impl TelnetSession {
    /// Opens a session, negotiating before returning so the first output the
    /// grid sees is already the login prompt.
    pub fn connect(address: &str, port: u16, cols: u16, rows: u16) -> Result<Self> {
        let cols = cols.max(2);
        let rows = rows.max(2);
        let endpoint = format!("{address}:{port}");

        // A wrong address must fail in a moment rather than hanging the tab for
        // the OS default of tens of seconds.
        let target = std::net::ToSocketAddrs::to_socket_addrs(&endpoint)
            .with_context(|| format!("resolving {endpoint}"))?
            .next()
            .with_context(|| format!("no address for {endpoint}"))?;
        let stream = TcpStream::connect_timeout(&target, Duration::from_secs(10))
            .with_context(|| format!("connecting to {endpoint}"))?;
        // Interactive traffic is single keystrokes; Nagle would batch them into
        // a laggy session.
        let _ = stream.set_nodelay(true);

        let mut codec = Codec::default();
        codec.set_size(cols, rows);
        let greeting = codec.greeting();
        debug_assert!(!greeting.is_empty());

        let socket = Arc::new(Mutex::new(stream));
        {
            let mut guard = socket.lock().expect("fresh mutex");
            guard
                .write_all(&greeting)
                .and_then(|()| guard.flush())
                .with_context(|| format!("greeting {endpoint}"))?;
        }

        let reader = socket
            .lock()
            .expect("fresh mutex")
            .try_clone()
            .context("cloning the socket for reading")?;

        let codec = Arc::new(Mutex::new(codec));
        let (tx, rx) = crossbeam_channel::unbounded();
        let closed = Arc::new(AtomicBool::new(false));

        {
            let socket = Arc::clone(&socket);
            let codec = Arc::clone(&codec);
            let closed = Arc::clone(&closed);
            let mut reader = reader;
            std::thread::Builder::new()
                .name("iris-telnet-reader".into())
                .spawn(move || {
                    let mut buf = [0u8; 8192];
                    loop {
                        match reader.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                let decoded = match codec.lock() {
                                    Ok(mut codec) => codec.feed(&buf[..n]),
                                    Err(_) => break,
                                };
                                if !decoded.replies.is_empty() {
                                    let Ok(mut socket) = socket.lock() else { break };
                                    if socket.write_all(&decoded.replies).is_err() {
                                        break;
                                    }
                                    let _ = socket.flush();
                                }
                                // A chunk that was pure protocol is not silence:
                                // sending it would only wake the UI for nothing.
                                if !decoded.data.is_empty()
                                    && tx.send(SessionEvent::Output(decoded.data)).is_err()
                                {
                                    return;
                                }
                            }
                            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                            Err(_) => break,
                        }
                    }
                    closed.store(true, Ordering::Relaxed);
                    let _ = tx.send(SessionEvent::Closed);
                })
                .context("spawning the Telnet reader thread")?;
        }

        Ok(TelnetSession {
            socket,
            codec,
            events: rx,
            closed,
            cols,
            rows,
            endpoint,
        })
    }

    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        let encoded = self
            .codec
            .lock()
            .map_err(|_| anyhow::anyhow!("Telnet codec lock poisoned"))?
            .encode(bytes);
        self.send_raw(&encoded)
    }

    fn send_raw(&self, bytes: &[u8]) -> Result<()> {
        if bytes.is_empty() {
            return Ok(());
        }
        let mut socket = self
            .socket
            .lock()
            .map_err(|_| anyhow::anyhow!("Telnet socket lock poisoned"))?;
        socket.write_all(bytes)?;
        socket.flush()?;
        Ok(())
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        let cols = cols.max(2);
        let rows = rows.max(2);
        if cols == self.cols && rows == self.rows {
            return Ok(());
        }
        self.cols = cols;
        self.rows = rows;
        let report = self
            .codec
            .lock()
            .map_err(|_| anyhow::anyhow!("Telnet codec lock poisoned"))?
            .set_size(cols, rows);
        // Empty until the server has asked for sizes, which is not an error —
        // it just means this server does not want them.
        self.send_raw(&report)
    }

    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    pub fn is_alive(&mut self) -> bool {
        !self.closed.load(Ordering::Relaxed)
    }

    pub fn events(&self) -> &Receiver<SessionEvent> {
        &self.events
    }

    pub fn request_halt(&self) {
        let _ = self.write(b"HALT\r");
    }

    /// Drops the connection. There is no child process to kill: closing the
    /// socket is what ends the session, and IRIS reaps its own job.
    pub fn kill(&mut self) {
        if let Ok(socket) = self.socket.lock() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        self.closed.store(true, Ordering::Relaxed);
    }
}

impl Drop for TelnetSession {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn codec() -> Codec {
        Codec::default()
    }

    /// Verbatim from `localhost:23` on this machine: the first thing IRIS says
    /// is an option offer, and a terminal that treated it as text would print
    /// mojibake before the login prompt.
    #[test]
    fn the_opening_bytes_of_a_real_iris_telnet_service_are_not_text() {
        let mut codec = codec();
        let out = codec.feed(&[IAC, WILL, OPT_SGA]);
        assert!(out.data.is_empty(), "protocol leaked into the terminal");
        assert_eq!(out.replies, vec![IAC, DO, OPT_SGA]);
    }

    /// Verbatim from `10.0.0.102:23`: four requests, two of which we do not
    /// implement and must refuse rather than ignore.
    #[test]
    fn the_other_instances_opening_requests_are_answered_one_for_one() {
        let mut codec = codec();
        const OPT_TSPEED: u8 = 32;
        const OPT_XDISPLOC: u8 = 35;
        const OPT_NEW_ENVIRON: u8 = 39;
        let out = codec.feed(&[
            IAC,
            DO,
            OPT_TTYPE,
            IAC,
            DO,
            OPT_TSPEED,
            IAC,
            DO,
            OPT_XDISPLOC,
            IAC,
            DO,
            OPT_NEW_ENVIRON,
        ]);
        assert!(out.data.is_empty());
        assert_eq!(
            out.replies,
            vec![
                IAC,
                WILL,
                OPT_TTYPE,
                IAC,
                WONT,
                OPT_TSPEED,
                IAC,
                WONT,
                OPT_XDISPLOC,
                IAC,
                WONT,
                OPT_NEW_ENVIRON,
            ]
        );
    }

    #[test]
    fn the_terminal_type_request_is_answered_with_vt100() {
        let mut codec = codec();
        let out = codec.feed(&[IAC, SB, OPT_TTYPE, TTYPE_SEND, IAC, SE]);
        let mut expected = vec![IAC, SB, OPT_TTYPE, TTYPE_IS];
        expected.extend_from_slice(b"vt100");
        expected.extend_from_slice(&[IAC, SE]);
        assert_eq!(out.replies, expected);
        assert!(out.data.is_empty());
    }

    /// Agreeing to NAWS owes the size straight away, and the size known before
    /// the agreement is the one that must be reported.
    #[test]
    fn agreeing_to_report_the_window_size_reports_it_immediately() {
        let mut codec = codec();
        codec.us[OPT_NAWS as usize] = false;
        assert!(
            codec.set_size(200, 50).is_empty(),
            "a size must not be sent before the server asks for one"
        );

        let out = codec.feed(&[IAC, DO, OPT_NAWS]);
        assert_eq!(
            out.replies,
            vec![IAC, WILL, OPT_NAWS, IAC, SB, OPT_NAWS, 0, 200, 0, 50, IAC, SE]
        );

        // And a later resize goes out on its own.
        assert_eq!(
            codec.set_size(100, 40),
            vec![IAC, SB, OPT_NAWS, 0, 100, 0, 40, IAC, SE]
        );
    }

    /// A dimension of exactly 255 collides with IAC and has to be escaped, or
    /// the server reads the size as a command.
    #[test]
    fn a_255_wide_window_escapes_its_size_bytes() {
        let mut codec = codec();
        codec.feed(&[IAC, DO, OPT_NAWS]);
        assert_eq!(
            codec.set_size(255, 24),
            vec![IAC, SB, OPT_NAWS, 0, 255, 255, 0, 24, IAC, SE]
        );
    }

    /// The server declining something we never had does not put us in a
    /// negotiation loop.
    #[test]
    fn a_refusal_is_not_answered() {
        let mut codec = codec();
        assert_eq!(codec.feed(&[IAC, WONT, OPT_ECHO]).replies, Vec::<u8>::new());
    }

    /// RFC 854's loop rule, and the reason a Linux `telnetd` was flushing a
    /// SYNCH into the middle of the login banner: the greeting already asked
    /// for ECHO and SGA, so the server agreeing to them settles the matter and
    /// must not be acknowledged again.
    #[test]
    fn an_agreement_to_what_the_greeting_asked_for_is_not_re_acknowledged() {
        let mut codec = codec();
        let greeting = codec.greeting();
        assert!(greeting.windows(3).any(|w| w == [IAC, DO, OPT_ECHO]));

        let out = codec.feed(&[IAC, WILL, OPT_SGA, IAC, WILL, OPT_ECHO]);
        assert_eq!(
            out.replies,
            Vec::<u8>::new(),
            "re-acknowledging a state already held is what starts a loop"
        );
    }

    /// The same rule in the other direction: being told twice to do something
    /// is answered once.
    #[test]
    fn a_repeated_request_is_answered_once() {
        let mut codec = codec();
        codec.greeting();
        let first = codec.feed(&[IAC, DO, OPT_TTYPE]);
        assert_eq!(
            first.replies,
            Vec::<u8>::new(),
            "the greeting already offered TTYPE"
        );

        // Something the greeting did not claim is answered the first time and
        // not the second.
        let mut fresh = Codec::default();
        assert_eq!(
            fresh.feed(&[IAC, DO, OPT_BINARY]).replies,
            vec![IAC, WILL, OPT_BINARY]
        );
        assert_eq!(fresh.feed(&[IAC, DO, OPT_BINARY]).replies, Vec::<u8>::new());
    }

    /// A refusal must still be sent every time it is asked for: a server that
    /// gets no answer to `DO TSPEED` waits for one.
    #[test]
    fn an_unsupported_option_is_refused_every_time_it_is_requested() {
        let mut codec = codec();
        const OPT_TSPEED: u8 = 32;
        assert_eq!(
            codec.feed(&[IAC, DO, OPT_TSPEED]).replies,
            vec![IAC, WONT, OPT_TSPEED]
        );
        assert_eq!(
            codec.feed(&[IAC, DO, OPT_TSPEED]).replies,
            vec![IAC, WONT, OPT_TSPEED],
            "a server that hears nothing keeps waiting"
        );
    }

    /// IRIS echoes what is typed, so the terminal accepting that offer is what
    /// stops every keystroke appearing twice.
    #[test]
    fn the_servers_offer_to_echo_is_accepted() {
        let mut codec = codec();
        assert_eq!(
            codec.feed(&[IAC, WILL, OPT_ECHO]).replies,
            vec![IAC, DO, OPT_ECHO]
        );
    }

    /// A server withdrawing its echo is followed, or the terminal would stay
    /// silent while nothing else echoes either.
    #[test]
    fn a_withdrawn_echo_is_acknowledged() {
        let mut codec = codec();
        codec.feed(&[IAC, WILL, OPT_ECHO]);
        assert_eq!(
            codec.feed(&[IAC, WONT, OPT_ECHO]).replies,
            vec![IAC, DONT, OPT_ECHO]
        );
    }

    #[test]
    fn terminal_data_passes_through_with_the_protocol_removed() {
        let mut codec = codec();
        let mut wire = b"USER".to_vec();
        wire.extend_from_slice(&[IAC, WILL, OPT_SGA]);
        wire.extend_from_slice(b">write 1");
        let out = codec.feed(&wire);
        assert_eq!(out.data, b"USER>write 1");
    }

    /// A doubled 255 is one byte of data. It matters for the codepages: CP850
    /// uses the whole high half, so 0xFF is ordinary text here.
    #[test]
    fn a_doubled_escape_byte_is_one_byte_of_data() {
        let mut codec = codec();
        assert_eq!(
            codec.feed(&[b'a', IAC, IAC, b'b']).data,
            vec![b'a', IAC, b'b']
        );
    }

    /// The NUL that NVT pads a bare CR with is not content: printing it would
    /// step the cursor over a cell holding nothing.
    #[test]
    fn the_padding_nul_after_a_carriage_return_is_dropped() {
        let mut codec = codec();
        assert_eq!(codec.feed(b"a\r\0b").data, b"a\rb");
        // CR LF is a real pair and must survive intact.
        assert_eq!(codec.feed(b"a\r\nb").data, b"a\r\nb");
    }

    /// The rule holds across a chunk boundary, which is where a stream-based
    /// implementation gets it wrong.
    #[test]
    fn the_carriage_return_rule_survives_a_split_chunk() {
        let mut codec = codec();
        assert_eq!(codec.feed(b"a\r").data, b"a\r");
        assert_eq!(codec.feed(b"\0b").data, b"b");
    }

    /// A negotiation split across two reads must not be printed as text.
    #[test]
    fn a_command_split_across_chunks_is_still_a_command() {
        let mut codec = codec();
        assert!(codec.feed(&[b'x', IAC]).data == b"x");
        let out = codec.feed(&[DO, OPT_TTYPE]);
        assert!(out.data.is_empty());
        assert_eq!(out.replies, vec![IAC, WILL, OPT_TTYPE]);
    }

    /// Enter is a bare CR everywhere else in the app; NVT requires the NUL.
    #[test]
    fn enter_is_padded_until_the_server_agrees_to_binary() {
        let mut codec = codec();
        assert_eq!(codec.encode(b"zw\r"), b"zw\r\0");
        // CR LF is already unambiguous.
        assert_eq!(codec.encode(b"zw\r\n"), b"zw\r\n");

        codec.feed(&[IAC, DO, OPT_BINARY]);
        assert_eq!(
            codec.encode(b"zw\r"),
            b"zw\r",
            "binary mode needs no padding"
        );
    }

    /// Typed text containing 0xFF — an accented character in CP850 — must not
    /// be read by the server as the start of a command.
    #[test]
    fn an_escape_byte_in_typed_text_is_doubled() {
        let codec = codec();
        assert_eq!(codec.encode(&[b'a', IAC, b'b']), vec![b'a', IAC, IAC, b'b']);
    }

    /// A server that opens a sub-negotiation and never closes it must not grow
    /// the buffer for the rest of the session.
    #[test]
    fn an_unterminated_subnegotiation_is_capped() {
        let mut codec = codec();
        codec.feed(&[IAC, SB, OPT_TTYPE]);
        codec.feed(&vec![b'x'; 4096]);
        assert!(codec.sb.len() <= 64);
    }

    /// The greeting has to claim the terminal type: one of the two instances
    /// never asks, and IRIS falls back to dumb-terminal behaviour without it.
    #[test]
    fn the_greeting_offers_the_terminal_type_and_size() {
        let mut codec = codec();
        let greeting = codec.greeting();
        assert_eq!(
            greeting,
            vec![IAC, WILL, OPT_TTYPE, IAC, WILL, OPT_NAWS, IAC, DO, OPT_SGA, IAC, DO, OPT_ECHO,]
        );
    }
}
