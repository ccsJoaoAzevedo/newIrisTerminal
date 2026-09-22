//! One tab: a session, its screen, and everything that belongs to it.
//!
//! A tab owns the PTY, the parsed grid, the scrollback view and the log. When
//! it is split it owns the second session too, in [`Split`] - see
//! [`super::layout`] for how the two are addressed.

use super::*;

/// Rotate a transcript once it passes this size.
const LOG_ROTATE_BYTES: u64 = 64 * 1024 * 1024;

/// One open session and everything that hangs off it.
pub struct Tab {
    /// Stable for this tab's lifetime and unique across tabs, so the terminal
    /// widget keeps one identity even as tabs are opened, closed and reordered.
    pub uid: u64,
    pub profile: Profile,
    pub session: Option<Session>,
    pub grid: Grid,
    pub parser: vte::Parser,
    pub view: ViewState,
    pub autologon: Autologon,
    /// Live piece-structure lookups for the tooltip - see
    /// [`crate::features::doc_lookup`]. Not reset by [`Tab::start`]: a
    /// reconnect does not change what a global's pieces mean.
    pub doc_lookup: DocLookup,
    pub log: Option<SessionLog>,
    /// User-set name; falls back to the OSC title, then the profile name.
    pub custom_title: Option<String>,
    /// Namespace the session was last seen to be in, read off its prompt. The
    /// tab name carries it when `show_namespace_in_tab` is on, and it follows a
    /// `ZN` because the prompt does.
    pub namespace: Option<String>,
    /// Commands typed at this session's own prompt, oldest last.
    ///
    /// What Up offers back first: a tab recalls its own train of thought, not
    /// the one next to it. Only once these run out does the recall go on to the
    /// commands inherited from earlier runs - see
    /// [`crate::features::history::History::recall_list`].
    pub commands: Vec<String>,
    /// How far back through the recall this tab has walked, if at all. 0 is the
    /// newest candidate. Per session, so recalling in one pane does not move
    /// another.
    pub recall_step: Option<usize>,
    /// When a clear-screen was asked of IRIS, so a purge that never arrives can
    /// be called off. See [`Tab::request_clear`].
    pub(super) clear_asked: Option<std::time::Instant>,
    /// Set when the child exits, so the tab explains itself instead of freezing.
    pub ended: bool,
    pub error: Option<String>,
    /// The second session sharing this tab, once it has been split. Always
    /// `None` on the second session itself: a split tab cannot be split again.
    pub split: Option<Split>,
    /// Which pane has the keyboard. Meaningless until the tab is split, and
    /// what the strip entry's `1:` or `2:` reports.
    pub focus: Pane,
    /// The easter egg, in the tab `/snake` opened. A tab holding one holds no
    /// session and never will: it is drawn by
    /// [`crate::ui::snake_view`] instead of by the terminal, and every path
    /// that would pump, resize or type into a session asks this first. See
    /// `App::take_easter_egg`.
    pub game: Option<Box<Snake>>,
}

/// Source of [`Tab::uid`]. Never reused, so a closed tab's id cannot collide
/// with a later one and resurrect its focus or selection state.
static NEXT_TAB_UID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// What makes `program` clear its own screen, for [`Tab::request_clear`].
///
/// Ctrl+L everywhere it is understood, which is every shell with a line editor
/// worth the name: PSReadLine binds it in both PowerShells, and readline binds
/// it in `bash`, `zsh` and `fish`. A keystroke rather than a command line is
/// what lets the gesture work with something already half typed - the shell
/// clears the screen and paints the line back, losing nothing.
///
/// `cmd.exe` is the exception: its line editor has no such binding and echoes
/// `^L` as a character, so it gets the command instead, with an Escape in front
/// to empty the input line first - which is what Escape does there, and what
/// keeps `cls` from being appended to a half-typed command.
///
/// Hence a list rather than one string: the Escape has to be a write of its
/// own. A pseudoconsole turns what it reads back into key presses, and an
/// Escape with more bytes behind it in the same read is the start of a
/// sequence rather than the key - so sent in one piece it is swallowed, and
/// `cls` lands on the end of whatever was typed. Two writes are two reads even
/// back to back, with no pause between them.
fn clear_gesture(program: &std::path::Path) -> &'static [&'static [u8]] {
    let is_cmd = program
        .file_stem()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.eq_ignore_ascii_case("cmd"));
    if is_cmd {
        &[b"\x1b", b"cls\r"]
    } else {
        &[b"\x0c"]
    }
}

