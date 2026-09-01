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
    /// Index of the next screen line not yet flushed, in `Clean` mode.
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
        Ok(())
    }

    /// Clean mode: append the rendered lines that have scrolled out of view.
    ///
    /// `scrollback_len` is the grid's current scrollback length; everything
    /// between what we last wrote and that point is now final and safe to
    /// record. Lines still on screen may yet be overwritten by a full-screen
    /// routine, so they are deliberately not logged until they scroll away.
    /// Whether [`Log::write_settled`] would look at the lines it is handed.
    ///
    /// Transcribing the whole grid to build them costs a `String` per line of
    /// history, so the caller asks first rather than doing that work for a log
    /// that is off, raw, or muted across the password step.
    pub fn wants_settled(&self) -> bool {
        self.mode == LogMode::Clean && !self.muted
    }

    pub fn write_settled(&mut self, lines: &[String], scrollback_len: usize) -> Result<()> {
        if self.mode != LogMode::Clean || self.muted || self.rotate_if_full()? {
            return Ok(());
        }
        if scrollback_len < self.next_line {
            // The scrollback shrank, which only happens when it is deliberately
            // thrown away - Ctrl+Delete. Everything already written stays
            // written; counting restarts from what is there now, or the
            // transcript would go quiet until the history grew back past the
            // old mark.
            self.next_line = scrollback_len;
            return Ok(());
        }
        if scrollback_len == self.next_line {
            return Ok(());
        }
        for line in lines.iter().take(scrollback_len).skip(self.next_line) {
            writeln!(self.writer, "{line}")?;
            self.written += line.len() as u64 + 1;
        }
        self.next_line = scrollback_len;
        Ok(())
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
