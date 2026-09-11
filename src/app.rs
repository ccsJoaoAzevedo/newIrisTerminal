//! The application shell: owns the tabs, drains their PTYs each frame, and
//! draws the chrome around the terminal view.

use egui::{Context, Key, Modifiers};

use crate::config::{self, ensure_config_tree, load_themes, LogMode, Profile, Settings, Theme};
use crate::features::analyze;
use crate::features::autologon::{Autologon, State as AutoState};
use crate::features::export::{self, Range};
use crate::features::history::{self, History};
use crate::features::logging::{self, SessionLog};
use crate::features::macros::{self, MacroGroup};
use crate::features::update;
use crate::i18n::{tr, tr1, tr2};
use crate::plugins::PluginHost;
use crate::pty::launcher::launcher;
use crate::pty::Session;
use crate::term::{lineedit, Grid, Motion};
use crate::ui::chrome::{self, WindowAction};
use crate::ui::macro_manager;
use crate::ui::panels::{self, PanelState, PendingMacro, UiRequest};
use crate::ui::terminal_view::{self, RenderOpts, Selection, ViewState};
use crate::ui::theme_manager::{self, ThemeAction};
use crate::ui::{fonts, icons, input, shortcut};

/// What the app knows about a newer version.
///
/// The check runs on a thread, so this is the state machine between "asked" and
/// "the user has decided": nothing here blocks a frame.
#[derive(Default)]
pub struct UpdateState {
    events: Option<crossbeam_channel::Receiver<update::Event>>,
    /// A release newer than this build, once one has been found.
    pub available: Option<update::Release>,
    /// The downloaded executable, waiting to be put in place.
    pub staged: Option<std::path::PathBuf>,
    pub downloading: bool,
    /// Bytes of the download written so far, shared with the thread doing the
    /// writing. Read every frame while `downloading`, which is what lets the
    /// dialog say how far it has got instead of only that it is trying.
    progress: std::sync::Arc<std::sync::atomic::AtomicU64>,
    pub error: Option<String>,
    /// The dialog has been shown for this release and dismissed. Kept so a
    /// "later" is not undone by the next frame.
    pub asked: bool,
    /// The user asked for this check, so its answer is worth a status line even
    /// when the answer is "nothing new".
    announce: bool,
}

impl UpdateState {
    /// Starts a check on a thread of its own.
    fn start_check(&mut self) {
        let (tx, rx) = crossbeam_channel::unbounded();
        self.events = Some(rx);
        self.error = None;
        update::check_in_background(tx);
    }

    /// Starts a check the user asked for, whose answer is always reported.
    pub fn check_now(&mut self) {
        self.announce = true;
        self.start_check();
    }

    /// Starts downloading the release that was found.
    pub fn start_download(&mut self) {
        let Some(release) = self.available.clone() else {
            return;
        };
        let (tx, rx) = crossbeam_channel::unbounded();
        self.events = Some(rx);
        self.downloading = true;
        self.error = None;
        self.progress = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        update::download_in_background(release, self.progress.clone(), tx);
    }

    /// Bytes written so far, and the total if the release said how big it is.
    pub fn downloaded(&self) -> (u64, u64) {
        (
            self.progress.load(std::sync::atomic::Ordering::Relaxed),
            self.available.as_ref().map(|r| r.size).unwrap_or(0),
        )
    }

    /// Whether a thread is still working, so the frame loop knows to keep
    /// drawing.
    ///
    /// An idle terminal gives egui no reason to draw another frame, and the
    /// events from the check and the download are only picked up while frames
    /// are being drawn - so without this the answer could sit in the channel
    /// unread and the spinner never move.
    fn working(&self) -> bool {
        self.events.is_some()
    }

    fn drain(&mut self) -> Vec<update::Event> {
        let Some(rx) = self.events.as_ref() else {
            return Vec::new();
        };
        rx.try_iter().collect()
    }
}

/// Fallback PTY size, used only before the first frame has measured the
/// window. After that, new sessions open at the size the terminal is actually
/// being drawn at.
///
/// This matters more than it looks: IRIS truncates output at the device right
/// margin rather than wrapping it, so a session opened at 80 columns loses
/// everything past column 80 until it is resized. Opening at the real size
/// means nothing is cut in the first place.
/// A strip of the title bar kept free of tabs, in front of the window
/// controls.
///
/// The handle that is always there. A row of tabs long enough to fill the bar
/// would otherwise leave nothing to drag the window by, and with the system's
/// frame turned off there is then no way to move it at all. Held back at the
/// end of the row rather than in front of the tabs, where an empty gap looks
/// like a tab that failed to draw.
const TITLE_FREE_STRIP: f32 = 28.0;

/// How wide one title-bar control is, for working out what to keep back for
/// them. egui sizes them from the row height, which is `interact_size.y`.
const TITLE_CONTROL_SIDE: f32 = 24.0;

const FALLBACK_COLS: u16 = 80;
const FALLBACK_ROWS: u16 = 24;

/// How many frames the opening size may take to settle. A correction normally
/// lands on the first one; the budget is there so a window manager that refuses
/// the size it is asked for cannot leave the app resizing itself forever.
const FIT_ATTEMPTS: u8 = 8;

/// A window that has yet to be sized to the terminal geometry it was asked to
/// open at.
///
/// The size in the settings file is a character geometry, not a pixel one, and
/// the pixels it works out to depend on the font egui ends up with and on how
/// tall the panels above the terminal are laid out. Neither is known before the
/// first frame, so the window opens at an estimate and is corrected here once
/// there is a measurement to correct it with.
struct WindowFit {
    cols: u16,
    rows: u16,
    /// Frames left to get there.
    attempts: u8,
    /// Whether the window is being centred rather than opened at a saved
    /// position. Resizing anchors the top-left corner, which would walk a
    /// centred window off-centre, so its centre is held instead.
    recentre: bool,
    /// The centre to hold, taken from the first frame that had a window rect to
    /// take it from.
    centre: Option<egui::Pos2>,
}

/// Thickness of the divider between two panes, in points. Wide enough to be
/// aimed at with a mouse, which it has to be: it is the handle the split is
/// resized by.
const SPLIT_DIVIDER: f32 = 6.0;

/// Smallest a pane may be dragged down to, in points.
///
/// A pane thinner than this is not a pane, and a session in one would be
/// reporting a terminal a couple of characters wide to IRIS - which truncates
/// its output at the margin it is told about, so everything past it would be
/// lost rather than merely hidden. The divider stops here instead.
const MIN_PANE: f32 = 80.0;

/// How much of `usable` the first pane gets at `ratio`, never letting either
/// side fall below [`MIN_PANE`].
///
/// Clamped here rather than where the ratio is stored: the room a split has
/// changes with the window, and a ratio that was reachable in a wide window
/// must not squeeze a pane out of existence in a narrow one.
fn split_extent(usable: f32, ratio: f32) -> f32 {
    if usable <= MIN_PANE * 2.0 {
        // No room to honour a minimum on both sides, so the only fair split is
        // down the middle.
        return usable / 2.0;
    }
    (usable * ratio).clamp(MIN_PANE, usable - MIN_PANE)
}

/// The ratio a drag of the divider leaves behind.
///
/// Measured from where the divider actually is rather than added to the stored
/// ratio, so a drag that ran into the minimum does not bank the movement past
/// it and leave the handle lagging behind the pointer on the way back.
fn dragged_ratio(usable: f32, ratio: f32, delta: f32) -> f32 {
    if usable <= 0.0 {
        return ratio;
    }
    ((split_extent(usable, ratio) + delta) / usable).clamp(0.0, 1.0)
}

/// Which way the second pane sits next to the first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SplitDir {
    Right,
    Bottom,
}

/// A second session inside one tab, and which way it sits.
///
/// One split rather than a tree of them: two sessions in one tab is the thing
/// people actually ask for - a routine running in one and a global inspected in
/// the other - and a general pane tree would be a window manager inside a
/// terminal. So a split tab cannot be split again, and the strip entry has only
/// ever two names to carry.
pub struct Split {
    pub dir: SplitDir,
    /// The session the split opened. Boxed: a [`Tab`] holds one of these, so
    /// the type would otherwise have no size.
    pub tab: Box<Tab>,
    /// How much of the room the first pane gets, 0 to 1. Dragged by the
    /// divider between them; see [`split_extent`], which is what turns it into
    /// a size and keeps either pane from being squeezed away.
    pub ratio: f32,
}

/// Which of a tab's two sessions.
///
/// `First` is the left or top pane and is the only one an unsplit tab has. The
/// numbers are what the strip entry shows - `1:` and `2:` - so they are the
/// panes' names as far as the user is concerned.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Pane {
    #[default]
    First,
    Second,
}

impl Pane {
    /// What the strip entry calls this pane.
    pub fn number(self) -> u8 {
        match self {
            Pane::First => 1,
            Pane::Second => 2,
        }
    }
}

/// A change to the tab layout, asked for from inside a pane.
///
/// Deferred rather than carried out on the spot: the right-click menu is drawn
/// while the panes are, and splitting or closing a tab there would move the
/// tabs under the loop that is drawing them. Applied once the central panel has
/// finished - the same reason the tab strip collects its own actions and
/// applies them after its loop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LayoutAction {
    Split(usize, SplitDir),
    Unsplit(usize),
    ClosePane(At),
}

/// One session on screen: the tab it is in, and which of its panes.
///
/// Everything that acts on "the session" takes one of these rather than a tab
/// index, because with a split tab an index no longer names a session.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct At {
    tab: usize,
    pane: Pane,
}

/// What drawing one pane produced.
///
/// `Default` stands for a pane there was nothing to draw in - a tab that went
/// away between frames. A zero cell size leaves the window geometry alone
/// rather than dividing by it.
#[derive(Default)]
struct PaneMeasure {
    /// Grid size IRIS should be told about, which is wider than the window -
    /// see [`terminal_view::TERMINAL_COLS`].
    grid: (usize, usize),
    /// Size of the pane in characters, as drawn.
    view: (usize, usize),
    cell: egui::Vec2,
    /// The user clicked into this pane's terminal this frame.
    clicked: bool,
}

/// Takes one key press out of this frame's events, matching the modifiers
/// exactly, and reports whether it was there.
///
/// Consuming matters: without it a chord is handled here *and* translated into
/// a control code for IRIS, so Ctrl+T opened a tab and typed `0x14` into the
/// session. egui's own `consume_key` is not usable for this because it matches
/// leniently - `Ctrl+Shift+T` satisfies a pattern of `Ctrl` - which would let
/// an app shortcut swallow a macro bound to the same key plus Shift.
fn consume_exact(ctx: &Context, modifiers: Modifiers, key: Key) -> bool {
    ctx.input_mut(|i| {
        let mut hit = false;
        i.events.retain(|event| {
            let is_match = matches!(
                event,
                egui::Event::Key {
                    key: event_key,
                    modifiers: event_modifiers,
                    pressed: true,
                    ..
                } if *event_key == key && event_modifiers.matches_exact(modifiers)
            );
            hit |= is_match;
            !is_match
        });
        hit
    })
}