impl Tab {
    pub fn new(profile: Profile, settings: &Settings, cols: u16, rows: u16) -> Self {
        let log = open_log(&profile, settings);
        let mut tab = Tab {
            uid: NEXT_TAB_UID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            autologon: Autologon::new(&profile),
            doc_lookup: DocLookup::default(),
            grid: Grid::new(cols as usize, rows as usize, settings.scrollback_limit),
            profile,
            session: None,
            parser: vte::Parser::new(),
            view: ViewState::default(),
            log,
            custom_title: None,
            namespace: None,
            commands: Vec::new(),
            recall_step: None,
            clear_asked: None,
            ended: false,
            error: None,
            split: None,
            focus: Pane::First,
            game: None,
        };
        tab.start();
        tab
    }

    /// A tab holding the easter egg instead of a session.
    ///
    /// Nothing is started, nothing is logged and nothing is connected: this
    /// tab has no far side at all, which is what every `session.is_none()`
    /// path through the shell already copes with. The grid it carries is never
    /// drawn - [`crate::ui::snake_view`] draws the board instead - so it is
    /// made at the smallest size the rest of the code will accept rather than
    /// at the terminal's own.
    pub fn snake(game: Snake) -> Self {
        Tab {
            uid: NEXT_TAB_UID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            autologon: Autologon::new(&Profile::default()),
            doc_lookup: DocLookup::default(),
            grid: Grid::new(2, 2, 0),
            profile: Profile {
                name: crate::i18n::tr("Snake").to_string(),
                ..Profile::default()
            },
            session: None,
            parser: vte::Parser::new(),
            view: ViewState::default(),
            log: None,
            // Left for the user to set, the way any other tab's is: the name
            // on the strip comes from the profile below, through the ordinary
            // path.
            custom_title: None,
            namespace: None,
            commands: Vec::new(),
            recall_step: None,
            clear_asked: None,
            ended: false,
            error: None,
            split: None,
            focus: Pane::First,
            game: Some(Box::new(game)),
        }
    }

    /// Whether this tab is the easter egg rather than a session. Asked by
    /// everything that would otherwise treat a tab as a terminal - drawing it,
    /// sizing it, splitting it.
    pub fn is_game(&self) -> bool {
        self.game.is_some()
    }

    pub fn start(&mut self) {
        self.ended = false;
        self.error = None;
        self.autologon = Autologon::new(&self.profile);

        let cols = self.grid.cols as u16;
        let rows = self.grid.rows as u16;

        // Three things this could be, and the profile has already decided
        // which. A shell is a program on this machine with no instance and
        // nothing to log in to; a remote server has no process here at all and
        // is reached by logging in to its Telnet service, the same way the
        // launcher's own Terminal reaches it. Everything above this point is
        // identical for all three.
        let opened = match (self.profile.shell.as_ref(), self.profile.remote.as_ref()) {
            (Some(shell), _) => Session::shell(
                &shell.program,
                &shell.args,
                shell.cwd.as_deref(),
                cols,
                rows,
            ),
            (None, Some(remote)) => Session::telnet(&remote.address, remote.port, cols, rows),
            (None, None) => {
                let launcher = launcher();
                Session::local(launcher.as_ref(), &self.profile.launch_spec(), cols, rows)
            }
        };

        match opened {
            Ok(session) => {
                self.session = Some(session);
                let endpoint = self.profile.endpoint();
                self.note(&crate::i18n::tr1("session started ({})", &endpoint));
            }
            Err(e) => {
                // Surface the failure in the tab rather than a popup — the user
                // may have several tabs and needs to know which one broke.
                self.error = Some(format!("{e:#}"));
                self.ended = true;
                self.session = None;
            }
        }
    }

