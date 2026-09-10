//! Handing terminal output to Claude Code.
//!
//! The output is written to a file, and a `claude` session is opened in a tab
//! of its own with that file loaded into its context
//! (`--append-system-prompt-file`). No question is asked on the user's behalf:
//! the session comes up idle, knowing what is on the terminal, and waits for
//! whatever the user actually wanted to ask about it.
//!
//! The tab is an ordinary shell tab - see [`tab_profile`] - so nothing here
//! starts a process or waits on one. That was not always true: this used to
//! open a console window of the operating system's, which put the
//! conversation about the terminal somewhere other than the terminal.
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

use crate::config::profile::{Profile, ShellCommand};
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

/// Which of a split tab's panes to hand over.
///
/// Only ever a question when a tab *is* split. One pane is the honest default -
/// it is the one being typed in - but a split is usually two halves of one
/// problem (a routine running on the left, a global inspected on the right),
/// and cutting one of them out of the context is exactly what makes the answer
/// useless.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Panes {
    /// The pane the keyboard is in.
    #[default]
    Focused,
    /// Both of a split tab's sessions, each under a heading of its own.
    Both,
}

impl Panes {
    /// Menu entry. Keyed for translation like every other label.
    pub fn label(self) -> &'static str {
        match self {
            Panes::Focused => "This pane",
            Panes::Both => "Both panes",
        }
    }
}

/// One session's contribution to a context file.
pub struct Source<'a> {
    /// What the session is called - the profile's endpoint.
    pub endpoint: &'a str,
    pub grid: &'a Grid,
    /// What is highlighted in this session, for [`Scope::Selection`]. Lives in
    /// the view, not in the grid, so only the caller knows it.
    pub selection: Option<&'a str>,
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
///
/// `sources` is one session, or both panes of a split tab. Two of them arrive
/// as two headed sections rather than one run of text, so the model can tell
/// which prompt said what.
pub fn write_context(sources: &[Source], scope: Scope) -> Result<PathBuf> {
    write_context_in(&analysis_dir(), sources, scope)
}

/// [`write_context`], into a directory of the caller's choosing.
///
/// Split out for the tests, which have no business writing into the config
/// directory of whoever is running them - and which, hammering one directory in
/// parallel, were occasionally losing a race with the filesystem there.
fn write_context_in(dir: &Path, sources: &[Source], scope: Scope) -> Result<PathBuf> {
    // A pane with nothing in it is dropped rather than refused: asking about
    // both halves of a split where only one has been used is a reasonable
    // thing to do, and the empty half has nothing to contribute either way.
    let captured: Vec<(&str, String)> = sources
        .iter()
        .map(|source| (source.endpoint, text_of(source, scope)))
        .filter(|(_, text)| !text.trim().is_empty())
        .collect();
    if captured.is_empty() {
        if scope.is_selection() {
            bail!("nothing is selected");
        }
        bail!("there is no output to analyze yet");
    }

    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let path = free_path(dir)?;

    let mut body = format!(
        "# Context: the user's InterSystems IRIS terminal\n\n\
         Below is output captured from the terminal the user is working in - \
         ObjectScript on InterSystems IRIS/Caché. It is background for whatever \
         they are about to ask; there is no question in it, so wait for theirs \
         rather than volunteering an analysis of it.\n\n\
         - Captured: {}\n\
         - Scope: {}\n",
        chrono::Local::now().format("%Y-%m-%d %H:%M:%S"),
        scope.label(),
    );
    // One session reads as one capture, so it keeps the flat shape it always
    // had. Two get a heading each, and a line saying they were side by side -
    // which is the fact that makes reading them together worth anything.
    if let [(endpoint, text)] = captured.as_slice() {
        body.push_str(&format!("- Session: {endpoint}\n\n```text\n{text}\n```\n"));
    } else {
        body.push_str(&format!(
            "- Panes: {} sessions, side by side in one split view\n",
            captured.len()
        ));
        for (n, (endpoint, text)) in captured.iter().enumerate() {
            body.push_str(&format!(
                "\n## Pane {} - {endpoint}\n\n```text\n{text}\n```\n",
                n + 1
            ));
        }
    }
    std::fs::write(&path, body).with_context(|| format!("writing {}", path.display()))?;
    Ok(path)
}

/// What `scope` covers in one session.
fn text_of(source: &Source, scope: Scope) -> String {
    if scope.is_selection() {
        // The selection is the caller's to know: it lives in the view, not in
        // the grid, and the grid has no idea what the mouse has been doing.
        source.selection.unwrap_or_default().trim_end().to_string()
    } else {
        transcript(source.grid, scope)
    }
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

/// The profile that opens a Claude Code session on `path`, as a tab.
///
/// A tab rather than a window of the operating system's, which is what this
/// used to open: the conversation is about what is on the terminal, and having
/// it in the terminal is the difference between a second window to find on the
/// task bar and a tab beside the session it is about. Everything a shell tab
/// already does - the pseudo-terminal, the reader thread, the resize - applies
/// unchanged, because a `claude` is a program in a terminal like any other.
///
/// The program is named bare rather than resolved: on Windows `claude` is a
/// `.cmd` shim, which `CreateProcess` cannot start but the `cmd` wrapper a
/// shell tab is already given resolves off PATH. And the file is passed by
/// *name*, with the session started in the folder that holds it, because that
/// command line has no way to quote a space - see `escape_for_cmd` in
/// [`crate::pty::launcher`].
pub fn tab_profile(path: &Path) -> Profile {
    let dir = path
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(analysis_dir);
    let file = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string());
    Profile {
        name: CLAUDE.to_string(),
        instance: CLAUDE.to_string(),
        namespace: String::new(),
        shell: Some(ShellCommand {
            program: PathBuf::from(CLAUDE),
            args: vec![CONTEXT_FLAG.to_string(), file],
            cwd: Some(dir),
        }),
        ..Profile::default()
    }
}

