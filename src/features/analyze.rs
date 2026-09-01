//! Handing terminal output to Claude Code.
//!
//! The output is written to a file, and a `claude` session is opened in a
//! terminal of its own with that file loaded into its context
//! (`--append-system-prompt-file`). No question is asked on the user's behalf:
//! the session comes up idle, knowing what is on the terminal, and waits for
//! whatever the user actually wanted to ask about it.
//!
//! A file rather than an argument because a screen of IRIS output is routinely
//! tens of kilobytes, which is past what Windows accepts on a command line.
//!
//! What "the output" means is the user's choice. Everything is the honest
//! default, but a working scrollback holds a whole afternoon; the other two
//! scopes cut it back to the last few commands, which is nearly always where
//! the thing being asked about is.

use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

use crate::term::{syntax, Grid, Row};

/// How much of the session to hand over.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    /// Scrollback and screen, from the first row the terminal still holds.
    All,
    LastTen,
    LastFive,
    /// Only what is highlighted - the one scope the user draws by hand, for a
    /// stack trace or a single line of output in the middle of a long session.
    Selection,
}

impl Scope {
    pub const ALL: [Scope; 4] = [
        Scope::All,
        Scope::LastTen,
        Scope::LastFive,
        Scope::Selection,
    ];

    /// Whether this scope comes from the mouse rather than from the grid.
    pub fn is_selection(self) -> bool {
        self == Scope::Selection
    }

    /// How many commands back to reach, or `None` for everything.
    fn commands(self) -> Option<usize> {
        match self {
            Scope::All | Scope::Selection => None,
            Scope::LastTen => Some(10),
            Scope::LastFive => Some(5),
        }
    }

    /// Menu entry. Keyed for translation like every other label.
    pub fn label(self) -> &'static str {
        match self {
            Scope::All => "All output",
            Scope::LastTen => "Last 10 commands",
            Scope::LastFive => "Last 5 commands",
            Scope::Selection => "Selection",
        }
    }
}

/// Whether this row is a command line: an IRIS prompt with something typed
/// after it.
///
/// The prompt alone is not enough. The last row of a session is almost always
/// a bare prompt waiting for input, and counting it as a command would spend
/// one of the five the user asked for on nothing at all.
fn is_command(row: &Row) -> bool {
    let Some(end) = syntax::prompt_end_of(&row.cells) else {
        return false;
    };
    row.cells
        .get(end..)
        .is_some_and(|rest| rest.iter().any(|cell| !cell.ch.is_whitespace()))
}

/// The rows `scope` covers, oldest first.
fn rows(grid: &Grid, scope: Scope) -> Vec<&Row> {
    let all: Vec<&Row> = grid.scrollback.iter().chain(grid.screen.iter()).collect();
    let Some(wanted) = scope.commands() else {
        return all;
    };
    // Where each command starts. Counting from the end, so a session with
    // fewer commands than asked for hands over what there is rather than
    // nothing.
    let marks: Vec<usize> = all
        .iter()
        .enumerate()
        .filter(|(_, row)| is_command(row))
        .map(|(index, _)| index)
        .collect();
    let start = marks
        .len()
        .checked_sub(wanted)
        .and_then(|nth| marks.get(nth))
        .copied()
        .unwrap_or(0);
    all[start..].to_vec()
}

/// The output itself, trailing blank rows trimmed.
pub fn transcript(grid: &Grid, scope: Scope) -> String {
    let mut text = rows(grid, scope)
        .into_iter()
        .map(Row::to_text)
        .collect::<Vec<_>>()
        .join("\n");
    while text.ends_with('\n') || text.ends_with(' ') {
        text.pop();
    }
    text
}

/// Where the files handed to Claude are kept.
///
/// Under the config directory rather than the system temp folder: the session
/// reads the file after the app has moved on, and a cleaner emptying `%TEMP%`
/// mid-analysis would take the context out from under it.
pub fn analysis_dir() -> PathBuf {
    crate::config::config_dir().join("analysis")
}