/// A tab name being edited.
struct Renaming {
    tab: usize,
    draft: String,
    /// The second pane's name, when the tab is split. Two fields because the
    /// entry names two sessions; the `1:` and `2:` in front of them are the
    /// panes themselves and not part of either name.
    second: Option<String>,
    /// Cleared after the field has been given focus once. Without this the
    /// terminal claims the keyboard back - it grabs focus whenever nothing else
    /// holds it - and the new name would be typed into IRIS.
    focus: bool,
}

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
    clear_asked: Option<std::time::Instant>,
    /// Set when the child exits, so the tab explains itself instead of freezing.
    pub ended: bool,
    pub error: Option<String>,
    /// The second session sharing this tab, once it has been split. Always
    /// `None` on the second session itself: a split tab cannot be split again.
    pub split: Option<Split>,
    /// Which pane has the keyboard. Meaningless until the tab is split, and
    /// what the strip entry's `1:` or `2:` reports.
    pub focus: Pane,
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
        };
        tab.start();
        tab
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

    fn note(&mut self, text: &str) {
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
    fn split_ratio(&self) -> f32 {
        self.split.as_ref().map(|split| split.ratio).unwrap_or(0.5)
    }

    /// Drops one of this tab's two sessions, leaving the other as the tab's
    /// only one.
    ///
    /// Only called on a split tab: a tab with one session in it has nothing
    /// left to be, so closing that pane closes the tab - see
    /// [`App::close_pane`], which is where that half of the decision is.
    fn close_pane(&mut self, going: Pane) {
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
    /// [`App::clear_active_terminal`], which is what keeps the command from
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
fn cursor_bytes(columns: i64, app_cursor: bool) -> Vec<u8> {
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
fn selection_in_line(tab: &Tab, line: crate::term::LineEdit) -> Option<(usize, usize)> {
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

pub struct App {
    pub settings: Settings,
    pub themes: Vec<Theme>,
    pub tabs: Vec<Tab>,
    pub active: usize,
    /// Instances found on this machine, for the new-tab dialog.
    pub instances: Vec<String>,
    /// Servers the InterSystems launcher knows about, and which one it treats
    /// as preferred. Read once at startup: the Server Manager is a separate
    /// program, and a list that changed under the app mid-session would only
    /// ever surprise the user. Restarting picks up an edit.
    pub servers: crate::config::ServerList,
    pub macro_groups: Vec<MacroGroup>,
    /// The rectangles the terminal panes drew into this frame.
    ///
    /// Collected so the window's resize grips can keep off them: a grip is in a
    /// foreground layer and outranks whatever is under it, and a terminal
    /// reaching the window edge would lose its first column to one. Cleared and
    /// refilled every frame - a pane that has gone must not still be claiming
    /// the space it used to be in.
    pane_rects: Vec<egui::Rect>,
    /// Commands typed at an IRIS prompt, shared by every tab so a new one opens
    /// knowing what was run in the last.
    history: History,
    plugins: PluginHost,
    panels: PanelState,
    /// The tab whose terminal last took keyboard focus, so a tab switch can
    /// hand focus over exactly once instead of fighting dialogs every frame.
    focused_tab: Option<u64>,
    /// Size the terminal was last drawn at. New sessions open at this size so
    /// their output is never truncated at a stale width.
    terminal_size: (u16, u16),
    /// Whether the window is minimized right now.
    ///
    /// Read every frame and used to hold every session at the size it had: a
    /// minimized window still draws, and the pane it draws measures as almost
    /// nothing, so obeying that measurement reflowed the grid and told the far
    /// side to redraw into a sliver. Coming back up then showed a session with
    /// its output gone.
    minimized: bool,
    /// Size of the window in character cells, as last drawn. Distinct from
    /// `terminal_size`, which is the wider grid IRIS is told about; this is the
    /// geometry the user is actually looking at and the one worth reporting.
    view_size: (usize, usize),
    /// Profile a new tab opens with. There is no dialog in front of it: the
    /// `+` button and Ctrl+T connect straight away, and this is what they
    /// connect to. Right-clicking `+` picks a different one.
    new_tab_profile: Profile,
    status: Option<String>,
    /// When the message in the footer went up, so it can be taken down again
    /// once `status_timeout_secs` has passed.
    status_at: Option<std::time::Instant>,
    /// What the update thread has told us so far, and what the dialog is
    /// showing. Every field is `None` on a build that has never checked.
    updates: UpdateState,
    /// Font family egui has actually been given, which is not always the one
    /// in `settings`: a family that is no longer installed has to degrade to
    /// the bundled monospace, because naming an unregistered family panics
    /// inside egui's glyph measurement.
    font_family: String,
    /// Family last handed to [`fonts::install`], successfully or not. Settings
    /// changes arrive every frame while a slider is dragged, and reinstalling
    /// rebuilds the glyph atlas, so the work is skipped unless the name moved.
    font_request: String,
    /// Tab whose name is being edited, if any.
    renaming: Option<Renaming>,
    /// Size and position of the Settings window: restored from the settings
    /// file at startup, and written back on exit for whichever of the two
    /// window switches is on.
    settings_placement: crate::ui::detach::Placement,
    /// Set when a close was intercepted to ask about live sessions.
    confirm_close: bool,
    /// Set once the user has said to close anyway, so the confirmation cannot
    /// cancel the very close it just approved.
    close_confirmed: bool,
    /// The opening size still to be applied, while there is one.
    fit: Option<WindowFit>,
    /// Window geometry as last seen by [`App::track_window_geometry`], which is
    /// what gets written back on exit for the next launch to open at. Kept
    /// frame by frame because a window that is closing no longer has a rect to
    /// ask for.
    window_size: Option<[f32; 2]>,
    window_position: Option<[f32; 2]>,
    window_maximized: bool,
    /// A split, unsplit or close asked for from a pane's right-click menu,
    /// waiting for the panes to have finished drawing. See [`LayoutAction`].
    pending_layout: Option<LayoutAction>,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let _ = ensure_config_tree();
        let settings = Settings::load();
        // Before anything is drawn: every label the first frame asks for goes
        // through `tr`, which reads this.
        crate::i18n::set_language(settings.language);
        let themes = load_themes();

        // Housekeeping that would otherwise never happen.
        let _ = logging::prune(&settings.log_dir, settings.log_retention_days);

        let l = launcher();
        let instances = crate::pty::launcher::instances(l.as_ref())
            .into_iter()
            .map(|i| i.name)
            .collect::<Vec<_>>();

        // The launcher's own list, so the servers offered here are the ones the
        // rest of the toolchain already has configured.
        let servers = crate::config::servers::discover();

        let plugins = if settings.enable_plugins {
            PluginHost::load_from(&config::plugins_dir())
        } else {
            PluginHost::disabled()
        };

        // What a new tab connects to, in order of how deliberate the choice
        // was: a startup profile the user configured here, then whatever the
        // launcher's tray menu is set to open, then whatever instance was found
        // first.
        let default_profile = settings
            .startup_profile()
            .cloned()
            .or_else(|| preferred_profile(&servers, &instances))
            .unwrap_or_else(|| Profile {
                instance: instances.first().cloned().unwrap_or_default(),
                ..Profile::default()
            });

        // Nothing to fit when a saved pixel size is being restored: that size
        // already produces the geometry it was recorded at. A window opening
        // maximized has no say in its size either.
        let fit = (settings.restored_window_size().is_none() && !settings.restored_maximized())
            .then(|| WindowFit {
                cols: settings.default_geometry().0,
                rows: settings.default_geometry().1,
                attempts: FIT_ATTEMPTS,
                recentre: settings.restored_window_position().is_none(),
                centre: None,
            });

        // Read before `settings` is moved into the app.
        let settings_placement = crate::ui::detach::Placement {
            restore: settings.restored_settings_placement(),
            seen: crate::ui::detach::Geometry::default(),
        };

        let mut app = App {
            new_tab_profile: default_profile,
            macro_groups: load_macros(&settings).groups,
            pane_rects: Vec::new(),
            history: History::load(
                &config::command_history_path(),
                settings.save_command_history,
            ),
            settings,
            themes,
            tabs: Vec::new(),
            active: 0,
            instances,
            servers,
            plugins,
            panels: PanelState::default(),
            focused_tab: None,
            terminal_size: (FALLBACK_COLS, FALLBACK_ROWS),
            minimized: false,
            view_size: (FALLBACK_COLS as usize, FALLBACK_ROWS as usize),
            status: None,
            status_at: None,
            updates: UpdateState::default(),
            font_family: String::new(),
            font_request: String::new(),
            renaming: None,
            pending_layout: None,
            settings_placement,
            confirm_close: false,
            close_confirmed: false,
            fit,
            window_size: None,
            window_position: None,
            window_maximized: false,
        };

        // Whatever the last update left behind is no longer running and can
        // go, and the check for the next one starts now: it is a request over
        // a corporate proxy, so it is not going to answer this frame.
        update::clean_up();
        // Before the check: it is what the check authenticates to the proxy
        // with, and a check that starts without it gets a 407 instead.
        update::configure_proxy_user(&app.settings.proxy_user);
        if app.settings.check_for_updates {
            app.updates.start_check();
        }

        App::apply_style(&cc.egui_ctx, &app.theme(), &app.settings);
        app.apply_font(&cc.egui_ctx);

        // Straight into the instance. Anyone with something to change has
        // Settings; everyone else was only ever going to press Connect.
        if app.settings.open_on_start {
            app.open_new_tab();
        }
        app
    }

    /// Applies theme colours *and* the style bits that are not part of
    /// `Visuals`. `set_visuals` replaces only the colours, so the scrollbar
    /// spacing has to be set separately or the switch would do nothing.
    fn apply_style(ctx: &Context, theme: &Theme, settings: &Settings) {
        ctx.set_visuals(theme.visuals());
        ctx.style_mut(|style| {
            // Floating bars are egui's default and are the reason the
            // scrollbars read as absent: they stay a hairline until hovered.
            style.spacing.scroll.floating = !settings.show_scrollbars;
            if settings.show_scrollbars {
                style.spacing.scroll.bar_width = 10.0;
            }
        });
    }

    /// Hands the configured font to egui and records what it actually got.
    ///
    /// The setting wins over the theme's suggestion; the theme's own
    /// `font_family` is the default for anyone who has not chosen one. A name
    /// that will not load leaves the bundled monospace in place and says so,
    /// rather than being passed on to panic later.
    fn apply_font(&mut self, ctx: &Context) {
        let wanted = if self.settings.font_family.is_empty() {
            self.theme().font_family
        } else {
            self.settings.font_family.clone()
        };

        if wanted == self.font_request {
            return;
        }
        self.font_request = wanted.clone();

        if fonts::install(ctx, &wanted) {
            self.font_family = wanted;
        } else {
            self.font_family = String::new();
            self.set_status(tr1(
                "Font {} is not installed; using the built-in monospace.",
                &format!("{wanted:?}"),
            ));
        }
    }

    /// How the terminal should be drawn, from the current settings.
    fn render_opts(&self) -> RenderOpts {
        RenderOpts {
            font_size: self.settings.font_size,
            font_family: self.font_family.clone(),
            cursor_style: self.settings.cursor_style,
            cursor_blink: self.settings.cursor_blink,
            scrollbar: self.settings.show_scrollbars,
            syntax: self.settings.terminal_syntax_highlight,
            wrap: self.settings.wrap_lines,
            copy_on_select: self.settings.copy_on_select,
            wide_grid: true,
        }
    }

    pub fn theme(&self) -> Theme {
        self.themes
            .iter()
            .find(|t| t.name == self.settings.theme)
            .cloned()
            .unwrap_or_default()
    }

    /// Clears the active terminal and forgets its history.
    ///
    /// The deliberate version of what `W #` only looks like: IRIS's own
    /// clear-screen files the screen into the scrollback, and this is the
    /// gesture for when the history really is meant to go. Also on the
    /// right-click menu, because a shortcut nobody can see is a shortcut nobody
    /// uses.
    ///
    /// Clearing the grid on its own is not enough, and this is the whole
    /// subtlety of the gesture: the far side keeps its own idea of where the
    /// cursor is and repaints by absolute position, so a screen wiped behind
    /// its back gets the next prompt painted back down at the row it had
    /// reached, with the cleared rows blank above it. The only thing that
    /// resets that idea is a clear-screen from the far side itself, so the
    /// clear is asked of it - of IRIS at an idle prompt, of a shell at any
    /// moment at all - and the echo of the ask and the pre-clear screen are
    /// both dropped, so nothing of it is left on screen or in the scrollback.
    ///
    /// Anywhere else - mid-line or mid-routine on IRIS, or on a session that
    /// has ended - the command could be swallowed as input or appended to what
    /// is being typed, so the grid is cleared locally instead, and the prompt
    /// stays where IRIS left it.
    ///
    /// Nothing is said about any of this in the status line. The gesture is a
    /// frequent one, and a note that appears under every clear costs a row of
    /// the terminal to say what the screen has already shown; what it does is
    /// on the right-click entry's tooltip instead.
    fn clear_active_terminal(&mut self) {
        self.clear_terminal(self.focused_at());
    }

    /// [`App::clear_active_terminal`] for one named pane, since with a tab
    /// split the right-click menu can be opened over either of them.
    fn clear_terminal(&mut self, at: At) {
        let Some(tab) = self.pane_mut(at) else {
            return;
        };
        tab.view.clear_selection();
        tab.view.scroll_to_bottom();

        // Safe to ask the far side to clear itself: on IRIS that means an
        // idle prompt - it is reading a command line and nothing has been
        // typed on it yet - because the ask is a command line of its own. A
        // shell is sent a keystroke instead, which no `read` can swallow and
        // nothing half typed can absorb, so there is no moment at which it is
        // the wrong thing to send.
        let can_ask = tab.profile.is_shell()
            || lineedit::current(&tab.grid).is_some_and(|line| line.is_empty());
        if tab.session.is_some() && can_ask {
            tab.request_clear();
        } else {
            tab.grid.hard_reset();
        }
    }

    /// The size a session for `profile` should open at.
    ///
    /// A shell gets the window's own width and IRIS the wide grid, for the
    /// reason `wide_grid` in [`terminal_view::RenderOpts`] gives. Both are
    /// only the starting point: the first frame the tab is drawn in resizes it
    /// to what the pane actually measured.
    fn initial_size(&self, profile: &Profile) -> (u16, u16) {
        let (cols, rows) = self.terminal_size;
        if profile.is_shell() {
            let view = (self.view_size.0.max(2)).min(u16::MAX as usize) as u16;
            return (view, rows);
        }
        (cols, rows)
    }

    /// Connects a new tab to whatever `new_tab_profile` currently names.
    pub fn open_new_tab(&mut self) {
        let profile = self.new_tab_profile.clone();
        self.open_tab(profile);
    }

    pub fn open_tab(&mut self, profile: Profile) {
        let (cols, rows) = self.initial_size(&profile);
        self.tabs
            .push(Tab::new(profile, &self.settings, cols, rows));
        self.active = self.tabs.len() - 1;
    }

    /// Closes a tab and everything in it - both sessions, when it is split.
    pub fn close_tab(&mut self, index: usize) {
        if index >= self.tabs.len() {
            return;
        }
        // Ask IRIS to halt so it releases locks; Drop kills anything that
        // ignores the request.
        for session in self.tabs[index].sessions() {
            if let Some(session) = session.session.as_ref() {
                session.request_halt();
            }
        }
        self.tabs.remove(index);
        if self.active >= self.tabs.len() {
            self.active = self.tabs.len().saturating_sub(1);
        }
    }

    /// Closes one pane, and nothing else.
    ///
    /// On a tab that is not split there is only the one session, so this is
    /// [`App::close_tab`]. On a split tab it closes the session the user
    /// pointed at and leaves the other one in the tab, unsplit - which is what
    /// the right-click menu over a pane has to mean, since the other pane
    /// belongs to whatever is running in it.
    fn close_pane(&mut self, at: At) {
        let Some(tab) = self.tabs.get_mut(at.tab) else {
            return;
        };
        if tab.split.is_none() {
            self.close_tab(at.tab);
            return;
        }
        // Ask IRIS to halt so it releases its locks; dropping the session kills
        // anything that ignores the request.
        if let Some(going) = tab.pane(at.pane).and_then(|pane| pane.session.as_ref()) {
            going.request_halt();
        }
        tab.close_pane(at.pane);
    }

    /// The session the user is working in: the focused pane of the active tab.
    fn active_tab(&self) -> Option<&Tab> {
        self.pane(self.focused_at())
    }

    /// Where that session lives.
    fn focused_at(&self) -> At {
        At {
            tab: self.active,
            pane: self
                .tabs
                .get(self.active)
                .map_or(Pane::First, |tab| tab.focus),
        }
    }

    /// The session at `at`, while both the tab and the pane are still there.
    fn pane(&self, at: At) -> Option<&Tab> {
        self.tabs.get(at.tab)?.pane(at.pane)
    }

    fn pane_mut(&mut self, at: At) -> Option<&mut Tab> {
        self.tabs.get_mut(at.tab)?.pane_mut(at.pane)
    }

    /// Every session open, in every tab. What "is anything still connected"
    /// has to count, since a split tab holds two.
    fn sessions(&self) -> impl Iterator<Item = &Tab> {
        self.tabs.iter().flat_map(|tab| tab.sessions())
    }

    fn set_status(&mut self, text: impl Into<String>) {
        self.status = Some(text.into());
        self.status_at = Some(std::time::Instant::now());
    }

    /// Takes the footer message down once it has had its time, and asks for the
    /// frame that will do it.
    ///
    /// Without the repaint request the message would sit there until something
    /// else happened to draw a frame - which, at an idle prompt, is nothing at
    /// all.
    fn expire_status(&mut self, ctx: &Context) {
        let seconds = self.settings.status_timeout_secs;
        if seconds == 0 || self.status.is_none() {
            return;
        }
        let life = std::time::Duration::from_secs(u64::from(seconds));
        let Some(at) = self.status_at else {
            return;
        };
        match life.checked_sub(at.elapsed()) {
            Some(left) if !left.is_zero() => ctx.request_repaint_after(left),
            _ => {
                self.status = None;
                self.status_at = None;
            }
        }
    }

    /// Whether the active terminal currently owns the keyboard.
    ///
    /// Macro shortcuts are gated on this. `Ctrl+Shift+G` is a shortcut when
    /// the terminal has focus and an ordinary editing gesture when the cursor
    /// is in a text field, and only the widget with focus can say which.
    fn terminal_has_focus(&self, ctx: &Context) -> bool {
        let Some(tab) = self.active_tab() else {
            return false;
        };
        let id = egui::Id::new(("nit-terminal", tab.uid));
        ctx.memory(|m| m.focused() == Some(id))
    }

    fn handle_shortcuts(&mut self, ctx: &Context) {
        // The macro editor is listening for a chord: Ctrl+T there means "bind
        // this macro to Ctrl+T", and opening a tab instead would make the app's
        // own shortcuts the only ones that could never be recorded.
        if self.panels.macros.capture_shortcut || self.panels.capture_manager_shortcut {
            return;
        }
        // Nor while the window is not the active one: a macro shortcut sends
        // its lines to a live session, so it answers to the same rule the
        // keyboard does - see [`App::terminal_pane`].
        if !ctx.input(|i| i.viewport().focused.unwrap_or(true)) {
            return;
        }
        let cmd = Modifiers::COMMAND;
        let new_tab = consume_exact(ctx, cmd, Key::T);
        let close_tab = consume_exact(ctx, cmd, Key::W);
        let next_tab = consume_exact(ctx, cmd, Key::Tab);
        let zoom_in = consume_exact(ctx, cmd, Key::Plus) | consume_exact(ctx, cmd, Key::Equals);
        let zoom_out = consume_exact(ctx, cmd, Key::Minus);

        let mut jump = None;
        for (n, key) in [
            Key::Num1,
            Key::Num2,
            Key::Num3,
            Key::Num4,
            Key::Num5,
            Key::Num6,
            Key::Num7,
            Key::Num8,
            Key::Num9,
        ]
        .iter()
        .enumerate()
        {
            if consume_exact(ctx, cmd, *key) {
                jump = Some(n);
            }
        }

        if new_tab {
            self.open_new_tab();
        }
        if close_tab && !self.tabs.is_empty() {
            self.close_tab(self.active);
        }
        if next_tab && !self.tabs.is_empty() {
            self.activate_tab((self.active + 1) % self.tabs.len());
        }
        if let Some(n) = jump {
            self.activate_tab(n);
        }
        // Font size drives cell size, which drives grid dimensions, so zooming
        // reflows and resizes the PTY through the ordinary resize path.
        if zoom_in || zoom_out {
            let delta = if zoom_in { 1.0 } else { -1.0 };
            self.settings.font_size = (self.settings.font_size + delta).clamp(8.0, 28.0);
            let _ = self.settings.save();
        }

        // Ctrl+Delete: reset the terminal and drop the history, the one
        // gesture that is meant to destroy the transcript. Gated on the
        // terminal having focus so it stays "delete word" in a text field.
        let terminal_focus = self.terminal_has_focus(ctx);
        if terminal_focus && consume_exact(ctx, Modifiers::CTRL, Key::Delete) {
            self.clear_active_terminal();
        }

        // The macro manager's own chord, if the user has set one. Before the
        // macros themselves so that a chord bound to both opens the manager -
        // the one of the two that cannot send anything to a live session.
        if terminal_focus {
            if let Some((modifiers, key)) = self
                .settings
                .macro_manager_shortcut
                .as_deref()
                .and_then(shortcut::parse)
            {
                if consume_exact(ctx, modifiers, key) {
                    self.panels.macros.open = true;
                }
            }
        }

        // Macro shortcuts come after the app's own, which is what the editor's
        // "the app already uses this" warning promises. A macro whose `key` is
        // missing or unparseable simply never fires; the text is shared and
        // hand-edited, so it cannot be trusted to mean anything.
        if terminal_focus {
            let mut fire = None;
            for group in &self.macro_groups {
                for m in &group.macros {
                    let Some((modifiers, key)) = m.key.as_deref().and_then(shortcut::parse) else {
                        continue;
                    };
                    if consume_exact(ctx, modifiers, key) {
                        fire = Some(m.clone());
                    }
                }
            }
            // Routed through the ordinary request path, so `confirm` and
            // parameter prompting apply exactly as they do to a click.
            if let Some(m) = fire {
                self.handle_request(ctx, UiRequest::RunMacro(m));
            }
        }
    }

    /// The top row: app controls on the left, and - when the app is drawing its
    /// own frame - the window buttons and the draggable area on the right.
    fn menu_bar(
        &mut self,
        ui: &mut egui::Ui,
        buttons: &crate::config::theme::WindowButtons,
    ) -> Option<WindowAction> {
        let mut action = None;
        // Opening a tab has to happen after the closure: it borrows `self`
        // mutably, and the bar is already holding it.
        let mut open_default = false;
        let mut pick: Option<Profile> = None;
        // The system bar brings its own controls, and the setting can turn the
        // app's off entirely.
        let own_buttons = self.settings.show_window_buttons;
        // Whether the tabs share this row. Read before the closure: it decides
        // both what goes in the middle of the bar and whether the session's own
        // line is drawn at all.
        let inline_tabs = self.settings.tabs_in_title_bar && !self.tabs.is_empty();
        // Taken out of `self` for the length of the row, because the gear is
        // handed to `chrome` as a `&mut bool` while the closure still holds
        // `self` for the tab strip.
        let mut show_settings = self.panels.show_settings;
        // Which end the gear goes on: beside minimize, which means the leading
        // group when that group is the one being drawn. Worked out once,
        // because getting it from each end separately is how the gear ended up
        // on neither - a theme with left-hand buttons and the buttons switched
        // off drew no leading group for it to join and skipped the trailing
        // one because the theme said left. Settings was then unreachable.
        let leading_buttons = own_buttons && buttons.left;
        ui.horizontal(|ui| {
            // Drawn before anything else when the theme puts them on the left,
            // which is where Aqua has them.
            if leading_buttons {
                if let Some(asked) =
                    chrome::leading_window_buttons(ui, buttons, Some(&mut show_settings))
                {
                    action = Some(asked);
                }
                ui.add_space(6.0);
            }
            let endpoint = self.new_tab_profile.endpoint();
            let new_tab = icon_button(ui, icons::Glyph::Plus, buttons.new_tab).on_hover_text(tr1(
                "New session on {} (Ctrl+T).\nRight-click to connect somewhere else.",
                &endpoint,
            ));
            let new_tab_right = new_tab.rect.right();
            if new_tab.clicked() {
                open_default = true;
            }
            // The escape hatch that replaces the dialog: everything it used to
            // offer - the profiles and the instances found on this machine -
            // one click away instead of in front of every new session.
            new_tab.context_menu(|ui| {
                // The launcher's servers first: they are the whole reason this
                // menu is worth opening, and the preferred one is already what
                // the button does.
                if !self.servers.is_empty() {
                    ui.weak(tr("IRIS servers"));
                    let preferred = self.new_tab_profile.name.clone();
                    for server in self.servers.others(Some(&preferred)) {
                        let mut button = ui.button(server.menu_label());
                        // What the entry actually does, since a local server
                        // opens an instance and a remote one asks for a login.
                        let hint = match server.target(&self.instances) {
                            crate::config::servers::Target::Local { instance } => {
                                tr1("Local session on instance {}", &instance)
                            }
                            crate::config::servers::Target::Telnet { address, port } => {
                                tr1("Telnet login to {}", &format!("{address}:{port}"))
                            }
                        };
                        let hint = if server.comment.trim().is_empty() {
                            hint
                        } else {
                            format!("{hint}\n{}", server.comment.trim())
                        };
                        button = button.on_hover_text(hint);
                        if button.clicked() {
                            pick = Some(Profile::for_server(
                                server,
                                &self.instances,
                                &self.new_tab_profile,
                            ));
                            ui.close_menu();
                        }
                    }
                    let loose_instances = self
                        .instances
                        .iter()
                        .any(|name| !self.servers.covers_instance(&self.instances, name));
                    if !self.settings.profiles.is_empty() || loose_instances {
                        ui.separator();
                    }
                }
                for profile in &self.settings.profiles {
                    let label = if profile.instance.is_empty() {
                        profile.name.clone()
                    } else {
                        format!("{}  ({})", profile.name, profile.instance)
                    };
                    if ui.button(label).clicked() {
                        pick = Some(profile.clone());
                        ui.close_menu();
                    }
                }
                if !self.settings.profiles.is_empty() && !self.instances.is_empty() {
                    ui.separator();
                }
                for name in &self.instances {
                    // Skipped when a server entry already opens it: the same
                    // session under two names is not a choice.
                    if self.servers.covers_instance(&self.instances, name) {
                        continue;
                    }
                    if ui.button(name).clicked() {
                        // Built through the same path as a server, which is what
                        // guarantees a local instance opens locally. Inheriting
                        // the current profile wholesale used to carry its
                        // `remote` across, so picking the instance `CONSISTEM`
                        // while a Telnet tab was current opened Telnet again.
                        pick = Some(Profile::for_server(
                            &crate::config::servers::Server::for_instance(name),
                            &self.instances,
                            &self.new_tab_profile,
                        ));
                        ui.close_menu();
                    }
                }
                if self.settings.profiles.is_empty()
                    && self.instances.is_empty()
                    && self.servers.is_empty()
                {
                    ui.weak(tr("No servers, profiles or instances found."));
                }

                // The shells this machine has, under the IRIS entries rather
                // than among them: they are a different kind of session, and
                // nothing about a profile or a namespace applies to one. See
                // [`crate::plugins::shells`].
                let shells = crate::plugins::shells::available();
                if !shells.is_empty() {
                    ui.separator();
                    ui.weak(tr("Shells"));
                    for shell in &shells {
                        if ui
                            .button(&shell.name)
                            .on_hover_text(shell.command_line())
                            .clicked()
                        {
                            pick = Some(Profile::for_shell(shell));
                            ui.close_menu();
                        }
                    }
                }
            });
            // One rule after the new-session button, and none at all when the
            // tabs are up here: the first tab's own edge is the divider, and a
            // rule in front of it - there used to be two, with a gap between
            // them - reads as a slot with something missing out of it.
            //
            // Macros, Export and the IRIS utilities all used to sit between
            // those two rules. Every one of them was about the output rather
            // than about the app, and all three are now on the terminal's own
            // right-click menu, beside the session they act on; writing macros
            // is in Settings, with the themes.
            if !inline_tabs {
                ui.separator();
            }

            // The tabs, when the setting has moved them up here. Drawn in the
            // row rather than into a rectangle handed to `chrome`, which is
            // what lets the row's own cursor measure them: everything after
            // them is then the space they left, and that space is what the
            // window is dragged by.
            if inline_tabs {
                // Bounded, or a strip of tabs long enough would run under the
                // window buttons: a scroll area takes the width it is offered,
                // and what is offered here has the buttons at the end of it.
                // The reserve also leaves a strip in front of them that is
                // always free, so a row filled with tabs still has somewhere
                // to take hold of the window.
                let tabs_from = ui.cursor().min.x;
                let room = ui.available_width() - self.title_bar_reserve();
                self.tab_strip_bounded(ui, room.max(60.0));
                // The sliver between the button and the first tab. Nothing is
                // allocated for it - it is the row's own spacing - it simply
                // drags the window now instead of doing nothing.
                if let Some(asked) = chrome::drag_span(
                    ui,
                    new_tab_right..tabs_from,
                    true,
                    "nit-main",
                    "before-tabs",
                ) {
                    action = Some(asked);
                }
            }

            // The session's own line - instance, PID, geometry - unless the
            // tabs have moved up here, in which case the row is theirs: the
            // two cannot both have the middle of the bar, and the tabs say
            // which session it is anyway.
            if !inline_tabs {
                if let Some(tab) = self.active_tab() {
                    let (cols, rows) = self.view_size;
                    // The instance, then what identifies this session of it,
                    // then how big the window is - in that order because that
                    // is how specific each one is.
                    let pid = match tab.pid().filter(|_| self.settings.show_pid) {
                        Some(pid) => format!("  PID {pid}"),
                        None => String::new(),
                    };
                    let info = format!("{}{pid}  {cols}x{rows}", tab.profile.endpoint());
                    // Reading matter, and nothing else: the window is dragged
                    // by it like any other empty stretch of the bar.
                    if let Some(asked) = chrome::drag_text(
                        ui,
                        egui::RichText::new(info).weak(),
                        true,
                        "nit-main",
                        "session",
                    ) {
                        action = Some(asked);
                    }
                }
            }

            // Last, so the space it claims for dragging is whatever the items
            // above did not take. Claimed even when the window controls are
            // hidden or already drawn on the left: without it there is nothing
            // to drag the window by.
            let trailing = own_buttons && !buttons.left;
            let gear = (!leading_buttons).then_some(&mut show_settings);
            if let Some(asked) = chrome::title_bar_controls(ui, buttons, trailing, "nit-main", gear)
            {
                action = Some(asked);
            }
        });
        self.panels.show_settings = show_settings;

        // A pick opens that session and leaves the default alone. Making the
        // choice stick was worse than it sounds: after one Telnet server, the
        // plain "+" kept reconnecting to it, and there was no longer any way to
        // get back to the preferred server except through the menu.
        if let Some(profile) = pick {
            self.open_tab(profile);
        } else if open_default {
            self.open_new_tab();
        }
        action
    }

    /// Notes where the window is and how big it is, so that the values are at
    /// hand when the app exits and the window has already gone.
    ///
    /// A minimized window is skipped rather than recorded: Windows parks one
    /// off-screen at a nonsense position, and reopening there would put the app
    /// somewhere the user cannot reach it. A maximized one keeps whatever
    /// restored geometry was recorded before it was maximized, which is exactly
    /// what its own restore button would give back.
    fn track_window_geometry(&mut self, ctx: &Context) {
        self.minimized = ctx.input(|i| i.viewport().minimized.unwrap_or(false));
        ctx.input(|i| {
            let viewport = i.viewport();
            if viewport.minimized.unwrap_or(false) {
                return;
            }
            self.window_maximized = viewport.maximized.unwrap_or(false);
            if self.window_maximized || viewport.fullscreen.unwrap_or(false) {
                return;
            }
            if let Some(rect) = viewport.inner_rect {
                if rect.width() >= 1.0 && rect.height() >= 1.0 {
                    self.window_size = Some([rect.width(), rect.height()]);
                }
            }
            if let Some(rect) = viewport.outer_rect {
                // The off-screen parking spot again, for the platforms that do
                // not report `minimized` at all.
                if rect.min.x.is_finite() && rect.min.y.is_finite() && rect.min.x > -30_000.0 {
                    self.window_position = Some([rect.min.x, rect.min.y]);
                }
            }
        });
    }

    /// Writes the geometry back to the settings file, for whichever of the two
    /// switches is on. Called as the app exits.
    fn persist_window_geometry(&mut self) {
        let mut changed = false;
        let settings_seen = self.settings_placement.seen;
        if self.settings.save_terminal_size {
            if self.window_size.is_some() && self.settings.window_size != self.window_size {
                self.settings.window_size = self.window_size;
                changed = true;
            }
            if self.settings.window_maximized != self.window_maximized {
                self.settings.window_maximized = self.window_maximized;
                changed = true;
            }
            // The Settings window follows the same two switches rather than
            // having a pair of its own: it is a window of the app's, and one
            // answer to "remember where my windows were" is enough.
            if settings_seen.size.is_some()
                && self.settings.settings_window_size != settings_seen.size
            {
                self.settings.settings_window_size = settings_seen.size;
                changed = true;
            }
        }
        if self.settings.save_window_position {
            if self.window_position.is_some()
                && self.settings.window_position != self.window_position
            {
                self.settings.window_position = self.window_position;
                changed = true;
            }
            if settings_seen.position.is_some()
                && self.settings.settings_window_position != settings_seen.position
            {
                self.settings.settings_window_position = settings_seen.position;
                changed = true;
            }
        }
        if changed {
            if let Err(e) = self.settings.save() {
                log::warn!("could not save the window geometry: {e}");
            }
        }
    }

    /// Nudges the window until the terminal measures exactly the geometry it
    /// was asked to open at, then stops touching it for the rest of the run.
    ///
    /// `view_cols` and `view_rows` are what the last frame actually drew, so
    /// the difference from the target converts straight into points through
    /// `cell`. The half point of slack is there because the view floors the
    /// space it is given: a window one rounding error short of the target would
    /// otherwise come out a column narrower.
    fn fit_window(&mut self, ctx: &Context, view_cols: usize, view_rows: usize, cell: egui::Vec2) {
        let Some(fit) = self.fit.as_mut() else {
            return;
        };
        let target = (fit.cols as usize, fit.rows as usize);
        let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
        if maximized || fit.attempts == 0 || (view_cols, view_rows) == target {
            let (want_cols, want_rows) = target;
            log::debug!(
                "window opened at {view_cols}x{view_rows} characters, asked for {want_cols}x{want_rows}"
            );
            self.fit = None;
            return;
        }
        let Some(inner) = ctx.input(|i| i.viewport().inner_rect) else {
            return;
        };
        if fit.recentre && fit.centre.is_none() {
            fit.centre = ctx.input(|i| i.viewport().outer_rect).map(|r| r.center());
        }
        fit.attempts -= 1;

        let size = fitted_inner_size(inner.size(), (view_cols, view_rows), target, cell);
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(size));
        if let Some(centre) = fit.centre {
            // The position is the frame's, the size asked for is the content's,
            // so a system title bar has to be added back before the two can be
            // put on top of each other. Zero with the app's own chrome.
            let border = ctx
                .input(|i| i.viewport().outer_rect)
                .map(|outer| outer.size() - inner.size())
                .unwrap_or_default();
            ctx.send_viewport_cmd(egui::ViewportCommand::OuterPosition(
                centre - (size + border) / 2.0,
            ));
        }
        // The measurement that says whether this worked only exists on the next
        // frame, and an idle terminal has no other reason to draw one.
        ctx.request_repaint();
    }

    /// Whether closing should stop and ask first.
    fn should_confirm_close(&self) -> bool {
        !self.close_confirmed
            && self.settings.confirm_close_with_live_session
            && self.sessions().any(|t| t.session.is_some())
    }

    /// Asks before dropping live sessions.
    ///
    /// Honours `confirm_close_with_live_session`, which until now was a setting
    /// nothing read. It covers both routes in: the app's own close button and
    /// the window manager's, since with the system frame gone the second is
    /// often the only one left.
    fn close_confirm_dialog(&mut self, ctx: &Context) {
        if !self.confirm_close {
            return;
        }

        let live = self.sessions().filter(|t| t.session.is_some()).count();
        let mut close_anyway = false;
        let mut cancel = false;
        let mut open = true;

        egui::Window::new(tr("Close newIrisTerminal?"))
            .id(egui::Id::new("nit-close-confirm"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                ui.label(match live {
                    1 => tr("1 session is still connected.").to_string(),
                    n => tr1("{} sessions are still connected.", &n.to_string()),
                });
                ui.small(tr("Closing sends HALT to each of them."));
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(tr("Close anyway")).clicked() {
                        close_anyway = true;
                    }
                    if ui.button(tr("Keep working")).clicked() {
                        cancel = true;
                    }
                });
            });

        if close_anyway {
            self.confirm_close = false;
            self.close_confirmed = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if cancel || !open {
            self.confirm_close = false;
        }
    }

    /// How much of the title-bar row is kept back from the tabs: the window
    /// controls and the gear, plus the strip that is always draggable.
    ///
    /// Measured rather than guessed, because a theme can hide any of the three
    /// controls and the row height decides how wide one is. The tabs are given
    /// what is left, so a long strip of them scrolls instead of running under
    /// the close button. See [`TITLE_FREE_STRIP`] for the rest of it.
    fn title_bar_reserve(&self) -> f32 {
        // The gear is always there; the other three are the theme's to hide.
        let buttons = if self.settings.show_window_buttons {
            4
        } else {
            1
        };
        let side = TITLE_CONTROL_SIDE;
        buttons as f32 * side + TITLE_FREE_STRIP
    }

    /// The tab strip, in no more than `width` of the row it is drawn in.
    ///
    /// Only ever called from a left-to-right row, and that matters:
    /// `allocate_ui_at_rect` gives the child the *parent's* layout, and what it
    /// then advances the row by is the child's `min_rect`. In a right-to-left
    /// parent that rect starts at the right-hand edge, so a narrow strip of
    /// tabs would measure as ending where the row ends and leave nothing after
    /// it - which is precisely how the title bar lost its drag area.
    fn tab_strip_bounded(&mut self, ui: &mut egui::Ui, width: f32) {
        let height = ui.available_height().max(ui.spacing().interact_size.y);
        let rect = egui::Rect::from_min_size(ui.cursor().min, egui::Vec2::new(width, height));
        ui.allocate_ui_at_rect(rect, |ui| self.tab_strip(ui));
    }

    fn tab_strip(&mut self, ui: &mut egui::Ui) {
        let mut to_close = None;
        let mut to_rename = None;
        let mut to_split = None;
        let mut to_unsplit = None;
        let mut to_activate = None;
        let with_namespace = self.settings.show_namespace_in_tab;
        // Shrunk to the tabs rather than filling the row: in the title bar the
        // space left over is what the window is dragged by, and a scroll area
        // that claimed the whole width would take all of it.
        // The strip's own bar follows the same setting as the terminal's, so
        // "show scrollbars" means every scrollbar in the app rather than every
        // scrollbar except this one. Off still scrolls - the wheel and a drag
        // both work - it simply draws nothing along the bottom of the tabs.
        let visibility = if self.settings.show_scrollbars {
            egui::scroll_area::ScrollBarVisibility::VisibleWhenNeeded
        } else {
            egui::scroll_area::ScrollBarVisibility::AlwaysHidden
        };

        // The wheel over the strip, before the scroll area reads the input.
        //
        // egui turns a shifted wheel into horizontal scrolling for us, which
        // left the plain wheel - the one that is actually under the finger -
        // doing nothing at all over a row of tabs. The two are swapped back
        // here: the plain wheel scrolls the strip, and shift walks the
        // selection from tab to tab.
        //
        // Rewritten rather than handled afterwards so the scroll lands on the
        // same frame it was rolled on, and consumed when it changes tabs so
        // the strip does not also slide sideways under the pointer.
        let mut step = 0.0_f32;
        if ui.rect_contains_pointer(ui.available_rect_before_wrap()) {
            ui.input_mut(|i| {
                if i.modifiers.shift {
                    // Already swapped by egui, so the shifted wheel arrives on
                    // x. `raw` rather than the smoothed delta: smoothing
                    // spreads one notch over several frames, which would walk
                    // the selection several tabs from one flick.
                    step = i.raw_scroll_delta.x;
                    i.raw_scroll_delta = egui::Vec2::ZERO;
                    i.smooth_scroll_delta = egui::Vec2::ZERO;
                } else {
                    i.raw_scroll_delta = egui::vec2(i.raw_scroll_delta.y, 0.0);
                    i.smooth_scroll_delta = egui::vec2(i.smooth_scroll_delta.y, 0.0);
                }
            });
        }
        if step != 0.0 && !self.tabs.is_empty() {
            // Up is back, the way it is in a list. Wrapped at both ends: the
            // wheel has no stop, and a selection that silently refuses to move
            // reads as the gesture not working.
            let last = self.tabs.len() - 1;
            to_activate = Some(if step > 0.0 {
                self.active.checked_sub(1).unwrap_or(last)
            } else if self.active >= last {
                0
            } else {
                self.active + 1
            });
        }

        egui::ScrollArea::horizontal()
            .auto_shrink([true, false])
            .scroll_bar_visibility(visibility)
            .show(ui, |ui| {
            ui.horizontal(|ui| {
                for index in 0..self.tabs.len() {
                    let selected = index == self.active;
                    let split = self.tabs[index].split.is_some();
                    let mut label = self.tabs[index].strip_label(with_namespace);
                    if self.tabs[index].focused().ended {
                        label.push_str(" (ended)");
                    }
                    let response = ui
                        .selectable_label(selected, label)
                        .on_hover_text(tr("Double-click to rename."));
                    if response.clicked() {
                        to_activate = Some(index);
                    }
                    if response.double_clicked() {
                        to_rename = Some(index);
                    }
                    // What every other tabbed program does, and the reason the
                    // small cross is not the only way out: closing several
                    // tabs in a row means aiming at a cross the width of a
                    // character each time, and the middle button closes
                    // whatever is under the pointer.
                    if response.middle_clicked() {
                        to_close = Some(index);
                    }
                    response.context_menu(|ui| {
                        if ui.button(tr("Rename...")).clicked() {
                            to_rename = Some(index);
                            ui.close_menu();
                        }
                        ui.separator();
                        // A tab already holding two sessions has nowhere to put
                        // a third, so the only thing offered is the way back.
                        if split {
                            if ui
                                .button(tr("Remove split"))
                                .on_hover_text(tr(
                                    "Gives the second session a tab of its own. Nothing is closed.",
                                ))
                                .clicked()
                            {
                                to_unsplit = Some(index);
                                ui.close_menu();
                            }
                        } else {
                            if ui
                                .button(tr("Split to right"))
                                .on_hover_text(tr(
                                    "Opens a second session in this tab, beside this one. Click into a pane to type in it.",
                                ))
                                .clicked()
                            {
                                to_split = Some((index, SplitDir::Right));
                                ui.close_menu();
                            }
                            if ui.button(tr("Split to bottom")).clicked() {
                                to_split = Some((index, SplitDir::Bottom));
                                ui.close_menu();
                            }
                        }
                        ui.separator();
                        if ui
                            .button(tr("Close"))
                            .on_hover_text(if split {
                                tr("Closes both sessions in this tab.")
                            } else {
                                tr("Closes this session.")
                            })
                            .clicked()
                        {
                            to_close = Some(index);
                            ui.close_menu();
                        }
                    });
                    if icon_button(ui, icons::Glyph::SmallCross, None)
                        .on_hover_text(tr("Close this tab."))
                        .clicked()
                    {
                        to_close = Some(index);
                    }
                    ui.separator();
                }
            });
        });

        if let Some(index) = to_activate {
            self.activate_tab(index);
        }
        if let Some((index, dir)) = to_split {
            self.split_tab(index, dir);
        }
        if let Some(index) = to_unsplit {
            self.remove_split(index);
        }
        if let Some(index) = to_rename {
            // Seeded with the names on screen, so renaming is an edit rather
            // than starting from nothing.
            let tab = &self.tabs[index];
            self.renaming = Some(Renaming {
                tab: index,
                draft: tab.title(with_namespace),
                second: tab
                    .pane(Pane::Second)
                    .map(|second| second.title(with_namespace)),
                focus: true,
            });
        }
        // Applied after the loop: closing a tab shifts every index after it.
        if let Some(index) = to_close {
            self.close_tab(index);
        }
    }

    /// Makes a tab the active one, which is the tab drawn and the one every
    /// other feature works on. Which of its panes is current is the tab's own
    /// business - see [`Tab::focus`].
    fn activate_tab(&mut self, index: usize) {
        if index < self.tabs.len() {
            self.active = index;
        }
    }

    /// Splits a tab in two, opening a second session in the pane it makes.
    ///
    /// The new session goes where the pane appears - to the right, or at the
    /// bottom - and takes the keyboard, the way a newly opened tab does. A tab
    /// that is already split is left alone: it has two names to show and no
    /// room for a third.
    fn split_tab(&mut self, index: usize, dir: SplitDir) {
        if self.tabs.get(index).is_none_or(|tab| tab.split.is_some()) {
            return;
        }
        let (cols, rows) = self.initial_size(&self.new_tab_profile.clone());
        let opened = Tab::new(self.new_tab_profile.clone(), &self.settings, cols, rows);
        self.tabs[index].split = Some(Split {
            dir,
            tab: Box::new(opened),
            // Down the middle to start with. The divider between them is what
            // moves it from there.
            ratio: 0.5,
        });
        self.tabs[index].focus = Pane::Second;
        self.active = index;
    }

    /// Takes a split tab back to one session, and gives the other one a tab of
    /// its own.
    ///
    /// Promoted rather than closed: it is a live IRIS session, and "remove
    /// split" is a sentence about the layout. Closing the tab is what closes
    /// both. The keyboard stays with whichever of the two had it.
    fn remove_split(&mut self, index: usize) {
        let Some(split) = self.tabs.get_mut(index).and_then(|tab| tab.split.take()) else {
            return;
        };
        let had_focus = std::mem::take(&mut self.tabs[index].focus);
        self.tabs.insert(index + 1, *split.tab);
        self.active = if had_focus == Pane::Second {
            index + 1
        } else {
            index
        };
    }

    /// Names one tab.
    ///
    /// A window rather than an editable label in the strip: the terminal claims
    /// the keyboard whenever nothing else holds it, and a field that has to win
    /// that fight every frame is a worse trade than one dialog.
    fn rename_tab_dialog(&mut self, ctx: &Context) {
        let Some(mut renaming) = self.renaming.take() else {
            return;
        };
        if renaming.tab >= self.tabs.len() {
            return;
        }

        let mut open = true;
        let mut commit = false;
        let mut cancel = false;

        egui::Window::new(tr("Rename tab"))
            .id(egui::Id::new("nit-rename-tab"))
            .collapsible(false)
            .resizable(false)
            .open(&mut open)
            .show(ctx, |ui| {
                // A split tab is two sessions under one entry, so it is renamed
                // two names at a time. The `1:` and `2:` are the panes
                // themselves and cannot be edited away.
                let split = renaming.second.is_some();
                let field = |ui: &mut egui::Ui, label: Option<&str>, text: &mut String| {
                    let mut response = None;
                    ui.horizontal(|ui| {
                        if let Some(label) = label {
                            ui.label(label);
                        }
                        response =
                            Some(ui.add(egui::TextEdit::singleline(text).desired_width(220.0)));
                    });
                    response.expect("the field is always added")
                };

                let response = field(ui, split.then_some("1:"), &mut renaming.draft);
                if renaming.focus {
                    response.request_focus();
                    renaming.focus = false;
                }
                if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    commit = true;
                }
                if let Some(second) = renaming.second.as_mut() {
                    let response = field(ui, Some("2:"), second);
                    if response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        commit = true;
                    }
                }

                // The one button there used to be for this is what an empty
                // field already does, and two ways to say the same thing in a
                // dialog this small only asks the user which one is real.
                ui.small(tr("Empty goes back to default."));
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button(tr("Rename")).clicked() {
                        commit = true;
                    }
                    if ui.button(tr("Cancel")).clicked() {
                        cancel = true;
                    }
                });
            });

        if commit {
            let named = |text: &str| {
                let name = text.trim().to_string();
                (!name.is_empty()).then_some(name)
            };
            self.tabs[renaming.tab].custom_title = named(&renaming.draft);
            if let (Some(text), Some(second)) = (
                renaming.second.as_deref(),
                self.tabs[renaming.tab].pane_mut(Pane::Second),
            ) {
                second.custom_title = named(text);
            }
        } else if !(cancel || !open) {
            // Still open, so the draft survives to the next frame.
            self.renaming = Some(renaming);
        }
    }

    /// Carries out whatever a panel asked for.
    fn handle_request(&mut self, ctx: &Context, request: UiRequest) {
        match request {
            UiRequest::RunMacro(m) => {
                // Parameters or a confirmation flag both mean "ask first".
                if m.needs_input() || m.confirm {
                    self.panels.pending = Some(PendingMacro::new(m));
                } else {
                    let lines = m.expand(&[]);
                    self.send_lines_to_active(&lines);
                }
            }
            UiRequest::RunNative(native, values) => {
                let invocation = native.build(&values);
                self.send_lines_to_active(&invocation.lines);
                self.set_status(invocation.summary);
            }
            UiRequest::SendLines(lines) => self.send_lines_to_active(&lines),

            UiRequest::ExportText(range) => self.export(range, export::Format::Text),
            UiRequest::ExportHtml(range) => self.export(range, export::Format::Html),
            UiRequest::CopyRange(range) => {
                if let Some(tab) = self.active_tab() {
                    let text = export::to_text(&tab.grid, range);
                    ctx.copy_text(text);
                    self.set_status(tr("Copied to the clipboard."));
                }
            }

            UiRequest::SettingsChanged => {
                // The style is reapplied either way: a failed write still has
                // to be reflected on screen, or the UI would disagree with the
                // settings the user just changed.
                App::apply_style(ctx, &self.theme(), &self.settings);
                self.apply_font(ctx);
                self.history.set_persist(
                    &config::command_history_path(),
                    self.settings.save_command_history,
                );
                // The next check has to authenticate as whoever the field now
                // names, not as whoever it named when the app started.
                update::configure_proxy_user(&self.settings.proxy_user);
                if let Err(e) = self.settings.save() {
                    self.set_status(tr1("Could not save settings: {}", &format!("{e:#}")));
                }
            }
            UiRequest::OpenFolder(path) => {
                if let Err(e) = config::open_in_file_manager(&path) {
                    self.set_status(tr2(
                        "Could not open {}: {}",
                        &path.display().to_string(),
                        &format!("{e:#}"),
                    ));
                }
            }
            UiRequest::CheckForUpdates => self.updates.check_now(),
            UiRequest::SetProxyPassword(password) => match update::set_proxy_password(&password) {
                Ok(()) if password.is_empty() => self.set_status(tr("Proxy password forgotten.")),
                Ok(()) => self.set_status(tr("Proxy password saved.")),
                Err(e) => self.set_status(tr1(
                    "Could not save the proxy password: {}",
                    &format!("{e:#}"),
                )),
            },
            UiRequest::SavePersonalMacros => {
                let path = config::personal_macros_path();
                let xml = macros::to_xml(&self.macro_groups);
                match std::fs::write(&path, xml) {
                    Ok(()) => self.set_status(tr1(
                        "Saved personal macros to {}",
                        &path.display().to_string(),
                    )),
                    Err(e) => self.set_status(tr1("Could not save macros: {}", &format!("{e:#}"))),
                }
            }
        }
    }

    /// Takes whatever the update thread has said since the last frame.
    fn poll_updates(&mut self) {
        for event in self.updates.drain() {
            match event {
                update::Event::Available(release) => {
                    self.updates.available = Some(release);
                    self.updates.asked = true;
                }
                update::Event::UpToDate => {
                    // Only worth saying when the user asked the question.
                    if self.updates.announce {
                        self.set_status(tr1("This is the newest version ({}).", update::CURRENT));
                    }
                }
                update::Event::Downloaded(path) => {
                    self.updates.downloading = false;
                    self.updates.staged = Some(path);
                }
                update::Event::Failed(why) => {
                    let downloading = self.updates.downloading;
                    self.updates.downloading = false;
                    if downloading {
                        // The failure this used to swallow. A download that
                        // stopped left the dialog showing its Download button
                        // again with nothing said, which reads exactly like a
                        // button that does nothing - and was how a timeout on
                        // a slow connection looked. It is the user's problem
                        // now: they pressed the button.
                        self.updates.error = Some(why.clone());
                        self.set_status(tr1("Could not download the update: {}", &why));
                    } else if self.updates.announce {
                        // A failed check at startup is not the user's problem:
                        // it goes to the log. One they asked for is answered.
                        self.set_status(tr1("Could not check for updates: {}", &why));
                    } else {
                        log::warn!("update check failed: {why}");
                    }
                }
            }
            self.updates.announce = false;
            // The exchange is over, so nothing is left to poll for: this is
            // what stops `working` from keeping the frame loop awake for the
            // rest of the session.
            self.updates.events = None;
        }
    }

    /// Puts the downloaded build in place and closes, so the copy that is
    /// starting takes over.
    fn apply_update(&mut self, ctx: &Context) {
        let Some(staged) = self.updates.staged.clone() else {
            return;
        };
        match update::install(&staged) {
            Ok(()) => {
                // The new copy is already starting; this one has to go, and it
                // goes the way Alt+F4 does so a live session is still asked
                // about.
                self.updates.available = None;
                self.updates.staged = None;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
            Err(e) => {
                self.updates.error = Some(format!("{e:#}"));
                self.set_status(tr1("Could not apply the update: {}", &format!("{e:#}")));
            }
        }
    }

    /// Hands this session over to Claude Code, with `panes` deciding whether
    /// the other half of a split goes with it.
    ///
    /// The focused pane is always in the file. A split is usually two halves of
    /// one problem - a routine running on one side, a global inspected on the
    /// other - so `Both` is there for when leaving one out is what would make
    /// the answer useless. On a tab that is not split it means the same thing
    /// as `Focused`.
    fn analyze_with_claude(&mut self, at: At, scope: analyze::Scope, panes: analyze::Panes) {
        if self.pane(at).is_none() {
            self.set_status(tr("No active session."));
            return;
        }
        // Both panes in the order they are on screen, so "pane 1" in the file
        // is the pane called `1:` in the tab strip - rather than in the order
        // the keyboard happens to be in.
        let wanted: Vec<At> = match panes {
            analyze::Panes::Both if self.tabs.get(at.tab).is_some_and(|t| t.split.is_some()) => {
                vec![
                    At {
                        tab: at.tab,
                        pane: Pane::First,
                    },
                    At {
                        tab: at.tab,
                        pane: Pane::Second,
                    },
                ]
            }
            _ => vec![at],
        };

        // The endpoint and the selection come out in one pass, and the sources
        // borrow them: both are owned values a `Source` only points at, so they
        // have to outlive it.
        let captured: Vec<(&Tab, String, Option<String>)> = wanted
            .iter()
            .filter_map(|&at| self.pane(at))
            .map(|tab| {
                (
                    tab,
                    tab.profile.endpoint(),
                    tab.view.selected_text(&tab.grid),
                )
            })
            .collect();
        let sources: Vec<analyze::Source> = captured
            .iter()
            .map(|(tab, endpoint, selection)| analyze::Source {
                endpoint,
                grid: &tab.grid,
                selection: selection.as_deref(),
            })
            .collect();

        let path = match analyze::write_context(&sources, scope) {
            Ok(path) => path,
            Err(e) => {
                self.set_status(tr1("Could not prepare the output: {}", &format!("{e:#}")));
                return;
            }
        };
        // A tab of its own rather than a window of the operating system's: the
        // conversation is about what is on the terminal, so it belongs beside
        // it. A `claude` that is not installed fails the way any other shell
        // tab does - the tab opens and says what went wrong - which is more
        // use than a status line, because the message is in front of the file
        // it was going to read.
        self.open_tab(analyze::tab_profile(&path));
        self.set_status(tr1(
            "Claude is opening with this output in context ({}). Ask it whatever you like.",
            &path.display().to_string(),
        ));
    }

    /// Carries out what the theme manager asked for.
    ///
    /// The manager edits the themes in place so the window behind it repaints
    /// as a colour is dragged; this is the half that reaches the disk, which a
    /// paint pass has no business doing.
    fn apply_theme_action(&mut self, ctx: &Context, action: ThemeAction) {
        match action {
            ThemeAction::Activate(name) => {
                self.settings.theme = name;
                if let Err(e) = self.settings.save() {
                    self.set_status(tr1("Could not save settings: {}", &format!("{e:#}")));
                }
                App::apply_style(ctx, &self.theme(), &self.settings);
                self.apply_font(ctx);
            }
            ThemeAction::Save(name) => {
                let Some(index) = self.themes.iter().position(|t| t.name == name) else {
                    return;
                };
                // A built-in lives in the binary. Nothing here can edit one, so
                // nothing here writes one out either.
                if self.themes[index].builtin {
                    return;
                }
                let path = match self.themes[index].path.clone() {
                    Some(path) => path,
                    None => {
                        let path = config::theme_path_for(&name);
                        self.themes[index].path = Some(path.clone());
                        path
                    }
                };
                if let Err(e) = config::save_theme(&self.themes[index], &path) {
                    self.set_status(tr1("Could not save theme: {}", &format!("{e:#}")));
                }
            }
            ThemeAction::Delete(name) => {
                let Some(index) = self.themes.iter().position(|t| t.name == name) else {
                    return;
                };
                if self.themes[index].builtin {
                    return;
                }
                let removed = self.themes.remove(index);
                if let Some(path) = removed.path.as_ref() {
                    if let Err(e) = std::fs::remove_file(path) {
                        self.set_status(tr2(
                            "Could not delete {}: {}",
                            &path.display().to_string(),
                            &e.to_string(),
                        ));
                    }
                }
                // Deleting the theme in use has to leave something on screen.
                if self.settings.theme == name {
                    let fallback = self
                        .themes
                        .first()
                        .map(|t| t.name.clone())
                        .unwrap_or_default();
                    self.apply_theme_action(ctx, ThemeAction::Activate(fallback));
                } else {
                    self.set_status(tr1("Deleted theme {}.", &name));
                }
            }
        }
    }

    /// Remembers the command sitting on the active tab's prompt line.
    ///
    /// Read off the screen rather than accumulated from keystrokes, so a line
    /// that arrived by paste, or that IRIS recalled itself, counts exactly like
    /// a typed one. A row with no IRIS prompt on it is not a command line and
    /// is not recorded - which is also what keeps a password out of the file,
    /// since a credential prompt carries no `>` and is never echoed.
    /// `unechoed` is text typed in the same frame as the Enter, which IRIS has
    /// not sent back yet and which the screen therefore does not show.
    fn record_command(&mut self, at: At, unechoed: &str) {
        let Some(tab) = self.pane_mut(at) else {
            return;
        };
        tab.recall_step = None;
        if tab.autologon.state() == AutoState::WaitPassword {
            return;
        }
        let Some(text) = lineedit::typed_text(&tab.grid) else {
            return;
        };
        let command = format!("{text}{unechoed}");
        // Twice over, because the two lists answer different questions: this
        // session's own recall, and what the next start will inherit.
        tab.remember_command(&command);
        self.history.record(&command);
    }

    /// Replaces the line being typed with the next command from the history.
    ///
    /// IRIS keeps its own recall, but only for as long as the process lives and
    /// only for what was typed into it; this is the app's - this session's own
    /// commands first, then the ones every session that came before left
    /// behind, when the setting allows. What another tab has typed while this
    /// one was open belongs to that tab and is not offered here.
    ///
    /// Done by rubbing the line out and typing the replacement, because IRIS
    /// owns the read buffer and the only way to change it is to send the keys
    /// that would have changed it.
    ///
    /// The cursor need not be at the end of the line: rubout erases backwards,
    /// so it is walked to the end first and the whole replacement goes out as
    /// one write. Whether Up mid-line recalls at all is
    /// `settings.recall_mid_line`, decided back in [`input::translate`].
    fn recall(&mut self, at: At, direction: input::Recall) {
        let Some(tab) = self.pane(at) else {
            return;
        };
        // The state could have moved off the command line between the key
        // press and here.
        let Some(line) = lineedit::current(&tab.grid) else {
            return;
        };
        let app_cursor = tab.grid.app_cursor_keys;
        let encoding = tab.profile.wire_encoding();
        let walked_to = tab.recall_step;

        let list = self.history.recall_list(&tab.commands);
        let next = match direction {
            // Already at the oldest command, or nothing recalled to walk
            // forward from: leave the line exactly as it is rather than
            // clearing it.
            input::Recall::Back => match history::step_back(walked_to, list.len()) {
                Some(next) => next,
                None => return,
            },
            input::Recall::Forward => match history::step_forward(walked_to) {
                Some(next) => next,
                None => return,
            },
        };

        // No step left is the line the user started on, which is an empty one.
        let text = next
            .and_then(|step| list.get(step))
            .copied()
            .unwrap_or_default()
            .to_string();

        let mut wire = cursor_bytes(line.end as i64 - line.cursor as i64, app_cursor);
        wire.extend(std::iter::repeat_n(0x7f, line.len()));
        wire.extend_from_slice(&encoding.encode(&text));
        let wire = self.plugins.on_input(&wire);
        if let Some(tab) = self.pane_mut(at) {
            tab.send(&wire);
            tab.recall_step = next;
            tab.view.scroll_to_bottom();
        }
    }

    /// Selects the command being typed, so it can be copied or rubbed out in
    /// one gesture. Ctrl+A.
    fn select_typed_line(&mut self, at: At) {
        let Some(tab) = self.pane_mut(at) else {
            return;
        };
        let Some(line) = lineedit::current(&tab.grid).filter(|l| !l.is_empty()) else {
            return;
        };
        let row = tab.grid.scrollback.len() + tab.grid.cursor.row;
        tab.view.selection = Some(Selection::across(row, line.start, line.end));
    }

    /// Drags the loose end of the selection over the command line, the way
    /// Shift plus a movement key does in a text field.
    ///
    /// The anchor is where the selection was started from - IRIS's cursor, the
    /// first time - and only the other end moves, so shift-left and then
    /// shift-right walks back over what was just selected. Nothing is sent:
    /// IRIS's cursor stays put and only the highlight moves.
    fn extend_selection(&mut self, at: At, motion: Motion) {
        let Some(tab) = self.pane_mut(at) else {
            return;
        };
        let Some(line) = lineedit::current(&tab.grid) else {
            return;
        };
        let row = tab.grid.scrollback.len() + tab.grid.cursor.row;

        // An existing selection on this line continues; anything else - none at
        // all, or one left over in the scrollback - starts again from the
        // cursor.
        let (anchor, focus) = tab
            .view
            .selection
            .filter(|s| s.span_on(row).is_some())
            .map(|s| (s.start.1, s.end.1))
            .unwrap_or((line.cursor, line.cursor));
        let anchor = anchor.clamp(line.start, line.end);
        let focus = lineedit::target(&tab.grid, line, focus, motion);

        // Walking the loose end back onto the anchor selects nothing, which is
        // no selection at all rather than an empty one - an empty selection
        // would go on quietly claiming Ctrl+C.
        tab.view.selection = (focus != anchor).then_some(Selection {
            start: (row, anchor),
            end: (row, focus),
        });
    }

    /// Walks IRIS's cursor over the command line by a whole motion - a word at
    /// a time, for Ctrl plus an arrow.
    ///
    /// Only the app knows where the word boundaries are, since only the app can
    /// see the line; IRIS is then told in the one language it acts on, which is
    /// arrow keys. The selection goes, exactly as it would in a text field when
    /// the cursor walks away from it.
    fn move_by(&mut self, at: At, motion: Motion) {
        let Some(tab) = self.pane(at) else {
            return;
        };
        let Some(line) = lineedit::current(&tab.grid) else {
            return;
        };
        let to = lineedit::target(&tab.grid, line, line.cursor, motion);
        let columns = to as i64 - line.cursor as i64;
        if let Some(tab) = self.pane_mut(at) {
            tab.view.clear_selection();
        }
        self.move_cursor(at, columns);
    }

    /// Rubs the selected part of the command line out of IRIS's read buffer.
    ///
    /// Rubout erases the character *before* the cursor, so the cursor is walked
    /// to the end of the selection first and the whole thing goes out as one
    /// write - a half-applied erase would leave the line in a state neither
    /// side agrees on.
    fn erase_selection(&mut self, at: At) {
        let Some(tab) = self.pane(at) else {
            return;
        };
        let Some(line) = lineedit::current(&tab.grid) else {
            return;
        };
        let Some((from, to)) = selection_in_line(tab, line) else {
            return;
        };

        let mut wire = cursor_bytes(to as i64 - line.cursor as i64, tab.grid.app_cursor_keys);
        wire.extend(std::iter::repeat_n(0x7f, to - from));
        let wire = self.plugins.on_input(&wire);
        if let Some(tab) = self.pane_mut(at) {
            tab.send(&wire);
            tab.view.clear_selection();
            tab.recall_step = None;
        }
    }

    /// Walks IRIS's cursor `columns` to the right, or to the left when
    /// negative, with the arrow keys it does act on.
    fn move_cursor(&mut self, at: At, columns: i64) {
        let app_cursor = self.pane(at).is_some_and(|tab| tab.grid.app_cursor_keys);
        let wire = cursor_bytes(columns, app_cursor);
        if wire.is_empty() {
            return;
        }
        let wire = self.plugins.on_input(&wire);
        if let Some(tab) = self.pane(at) {
            tab.send(&wire);
        }
    }

    /// Puts one pane's selection on the clipboard, if it has one.
    fn copy_selection(&self, ctx: &Context, at: At) {
        let Some(tab) = self.pane(at) else {
            return;
        };
        if let Some(text) = tab.view.selected_text(&tab.grid) {
            ctx.copy_text(text);
        }
    }

    /// Copies the selection and types it straight back into the same session.
    ///
    /// The gesture this terminal is actually asked for: a global name, a
    /// routine label or an error location is on screen, and it has to reach the
    /// command line. By hand that is a copy, a click into the prompt and a
    /// paste. The clipboard is filled all the same, so what was picked up is
    /// still available to whatever else the user is working in.
    ///
    /// It goes through the same sanitiser a real paste does, so a selection
    /// spanning two lines arrives as two lines - which at a prompt means IRIS
    /// runs the first of them. That is what pasting the same text does, and
    /// pretending otherwise would be a different gesture with the same name.
    fn copy_and_paste(&mut self, ctx: &Context, at: At) {
        let Some((text, encoding)) = self.pane(at).and_then(|tab| {
            Some((
                tab.view.selected_text(&tab.grid)?,
                tab.profile.wire_encoding(),
            ))
        }) else {
            return;
        };
        ctx.copy_text(text.clone());
        let wire = self
            .plugins
            .on_input(&encoding.encode(&input::sanitize_paste(&text)));
        if let Some(tab) = self.pane_mut(at) {
            // The same two things typing does: what the recall had walked to is
            // no longer where the line came from, and the view returns to the
            // live output.
            tab.recall_step = None;
            tab.view.scroll_to_bottom();
            tab.send(&wire);
        }
    }

    fn send_lines_to_active(&mut self, lines: &[String]) {
        let at = self.focused_at();
        let Some(tab) = self.pane(at) else {
            self.set_status(tr("No active session."));
            return;
        };
        if tab.session.is_none() {
            self.set_status(tr("That session has ended."));
            return;
        }
        // Whatever was half-typed goes first. A macro is a whole command, and
        // appended to the tail of an unfinished one it is not the command
        // either of them was: IRIS would read `write 1ZWRITE ^CSW1`.
        self.clear_typed_line(at);
        if let Some(tab) = self.pane(at) {
            tab.send_lines(lines);
        }
    }

    /// Rubs out whatever is sitting on a pane's prompt line, so something else
    /// can be sent to it.
    ///
    /// The same mechanics as [`App::recall`]: the cursor is walked to the end
    /// of the line first, because rubout erases backwards, and the whole thing
    /// goes out as one write. Does nothing off a command line - a routine
    /// reading a line of its own owns those keys.
    fn clear_typed_line(&mut self, at: At) {
        let Some(tab) = self.pane(at) else {
            return;
        };
        let Some(line) = lineedit::current(&tab.grid).filter(|line| !line.is_empty()) else {
            return;
        };
        let mut wire = cursor_bytes(
            line.end as i64 - line.cursor as i64,
            tab.grid.app_cursor_keys,
        );
        wire.extend(std::iter::repeat_n(0x7f, line.len()));
        let wire = self.plugins.on_input(&wire);
        if let Some(tab) = self.pane_mut(at) {
            tab.send(&wire);
            tab.recall_step = None;
        }
    }

    /// How far the terminal is held back from the window's own edges.
    ///
    /// The resize grips live in a foreground layer along each edge, which
    /// outranks whatever is under them for hit-testing - and the terminal
    /// reaches three of those edges. Its first column was therefore inside the
    /// left-hand grip: the pointer turned into a resize arrow over it, and a
    /// drag starting there resized the window instead of selecting the text.
    /// Holding the grid back by exactly the width of the grip gives the column
    /// back without taking anything away from the grip.
    ///
    /// A point wider than the grip rather than exactly as wide: at exactly as
    /// wide, the first column began on the last coordinate the grip still
    /// answered to, which is the boundary case that left this reported as
    /// unfixed. The grip is also told to keep off the panes outright - see
    /// `keep_out` in [`chrome::resize_grips`] - so the two now disagree in the
    /// terminal's favour whatever the arithmetic works out to.
    ///
    /// Nothing is held back at the top - the tab strip is there, not the
    /// terminal.
    fn terminal_inset(&self) -> egui::Margin {
        let gutter = chrome::RESIZE_GRAB + 1.0;
        egui::Margin {
            left: gutter,
            right: gutter,
            top: 0.0,
            bottom: gutter,
        }
    }

    /// Draws one pane - a tab's own notes and its terminal - and carries out
    /// everything the mouse and the keyboard did in it.
    ///
    /// `focused` marks the pane the keyboard is in - the focused pane of the
    /// active tab, and the only one drawn when a tab is not split. It is the
    /// one that takes the keyboard back when no other widget wants it; the
    /// other pane takes it when it is clicked into, which is also what moves
    /// the tab's focus there.
    fn terminal_pane(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &Context,
        theme: &Theme,
        at: At,
        focused: bool,
    ) -> PaneMeasure {
        let mut opts = self.render_opts();
        // ObjectScript colouring on a `bash` session colours the wrong things:
        // a `$` is a variable to the highlighter and a prompt to the shell.
        // Turned off for the pane rather than for the app, so an IRIS tab
        // beside a shell tab keeps its colours.
        if self.pane(at).is_some_and(|tab| tab.profile.is_shell()) {
            opts.syntax = false;
            // A shell draws to the width it is told, so telling it the grid's
            // 16384 columns makes every full-screen program lay itself out
            // that wide and then wrap into screenfuls of padding. Only IRIS,
            // which truncates instead of drawing, needs the wide margin.
            opts.wide_grid = false;
        }
        let uid = match self.pane(at) {
            Some(tab) => tab.uid,
            // Nothing there to draw: a tab that went away between frames.
            None => return PaneMeasure::default(),
        };
        // Only the focused pane follows the active session, and only it claims
        // the keyboard when nothing else holds it. Two panes both claiming it
        // would take it from each other every frame.
        let role = terminal_view::PaneRole {
            focused,
            take_focus: focused && self.focused_tab != Some(uid),
            split: self.tabs.get(at.tab).is_some_and(|tab| tab.split.is_some()),
        };
        if focused {
            self.focused_tab = Some(uid);
        }

        // The macro list is lent to the pane for the length of the draw, and
        // taken back straight afterwards. Moved out rather than borrowed
        // because drawing a pane borrows the whole app mutably to reach the
        // tab, and the menu is drawn from inside that - the list itself is
        // never written from there.
        let groups = std::mem::take(&mut self.macro_groups);
        let (result, has_selection) = {
            let Some(tab) = self.pane_mut(at) else {
                self.macro_groups = groups;
                return PaneMeasure::default();
            };
            let has_selection = tab.view.selection.map(|s| !s.is_empty()).unwrap_or(false);
            let result = draw_pane(ui, tab, theme, &opts, role, &groups);
            (result, has_selection)
        };
        self.macro_groups = groups;

        // This pane's own session, sized to this pane: a pane is a window onto
        // one session, and telling IRIS about the whole split area would
        // truncate its output at a width nothing is drawn at.
        if !self.minimized {
            if let Some(tab) = self.pane_mut(at) {
                tab.resize(result.cols, result.rows);
            }
        }
        self.pane_rects.push(result.response.rect);
        let measure = PaneMeasure {
            grid: (result.cols, result.rows),
            view: (result.view_cols, result.view_rows),
            cell: result.cell,
            clicked: result.response.clicked(),
        };

        // Right-click menu actions reuse the same paths as the keyboard
        // shortcuts. Copy-on-select: a finished drag goes straight to the
        // clipboard, without waiting for Ctrl+C.
        if result.copy_selection {
            self.copy_selection(ctx, at);
        }
        if let Some(columns) = result.cursor_move {
            self.move_cursor(at, columns);
        }

        if let Some(action) = result.context_action {
            use terminal_view::ContextAction;
            match action {
                ContextAction::CopySelection => self.copy_selection(ctx, at),
                ContextAction::CopyAndPaste => self.copy_and_paste(ctx, at),
                ContextAction::Paste => {
                    // The clipboard is only readable through egui's paste
                    // event, so ask for one rather than reaching for the OS
                    // clipboard behind egui's back.
                    ctx.send_viewport_cmd(egui::ViewportCommand::RequestPaste);
                }
                ContextAction::SelectAll => {
                    if let Some(tab) = self.pane_mut(at) {
                        let grid = &tab.grid;
                        tab.view.select_all(grid);
                    }
                }
                ContextAction::ClearSelection => {
                    if let Some(tab) = self.pane_mut(at) {
                        tab.view.clear_selection();
                    }
                }
                ContextAction::Export(format, range) => self.export(range, format),
                ContextAction::CopyRange(range) => {
                    self.handle_request(ctx, UiRequest::CopyRange(range));
                }
                // Both go through the ordinary request path, which is what
                // decides between sending at once and opening the dialog that
                // asks for the parameters or the confirmation first.
                ContextAction::RunMacro(m) => self.handle_request(ctx, UiRequest::RunMacro(m)),
                ContextAction::RunNative(native) => {
                    self.panels.pending_native = Some(panels::PendingNative::new(native));
                }
                ContextAction::Analyze(scope, panes) => self.analyze_with_claude(at, scope, panes),
                ContextAction::ClearTerminal => self.clear_terminal(at),
                // The layout of the tab this pane is in. Recorded rather than
                // done, because the tabs are being drawn - see
                // [`LayoutAction`]. `at` is the pane the menu was opened over,
                // so Close closes that session and nothing else.
                ContextAction::SplitRight => {
                    self.pending_layout = Some(LayoutAction::Split(at.tab, SplitDir::Right));
                }
                ContextAction::SplitBottom => {
                    self.pending_layout = Some(LayoutAction::Split(at.tab, SplitDir::Bottom));
                }
                ContextAction::Unsplit => {
                    self.pending_layout = Some(LayoutAction::Unsplit(at.tab));
                }
                ContextAction::ClosePane => {
                    self.pending_layout = Some(LayoutAction::ClosePane(at));
                }
            }
        }

        // Only the focused terminal consumes keystrokes.
        if result.response.clicked() {
            result.response.request_focus();
        }
        // While the macro editor is listening for a chord, the keys belong to
        // the shortcut being recorded. The editor is drawn after this and takes
        // them out of the frame's events itself, but that is too late to stop
        // them reaching IRIS from here - and a recorded Ctrl+D would otherwise
        // have ended the session it was recorded in.
        //
        // And nothing is typed into IRIS while this window is not the active
        // one. That should never arise - the system sends keys to the window
        // that has them - but the shell puts surfaces of its own over the top
        // of whatever is in front, the clipboard history (Win+V) among them,
        // and a keystroke meant for one of those must not reach a live session:
        // Up and Down at an IRIS prompt walk the command history, and the
        // session is on a database the whole team shares. `focused` is unknown
        // on a platform that does not report it, and unknown counts as focused
        // rather than leaving the terminal unable to type at all.
        let window_active = ctx.input(|i| i.viewport().focused.unwrap_or(true));
        // Either of the two shortcut pickers - a macro's binding, or the macro
        // manager's own chord - is listening, and the keys belong to whichever
        // one it is.
        let capturing = self.panels.macros.capture_shortcut || self.panels.capture_manager_shortcut;
        if !result.response.has_focus() || capturing || !window_active {
            return measure;
        }

        let events = ui.input(|i| i.events.clone());
        let (insert_down, delete_down) = input::chord_keys_down();
        let (input_ctx, encoding) = {
            let Some(tab) = self.pane(at) else {
                return measure;
            };
            let line = lineedit::current(&tab.grid);
            let input_ctx = input::InputContext {
                has_selection,
                line,
                app_cursor_keys: tab.grid.app_cursor_keys,
                recall_mid_line: self.settings.recall_mid_line,
                // This session's own commands, or anything an earlier run
                // left behind. Not what another tab has typed since.
                can_recall: !tab.commands.is_empty() || !self.history.earlier().is_empty(),
                selected_span: line.and_then(|line| selection_in_line(tab, line)),
                insert_down,
                delete_down,
            };
            (input_ctx, tab.profile.wire_encoding())
        };
        let mut action = input::translate(&events, &input_ctx);

        if action.select_line {
            self.select_typed_line(at);
        }
        if let Some(motion) = action.extend_selection {
            self.extend_selection(at, motion);
        }
        if let Some(motion) = action.move_cursor {
            self.move_by(at, motion);
        }
        if action.collapse_selection {
            if let Some(tab) = self.pane_mut(at) {
                tab.view.clear_selection();
            }
        }
        // A selection inside the command line behaves the way one in a text
        // field does: an erase key takes it out, and so does typing or pasting
        // over it, before the new text goes in behind it.
        let replaced = input_ctx.selected_span.is_some() && !action.select_line && {
            !action.text.is_empty() || action.paste.is_some()
        };
        if action.erase_selection || replaced {
            self.erase_selection(at);
        }

        // Recorded before the Enter reaches IRIS, while the line is still on
        // screen to be read.
        if let Some(unechoed) = action.submitted.as_deref() {
            // Submitting ends the selection with the line it was on, rather
            // than leaving it highlighted in the scrollback.
            if let Some(tab) = self.pane_mut(at) {
                tab.view.clear_selection();
            }
            self.record_command(at, unechoed);
        }
        if let Some(direction) = action.recall {
            self.recall(at, direction);
        }
        // Typing abandons wherever the recall had walked to.
        if !action.text.is_empty() {
            if let Some(tab) = self.pane_mut(at) {
                tab.recall_step = None;
            }
        }

        if action.copy {
            self.copy_selection(ctx, at);
        }
        if let Some(text) = action.paste.take() {
            let text = input::sanitize_paste(&text);
            let encoded = self.plugins.on_input(&encoding.encode(&text));
            if let Some(tab) = self.pane_mut(at) {
                // A paste rewrites the line, so wherever the recall had walked
                // to is no longer where the line came from.
                tab.recall_step = None;
                tab.send(&encoded);
            }
        }
        // IRIS never reports its insert/replace state, so the keystroke is the
        // only signal there is. Display only - see `Grid::insert_mode`.
        if action.toggle_insert {
            if let Some(tab) = self.pane_mut(at) {
                tab.grid.insert_mode = !tab.grid.insert_mode;
            }
        }
        if !action.is_empty() {
            let mut wire = encoding.encode(&action.text);
            wire.extend_from_slice(&action.bytes);
            let wire = self.plugins.on_input(&wire);
            if let Some(tab) = self.pane_mut(at) {
                // Typing always returns the view to the live output.
                tab.view.scroll_to_bottom();
                tab.send(&wire);
            }
        }

        measure
    }

    /// [`App::terminal_pane`] in a box of a given size, for one half of a
    /// split.
    fn sized_pane(
        &mut self,
        ui: &mut egui::Ui,
        ctx: &Context,
        theme: &Theme,
        at: At,
        focused: bool,
        size: egui::Vec2,
    ) -> PaneMeasure {
        ui.allocate_ui_with_layout(size, egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_min_size(size);
            self.terminal_pane(ui, ctx, theme, at, focused)
        })
        .inner
    }

    fn export(&mut self, range: Range, format: export::Format) {
        let Some(tab) = self.active_tab() else {
            self.set_status(tr("No active session to export."));
            return;
        };
        let theme = self.theme();
        let contents = match format {
            export::Format::Text => export::to_text(&tab.grid, range),
            export::Format::Html => export::to_html(&tab.grid, range, &theme),
        };
        let name = export::suggested_name(&tab.profile.name, format);
        let path = self.settings.log_dir.join(name);

        match export::write_file(&path, &contents) {
            Ok(()) => self.set_status(tr1("Exported to {}", &path.display().to_string())),
            Err(e) => self.set_status(tr1("Export failed: {}", &format!("{e:#}"))),
        }
    }
}