    pub(super) fn note(&mut self, text: &str) {
        if let Some(log) = self.log.as_mut() {
            let _ = log.write_note(text);
        }
    }

    /// What the tab strip calls this session.
    ///
    /// `with_namespace` adds the namespace the session is in to the automatic
    /// name - `CONSISTEM | RDB76-TR` - which is the one thing two tabs on the
    /// same instance do not otherwise say. A name the user typed is left
    /// exactly as they typed it either way.
    pub fn title(&self, with_namespace: bool) -> String {
        if let Some(custom) = self.custom_title.clone() {
            return custom;
        }
        // IRIS sets the window title to the executable path, which is useless
        // as a tab name.
        let base = self
            .grid
            .title
            .clone()
            .filter(|t| !t.contains(".exe"))
            .unwrap_or_else(|| {
                // The instance first: it is what tells two tabs apart, whereas
                // the profile name is often left at its default and the same on
                // every tab.
                if self.profile.instance.is_empty() {
                    self.profile.name.clone()
                } else {
                    self.profile.instance.clone()
                }
            });
        match self.namespace.as_deref().filter(|_| with_namespace) {
            // Not when the name already is the namespace, which is what a
            // server called after its namespace comes out as.
            Some(namespace) if namespace != base => format!("{base} | {namespace}"),
            _ => base,
        }
    }

    /// What the tab strip calls this tab.
    ///
    /// A split tab is two sessions in one entry, so the entry says which of
    /// them the keyboard is in - `1: CONSISTEM | COMP80` for the left or top
    /// pane, `2: ...` for the other - and follows the focus as it moves. An
    /// unsplit tab is one session and needs no number.
    pub fn strip_label(&self, with_namespace: bool) -> String {
        if self.split.is_none() {
            return self.title(with_namespace);
        }
        format!(
            "{}: {}",
            self.focus.number(),
            self.focused().title(with_namespace)
        )
    }

    /// How much of the room the first pane gets. 0.5 when the tab is not
    /// split, which is the value nothing reads.
    pub(super) fn split_ratio(&self) -> f32 {
        self.split.as_ref().map(|split| split.ratio).unwrap_or(0.5)
    }

    /// Drops one of this tab's two sessions, leaving the other as the tab's
    /// only one.
    ///
    /// Only called on a split tab: a tab with one session in it has nothing
    /// left to be, so closing that pane closes the tab - see
    /// [`App::close_pane`], which is where that half of the decision is.
    pub(super) fn close_pane(&mut self, going: Pane) {
        match going {
            // The second pane goes and the tab stops being split.
            Pane::Second => self.split = None,
            // The first one goes, so the second takes the tab over: its
            // session, its scrollback, its name. Its `uid` comes with it, so
            // the terminal widget it has been drawn as keeps the scroll
            // position it had.
            Pane::First => {
                if let Some(split) = self.split.take() {
                    *self = *split.tab;
                }
            }
        }
        self.focus = Pane::First;
    }

    /// One of this tab's panes. `Second` is there only while the tab is split.
    pub fn pane(&self, pane: Pane) -> Option<&Tab> {
        match pane {
            Pane::First => Some(self),
            Pane::Second => self.split.as_ref().map(|split| split.tab.as_ref()),
        }
    }

    pub fn pane_mut(&mut self, pane: Pane) -> Option<&mut Tab> {
        match pane {
            Pane::First => Some(self),
            Pane::Second => self.split.as_mut().map(|split| split.tab.as_mut()),
        }
    }

    /// The session the keyboard is in, which is this one unless the tab is
    /// split and the second pane has it.
    pub fn focused(&self) -> &Tab {
        self.pane(self.focus).unwrap_or(self)
    }

    /// Every session this tab holds: itself, and the second pane when split.
    pub fn sessions(&self) -> impl Iterator<Item = &Tab> {
        std::iter::once(self).chain(self.split.as_ref().map(|split| split.tab.as_ref()))
    }

