//! Per-session transcript logging.
//!
//! Two modes, because they serve different purposes: `Raw` keeps the byte
//! stream exactly as it arrived (escape sequences included) so a session can be
//! replayed, while `Clean` writes rendered text lines as they scroll away, for
//! reading and grepping.
//!
//! Redaction matters here. Autologon types a password into the same stream
//! everything else flows through, so the writer is muted around that moment;
//! a transcript must never become a plaintext credential store.
//!
//! Every write reaches the file before it returns. A transcript is read while
//! the session it belongs to is still open - that is the whole point of one -
//! and buffering meant the file sat empty until 8 KB had piled up or the app
//! closed. One `flush` per chunk of output is nothing next to what arriving at
//! that chunk already cost.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::LogMode;

pub struct SessionLog {
    mode: LogMode,
    writer: BufWriter<File>,
    path: PathBuf,
    /// Bytes written, for size-based rotation.
    written: u64,
    max_bytes: u64,
    /// While true, output is dropped instead of written.
    muted: bool,
    /// Index of the next line not yet written, in `Clean` mode. Absolute over
    /// scrollback-then-screen, which is a coordinate a line keeps as it scrolls
    /// out of view.
    next_line: usize,
}

impl SessionLog {
    /// Opens a transcript for `profile_name` under `dir`.
    pub fn open(dir: &Path, profile_name: &str, mode: LogMode, max_bytes: u64) -> Result<Self> {
        std::fs::create_dir_all(dir)
            .with_context(|| format!("creating log directory {}", dir.display()))?;

        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
        let path = dir.join(format!("{}-{stamp}.log", sanitize(profile_name)));
        let file =
            File::create(&path).with_context(|| format!("creating log file {}", path.display()))?;

        Ok(SessionLog {
            mode,
            writer: BufWriter::new(file),
            path,
            written: 0,
            max_bytes,
            muted: false,
            next_line: 0,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn mode(&self) -> LogMode {
        self.mode
    }

    /// Suppresses writing until [`unmute`](Self::unmute). Used around the
    /// password step of autologon.
    pub fn mute(&mut self) {
        self.muted = true;
    }

    pub fn unmute(&mut self) {
        self.muted = false;
    }

    pub fn is_muted(&self) -> bool {
        self.muted
    }

    /// Raw mode: append bytes exactly as received.
    pub fn write_raw(&mut self, bytes: &[u8]) -> Result<()> {
        if self.mode != LogMode::Raw || self.muted || self.rotate_if_full()? {
            return Ok(());
        }
        self.writer.write_all(bytes)?;
        self.written += bytes.len() as u64;
        self.flush()
    }

    /// Clean mode: append the rendered lines the session has finished with.
    ///
    /// `settled` is the number of lines the caller considers final - everything
    /// above the row the cursor is on. Each command and its output therefore
    /// reaches the file as soon as the next prompt is printed, which is what a
    /// transcript being read alongside the session has to do.
    ///
    /// The row the cursor is on is deliberately left out: it is the line being
    /// typed or written, and logging it would put half a line in the file and
    /// the other half on the next one. A routine that addresses the cursor and
    /// paints over a row already written keeps the version that was there when
    /// the cursor passed it - the price of writing in real time rather than
    /// waiting for a row to scroll away for good.
    ///
    /// Whether [`SessionLog::write_settled`] would look at the lines it is
    /// handed.
    ///
    /// Transcribing the whole grid to build them costs a `String` per line of
    /// history, so the caller asks first rather than doing that work for a log
    /// that is off, raw, or muted across the password step.
    pub fn wants_settled(&self) -> bool {
        self.mode == LogMode::Clean && !self.muted
    }

    pub fn write_settled(&mut self, lines: &[String], settled: usize) -> Result<()> {
        if self.mode != LogMode::Clean || self.muted || self.rotate_if_full()? {
            return Ok(());
        }
        if lines.len() < self.next_line {
            // The history the count refers to is gone: fewer lines exist than
            // have already been written, which only happens when the scrollback
            // is deliberately thrown away - Ctrl+Delete. Everything already
            // written stays written; counting restarts from what is there now,
            // or the transcript would go quiet until the history grew back past
            // the old mark.
            self.next_line = settled;
            return Ok(());
        }
        if settled <= self.next_line {
            // Nothing new has settled. The cursor moving back up the screen
            // gets here, and must not rewind the count: the lines it moved back
            // over are already in the file, and writing them again on the way
            // down would double them.
            return Ok(());
        }
        for line in lines.iter().take(settled).skip(self.next_line) {
            writeln!(self.writer, "{line}")?;
            self.written += line.len() as u64 + 1;
        }
        self.next_line = settled;
        self.flush()
    }

    /// Records a note from the app itself (session started, ended, reconnected).
    pub fn write_note(&mut self, note: &str) -> Result<()> {
        if self.mode == LogMode::Off {
            return Ok(());
        }
        let stamp = chrono::Local::now().format("%Y-%m-%d %H:%M:%S");
        writeln!(self.writer, "\n=== {stamp} {note} ===")?;
        self.flush()
    }

    pub fn flush(&mut self) -> Result<()> {
        self.writer.flush()?;
        Ok(())
    }

    /// Starts a new file once the current one passes the size limit. Returns
    /// true if a rotation happened, in which case the current write is skipped
    /// and lands in the next file instead.
    fn rotate_if_full(&mut self) -> Result<bool> {
        if self.max_bytes == 0 || self.written < self.max_bytes {
            return Ok(false);
        }
        self.writer.flush()?;

        let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S%.3f");
        let stem = self
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("session");
        let dir = self.path.parent().unwrap_or_else(|| Path::new("."));
        let next = dir.join(format!("{stem}-{stamp}.log"));

        self.writer = BufWriter::new(File::create(&next)?);
        self.path = next;
        self.written = 0;
        Ok(true)
    }
}

impl Drop for SessionLog {
    fn drop(&mut self) {
        let _ = self.writer.flush();
    }
}

/// Deletes logs older than `days`. A no-op when `days` is 0.
pub fn prune(dir: &Path, days: u32) -> Result<usize> {
    if days == 0 || !dir.is_dir() {
        return Ok(0);
    }
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(u64::from(days) * 86_400))
        .unwrap_or(std::time::UNIX_EPOCH);

    let mut removed = 0;
    for entry in std::fs::read_dir(dir)?.flatten() {
        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("log") {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        if modified < cutoff && std::fs::remove_file(&path).is_ok() {
            removed += 1;
        }
    }
    Ok(removed)
}

/// Keeps a profile name usable as a filename on every platform.
fn sanitize(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '-'
            }
        })
        .collect();
    if cleaned.trim_matches('-').is_empty() {
        "session".to_string()
    } else {
        cleaned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nit-log-test-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn raw_mode_writes_bytes_verbatim() {
        let dir = tempdir("raw");
        let mut log = SessionLog::open(&dir, "test", LogMode::Raw, 0).unwrap();
        log.write_raw(b"\x1b[2JUSER>").unwrap();
        log.flush().unwrap();

        let content = std::fs::read(log.path()).unwrap();
        assert_eq!(content, b"\x1b[2JUSER>");
    }

    /// The bug this guards: the transcript is read while the session is still
    /// open, and a buffered writer left the file empty until 8 KB had piled up.
    /// Nothing here calls `flush` - the write is expected to have reached the
    /// disk on its own.
    #[test]
    fn output_is_on_disk_before_the_session_ends() {
        let dir = tempdir("realtime");
        let mut log = SessionLog::open(&dir, "test", LogMode::Raw, 0).unwrap();
        log.write_raw(b"USER>write 1").unwrap();
        assert_eq!(
            std::fs::read(log.path()).unwrap(),
            b"USER>write 1",
            "the chunk should already be in the file"
        );

        let mut clean = SessionLog::open(&dir, "clean", LogMode::Clean, 0).unwrap();
        clean.write_settled(&["one".to_string()], 1).unwrap();
        assert_eq!(std::fs::read_to_string(clean.path()).unwrap(), "one\n");
    }

    #[test]
    fn clean_mode_only_writes_lines_that_have_scrolled_away() {
        let dir = tempdir("clean");
        let mut log = SessionLog::open(&dir, "test", LogMode::Clean, 0).unwrap();

        let lines = vec!["one".to_string(), "two".to_string(), "three".to_string()];
        // Only the first two have left the screen.
        log.write_settled(&lines, 2).unwrap();
        log.flush().unwrap();

        let content = std::fs::read_to_string(log.path()).unwrap();
        assert_eq!(content, "one\ntwo\n");

        // The third settles later and must not duplicate the first two.
        log.write_settled(&lines, 3).unwrap();
        log.flush().unwrap();
        let content = std::fs::read_to_string(log.path()).unwrap();
        assert_eq!(content, "one\ntwo\nthree\n");
    }

    /// The count follows the cursor, and a routine that addresses the cursor
    /// moves it back up the screen. That must not rewind the count: the lines
    /// it moved back over are already in the file, and writing them again on
    /// the way down would double them.
    #[test]
    fn a_cursor_moving_back_up_the_screen_does_not_double_the_lines() {
        let dir = tempdir("rewind");
        let mut log = SessionLog::open(&dir, "test", LogMode::Clean, 0).unwrap();

        let lines: Vec<String> = ["one", "two", "three", "four"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        log.write_settled(&lines, 3).unwrap();
        // The cursor goes back to the second row, then walks down again.
        log.write_settled(&lines, 1).unwrap();
        log.write_settled(&lines, 4).unwrap();

        let content = std::fs::read_to_string(log.path()).unwrap();
        assert_eq!(content, "one\ntwo\nthree\nfour\n");
    }

    /// Ctrl+Delete throws the scrollback away, so the count the log follows
    /// drops. It has to start counting again from there, or the transcript goes
    /// quiet until the history grows back past the old mark.
    #[test]
    fn a_cleared_scrollback_does_not_silence_the_log() {
        let dir = tempdir("cleared");
        let mut log = SessionLog::open(&dir, "test", LogMode::Clean, 0).unwrap();

        let before: Vec<String> = ["one", "two", "three"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        log.write_settled(&before, 3).unwrap();

        // The clear, and then a fresh line settling on the empty scrollback.
        let after: Vec<String> = vec!["afterwards".to_string()];
        log.write_settled(&after, 0).unwrap();
        log.write_settled(&after, 1).unwrap();
        log.flush().unwrap();

        let content = std::fs::read_to_string(log.path()).unwrap();
        assert_eq!(content, "one\ntwo\nthree\nafterwards\n");
    }

    /// The whole point of muting: an autologon password must never reach disk.
    #[test]
    fn muted_output_is_not_written() {
        let dir = tempdir("mute");
        let mut log = SessionLog::open(&dir, "test", LogMode::Raw, 0).unwrap();

        log.write_raw(b"Password:").unwrap();
        log.mute();
        log.write_raw(b"hunter2\r").unwrap();
        log.unmute();
        log.write_raw(b"USER>").unwrap();
        log.flush().unwrap();

        let content = String::from_utf8(std::fs::read(log.path()).unwrap()).unwrap();
        assert!(
            !content.contains("hunter2"),
            "password leaked into {content:?}"
        );
        assert!(content.contains("USER>"));
    }

    #[test]
    fn a_mode_mismatch_writes_nothing() {
        let dir = tempdir("mismatch");
        let mut log = SessionLog::open(&dir, "test", LogMode::Clean, 0).unwrap();
        log.write_raw(b"ignored").unwrap();
        log.flush().unwrap();
        assert_eq!(std::fs::read(log.path()).unwrap().len(), 0);
    }

    #[test]
    fn rotation_starts_a_new_file_once_the_limit_is_passed() {
        let dir = tempdir("rotate");
        let mut log = SessionLog::open(&dir, "test", LogMode::Raw, 8).unwrap();
        let first = log.path().to_path_buf();

        log.write_raw(b"0123456789").unwrap(); // past the 8-byte limit
        log.write_raw(b"next").unwrap(); // triggers rotation
        log.flush().unwrap();

        assert_ne!(log.path(), first.as_path(), "should have rotated");
        let count = std::fs::read_dir(&dir).unwrap().flatten().count();
        assert_eq!(count, 2);
    }

    #[test]
    fn profile_names_with_path_characters_are_made_safe() {
        assert_eq!(sanitize("RDB/test:1"), "RDB-test-1");
        assert_eq!(sanitize("///"), "session");
    }

    #[test]
    fn prune_is_a_no_op_when_retention_is_zero() {
        let dir = tempdir("prune");
        std::fs::write(dir.join("a.log"), b"x").unwrap();
        assert_eq!(prune(&dir, 0).unwrap(), 0);
        assert!(dir.join("a.log").exists());
    }
}