/// The handle between two panes: a separator that can be dragged.
///
/// Returns the new ratio while it is being dragged, and `None` otherwise. A
/// double-click puts the split back down the middle, which is the way out of a
/// layout dragged somewhere useless.
///
/// `across` is the length of the divider - the height of a left/right split,
/// the width of a top/bottom one - and `usable` is the room the two panes
/// share.
fn split_divider(
    ui: &mut egui::Ui,
    dir: SplitDir,
    across: f32,
    usable: f32,
    ratio: f32,
) -> Option<f32> {
    let size = match dir {
        SplitDir::Right => egui::Vec2::new(SPLIT_DIVIDER, across),
        SplitDir::Bottom => egui::Vec2::new(across, SPLIT_DIVIDER),
    };
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click_and_drag());

    // A hairline down the middle of the hit area rather than a fill of it: the
    // grab area has to be wide enough to aim a mouse at, and a 6-point band of
    // colour between two terminals would read as a gap in the window.
    let visuals = ui.style().interact(&response);
    let active = response.hovered() || response.dragged();
    let stroke = if active {
        egui::Stroke::new(2.0_f32, visuals.fg_stroke.color)
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke
    };
    let painter = ui.painter();
    match dir {
        SplitDir::Right => {
            painter.vline(rect.center().x, rect.y_range(), stroke);
        }
        SplitDir::Bottom => {
            painter.hline(rect.x_range(), rect.center().y, stroke);
        }
    }
    if active {
        ui.ctx().set_cursor_icon(match dir {
            SplitDir::Right => egui::CursorIcon::ResizeHorizontal,
            SplitDir::Bottom => egui::CursorIcon::ResizeVertical,
        });
    }

    if response.double_clicked() {
        return Some(0.5);
    }
    if !response.dragged() {
        return None;
    }
    let delta = match dir {
        SplitDir::Right => response.drag_delta().x,
        SplitDir::Bottom => response.drag_delta().y,
    };
    (delta != 0.0).then(|| dragged_ratio(usable, ratio, delta))
}