    /// Remembers a command this session ran, for its own recall.
    pub fn remember_command(&mut self, command: &str) {
        crate::features::history::push_recent(&mut self.commands, command);
    }

    /// Process id of the session on this machine, while it is running.
    pub fn pid(&self) -> Option<u32> {
        self.session.as_ref()?.process_id()
    }

    /// Pulls output, parses it, answers device reports, and runs autologon.
    pub fn pump(&mut self, plugins: &mut PluginHost) {
        // The piece tooltip's own session, which is not this tab's and so is
        // driven whether or not this tab still has one of its own.
        self.doc_lookup.pump(&self.profile);

        let Some(session) = self.session.as_mut() else {
            return;
        };

        let (bytes, ended) = session.drain();
        if !bytes.is_empty() {
            // Plugins see the raw stream first, so they can rewrite before it
            // is interpreted.
            let bytes = plugins.on_output(&bytes);

            // Mute logging across the password step so a credential never
            // reaches disk.
            let muting = self.autologon.state() == AutoState::WaitPassword;
            if let Some(log) = self.log.as_mut() {
                if muting {
                    log.mute();
                }
                let _ = log.write_raw(&bytes);
            }

            // A Telnet session can speak a codepage of its own; transcode
            // before the parser, which assumes UTF-8. Escape sequences are
            // ASCII either way, and a local session needs no transcoding at
            // all - see `Profile::wire_encoding`.
            let decoded = self.profile.wire_encoding().decode(&bytes);
            let replies = crate::term::parser::advance(&mut self.parser, &mut self.grid, &decoded);
            if !replies.is_empty() {
                let _ = session.write(&replies);
            }

            // Autologon reads the decoded screen rather than raw bytes, so a
            // prompt split across two reads is still recognised.
            if let Some(to_send) = self.autologon.observe(&self.grid) {
                let _ = session.write(&self.profile.wire_encoding().encode(&to_send));
            }

            // Read after the parse, from the row the cursor is on: the prompt
            // is the only place a session says which namespace it is in, and it
            // says so again after every `ZN`. Kept when the screen moves off a
            // prompt, so the tab does not lose its name mid-routine.
            if let Some(namespace) = lineedit::namespace(&self.grid) {
                self.namespace = Some(namespace);
            }

            let still_on_password = self.autologon.state() == AutoState::WaitPassword;
            // Everything above the row the cursor is on. A command and its
            // output are final the moment the next prompt is printed, and the
            // transcript is read while the session is still open, so waiting
            // for a row to scroll off the screen for good left the file empty
            // for the whole of a short session.
            let settled = self.grid.scrollback.len() + self.grid.cursor.row;
            if let Some(log) = self.log.as_mut() {
                if !still_on_password {
                    log.unmute();
                }
                // Only transcribed once the log says it will read it: this runs
                // on every chunk of output, and the transcript is a `String`
                // per line of the whole history.
                if log.wants_settled() {
                    let lines = self.grid.all_text();
                    let _ = log.write_settled(&lines, settled);
                }
            }
        }

        // A clear that never came - the session was sitting in a `read`, say,
        // and swallowed the command as input. The purge is called off rather
        // than left armed to surprise the next clear-screen.
        if let Some(asked) = self.clear_asked {
            if asked.elapsed() > std::time::Duration::from_secs(2) {
                self.grid.cancel_purge();
                self.clear_asked = None;
            }
        }

        if ended {
            self.ended = true;
            self.session = None;
            self.note("session ended");
        }
    }

    /// Sends raw bytes (key sequences, already in wire form).
    pub fn send(&self, bytes: &[u8]) {
        if let Some(session) = self.session.as_ref() {
            let _ = session.write(bytes);
        }
    }

    /// Sends typed or pasted text, encoded into the instance's codepage.
    pub fn send_text(&self, text: &str) {
        if let Some(session) = self.session.as_ref() {
            let _ = session.write(&self.profile.wire_encoding().encode(text));
        }
    }

    /// Sends whole lines, each terminated the way Enter would terminate it.
    pub fn send_lines(&self, lines: &[String]) {
        for line in lines {
            self.send_text(line);
            self.send(b"\r");
        }
    }

