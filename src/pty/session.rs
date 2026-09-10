//! One live IRIS session: the child process, its pseudo-terminal, and the
//! reader thread that pumps its output.
//!
//! Ownership model — a blocking `read()` on the PTY cannot happen on the UI
//! thread, so each session spawns one reader thread that pushes byte chunks
//! into a channel. The UI drains that channel once per frame. Writes go
//! straight from the UI thread through a mutex-guarded writer.

use std::io::{Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use crossbeam_channel::{Receiver, Sender, TryRecvError};
use portable_pty::{Child, MasterPty, PtySize};

use std::path::Path;

use portable_pty::CommandBuilder;

use super::launcher::{IrisLauncher, LaunchSpec};
use super::telnet::TelnetSession;

/// What the reader thread reports back to the UI.
pub enum SessionEvent {
    Output(Vec<u8>),
    /// The reader hit EOF or an error — the child is gone.
    Closed,
}

pub struct PtySession {
    master: Box<dyn MasterPty + Send>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    child: Box<dyn Child + Send + Sync>,
    events: Receiver<SessionEvent>,
    /// Set once the reader observes EOF, so `is_alive` does not have to poll
    /// the child on every frame.
    closed: Arc<AtomicBool>,
    /// The session was started through a shell that puts the console into
    /// UTF-8 first, so the child owned here is that shell and the process worth
    /// reporting is the one under it. See `launcher::windows::session_command`.
    wrapped: bool,
    /// That process, once it has been found. Looked up lazily and remembered:
    /// it does not exist yet when the shell is spawned, and the menu bar asks
    /// for it every frame.
    session_pid: std::cell::Cell<Option<u32>>,
    cols: u16,
    rows: u16,
}

/// A pseudo-terminal of the given size, with the size it settled on.
///
/// The size matters at spawn time: IRIS reads the terminal dimensions once at
/// login, so opening at 80x24 and resizing immediately makes full-screen
/// routines repaint needlessly. A shell cares less, but there is no reason to
/// open one at the wrong size either.
fn open_pty(cols: u16, rows: u16) -> Result<(portable_pty::PtyPair, u16, u16)> {
    let cols = cols.max(2);
    let rows = rows.max(2);
    let pair = portable_pty::native_pty_system()
        .openpty(PtySize {
            rows,
            cols,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("opening a pseudo-terminal")?;
    Ok((pair, cols, rows))
}

impl PtySession {
    /// Starts a session at the given size. The size matters at spawn time:
    /// IRIS reads the terminal dimensions once at login, so opening at 80x24
    /// and resizing immediately makes full-screen routines repaint needlessly.
    pub fn spawn(
        launcher: &dyn IrisLauncher,
        spec: &LaunchSpec,
        cols: u16,
        rows: u16,
    ) -> Result<Self> {
        let (pair, cols, rows) = open_pty(cols, rows)?;
        let cmd = launcher.command(spec)?;
        Self::from_command(
            pair,
            cmd,
            cols,
            rows,
            &format!("IRIS session for instance {}", spec.instance),
        )
    }

    /// Starts a shell rather than an IRIS session.
    ///
    /// Everything below the command is the same - a pseudo-terminal, a child,
    /// and a reader thread - which is the whole reason a shell can be a plugin
    /// that only declares a program. See [`crate::plugins::shells`].
    pub fn spawn_shell(
        program: &Path,
        args: &[String],
        cwd: Option<&Path>,
        cols: u16,
        rows: u16,
    ) -> Result<Self> {
        let (pair, cols, rows) = open_pty(cols, rows)?;
        let cmd = super::launcher::shell_command(program, args, cwd);
        Self::from_command(
            pair,
            cmd,
            cols,
            rows,
            &format!("shell {}", program.display()),
        )
    }

    /// The half of starting a session that has nothing to do with what is being
    /// started: the child, the reader thread, and the handles they need.
    ///
    /// `what` names the thing being started, for the error a failed spawn
    /// produces - which is the message the tab shows.
    fn from_command(
        pair: portable_pty::PtyPair,
        cmd: CommandBuilder,
        cols: u16,
        rows: u16,
        what: &str,
    ) -> Result<Self> {
        // Whether the command is the session itself or a shell in front of it.
        let wrapped = cmd
            .get_argv()
            .first()
            .map(|program| {
                program
                    .to_string_lossy()
                    .to_lowercase()
                    .ends_with("cmd.exe")
            })
            .unwrap_or(false);
        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("starting {what}"))?;

        // The slave handle must be dropped, or the PTY never reports EOF when
        // the child exits and the reader thread hangs forever.
        drop(pair.slave);

        let reader = pair
            .master
            .try_clone_reader()
            .context("cloning the PTY reader")?;
        let writer = pair.master.take_writer().context("taking the PTY writer")?;

        let (tx, rx) = crossbeam_channel::unbounded();
        let closed = Arc::new(AtomicBool::new(false));
        spawn_reader(reader, tx, Arc::clone(&closed));

        Ok(PtySession {
            master: pair.master,
            writer: Arc::new(Mutex::new(writer)),
            child,
            events: rx,
            closed,
            wrapped,
            session_pid: std::cell::Cell::new(None),
            cols,
            rows,
        })
    }

    /// Non-blocking drain of everything the child has produced since the last
    /// call. Returns the concatenated bytes and whether the session ended.
    pub fn drain(&mut self) -> (Vec<u8>, bool) {
        drain_events(&self.events)
    }

    pub fn events(&self) -> &Receiver<SessionEvent> {
        &self.events
    }

    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        let mut writer = self
            .writer
            .lock()
            .map_err(|_| anyhow::anyhow!("PTY writer lock poisoned"))?;
        writer.write_all(bytes)?;
        writer.flush()?;
        Ok(())
    }

    pub fn write_str(&self, text: &str) -> Result<()> {
        self.write(text.as_bytes())
    }

    /// Sends a line the way pressing Enter would: IRIS expects CR, never LF,
    /// regardless of the host operating system.
    pub fn write_line(&self, text: &str) -> Result<()> {
        self.write(text.as_bytes())?;
        self.write(b"\r")
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        let cols = cols.max(2);
        let rows = rows.max(2);
        if cols == self.cols && rows == self.rows {
            return Ok(());
        }
        self.master
            .resize(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("resizing the pseudo-terminal")?;
        self.cols = cols;
        self.rows = rows;
        Ok(())
    }

    pub fn size(&self) -> (u16, u16) {
        (self.cols, self.rows)
    }

    /// Operating-system process id of the session, while it is running.
    ///
    /// The process the user means, which is not always the child this owns: a
    /// session opened through a shell that sets the console codepage is the
    /// shell's child, and the shell's own id would say nothing.
    pub fn process_id(&self) -> Option<u32> {
        let own = self.child.process_id()?;
        if !self.wrapped {
            return Some(own);
        }
        if let Some(known) = self.session_pid.get() {
            return Some(known);
        }
        let found = session_under(own);
        self.session_pid.set(found);
        // Nothing under the shell yet: it is still starting, and saying
        // nothing is better than naming the shell.
        found
    }

    pub fn is_alive(&mut self) -> bool {
        if self.closed.load(Ordering::Relaxed) {
            return false;
        }
        matches!(self.child.try_wait(), Ok(None))
    }

    /// Asks IRIS to end the session cleanly. `HALT` is the correct way out —
    /// killing the process can leave the instance holding locks.
    pub fn request_halt(&self) {
        let _ = self.write_line("HALT");
    }

    pub fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.closed.store(true, Ordering::Relaxed);
    }
}

impl Drop for PtySession {
    fn drop(&mut self) {
        // A tab closing must not leave an orphaned irissession process behind.
        if matches!(self.child.try_wait(), Ok(None)) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

/// Non-blocking drain shared by both transports: the reader threads differ,
/// the channel they push into does not.
fn drain_events(events: &Receiver<SessionEvent>) -> (Vec<u8>, bool) {
    let mut bytes = Vec::new();
    let mut ended = false;
    loop {
        match events.try_recv() {
            Ok(SessionEvent::Output(chunk)) => bytes.extend_from_slice(&chunk),
            Ok(SessionEvent::Closed) => {
                ended = true;
                break;
            }
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Disconnected) => {
                ended = true;
                break;
            }
        }
    }
    (bytes, ended)
}

/// One session, whichever way it is reached.
///
/// A local instance is a child process on a pseudo-terminal; a remote server
/// from the launcher's Server Manager is a Telnet login (see
/// [`crate::pty::telnet`]). The two have nothing in common below this point and
/// everything in common above it — the grid, the parser, autologon, logging and
/// the renderer never ask which one they are attached to.
pub enum Session {
    Pty(PtySession),
    Telnet(TelnetSession),
}

impl Session {
    /// Opens a local session on an instance.
    pub fn local(
        launcher: &dyn IrisLauncher,
        spec: &LaunchSpec,
        cols: u16,
        rows: u16,
    ) -> Result<Self> {
        PtySession::spawn(launcher, spec, cols, rows).map(Session::Pty)
    }

    /// Opens a shell rather than an IRIS session. See
    /// [`crate::plugins::shells`].
    pub fn shell(
        program: &Path,
        args: &[String],
        cwd: Option<&Path>,
        cols: u16,
        rows: u16,
    ) -> Result<Self> {
        PtySession::spawn_shell(program, args, cwd, cols, rows).map(Session::Pty)
    }

    /// Opens a session on a remote server over Telnet.
    pub fn telnet(address: &str, port: u16, cols: u16, rows: u16) -> Result<Self> {
        TelnetSession::connect(address, port, cols, rows).map(Session::Telnet)
    }

    pub fn drain(&mut self) -> (Vec<u8>, bool) {
        match self {
            Session::Pty(s) => drain_events(s.events()),
            Session::Telnet(s) => drain_events(s.events()),
        }
    }

    pub fn write(&self, bytes: &[u8]) -> Result<()> {
        match self {
            Session::Pty(s) => s.write(bytes),
            Session::Telnet(s) => s.write(bytes),
        }
    }

    pub fn write_str(&self, text: &str) -> Result<()> {
        self.write(text.as_bytes())
    }

    /// Sends a line the way pressing Enter would: IRIS expects CR, never LF,
    /// regardless of the host operating system or the transport.
    pub fn write_line(&self, text: &str) -> Result<()> {
        self.write(text.as_bytes())?;
        self.write(b"\r")
    }

    pub fn resize(&mut self, cols: u16, rows: u16) -> Result<()> {
        match self {
            Session::Pty(s) => s.resize(cols, rows),
            Session::Telnet(s) => s.resize(cols, rows),
        }
    }

    pub fn size(&self) -> (u16, u16) {
        match self {
            Session::Pty(s) => s.size(),
            Session::Telnet(s) => s.size(),
        }
    }

    /// Process id of the session on this machine, when there is a process here
    /// to have one. A remote server is reached over Telnet, so its IRIS process
    /// runs on the far side and nothing local stands for it.
    pub fn process_id(&self) -> Option<u32> {
        match self {
            Session::Pty(s) => s.process_id(),
            Session::Telnet(_) => None,
        }
    }

    pub fn is_alive(&mut self) -> bool {
        match self {
            Session::Pty(s) => s.is_alive(),
            Session::Telnet(s) => s.is_alive(),
        }
    }

    pub fn request_halt(&self) {
        match self {
            Session::Pty(s) => s.request_halt(),
            Session::Telnet(s) => s.request_halt(),
        }
    }

    pub fn kill(&mut self) {
        match self {
            Session::Pty(s) => s.kill(),
            Session::Telnet(s) => s.kill(),
        }
    }
}

/// The process a shell started, on the platforms where sessions are opened
/// through one.
#[cfg(windows)]
fn session_under(shell: u32) -> Option<u32> {
    /// `PROCESSENTRY32W`, field for field.
    #[repr(C)]
    struct ProcessEntry {
        size: u32,
        usage: u32,
        pid: u32,
        default_heap: usize,
        module: u32,
        threads: u32,
        parent: u32,
        priority: i32,
        flags: u32,
        exe: [u16; 260],
    }

    #[link(name = "kernel32")]
    extern "system" {
        fn CreateToolhelp32Snapshot(flags: u32, pid: u32) -> isize;
        fn Process32FirstW(snapshot: isize, entry: *mut ProcessEntry) -> i32;
        fn Process32NextW(snapshot: isize, entry: *mut ProcessEntry) -> i32;
        fn CloseHandle(handle: isize) -> i32;
    }
    const SNAP_PROCESS: u32 = 0x0000_0002;
    const INVALID_HANDLE: isize = -1;

    // Safety: the snapshot is closed on every path out, and the entry is a
    // plain struct the API only ever writes into.
    unsafe {
        let snapshot = CreateToolhelp32Snapshot(SNAP_PROCESS, 0);
        if snapshot == INVALID_HANDLE {
            return None;
        }
        let mut entry: ProcessEntry = std::mem::zeroed();
        entry.size = std::mem::size_of::<ProcessEntry>() as u32;
        let mut found = None;
        let mut more = Process32FirstW(snapshot, &mut entry);
        while more != 0 {
            if entry.parent == shell {
                let end = entry.exe.iter().position(|c| *c == 0).unwrap_or(0);
                let name = String::from_utf16_lossy(&entry.exe[..end]).to_lowercase();
                // The session itself, rather than anything else the shell may
                // have run on its way there.
                if name.starts_with("irissession") || name.starts_with("csession") {
                    found = Some(entry.pid);
                    break;
                }
                found.get_or_insert(entry.pid);
            }
            more = Process32NextW(snapshot, &mut entry);
        }
        CloseHandle(snapshot);
        found
    }
}

#[cfg(not(windows))]
fn session_under(_shell: u32) -> Option<u32> {
    None
}

fn spawn_reader(
    mut reader: Box<dyn Read + Send>,
    tx: Sender<SessionEvent>,
    closed: Arc<AtomicBool>,
) {
    std::thread::Builder::new()
        .name("iris-pty-reader".into())
        .spawn(move || {
            let mut buf = [0u8; 8192];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) => break,
                    Ok(n) => {
                        if tx.send(SessionEvent::Output(buf[..n].to_vec())).is_err() {
                            // Receiver dropped — the tab is gone.
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
        .expect("spawning the PTY reader thread");
}