/// Draws one session: the notes above its terminal, and the terminal.
///
/// A free function rather than a method because it needs the session and
/// nothing else of the app, which is what keeps the pane's own drawing out of
/// the way of everything [`App::terminal_pane`] then does with the result.
fn draw_pane(
    ui: &mut egui::Ui,
    tab: &mut Tab,
    theme: &Theme,
    opts: &RenderOpts,
    role: terminal_view::PaneRole,
    macros: &[MacroGroup],
) -> terminal_view::RenderResult {
    // Autologon status belongs next to the terminal it applies to.
    if let Some(note) = tab.autologon.status_note() {
        ui.label(note);
    }
    if let Some(error) = tab.error.clone() {
        ui.colored_label(theme.ansi[9], error);
    }
    if tab.ended && tab.error.is_none() {
        let mut reconnect = false;
        ui.horizontal(|ui| {
            ui.label(tr("Session ended."));
            if ui.button(tr("Reconnect")).clicked() {
                reconnect = true;
            }
        });
        if reconnect {
            tab.start();
        }
    }

    let uid = tab.uid;
    terminal_view::show(ui, &tab.grid, &mut tab.view, theme, opts, uid, role, macros)
}

/// A square button carrying one of the app's own marks.
///
/// The alternative was a letter: the new-tab button was a `+` and the tab's
/// close button a lowercase `x`, which at this size is a small letter next to a
/// slightly larger one. A painted mark can be designed against its neighbours -
/// see [`icons`] - and it cannot come out as a tofu box in a font that has no
/// glyph for it.
/// `tint` is the theme's colour for this mark, when it names one. It wins over
/// the widget colours whether or not the pointer is on the button: a theme
/// picking out the `+` means it picked it out, not "unless you hover it".
fn icon_button(
    ui: &mut egui::Ui,
    glyph: icons::Glyph,
    tint: Option<egui::Color32>,
) -> egui::Response {
    let side = ui.spacing().interact_size.y;
    let (rect, response) = ui.allocate_exact_size(egui::Vec2::splat(side), egui::Sense::click());
    let visuals = ui.style().interact(&response);
    if response.hovered() {
        ui.painter()
            .rect_filled(rect, egui::Rounding::same(3.0), visuals.bg_fill);
    }
    icons::draw(
        ui.painter(),
        rect,
        glyph,
        tint.unwrap_or(visuals.fg_stroke.color),
        visuals.bg_fill,
    );
    response
}