/// Writes the context file and returns its path.
///
/// Markdown, with the output in a fenced block: it is read by a model, and the
/// fence is what keeps a line of IRIS output from being taken for a heading.
///
/// Written as context rather than as a request. It is appended to the session's
/// system prompt, so nothing here should read as a task - the user has not
/// asked anything yet, and the session should not answer a question nobody
/// put.
pub fn write_context(
    grid: &Grid,
    scope: Scope,
    endpoint: &str,
    selection: Option<&str>,
) -> Result<PathBuf> {
    write_context_in(&analysis_dir(), grid, scope, endpoint, selection)
}

/// [`write_context`], into a directory of the caller's choosing.
///
/// Split out for the tests, which have no business writing into the config
/// directory of whoever is running them - and which, hammering one directory in
/// parallel, were occasionally losing a race with the filesystem there.
fn write_context_in(
    dir: &Path,
    grid: &Grid,
    scope: Scope,
    endpoint: &str,
    selection: Option<&str>,
) -> Result<PathBuf> {
    // The selection is the caller's to know: it lives in the view, not in the
    // grid, and the grid has no idea what the mouse has been doing.
    let text = if scope.is_selection() {
        selection.unwrap_or_default().trim_end().to_string()
    } else {
        transcript(grid, scope)
    };
    if text.trim().is_empty() {
        if scope.is_selection() {
            bail!("nothing is selected");
        }
        bail!("there is no output to analyze yet");
    }

    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = free_path(dir)?;

    let body = format!(
        "# Context: the user's InterSystems IRIS terminal\n\n\
         Below is output captured from the terminal session the user is working \
         in - ObjectScript on InterSystems IRIS/Caché. It is background for \
         whatever they are about to ask; there is no question in it, so wait \
         for theirs rather than volunteering an analysis of it.\n\n\
         - Session: {endpoint}\n\
         - Captured: {}\n\
         - Scope: {}\n\n\
         ```text\n{text}\n```\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        scope.label(),
    );
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// A file in `dir` that does not exist yet, and now does.
///
/// The name is a timestamp, which is only unique to the second - and two
/// analyses of the same screen, one after the other, are comfortably inside one
/// second. The second would have overwritten the first, and a session opened on
/// the first would find the other one's output. Created with `create_new` so
/// the check and the claim are one step and two of these cannot pick the same
/// name.
fn free_path(dir: &Path) -> Result<PathBuf> {
    let stamp = chrono::Local::now().format("%Y%m%d-%H%M%S");
    for attempt in 1..100 {
        let path = if attempt == 1 {
            dir.join(format!("session-{stamp}.md"))
        } else {
            dir.join(format!("session-{stamp}-{attempt}.md"))
        };
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e).with_context(|| format!("creating {}", path.display())),
        }
    }
    bail!("could not find a free name in {}", dir.display())
}

/// The flag that loads a file into a session's context without asking it
/// anything. With no prompt after it, `claude` comes up interactive and idle.
const CONTEXT_FLAG: &str = "--append-system-prompt-file";

/// Opens a Claude Code session in a terminal window of its own, with `path`
/// already in its context.
///
/// A window rather than a captured child process: this is a conversation, and
/// the point is that the user carries on with it after the terminal has handed
/// it over. The app never waits on it and never reads its output.
pub fn launch(path: &Path) -> Result<()> {
    let dir = analysis_dir();

    #[cfg(target_os = "windows")]
    {
        windows_command(path, &dir)
            .spawn()
            .context("starting claude in a new window")?;
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        // Terminal.app takes a shell line, so the prompt has to survive one
        // round of AppleScript quoting and one of shell quoting.
        let script = format!(
            "tell application \"Terminal\" to do script \"cd {} && claude {CONTEXT_FLAG} {}\"",
            shell_quote(&dir.display().to_string()),
            shell_quote(&path.display().to_string()),
        );
        std::process::Command::new("osascript")
            .args(["-e", &script])
            .spawn()
            .context("starting claude in Terminal.app")?;
        return Ok(());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // No single terminal is a safe bet on Linux, so the ones that are
        // usually there are tried in turn. `x-terminal-emulator` is the
        // Debian alternative and comes first for that reason.
        let candidates = [
            "x-terminal-emulator",
            "gnome-terminal",
            "konsole",
            "xfce4-terminal",
            "xterm",
        ];
        for program in candidates {
            let spawned = std::process::Command::new(program)
                .arg("-e")
                .arg(format!(
                    "claude {CONTEXT_FLAG} {}",
                    shell_quote(&path.display().to_string())
                ))
                .current_dir(&dir)
                .spawn();
            if spawned.is_ok() {
                return Ok(());
            }
        }
        bail!("no terminal emulator found to run claude in");
    }
}