    /// Asks the far side to clear its own screen - the way typing `W #` would
    /// in IRIS, or Ctrl+L in a shell - and arranges for the clear that comes
    /// back to drop the transcript instead of filing it.
    ///
    /// The terminal cannot do this on its own. The far side keeps its own idea
    /// of where the cursor is and repaints by absolute position, so a grid
    /// cleared behind its back leaves the next prompt painted back down at the
    /// row it had reached, with blank rows above it. That goes double for a
    /// shell on Windows, where the pseudoconsole owns a screen buffer of its
    /// own and only ever sends the difference between it and the last one: a
    /// screen wiped here is a screen it believes is still there, so it repaints
    /// nothing and the tab stays blank until something scrolls. Asking is what
    /// resets that idea. Nothing of it stays on screen: the echo of the ask and
    /// the pre-clear screen are both dropped by the purge.
    ///
    /// On IRIS this is only ever called at an idle prompt - see
    /// `App::clear_active_terminal`, which is what keeps the command from
    /// being swallowed as input by a `read` or appended to a half-typed line.
    /// A shell has no such restriction, because what it is sent is a
    /// keystroke rather than a command line.
    pub fn request_clear(&mut self) {
        if self.session.is_none() {
            return;
        }
        self.grid.purge_history_on_next_clear();
        self.clear_asked = Some(std::time::Instant::now());
        match self.profile.shell.as_ref() {
            Some(shell) => {
                for write in clear_gesture(&shell.program) {
                    self.send(write);
                }
            }
            None => self.send_lines(&["W #".to_string()]),
        }
    }

    pub fn resize(&mut self, cols: usize, rows: usize) {
        // The easter egg has no grid worth the name and no far side to tell
        // about one. Its board is square and sizes itself to the pane it is
        // drawn in.
        if self.is_game() {
            return;
        }
        if cols == self.grid.cols && rows == self.grid.rows {
            return;
        }
        // A minimized window measures as nothing, and the pane it holds then
        // floors to a single cell. Resizing to that is what emptied a session
        // on minimize: the grid reflowed everything into one column, and the
        // far side redrew a one-cell screen over what had been there. Nothing
        // this small is a size a user asked for, so it is not a size to obey.
        if cols < 2 || rows < 2 {
            return;
        }
        self.grid.resize(cols, rows);
        if let Some(session) = self.session.as_mut() {
            let _ = session.resize(cols as u16, rows as u16);
        }
    }
}

/// Arrow keys that walk IRIS's cursor `columns` to the right, or to the left
/// when negative. Empty for no movement.
///
/// `app_cursor` picks the spelling the far side is expecting - see
/// [`input::cursor_key`]. Sending the other one is why click-to-position moved
/// nothing on IRIS 2023.
pub(super) fn cursor_bytes(columns: i64, app_cursor: bool) -> Vec<u8> {
    if columns == 0 {
        return Vec::new();
    }
    let final_byte = if columns < 0 { b'D' } else { b'C' };
    input::cursor_key(app_cursor, final_byte).repeat(columns.unsigned_abs() as usize)
}

/// Columns of the command line covered by the tab's selection.
///
/// `None` unless the whole selection sits inside the line being typed: a
/// selection that reaches into the scrollback is highlighted text, and there is
/// nothing in IRIS's read buffer that corresponds to it.
/// The one character `text` holds, or `None` if it holds anything else.
///
/// A frame's worth of typing is usually one keystroke, but it need not be: two
/// characters can land in the same frame, and an input method commits whole
/// words. Only a lone character can be the one that asked to wrap a selection.
pub(super) fn one_char(text: &str) -> Option<char> {
    let mut chars = text.chars();
    let first = chars.next()?;
    chars.next().is_none().then_some(first)
}

pub(super) fn selection_in_line(tab: &Tab, line: crate::term::LineEdit) -> Option<(usize, usize)> {
    let at = tab.grid.scrollback.len() + tab.grid.cursor.row;
    let (from, to) = tab.view.selection?.span_on(at)?;
    let from = from.max(line.start);
    let to = to.min(line.end);
    (from < to).then_some((from, to))
}