/// Loads the organisation file (if configured) and the personal one, and
/// reports anything that went wrong so a missing share is visible rather than
/// silently halving the macro list.
fn load_macros(settings: &Settings) -> macros::LoadReport {
    let personal = config::personal_macros_path();
    // Ships the sample on first run, and refreshes it while it is still
    // exactly as shipped, so a change to the bundled set reaches an install
    // that has never edited the file.
    macros::ensure_personal_file(&personal);
    macros::load_all(settings.org_macros(), &personal)
}

/// Inner window size that turns a terminal of `view` characters into one of
/// `target` characters.
///
/// Only the difference is worked out, so everything around the terminal - the
/// menu bar, the tab strip, the scrollbar it reserves - drops out of the sum
/// without having to be known. The half point of slack covers the view flooring
/// the space it is given: a window a rounding error short of a whole column
/// would otherwise come out one column narrower than asked for.
fn fitted_inner_size(
    inner: egui::Vec2,
    view: (usize, usize),
    target: (usize, usize),
    cell: egui::Vec2,
) -> egui::Vec2 {
    egui::Vec2::new(
        inner.x + (target.0 as f32 - view.0 as f32) * cell.x + 0.5,
        inner.y + (target.1 as f32 - view.1 as f32) * cell.y + 0.5,
    )
}