/// The program, which is also what the tab is called.
const CLAUDE: &str = "claude";

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
            grid.screen[r].set_text(line);
        }
        grid
    }

    /// One pane's worth of context, the shape nearly every test wants.
    fn source<'a>(grid: &'a Grid, selection: Option<&'a str>) -> Source<'a> {
        Source {
            endpoint: "TEST",
            grid,
            selection,
        }
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
        let result = write_context_in(&dir, &[source(&grid, None)], Scope::All);
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
        let first = write_context_in(&dir, &[source(&grid, None)], Scope::All).expect("first");
        let second = write_context_in(&dir, &[source(&grid, None)], Scope::All).expect("second");
        assert_ne!(first, second);
        assert!(first.exists() && second.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Both panes: each session under a heading of its own, so the model can
    /// tell which prompt said what instead of reading one run of interleaved
    /// output.
    #[test]
    fn both_panes_are_handed_over_as_two_headed_sections() {
        let dir = tempdir("both");
        let left = grid_with(&["USER>write 1", "1"]);
        let right = grid_with(&["%SYS>write 2", "2"]);
        let path = write_context_in(
            &dir,
            &[
                Source {
                    endpoint: "IRIS:USER",
                    grid: &left,
                    selection: None,
                },
                Source {
                    endpoint: "IRIS:%SYS",
                    grid: &right,
                    selection: None,
                },
            ],
            Scope::All,
        )
        .expect("written");
        let body = std::fs::read_to_string(&path).expect("read back");
        assert!(body.contains("## Pane 1 - IRIS:USER"), "{body}");
        assert!(body.contains("## Pane 2 - IRIS:%SYS"), "{body}");
        assert!(body.contains("USER>write 1") && body.contains("%SYS>write 2"));
        assert!(
            body.contains("side by side"),
            "the file should say the two were on screen together: {body}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A split where only one pane has been used still analyses: the empty
    /// half is dropped, and what is left reads as the single capture it is.
    #[test]
    fn an_empty_pane_is_left_out_rather_than_refusing_the_pair() {
        let dir = tempdir("half");
        let used = grid_with(&["USER>write 1", "1"]);
        let fresh = Grid::new(40, 6, 100);
        let path = write_context_in(
            &dir,
            &[source(&used, None), source(&fresh, None)],
            Scope::All,
        )
        .expect("written");
        let body = std::fs::read_to_string(&path).expect("read back");
        assert!(body.contains("USER>write 1"));
        assert!(
            !body.contains("## Pane"),
            "one capture, no headings: {body}"
        );
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
            &[source(&grid, Some("<UNDEFINED> zRun+7"))],
            Scope::Selection,
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
            assert!(write_context_in(&dir, &[source(&grid, selection)], Scope::Selection).is_err());
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The session must come up with the file loaded and *nothing* asked: no
    /// prompt argument, or it would answer a question the user never put.
    #[test]
    fn the_tab_loads_the_file_and_asks_nothing() {
        let path = Path::new("/tmp/an analysis/session-20260101-120000.md");
        let profile = tab_profile(path);
        let shell = profile.shell.expect("it has to be a shell profile");
        assert_eq!(shell.program, Path::new("claude"));
        assert_eq!(
            shell.args,
            ["--append-system-prompt-file", "session-20260101-120000.md"]
        );
        // The file is the last word: anything after it would be read as the
        // prompt, and the session would start by answering it.
        assert_eq!(
            shell.args.last().map(String::as_str),
            Some("session-20260101-120000.md")
        );
    }

    /// A space in the path is what a `cmd` command line cannot survive, so the
    /// file is named bare and the session is started in its folder. This is
    /// the pair that has to hold together, whatever the path looks like.
    #[test]
    fn the_file_is_named_bare_and_the_session_starts_beside_it() {
        let path = Path::new("/tmp/an analysis/session-20260101-120000.md");
        let shell = tab_profile(path).shell.unwrap();
        assert_eq!(shell.cwd.as_deref(), path.parent());
        assert!(
            !shell.args.iter().any(|arg| arg.contains(' ')),
            "nothing on the command line may carry a space"
        );
    }

    /// A shell tab, so nothing IRIS-shaped runs against it: no autologon
    /// typing a username at Claude, and no ObjectScript colouring.
    #[test]
    fn the_analysis_tab_is_a_shell_tab() {
        assert!(tab_profile(Path::new("x.md")).is_shell());
    }

    /// The file is context, not a request: it must not read as a task, or the
    /// session will act on it before the user has said anything.
    #[test]
    fn the_context_file_asks_for_nothing() {
        let dir = tempdir("context");
        let grid = grid_with(&["USER>write 1", "1"]);
        let path = write_context_in(&dir, &[source(&grid, None)], Scope::All).expect("written");
        let body = std::fs::read_to_string(&path).expect("read back");
        assert!(body.contains("USER>write 1"));
        assert!(body.contains("wait for theirs"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