fn open_log(profile: &Profile, settings: &Settings) -> Option<SessionLog> {
    let mode = settings.log_mode_for(profile);
    if mode == LogMode::Off {
        return None;
    }
    match SessionLog::open(&settings.log_dir, &profile.name, mode, LOG_ROTATE_BYTES) {
        Ok(log) => Some(log),
        Err(e) => {
            log::error!("logging disabled for {}: {e:#}", profile.name);
            None
        }
    }
}

/// Asks one session to halt and closes its transcript. Called for every
/// session there is as the app exits, panes included.
pub(super) fn close_down(tab: &mut Tab) {
    if let Some(session) = tab.session.as_ref() {
        session.request_halt();
    }
    if let Some(log) = tab.log.as_mut() {
        let _ = log.write_note("application closed");
        let _ = log.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ctrl+Delete asks the far side to clear itself, and what a shell answers
    /// to is the shell's business: Ctrl+L wherever a line editor binds it, and
    /// the command for `cmd.exe`, whose does not - with the Escape that empties
    /// its input line kept as a write of its own.
    #[test]
    fn a_shell_is_asked_to_clear_the_way_that_shell_understands() {
        use std::path::Path;

        assert_eq!(
            clear_gesture(Path::new("C:/Program Files/PowerShell/7/pwsh.exe")),
            [b"\x0c".as_slice()]
        );
        assert_eq!(
            clear_gesture(Path::new("/usr/bin/bash")),
            [b"\x0c".as_slice()]
        );
        assert_eq!(
            clear_gesture(Path::new("C:/Windows/System32/cmd.exe")),
            [b"\x1b".as_slice(), b"cls\r".as_slice()],
            "cmd echoes a Ctrl+L rather than acting on it"
        );
    }

    /// A tab with no session behind it. Closing a pane is about which session
    /// ends up in the tab, and needs no IRIS to answer.
    fn bare_tab(name: &str) -> Tab {
        let profile = Profile {
            name: name.to_string(),
            ..Profile::default()
        };
        Tab {
            uid: NEXT_TAB_UID.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            autologon: Autologon::new(&profile),
            doc_lookup: DocLookup::default(),
            grid: Grid::new(80, 24, 100),
            profile,
            session: None,
            parser: vte::Parser::new(),
            view: ViewState::default(),
            log: None,
            custom_title: None,
            namespace: None,
            commands: Vec::new(),
            recall_step: None,
            clear_asked: None,
            ended: false,
            error: None,
            split: None,
            focus: Pane::First,
            game: None,
        }
    }

    fn split_tab_of(first: &str, second: &str) -> Tab {
        let mut tab = bare_tab(first);
        tab.split = Some(Split {
            dir: SplitDir::Right,
            tab: Box::new(bare_tab(second)),
            ratio: 0.5,
        });
        tab.focus = Pane::Second;
        tab
    }

    /// Closing the second pane leaves the first where it was.
    #[test]
    fn closing_the_second_pane_leaves_the_first_in_the_tab() {
        let mut tab = split_tab_of("left", "right");
        tab.close_pane(Pane::Second);
        assert!(tab.split.is_none(), "the tab should no longer be split");
        assert_eq!(tab.profile.name, "left");
        assert_eq!(tab.focus, Pane::First);
    }

    /// Closing the first pane promotes the second into the tab, session and
    /// all - it is a live IRIS session, and the click was on the other one.
    #[test]
    fn closing_the_first_pane_promotes_the_second_into_the_tab() {
        let mut tab = split_tab_of("left", "right");
        let promoted = tab.pane(Pane::Second).map(|pane| pane.uid);
        tab.close_pane(Pane::First);
        assert!(tab.split.is_none(), "the tab should no longer be split");
        assert_eq!(tab.profile.name, "right");
        assert_eq!(Some(tab.uid), promoted, "the pane keeps its own identity");
        assert_eq!(tab.focus, Pane::First);
    }
}