impl eframe::App for App {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        let theme = self.theme();
        let mut requests: Vec<UiRequest> = Vec::new();
        // What the terminal measured this frame, filled in once it has drawn.
        let mut fit: Option<(usize, usize, egui::Vec2)> = None;

        self.track_window_geometry(ctx);
        // Refilled as the panes draw, below, and read by the resize grips at
        // the end of the frame.
        self.pane_rects.clear();

        for tab in &mut self.tabs {
            tab.pump(&mut self.plugins);
            // A split tab holds a second session, and one that is not drained
            // would stop reading its PTY and eventually block IRIS.
            if let Some(split) = tab.split.as_mut() {
                split.tab.pump(&mut self.plugins);
            }
        }

        // Plugins may have asked for things while transforming output.
        for hook in self.plugins.take_requests() {
            match hook {
                crate::plugins::api::Hook::SendText(text) => {
                    requests.push(UiRequest::SendLines(vec![text]))
                }
                crate::plugins::api::Hook::SetStatus(text) => self.set_status(text),
                crate::plugins::api::Hook::RegisterCommand(_) => {}
            }
        }

        self.poll_updates();
        self.expire_status(ctx);
        self.handle_shortcuts(ctx);

        // A close asked for by the window manager - Alt+F4, or the taskbar -
        // arrives as a flag rather than an event, and has to be caught before
        // anything else gets a chance to draw over the question.
        if ctx.input(|i| i.viewport().close_requested()) && self.should_confirm_close() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.confirm_close = true;
        }

        let mut window_action = None;
        egui::TopBottomPanel::top("menu").show(ctx, |ui| {
            window_action = self.menu_bar(ui, &theme.window_buttons);
        });
        // Not when the setting has moved them into the title bar, where
        // `menu_bar` has already drawn them - that is what was putting the same
        // tabs on screen twice.
        if !self.settings.tabs_in_title_bar || self.tabs.is_empty() {
            egui::TopBottomPanel::top("tabs").show(ctx, |ui| self.tab_strip(ui));
        }

        if let Some(action) = window_action {
            // Close is the one action that may be refused; the rest are
            // immediate.
            if action == WindowAction::Close && self.should_confirm_close() {
                self.confirm_close = true;
            } else {
                chrome::apply(ctx, action);
            }
        }

        if let Some(status) = self.status.clone() {
            egui::TopBottomPanel::bottom("status").show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.label(status);
                    if ui.small_button(tr("dismiss")).clicked() {
                        self.status = None;
                        self.status_at = None;
                    }
                });
            });
        }

        egui::CentralPanel::default()
            .frame(
                egui::Frame::none()
                    .fill(theme.background)
                    .inner_margin(self.terminal_inset()),
            )
            .show(ctx, |ui| {
                if self.tabs.is_empty() {
                    let mut open = false;
                    ui.centered_and_justified(|ui| {
                        if ui.button(tr("Open a session (Ctrl+T)")).clicked() {
                            open = true;
                        }
                    });
                    if open {
                        self.open_new_tab();
                    }
                    return;
                }

                self.active = self.active.min(self.tabs.len() - 1);
                let active = self.active;
                let focus = self.tabs[active].focus;
                let first = At {
                    tab: active,
                    pane: Pane::First,
                };
                let second = At {
                    tab: active,
                    pane: Pane::Second,
                };

                // `geometry` is the first pane's, which is what the window is
                // fitted from and what a new session opens at; `shown` is the
                // focused pane's, which is the size worth reporting because it
                // is the one being typed in.
                let (geometry, shown) = match self.tabs[active].split.as_ref().map(|s| s.dir) {
                    None => {
                        let measure = self.terminal_pane(ui, ctx, &theme, first, true);
                        let view = measure.view;
                        (measure, view)
                    }
                    Some(dir) => {
                        let room = ui.available_size();
                        let mut top = PaneMeasure::default();
                        let mut bottom = PaneMeasure::default();
                        let ratio = self.tabs[active].split_ratio();
                        // The panes are given exact sizes, so what the divider
                        // and the spacing around it take comes off the room
                        // first: a share that did not fit would be clipped at
                        // the window edge.
                        let mut dragged = None;
                        match dir {
                            SplitDir::Right => {
                                let gap = ui.spacing().item_spacing.x * 2.0 + SPLIT_DIVIDER;
                                let usable = room.x - gap;
                                let first_width = split_extent(usable, ratio);
                                ui.horizontal(|ui| {
                                    top = self.sized_pane(
                                        ui,
                                        ctx,
                                        &theme,
                                        first,
                                        focus == Pane::First,
                                        egui::Vec2::new(first_width, room.y),
                                    );
                                    dragged = split_divider(ui, dir, room.y, usable, ratio);
                                    bottom = self.sized_pane(
                                        ui,
                                        ctx,
                                        &theme,
                                        second,
                                        focus == Pane::Second,
                                        egui::Vec2::new(usable - first_width, room.y),
                                    );
                                });
                            }
                            SplitDir::Bottom => {
                                let gap = ui.spacing().item_spacing.y * 2.0 + SPLIT_DIVIDER;
                                let usable = room.y - gap;
                                let first_height = split_extent(usable, ratio);
                                top = self.sized_pane(
                                    ui,
                                    ctx,
                                    &theme,
                                    first,
                                    focus == Pane::First,
                                    egui::Vec2::new(room.x, first_height),
                                );
                                dragged = split_divider(ui, dir, room.x, usable, ratio);
                                bottom = self.sized_pane(
                                    ui,
                                    ctx,
                                    &theme,
                                    second,
                                    focus == Pane::Second,
                                    egui::Vec2::new(room.x, usable - first_height),
                                );
                            }
                        }
                        // Written back after both panes have been drawn: the
                        // frame the drag happened in is already laid out, and
                        // the next one opens at the new ratio.
                        if let Some(ratio) = dragged {
                            if let Some(split) = self.tabs[active].split.as_mut() {
                                split.ratio = ratio;
                            }
                        }
                        // Clicking a pane is how the keyboard is moved into it,
                        // and the strip entry says which one has it - `1:` or
                        // `2:` in front of that session name.
                        if top.clicked {
                            self.tabs[active].focus = Pane::First;
                        }
                        if bottom.clicked {
                            self.tabs[active].focus = Pane::Second;
                        }
                        let shown = if focus == Pane::Second {
                            bottom.view
                        } else {
                            top.view
                        };
                        (top, shown)
                    }
                };

                // A pane sizes its own session; every session that is not on
                // screen follows the first pane, because one left at an old
                // width would keep truncating its output at that width until it
                // was next looked at.
                //
                // Width per session rather than one for all of them: a shell
                // is told the window's width and IRIS the wide grid, and a
                // background tab has to follow its own kind or it would be
                // resized to whatever the tab on screen happens to be. See
                // `wide_grid` in [`terminal_view::RenderOpts`].
                let (_, rows) = geometry.grid;
                let view_cols = geometry.view.0;
                let minimized = self.minimized;
                let wide = terminal_view::TERMINAL_COLS.max(view_cols);
                // The wide grid rather than whatever the pane on screen was
                // told: this is what a *new* session opens at, and a shell
                // being active must not leave the next IRIS tab truncating at
                // the window width.
                if !minimized {
                    self.terminal_size =
                        (wide.min(u16::MAX as usize) as u16, geometry.grid.1 as u16);
                    self.view_size = shown;
                }
                fit = Some((geometry.view.0, geometry.view.1, geometry.cell));
                let width_for = |shell: bool| if shell { view_cols } else { wide };
                for (index, tab) in self.tabs.iter_mut().enumerate() {
                    if index == active || minimized {
                        continue;
                    }
                    tab.resize(width_for(tab.profile.is_shell()), rows);
                    if let Some(split) = tab.split.as_mut() {
                        let cols = width_for(split.tab.profile.is_shell());
                        split.tab.resize(cols, rows);
                    }
                }
            });

        // The panes have finished drawing, so the tabs can move again.
        if let Some(asked) = self.pending_layout.take() {
            match asked {
                LayoutAction::Split(index, dir) => self.split_tab(index, dir),
                LayoutAction::Unsplit(index) => self.remove_split(index),
                LayoutAction::ClosePane(at) => self.close_pane(at),
            }
        }

        self.rename_tab_dialog(ctx);
        self.close_confirm_dialog(ctx);
        if panels::update_dialog(ctx, &mut self.updates) {
            self.apply_update(ctx);
        }

        // Last, and in a foreground layer: the panels and the terminal reach
        // the window edge, and the terminal senses drags of its own.
        chrome::resize_grips(ctx, "nit-main", &self.pane_rects);

        if let Some(request) = panels::pending_macro_dialog(ctx, &mut self.panels) {
            requests.push(request);
        }
        if let Some(request) = panels::pending_native_dialog(ctx, &mut self.panels) {
            requests.push(request);
        }
        if let Some(request) = panels::settings_dialog(
            ctx,
            &mut self.settings,
            &self.themes,
            &mut self.panels,
            &self.instances,
            &self.servers,
            &mut self.settings_placement,
            &theme.window_buttons,
        ) {
            requests.push(request);
        }
        // After the settings window, which is where it is opened from, and
        // before the requests are carried out, so a colour changed this frame is
        // on screen in the next one.
        let active_theme = self.settings.theme.clone();
        let theme_actions = theme_manager::theme_manager(
            ctx,
            &mut self.panels.themes,
            &mut self.themes,
            &active_theme,
            &theme.window_buttons,
        );
        // An edit to the theme in use has to show at once, which is the whole
        // point of editing it with the terminal behind the window.
        if !theme_actions.is_empty() || self.panels.themes.open {
            App::apply_style(ctx, &self.theme(), &self.settings);
        }
        for action in theme_actions {
            self.apply_theme_action(ctx, action);
        }
        // Beside the theme manager, and opened from the same section of the
        // settings window.
        let macro_actions = macro_manager::macro_manager(
            ctx,
            &mut self.panels.macros,
            &mut self.macro_groups,
            &theme.window_buttons,
        );
        for action in macro_actions {
            match action {
                macro_manager::MacroAction::Save => {
                    requests.push(UiRequest::SavePersonalMacros);
                }
                // Through the ordinary request path, so `confirm` and the
                // parameter prompt apply exactly as they do to the menu.
                macro_manager::MacroAction::Run(m) => requests.push(UiRequest::RunMacro(m)),
            }
        }

        for request in requests {
            self.handle_request(ctx, request);
        }

        if let Some((view_cols, view_rows, cell)) = fit.filter(|_| !self.minimized) {
            self.fit_window(ctx, view_cols, view_rows, cell);
        }

        // A live session can produce output at any moment, so keep animating
        // while any tab is connected. A blinking cursor needs the same, because
        // an idle prompt gives the frame no other reason to be redrawn.
        if self.settings.cursor_blink || self.tabs.iter().any(|t| t.session.is_some()) {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        } else if self.updates.working() {
            // Nothing on screen is moving, but a thread is: without a frame to
            // read it in, the check's answer and the download's progress would
            // both sit in the channel unseen. Slower than the terminal's own
            // rate, because all it has to keep up with is a number.
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        for tab in &mut self.tabs {
            close_down(tab);
            if let Some(split) = tab.split.as_mut() {
                close_down(&mut split.tab);
            }
        }
        self.persist_window_geometry();
    }
}

