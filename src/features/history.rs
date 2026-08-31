//! Commands typed at an IRIS prompt, recalled with Up and Down.
//!
//! IRIS recalls within a session and forgets everything the moment the process
//! ends, so a command typed yesterday is gone. This keeps the same list on the
//! app's side, shared by every tab, and — when the setting allows it — on disk,
//! so Up reaches back past the session that is open now.
//!
//! Nothing is recorded unless the terminal could see an IRIS prompt on the
//! line, which is what keeps a password (never echoed, never prompted with a
//! `>`) out of the file.

use std::path::{Path, PathBuf};

/// How many commands are kept. Enough to cover a working day of a routine
/// being compiled and re-run; small enough to load without thinking about it.
const LIMIT: usize = 500;

/// Longest command that is worth keeping. A pasted block of ObjectScript is
/// not something anyone recalls with Up, and it would push out everything that
/// is.
const MAX_LEN: usize = 2_000;

#[derive(Debug, Default)]
pub struct History {
    /// Oldest first, so the newest entry is what Up reaches first.
    entries: Vec<String>,
    /// Where to write. `None` means memory only — the setting is off, so
    /// nothing touches the disk.
    file: Option<PathBuf>,
}

impl History {
    /// Loads the saved commands when `persist` is set, and otherwise starts
    /// empty and stays in memory.
    pub fn load(path: &Path, persist: bool) -> Self {
        if !persist {
            return History::default();
        }
        let entries = std::fs::read_to_string(path)
            .map(|text| Self::from_text(&text))
            .unwrap_or_default();
        History {
            entries,
            file: Some(path.to_path_buf()),
        }
    }

    fn from_text(text: &str) -> Vec<String> {
        let mut entries: Vec<String> = text
            .lines()
            .map(str::trim_end)
            .filter(|l| !l.is_empty())
            .map(str::to_string)
            .collect();
        // A file that grew past the limit under an older build, or was edited
        // by hand, must not make the list unbounded.
        if entries.len() > LIMIT {
            entries.drain(..entries.len() - LIMIT);
        }
        entries
    }

    /// Turns saving on or off, writing what is already remembered when it is
    /// turned on so the switch is not silently retroactive.
    pub fn set_persist(&mut self, path: &Path, persist: bool) {
        match (persist, self.file.is_some()) {
            (true, false) => {
                self.file = Some(path.to_path_buf());
                self.save();
            }
            (false, true) => self.file = None,
            _ => {}
        }
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Records a command, most recent last.
    ///
    /// A repeat is moved to the end rather than added again: recalling the same
    /// line twice is normal, and without this the list fills up with one
    /// command.
    pub fn record(&mut self, command: &str) {
        let command = command.trim();
        if command.is_empty() || command.len() > MAX_LEN {
            return;
        }
        self.entries.retain(|e| e != command);
        self.entries.push(command.to_string());
        if self.entries.len() > LIMIT {
            let excess = self.entries.len() - LIMIT;
            self.entries.drain(..excess);
        }
        self.save();
    }

    /// The entry one step back from `index`, where `None` means "nothing
    /// recalled yet, start at the newest".
    ///
    /// Returns the index to remember, or `None` when there is nothing to go
    /// back to — at which point the caller leaves the line as it is.
    pub fn back(&self, index: Option<usize>) -> Option<usize> {
        match index {
            None => self.entries.len().checked_sub(1),
            Some(0) => None,
            Some(i) => Some(i - 1),
        }
    }

    /// The entry one step forward from `index`. `Some(None)` means the user has
    /// walked back out to the line they started on, which is empty; `None`
    /// means there was nothing to walk forward from.
    pub fn forward(&self, index: Option<usize>) -> Option<Option<usize>> {
        let i = index?;
        if i + 1 < self.entries.len() {
            Some(Some(i + 1))
        } else {
            Some(None)
        }
    }

    pub fn get(&self, index: usize) -> Option<&str> {
        self.entries.get(index).map(String::as_str)
    }

    /// Forgets everything, on disk too when saving is on.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.save();
    }

