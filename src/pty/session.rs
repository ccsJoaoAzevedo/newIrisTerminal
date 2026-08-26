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

use super::launcher::{IrisLauncher, LaunchSpec};

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
    cols: u16,
    rows: u16,
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
        let cols = cols.max(2);
        let rows = rows.max(2);

        let pty_system = portable_pty::native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows,
                cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("opening a pseudo-terminal")?;

        let cmd = launcher.command(spec)?;
        let child = pair
            .slave
            .spawn_command(cmd)
            .with_context(|| format!("starting IRIS session for instance {}", spec.instance))?;

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
            cols,
            rows,
        })
    }

    /// Non-blocking drain of everything the child has produced since the last
    /// call. Returns the concatenated bytes and whether the session ended.
    pub fn drain(&mut self) -> (Vec<u8>, bool) {
        let mut bytes = Vec::new();
        let mut ended = false;
        loop {
            match self.events.try_recv() {
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