/// The command that opens the session on Windows.
///
/// `start` is what detaches the window; the inner `cmd /k` is what keeps it
/// open once the session ends, so an error from `claude` is readable instead of
/// vanishing with the window. The empty string is `start`'s window title, which
/// it would otherwise take from the first quoted argument - and the first
/// quoted argument here is the path to the context file.
///
/// Built here rather than inline so a test can read the arguments back: it is
/// the one part of this that is easy to get wrong and impossible to see.
#[cfg(target_os = "windows")]
fn windows_command(path: &Path, dir: &Path) -> std::process::Command {
    let mut command = std::process::Command::new("cmd");
    command
        .args(["/c", "start", "", "cmd", "/k", "claude", CONTEXT_FLAG])
        .arg(path)
        .current_dir(dir);
    command
}

/// Wraps a string in single quotes for a POSIX shell.
#[cfg(unix)]
fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', "'\\''"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::term::Grid;

    /// A directory of this test's own, gone by the time it returns.
    fn tempdir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("nit-analyze-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create");
        dir
    }

    /// One row per line, written straight into the cells - the same shortcut
    /// the terminal view's own tests use.
    fn grid_with(lines: &[&str]) -> Grid {
        let mut grid = Grid::new(40, lines.len().max(1), 100);
        for (r, line) in lines.iter().enumerate() {
            for (c, ch) in line.chars().enumerate() {
                grid.screen[r].cells[c].ch = ch;
            }
        }
        grid
    }

    #[test]
    fn a_prompt_with_a_command_on_it_is_a_command() {
        let grid = grid_with(&["USER>write 1", "1", "USER>"]);
        let rows: Vec<&Row> = grid.scrollback.iter().chain(grid.screen.iter()).collect();
        let commands: Vec<String> = rows
            .iter()
            .filter(|row| is_command(row))
            .map(|row| row.to_text())
            .collect();
        assert_eq!(commands, vec!["USER>write 1".to_string()]);
    }

    /// Everything means everything: no command counting, no truncation.
    #[test]
    fn the_whole_scope_keeps_the_first_line() {
        let grid = grid_with(&["USER>write 1", "1", "USER>write 2", "2"]);
        let text = transcript(&grid, Scope::All);
        assert!(text.starts_with("USER>write 1"), "unexpected: {text:?}");
        assert!(text.contains("USER>write 2"));
    }

    /// The point of the feature: the last few commands, not the afternoon.
    #[test]
    fn a_narrow_scope_starts_at_the_nth_command_from_the_end() {
        let mut lines: Vec<String> = Vec::new();
        for n in 0..8 {
            lines.push(format!("USER>write {n}"));
            lines.push(format!("{n}"));
        }
        let refs: Vec<&str> = lines.iter().map(String::as_str).collect();
        let grid = grid_with(&refs);

        let text = transcript(&grid, Scope::LastFive);
        assert!(
            text.starts_with("USER>write 3"),
            "should start at the fifth command from the end: {text:?}"
        );
        assert!(text.contains("USER>write 7"));
        assert!(!text.contains("USER>write 2"));
    }

    /// Fewer commands than asked for hands over what there is, rather than
    /// nothing or a panic.
    #[test]
    fn a_short_session_is_handed_over_whole() {
        let grid = grid_with(&["USER>write 1", "1"]);
        let text = transcript(&grid, Scope::LastTen);
        assert!(text.starts_with("USER>write 1"), "unexpected: {text:?}");
    }

    /// A session that has only ever shown a prompt has nothing to analyze, and
    /// must say so rather than opening a session over an empty file.
    #[test]
    fn an_empty_session_is_refused() {
        let dir = tempdir("empty");
        let grid = Grid::new(40, 6, 100);
        let result = write_context_in(&dir, &grid, Scope::All, "TEST", None);
        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Two analyses in the same second must not land on one file: the second
    /// would overwrite the first, and the session opened on the first would
    /// read the other one's output.
    #[test]
    fn two_captures_in_the_same_second_get_two_files() {
        let dir = tempdir("twice");
        let grid = grid_with(&["USER>write 1", "1"]);
        let first = write_context_in(&dir, &grid, Scope::All, "TEST", None).expect("first");
        let second = write_context_in(&dir, &grid, Scope::All, "TEST", None).expect("second");
        assert_ne!(first, second);
        assert!(first.exists() && second.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The selection is handed over as it is, and nothing of the surrounding
    /// session goes with it: picking three lines out of a long session is the
    /// whole point of the scope.
    #[test]
    fn the_selection_scope_uses_only_what_was_selected() {
        let dir = tempdir("selection");
        let grid = grid_with(&["USER>write 1", "1", "USER>write 2", "2"]);
        let path = write_context_in(
            &dir,
            &grid,
            Scope::Selection,
            "TEST",
            Some("<UNDEFINED> zRun+7"),
        )
        .expect("written");
        let body = std::fs::read_to_string(&path).expect("read back");
        assert!(body.contains("<UNDEFINED> zRun+7"));
        assert!(
            !body.contains("USER>write 1"),
            "the rest of the session came along: {body}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Asking for the selection without one must say so rather than handing
    /// over an empty file, or the whole session as a "helpful" substitute.
    #[test]
    fn the_selection_scope_needs_a_selection() {
        let dir = tempdir("no-selection");
        let grid = grid_with(&["USER>write 1", "1"]);
        for selection in [None, Some(""), Some("   ")] {
            assert!(write_context_in(&dir, &grid, Scope::Selection, "TEST", selection).is_err());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The session must come up with the file loaded and *nothing* asked: no
    /// prompt argument, or it would answer a question the user never put.
    #[cfg(target_os = "windows")]
    #[test]
    fn the_windows_command_loads_the_file_and_asks_nothing() {
        let dir = analysis_dir();
        let path = Path::new("C:\\Program Files\\x\\session.md");
        let command = windows_command(path, &dir);
        let args: Vec<String> = command
            .get_args()
            .map(|a| a.to_string_lossy().to_string())
            .collect();
        assert_eq!(command.get_program(), "cmd");
        assert_eq!(
            args,
            [
                "/c",
                "start",
                "",
                "cmd",
                "/k",
                "claude",
                "--append-system-prompt-file",
                "C:\\Program Files\\x\\session.md",
            ]
        );
        // The path is the last word: anything after it would be read as the
        // prompt, and the session would start by answering it.
        assert_eq!(
            args.last().map(String::as_str),
            Some(path.to_str().unwrap())
        );
        assert_eq!(command.get_current_dir(), Some(dir.as_path()));
    }

    /// The file is context, not a request: it must not read as a task, or the
    /// session will act on it before the user has said anything.
    #[test]
    fn the_context_file_asks_for_nothing() {
        let dir = tempdir("context");
        let grid = grid_with(&["USER>write 1", "1"]);
        let path = write_context_in(&dir, &grid, Scope::All, "TEST", None).expect("written");
        let body = std::fs::read_to_string(&path).expect("read back");
        assert!(body.contains("USER>write 1"));
        assert!(body.contains("wait for theirs"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