    fn save(&self) {
        let Some(path) = self.file.as_ref() else {
            return;
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut text = self.entries.join("\n");
        text.push('\n');
        if let Err(e) = std::fs::write(path, text) {
            // Worth a line in the log, but never worth interrupting a session
            // over: the history is a convenience.
            log::warn!("could not save command history to {}: {e}", path.display());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nit-history-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn commands_are_recorded_newest_last() {
        let mut h = History::default();
        h.record("write 1");
        h.record("write 2");
        assert_eq!(h.entries(), ["write 1", "write 2"]);
    }

    #[test]
    fn blank_and_oversized_commands_are_ignored() {
        let mut h = History::default();
        h.record("   ");
        h.record("");
        h.record(&"x".repeat(MAX_LEN + 1));
        assert!(h.is_empty());
    }

    /// Recalling and re-running a command must not fill the list with it.
    #[test]
    fn a_repeat_moves_to_the_end_instead_of_being_added_again() {
        let mut h = History::default();
        h.record("a");
        h.record("b");
        h.record("a");
        assert_eq!(h.entries(), ["b", "a"]);
    }

    #[test]
    fn the_list_is_capped() {
        let mut h = History::default();
        for i in 0..LIMIT + 10 {
            h.record(&format!("write {i}"));
        }
        assert_eq!(h.entries().len(), LIMIT);
        assert_eq!(h.entries()[0], format!("write {}", 10));
    }

    /// Up walks back from the newest and stops at the oldest.
    #[test]
    fn walking_back_starts_at_the_newest_and_stops_at_the_oldest() {
        let mut h = History::default();
        h.record("first");
        h.record("second");

        let at = h.back(None);
        assert_eq!(at, Some(1));
        assert_eq!(h.get(1), Some("second"));

        let at = h.back(at);
        assert_eq!(at, Some(0));
        assert_eq!(h.back(at), None, "there is nothing older to reach");
    }

    /// Down walks forward and then back out to the empty line the user was on.
    #[test]
    fn walking_forward_ends_on_the_line_the_user_started_from() {
        let mut h = History::default();
        h.record("first");
        h.record("second");

        assert_eq!(h.forward(Some(0)), Some(Some(1)));
        assert_eq!(h.forward(Some(1)), Some(None));
        assert_eq!(
            h.forward(None),
            None,
            "Down with nothing recalled is not ours to handle"
        );
    }

    #[test]
    fn an_empty_history_has_nothing_to_recall() {
        let h = History::default();
        assert_eq!(h.back(None), None);
        assert_eq!(h.forward(None), None);
    }

    #[test]
    fn saving_round_trips_through_the_file() {
        let dir = tempdir("round-trip");
        let path = dir.join("history.txt");

        let mut h = History::load(&path, true);
        h.record("write 1");
        h.record("zn \"USER\"");

        let back = History::load(&path, true);
        assert_eq!(back.entries(), ["write 1", "zn \"USER\""]);
    }

    /// Off means off: nothing may be written, and nothing already written may
    /// be read back.
    #[test]
    fn nothing_touches_the_disk_when_saving_is_off() {
        let dir = tempdir("off");
        let path = dir.join("history.txt");
        std::fs::write(&path, "earlier\n").unwrap();

        let mut h = History::load(&path, false);
        assert!(h.is_empty(), "an earlier file must not be loaded");
        h.record("write 1");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "earlier\n");
    }

    /// Turning the setting on mid-session writes what is already remembered,
    /// so the switch is not a promise that only starts applying tomorrow.
    #[test]
    fn turning_saving_on_writes_what_is_already_remembered() {
        let dir = tempdir("switch");
        let path = dir.join("history.txt");

        let mut h = History::load(&path, false);
        h.record("write 1");
        h.set_persist(&path, true);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "write 1\n");

        h.set_persist(&path, false);
        h.record("write 2");
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "write 1\n",
            "turning it off must stop the writes"
        );
    }

    #[test]
    fn a_hand_edited_file_is_trimmed_on_load() {
        let text: String = (0..LIMIT + 5)
            .map(|i| format!("write {i}\n"))
            .collect::<Vec<_>>()
            .concat();
        let entries = History::from_text(&text);
        assert_eq!(entries.len(), LIMIT);
        assert_eq!(entries[0], "write 5");
    }

    #[test]
    fn clearing_empties_the_file_too() {
        let dir = tempdir("clear");
        let path = dir.join("history.txt");

        let mut h = History::load(&path, true);
        h.record("write 1");
        h.clear();
        assert!(h.is_empty());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "\n");
    }
}