/// Asks one session to halt and closes its transcript. Called for every
/// session there is as the app exits, panes included.
fn close_down(tab: &mut Tab) {
    if let Some(session) = tab.session.as_ref() {
        session.request_halt();
    }
    if let Some(log) = tab.log.as_mut() {
        let _ = log.write_note("application closed");
        let _ = log.flush();
    }
}

/// The profile for whatever the launcher's tray menu is set to open.
///
/// Named for the target rather than built from it directly, because the tray's
/// "Este Servidor" entry names an instance and the other entries name servers;
/// [`crate::config::servers::preferred_target`] is what tells the two apart.
fn preferred_profile(servers: &crate::config::ServerList, instances: &[String]) -> Option<Profile> {
    use crate::config::servers::{Server, Target};
    let base = Profile::default();
    match crate::config::servers::preferred_target(servers, instances)? {
        Target::Local { instance } => Some(Profile::for_server(
            &Server::for_instance(&instance),
            instances,
            &base,
        )),
        Target::Telnet { .. } => {
            // The entry itself carries the address, the port and the name the
            // tab should show, so the server is looked up rather than rebuilt
            // from the bare target.
            let server = servers.preferred()?;
            Some(Profile::for_server(server, instances, &base))
        }
    }
}

/// Convenience for callers that want the discovered instances without an app.
pub fn discover_instances() -> Vec<String> {
    let l = launcher();
    crate::pty::launcher::instances(l.as_ref())
        .into_iter()
        .map(|i| i.name)
        .collect()
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

    /// The divider can be dragged anywhere, and neither pane may be squeezed
    /// away: a pane a couple of characters wide would have IRIS truncating
    /// every line it wrote at that margin.
    #[test]
    fn a_dragged_split_never_squeezes_a_pane_below_the_minimum() {
        let usable = 900.0_f32;
        for ratio in [-1.0, 0.0, 0.01, 0.5, 0.99, 1.0, 2.0] {
            let first = split_extent(usable, ratio);
            assert!(
                first >= MIN_PANE && usable - first >= MIN_PANE,
                "ratio {ratio} left {first} of {usable}"
            );
        }
    }

    /// A window too narrow to give both sides a minimum has no fair answer but
    /// half each, and must not come back with a negative pane.
    #[test]
    fn a_window_with_no_room_for_two_minimums_splits_evenly() {
        let usable = MIN_PANE;
        assert_eq!(split_extent(usable, 0.9), usable / 2.0);
    }

    /// Dragging left then right by the same amount comes back to where it
    /// started, which is what stops the handle lagging behind the pointer.
    #[test]
    fn dragging_the_divider_and_back_returns_to_the_same_place() {
        let usable = 800.0_f32;
        let moved = dragged_ratio(usable, 0.5, 120.0);
        let back = dragged_ratio(usable, moved, -120.0);
        assert!((back - 0.5).abs() < 0.001, "came back to {back}");
        assert!(split_extent(usable, moved) > split_extent(usable, 0.5));
    }

    /// A drag past the end stops at the minimum rather than banking movement
    /// nothing can be done with.
    #[test]
    fn a_drag_past_the_edge_stops_at_the_minimum() {
        let usable = 600.0_f32;
        let far = dragged_ratio(usable, 0.5, -10_000.0);
        assert_eq!(split_extent(usable, far), MIN_PANE);
        // And one step back off the edge moves again straight away.
        let back = dragged_ratio(usable, far, 50.0);
        assert!((split_extent(usable, back) - (MIN_PANE + 50.0)).abs() < 0.001);
    }

    /// The window is corrected from a measurement, so the check that matters is
    /// that measuring the corrected window gives the geometry that was asked
    /// for - in one step, at any font size and whatever the chrome around the
    /// terminal happens to take up.
    #[test]
    fn one_correction_lands_on_the_geometry_that_was_asked_for() {
        let target = (
            config::DEFAULT_TERMINAL_COLS as usize,
            config::DEFAULT_TERMINAL_ROWS as usize,
        );

        for cell in [
            egui::Vec2::new(8.0, 16.0),
            egui::Vec2::new(9.0, 21.0),
            egui::Vec2::new(13.0, 30.0),
        ] {
            for chrome in [egui::Vec2::new(24.0, 78.0), egui::Vec2::new(12.0, 61.0)] {
                for inner in [
                    egui::Vec2::new(1000.0, 640.0),
                    egui::Vec2::new(400.0, 240.0),
                    egui::Vec2::new(1913.0, 1027.0),
                ] {
                    // What `terminal_view::show` would measure in that window.
                    let measure = |inner: egui::Vec2| {
                        (
                            (((inner.x - chrome.x) / cell.x).floor() as usize).max(1),
                            (((inner.y - chrome.y) / cell.y).floor() as usize).max(1),
                        )
                    };

                    let fitted = fitted_inner_size(inner, measure(inner), target, cell);
                    assert_eq!(
                        measure(fitted),
                        target,
                        "cell {cell:?}, chrome {chrome:?}, from {inner:?}"
                    );
                }
            }
        }
    }

    /// A window already at the right size is left exactly where it is, so the
    /// fit cannot drift the window it was meant to leave alone.
    #[test]
    fn a_window_that_already_fits_is_not_moved() {
        let inner = egui::Vec2::new(1000.0, 640.0);
        let size = fitted_inner_size(inner, (100, 30), (100, 30), egui::Vec2::new(8.0, 16.0));
        assert!((size.x - inner.x).abs() <= 0.5 && (size.y - inner.y).abs() <= 0.5);
    }
}
