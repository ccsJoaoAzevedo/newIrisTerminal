//! Commands typed at an IRIS prompt, recalled with Up and Down.
//!
//! IRIS recalls within a session and forgets everything the moment the process
//! ends, so a command typed yesterday is gone. This keeps the same list on the
//! app's side and — when the setting allows it — on disk, so Up reaches back
//! past the session that is open now.
//!
//! What Up reaches is per session, not per app: a tab offers back what was
//! typed at *its* prompt, and only once those run out does it go on to the
//! commands inherited from earlier runs. A command typed in another tab a
//! minute ago is not offered here at all — it belongs to that session's train
//! of thought, and it will be inherited soon enough, at the next start. See
//! [`History::recall_list`].
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
    /// Oldest first, so the newest entry is what Up reaches first. Everything
    /// typed in this run, in any tab, plus whatever was loaded from the file.
    /// This is what is saved for the next start.
    entries: Vec<String>,
    /// The commands as they were when the app started: the ones from earlier
    /// sessions. Fixed for the run, because it is what every tab falls back to
    /// once its own commands run out - and a tab must not start offering back
    /// what another tab typed while it was open.
    earlier: Vec<String>,
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
            earlier: entries.clone(),
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

    /// The commands from earlier sessions, oldest first. What a tab falls back
    /// to once its own are exhausted.
    pub fn earlier(&self) -> &[String] {
        &self.earlier
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// What Up walks through for a session whose own commands are `own`,
    /// newest first.
    ///
    /// The session's own come first, so Up in a tab reaches what was typed in
    /// that tab; then the ones inherited from earlier runs, with anything the
    /// session has already typed left out so no command is offered twice.
    pub fn recall_list<'a>(&'a self, own: &'a [String]) -> Vec<&'a str> {
        let mut list: Vec<&str> = own.iter().rev().map(String::as_str).collect();
        list.extend(
            self.earlier
                .iter()
                .rev()
                .map(String::as_str)
                .filter(|command| !own.iter().any(|mine| mine == command)),
        );
        list
    }

    /// Records a command, most recent last.
    ///
    /// A repeat is moved to the end rather than added again: recalling the same
    /// line twice is normal, and without this the list fills up with one
    /// command.
    pub fn record(&mut self, command: &str) {
        if push_recent(&mut self.entries, command) {
            self.save();
        }
    }

    /// Forgets everything, on disk too when saving is on.
    pub fn clear(&mut self) {
        self.entries.clear();
        self.earlier.clear();
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

/// Adds a command to a most-recent-last list, and says whether it changed.
///
/// One definition of what is worth keeping, shared by the file and by each
/// session's own list: blank and enormous lines are not commands, a repeat is
/// moved to the end rather than added again - recalling the same line twice is
/// normal, and without this the list fills up with one command - and the list
/// is capped.
pub fn push_recent(entries: &mut Vec<String>, command: &str) -> bool {
    let command = command.trim();
    if command.is_empty() || command.len() > MAX_LEN {
        return false;
    }
    entries.retain(|e| e != command);
    entries.push(command.to_string());
    if entries.len() > LIMIT {
        let excess = entries.len() - LIMIT;
        entries.drain(..excess);
    }
    true
}

/// Where Up lands, one step back from `at` through `len` candidates.
///
/// `Some(Some(step))` is a command to put on the line and `Some(None)` the line
/// the user started on; the outer `None` means there is nowhere to go, and the
/// line is left exactly as it is rather than being cleared.
pub fn step_back(at: Option<usize>, len: usize) -> Option<Option<usize>> {
    let next = at.map_or(0, |step| step + 1);
    (next < len).then_some(Some(next))
}

/// Where Down lands. `None` when nothing has been recalled, which is not the
/// app's key to act on.
pub fn step_forward(at: Option<usize>) -> Option<Option<usize>> {
    Some(at?.checked_sub(1))
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
        assert_eq!(step_back(None, 2), Some(Some(0)), "the newest first");
        assert_eq!(step_back(Some(0), 2), Some(Some(1)));
        assert_eq!(
            step_back(Some(1), 2),
            None,
            "there is nothing older to reach"
        );
    }

    /// Down walks forward and then back out to the empty line the user was on.
    #[test]
    fn walking_forward_ends_on_the_line_the_user_started_from() {
        assert_eq!(step_forward(Some(1)), Some(Some(0)));
        assert_eq!(step_forward(Some(0)), Some(None), "back to an empty line");
        assert_eq!(
            step_forward(None),
            None,
            "Down with nothing recalled is not ours to handle"
        );
    }

    #[test]
    fn an_empty_history_has_nothing_to_recall() {
        assert_eq!(step_back(None, 0), None);
        assert_eq!(step_forward(None), None);
    }

    /// The whole point of the split list: a tab offers back what was typed in
    /// it, and only then what earlier runs left behind. What another tab typed
    /// while this one was open is not offered at all.
    #[test]
    fn a_session_recalls_its_own_commands_before_the_inherited_ones() {
        let dir = tempdir("per-session");
        let path = dir.join("history.txt");
        std::fs::write(&path, "old one\nold two\n").unwrap();

        let mut h = History::load(&path, true);
        // Typed in another tab during this run.
        h.record("someone else's");

        let own = vec!["mine one".to_string(), "mine two".to_string()];
        assert_eq!(
            h.recall_list(&own),
            ["mine two", "mine one", "old two", "old one"]
        );
        // ... and it is still saved, for the next start to inherit.
        assert!(h.entries().iter().any(|e| e == "someone else's"));
    }

    /// A command the session has already typed is not offered twice just
    /// because an earlier run left it in the file.
    #[test]
    fn an_inherited_command_the_session_has_retyped_is_offered_once() {
        let dir = tempdir("dedupe");
        let path = dir.join("history.txt");
        std::fs::write(&path, "write 1\nwrite 2\n").unwrap();

        let h = History::load(&path, true);
        let own = vec!["write 2".to_string()];
        assert_eq!(h.recall_list(&own), ["write 2", "write 1"]);
    }

    /// A session with nothing typed in it yet still reaches yesterday's work.
    #[test]
    fn a_fresh_session_falls_straight_through_to_the_inherited_commands() {
        let dir = tempdir("fresh");
        let path = dir.join("history.txt");
        std::fs::write(&path, "old one\n").unwrap();

        let h = History::load(&path, true);
        assert_eq!(h.recall_list(&[]), ["old one"]);
        assert!(
            History::default().recall_list(&[]).is_empty(),
            "and one with nothing behind it recalls nothing"
        );
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

    /// The list a session builds up follows the same rules as the file's.
    #[test]
    fn a_sessions_own_list_keeps_the_newest_last_and_never_repeats() {
        let mut own = Vec::new();
        assert!(push_recent(&mut own, "write 1"));
        assert!(push_recent(&mut own, "write 2"));
        assert!(
            push_recent(&mut own, "write 1"),
            "a repeat moves to the end"
        );
        assert_eq!(own, ["write 2", "write 1"]);
        assert!(!push_recent(&mut own, "   "), "blank is not a command");
        assert!(!push_recent(&mut own, &"x".repeat(MAX_LEN + 1)));
        assert_eq!(own.len(), 2);
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
