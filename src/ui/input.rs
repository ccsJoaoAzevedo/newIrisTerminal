//! Translating egui input events into the bytes a VT terminal would send.
//!
//! The one platform-dependent thing here is which modifier means "terminal
//! shortcut": Ctrl everywhere except macOS, where Cmd is idiomatic for
//! copy/paste while Ctrl must stay available for control characters.

use egui::{Event, Key, Modifiers};

use crate::term::{LineEdit, Motion};

/// True when the modifier used for app shortcuts (copy, paste, new tab) is
/// held. `Modifiers::command` is already Cmd on macOS and Ctrl elsewhere.
pub fn is_command(m: &Modifiers) -> bool {
    m.command
}

/// What the app knows about the session that the meaning of a key depends on.
#[derive(Clone, Copy, Debug, Default)]
pub struct InputContext {
    /// Decides the Ctrl+C ambiguity: with text selected it copies, otherwise it
    /// interrupts the running routine — the behaviour every terminal converges
    /// on.
    pub has_selection: bool,
    /// The line being typed, when the cursor is at an IRIS prompt. `None` means
    /// the keys that would edit a line pass through to IRIS untouched, which is
    /// what a full-screen routine such as `^%G` needs.
    pub line: Option<LineEdit>,
    /// There is a command to recall, so Up and Down at the prompt belong to the
    /// app's history rather than to IRIS's.
    pub can_recall: bool,
    /// Recall from anywhere on the line, not only from the end of it -
    /// `settings.recall_mid_line`. Off, a cursor left in the middle of a line
    /// means the line is being edited, and the arrows leave it alone.
    pub recall_mid_line: bool,
    /// Columns of the command line that are selected, when the whole selection
    /// sits inside it. That is the only selection Backspace and Delete can rub
    /// out of IRIS's read buffer; one in the scrollback is just highlighted
    /// text.
    pub selected_span: Option<(usize, usize)>,
    /// Insert is physically down, so a copy event is Ctrl+Insert. See
    /// [`chord_keys_down`].
    pub insert_down: bool,
    /// Delete is physically down, so a cut event is Shift+Delete.
    pub delete_down: bool,
    /// The session has asked for application cursor keys, so an arrow goes out
    /// as `ESC O A` rather than `ESC [ A`. See [`cursor_key`].
    pub app_cursor_keys: bool,
    /// The session is a local shell rather than an IRIS one. Not derivable
    /// from `line`: a shell never shows an IRIS prompt, so without this it is
    /// indistinguishable from a routine painting its own screen.
    pub shell: bool,
}

impl InputContext {
    /// Who is reading what this terminal sends. See [`Reader`].
    fn reader(&self) -> Reader {
        match (self.shell, self.line) {
            (true, _) => Reader::Shell,
            (false, Some(line)) => Reader::IrisPrompt(line),
            (false, None) => Reader::IrisRoutine,
        }
    }
}

/// Which way through the command history a key press asked to go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Recall {
    /// Up: towards older commands.
    Back,
    /// Down: towards the line the user started on.
    Forward,
}

/// The movement a key press asks for over the command line, and which of the
/// two things that move should do the moving.
///
/// Ctrl+Left and Ctrl+Shift+Left ask for the same [`Motion`]; the only
/// difference is whether it carries the cursor or the loose end of the
/// selection - exactly as in a text field.
fn motion_for(key: &Key, modifiers: &Modifiers) -> Option<(Motion, bool)> {
    // Spelled out field by field rather than through `matches_exact`, which
    // takes Ctrl and Cmd to be the same key: here they are not interchangeable,
    // since Ctrl is also how every control code is typed.
    let alone = !modifiers.alt && !modifiers.mac_cmd;
    let by_word = alone && modifiers.ctrl;
    let by_column = alone && !modifiers.ctrl && !modifiers.command && modifiers.shift;

    let motion = if by_word {
        match key {
            Key::ArrowLeft => Motion::WordLeft,
            Key::ArrowRight => Motion::WordRight,
            // Ctrl+Home/End are left alone: they are already the plain Home and
            // End of a single-line field, and a terminal may yet want them for
            // the scrollback.
            _ => return None,
        }
    } else if by_column {
        match key {
            Key::ArrowLeft => Motion::Left,
            Key::ArrowRight => Motion::Right,
            Key::Home => Motion::LineStart,
            Key::End => Motion::LineEnd,
            _ => return None,
        }
    } else {
        return None;
    };
    // Shift is the whole difference between carrying the cursor and carrying
    // the loose end of the selection.
    Some((motion, modifiers.shift))
}

/// Who is reading the keystrokes on the far side.
///
/// The three readers want different bytes for the same key, and guessing from
/// "is there an IRIS prompt on this row" alone is what sent a shell the wrong
/// erase byte: a shell has no IRIS prompt either, and looked like a routine.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reader {
    /// IRIS's own line editor, at a prompt, reading the line it carries.
    IrisPrompt(LineEdit),
    /// An IRIS routine painting its own screen, reading raw keystrokes
    /// through `RCar^%SMW` and matching them against the ERP's keyboard
    /// table - see [`key_bytes`].
    IrisRoutine,
    /// A local shell. It does its own line editing everywhere, and speaks the
    /// plain VT spellings a terminal is expected to send.
    Shell,
}

impl Reader {
    /// The command line, where the far side has one this terminal can read.
    fn line(self) -> Option<LineEdit> {
        match self {
            Reader::IrisPrompt(line) => Some(line),
            _ => None,
        }
    }

    /// Whether the ERP's keyboard table is what will read these bytes, rather
    /// than a terminal's own idea of what a key is spelled like.
    fn erp(self) -> bool {
        !matches!(self, Reader::Shell)
    }
}

/// Bytes for a key press, or `None` when the key is not something we send.
///
/// Most keys are spelled the same whoever is reading, because the xterm tilde
/// forms and the ERP's own keyboard table agree on them. Four do not, and
/// `reader` is what picks between them:
///
/// | key | IRIS | shell |
/// | --- | --- | --- |
/// | Backspace, off a prompt | `BS` | `DEL` |
/// | Home | `ESC [ 1 ~` | `ESC [ H` |
/// | End | `ESC [ 4 ~` | `ESC [ F` |
/// | F5 | `ESC O T` | `ESC [ 1 5 ~` |
///
/// The IRIS column is `^%SM(301,"KeyTr")`, the table `RCar^%SMW` matches
/// against for the device type the ERP's Telnet device is configured as
/// ("Cache Terminal Versao 5.2"). A sequence that is not in it is not
/// misread, it is not recognised at all - `RCar` hands back the bare `ESC`,
/// which `%CSLE` treats as "leave the field", and the rest of the sequence
/// arrives as typed text.
///
/// At an IRIS prompt Home and End are built out of arrow keys against the
/// line instead, because IRIS does the line editing there and does not act on
/// the Home/End sequences a VT terminal sends.
///
/// `app_cursor` is DECCKM: with it set the far side expects `ESC O A` where it
/// would otherwise expect `ESC [ A`, and the wrong one is not misread but
/// ignored. See [`cursor_key`].
pub fn key_bytes(
    key: Key,
    modifiers: &Modifiers,
    reader: Reader,
    app_cursor: bool,
) -> Option<Vec<u8>> {
    // Application shortcuts are handled by the caller, not sent to IRIS.
    let ctrl = modifiers.ctrl;
    let line = reader.line();

    // DECCKM is honoured everywhere a terminal's own conventions hold. Inside
    // an IRIS routine they do not: that keyboard table spells every arrow
    // `ESC [ <letter>` and has no entry for the `ESC O` form at all, which is
    // why the arrows went dead inside a routine on IRIS 2023 - the version
    // that turns the mode on.
    let app_cursor = app_cursor && reader != Reader::IrisRoutine;

    let bytes: Vec<u8> = match key {
        // IRIS expects CR for "line entered"; sending LF leaves it waiting.
        Key::Enter => vec![b'\r'],
        Key::Tab => vec![b'\t'],
        Key::Escape => vec![0x1b],
        // DEL everywhere a line editor is reading, BS inside an IRIS routine.
        // IRIS's own line editor erases on DEL, which is what terminfo's kbs
        // maps to, and so does a shell - where BS is Ctrl+Backspace and rubs
        // out the whole word. But the ERP's field editor erases on `%=8`
        // alone (`%CSLE`), and 127 falls through its printable range
        // (`%>31,%<128`) to be *inserted* as a character, so a backspace in a
        // routine typed a stray glyph instead of rubbing one out.
        Key::Backspace => match reader {
            Reader::IrisRoutine => vec![0x08],
            _ => vec![0x7f],
        },
        Key::Delete => b"\x1b[3~".to_vec(),
        Key::Insert => b"\x1b[2~".to_vec(),
        // Walked to with arrow keys when we can see where the line begins and
        // ends, and otherwise spelled the way whoever is reading expects.
        Key::Home => match line {
            Some(line) => {
                cursor_key(app_cursor, b'D').repeat(line.cursor.saturating_sub(line.start))
            }
            None if reader.erp() => b"\x1b[1~".to_vec(),
            None => b"\x1b[H".to_vec(),
        },
        Key::End => match line {
            Some(line) => cursor_key(app_cursor, b'C').repeat(line.end.saturating_sub(line.cursor)),
            None if reader.erp() => b"\x1b[4~".to_vec(),
            None => b"\x1b[F".to_vec(),
        },
        Key::PageUp => b"\x1b[5~".to_vec(),
        Key::PageDown => b"\x1b[6~".to_vec(),
        Key::ArrowUp => cursor_key(app_cursor, b'A'),
        Key::ArrowDown => cursor_key(app_cursor, b'B'),
        Key::ArrowRight => cursor_key(app_cursor, b'C'),
        Key::ArrowLeft => cursor_key(app_cursor, b'D'),
        // F1-F4 are the PF keys (`ESC O <letter>`) and F6 up are the xterm
        // tilde forms, which both worlds agree on. F5 is the one key of the
        // twelve where they differ: `^%SM(301,"KeyTr")` carries the PF-key
        // alphabet one letter further, where a terminal has already moved on
        // to the tilde forms.
        Key::F1 => b"\x1bOP".to_vec(),
        Key::F2 => b"\x1bOQ".to_vec(),
        Key::F3 => b"\x1bOR".to_vec(),
        Key::F4 => b"\x1bOS".to_vec(),
        Key::F5 if reader.erp() => b"\x1bOT".to_vec(),
        Key::F5 => b"\x1b[15~".to_vec(),
        Key::F6 => b"\x1b[17~".to_vec(),
        Key::F7 => b"\x1b[18~".to_vec(),
        Key::F8 => b"\x1b[19~".to_vec(),
        Key::F9 => b"\x1b[20~".to_vec(),
        Key::F10 => b"\x1b[21~".to_vec(),
        Key::F11 => b"\x1b[23~".to_vec(),
        Key::F12 => b"\x1b[24~".to_vec(),

        // Ctrl+letter -> 0x01..0x1a. Ctrl+C in particular must reach IRIS as
        // an interrupt; the copy shortcut only wins when there is a selection,
        // which the caller decides before consulting this function.
        k if ctrl => {
            let c = control_char(k)?;
            vec![c]
        }
        _ => return None,
    };

    Some(bytes)
}

/// One cursor-key sequence, in whichever of its two spellings the far side is
/// expecting.
///
/// `ESC [ A` is the ANSI form and `ESC O A` the application one, chosen by
/// DECCKM. A terminal that sends the wrong one is not misunderstood, it is
/// ignored - which is why the arrow keys, and clicking to put the cursor
/// somewhere (built out of the same bytes), moved nothing on IRIS 2023: it
/// turns the mode on where earlier versions left it off.
pub fn cursor_key(app_cursor: bool, final_byte: u8) -> Vec<u8> {
    vec![0x1b, if app_cursor { b'O' } else { b'[' }, final_byte]
}

/// Maps a key to its ASCII control code, covering the letters plus the handful
/// of punctuation codes (Ctrl+[, Ctrl+\, Ctrl+], Ctrl+_) that terminals use.
fn control_char(key: Key) -> Option<u8> {
    let name = key.name();
    let mut chars = name.chars();
    let first = chars.next()?;
    if chars.next().is_some() {
        // Multi-character key names are not single control codes.
        return match key {
            Key::OpenBracket => Some(0x1b),
            Key::CloseBracket => Some(0x1d),
            Key::Backslash => Some(0x1c),
            Key::Minus => Some(0x1f),
            Key::Space => Some(0x00),
            _ => None,
        };
    }
    let upper = first.to_ascii_uppercase();
    if upper.is_ascii_uppercase() {
        Some(upper as u8 - b'A' + 1)
    } else {
        None
    }
}

/// The character that closes `open`, when `open` is one a selection can be
/// wrapped in.
///
/// The set an editor treats this way: the two quotes and the three brackets.
/// Deliberately not every paired character - `<` is a comparison far more often
/// than it is a bracket, and wrapping a selection in it would be wrong nearly
/// every time it was typed.
pub fn surround_pair(open: char) -> Option<char> {
    Some(match open {
        '"' => '"',
        '\'' => '\'',
        '(' => ')',
        '[' => ']',
        '{' => '}',
        _ => return None,
    })
}

/// `line` with `from..to` wrapped in `open` and its closing character.
///
/// Columns are absolute grid columns and `start` is the column the typed text
/// begins at, because that is the shape everything on the command line is
/// measured in. Returns the new text and the span the selection should cover
/// afterwards, which is the same text one column further right: keeping it
/// selected is what lets a second quote or bracket wrap the same thing again,
/// exactly as it does in an editor.
pub fn surround(
    text: &str,
    start: usize,
    from: usize,
    to: usize,
    open: char,
) -> Option<(String, (usize, usize))> {
    let close = surround_pair(open)?;
    let chars: Vec<char> = text.chars().collect();
    let from = from.checked_sub(start)?;
    let to = to.checked_sub(start)?;
    if from >= to || to > chars.len() {
        return None;
    }

    let mut out = String::with_capacity(text.len() + 2);
    out.extend(&chars[..from]);
    out.push(open);
    out.extend(&chars[from..to]);
    out.push(close);
    out.extend(&chars[to..]);
    Some((out, (start + from + 1, start + to + 1)))
}

/// Whether Insert and Delete are physically down right now.
///
/// On Windows, egui-winit treats Ctrl+Insert as copy and Shift+Delete as cut
/// and hands them over as the very same `Event::Copy` / `Event::Cut` that
/// Ctrl+C and Ctrl+X produce, having swallowed the key event — so the events
/// alone cannot tell the two chords apart, and Ctrl+Insert was reaching IRIS as
/// an interrupt (`<INTERRUPT>`) on a session with nothing selected. Asking the
/// OS which key is down is the only thing left that distinguishes them.
///
/// Elsewhere those chords are not clipboard chords at all, so there is nothing
/// to disambiguate and this reports neither key.
pub fn chord_keys_down() -> (bool, bool) {
    #[cfg(target_os = "windows")]
    {
        // user32 is already linked by the windowing stack; naming it here keeps
        // that from being an accident.
        #[link(name = "user32")]
        extern "system" {
            fn GetAsyncKeyState(key: i32) -> i16;
        }
        const VK_INSERT: i32 = 0x2D;
        const VK_DELETE: i32 = 0x2E;
        // The high bit is "down now". A key is held for far longer than the
        // frame it is reported in, so by the time the event is processed it is
        // still down.
        let down = |key: i32| unsafe { GetAsyncKeyState(key) as u16 & 0x8000 != 0 };
        (down(VK_INSERT), down(VK_DELETE))
    }
    #[cfg(not(target_os = "windows"))]
    {
        (false, false)
    }
}

/// Turns a frame's worth of egui events into bytes for the PTY.
pub struct InputAction {
    /// Key sequences that are ASCII/control bytes by definition — arrows,
    /// Enter, Ctrl-codes. These go to the PTY verbatim.
    pub bytes: Vec<u8>,
    /// Characters the user typed. Kept as text, not bytes, because they must
    /// be encoded into the instance's codepage before being sent.
    pub text: String,
    pub copy: bool,
    pub paste: Option<String>,
    /// The Insert key was pressed. The bytes still go to IRIS; this only lets
    /// the caller keep its own idea of the mode in step, because IRIS never
    /// reports it back.
    pub toggle_insert: bool,
    /// Enter was pressed, so whatever is on the prompt line has just been
    /// submitted and is worth remembering. Carries the text typed earlier in
    /// this same frame, which IRIS has not echoed yet and which the screen
    /// therefore does not show: without it, typing the last character of a
    /// command and Enter close enough together to land in one frame would
    /// record the command a character short.
    pub submitted: Option<String>,
    /// Up or Down was pressed on a command line, and the app owns the recall.
    pub recall: Option<Recall>,
    /// Ctrl+A was pressed at a prompt: select the command being typed.
    pub select_line: bool,
    /// Shift plus a movement key: drag the loose end of the selection there.
    pub extend_selection: Option<Motion>,
    /// Ctrl plus an arrow: walk IRIS's cursor there, a word at a time. The
    /// single-column and whole-line movements need no help - they are the keys
    /// IRIS already acts on, and [`key_bytes`] turns them into those.
    pub move_cursor: Option<Motion>,
    /// A movement key was pressed without Shift while something was selected,
    /// which drops the selection the way it does in any text field.
    pub collapse_selection: bool,
    /// Backspace or Delete was pressed with part of the command line selected,
    /// so what is selected should go rather than the character at the cursor.
    pub erase_selection: bool,
}

impl InputAction {
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty() && self.text.is_empty()
    }
}

pub fn translate(events: &[Event], ctx: &InputContext) -> InputAction {
    let mut action = InputAction {
        bytes: Vec::new(),
        text: String::new(),
        copy: false,
        paste: None,
        toggle_insert: false,
        submitted: None,
        recall: None,
        select_line: false,
        extend_selection: None,
        move_cursor: None,
        collapse_selection: false,
        erase_selection: false,
    };

    // Anywhere on a command line when the setting allows it: the native IRIS
    // terminal swaps the line whatever column the cursor sits in, and the
    // caller walks the cursor to the end before rubbing the line out - see
    // `App::recall`. Otherwise only from the end, where a rubout erases the
    // last character typed rather than one in the middle.
    let recall_here = ctx.can_recall
        && ctx
            .line
            .is_some_and(|line| ctx.recall_mid_line || line.at_end());

    for event in events {
        match event {
            Event::Text(text) => {
                // egui already filters out text produced while a command
                // modifier is held, so this is genuine typed input.
                action.text.push_str(text);
            }
            // Text an input method composed and committed. It arrives here
            // rather than as `Event::Text` because the composition happens
            // outside the app - a CJK method, the emoji panel, the touch
            // keyboard - so a terminal that ignored it could not be typed in by
            // any of them. The composition itself (`Preedit`) is not shown:
            // IRIS owns the line being edited and there is nowhere to put a
            // half-finished word, so it stays in the system's own candidate
            // window until it is committed.
            Event::Ime(egui::ImeEvent::Commit(text)) => {
                action.text.push_str(text);
            }
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                // Up and Down over a command line are the app's history, and
                // nothing else reaches IRIS from them - not the bare arrow, and
                // not any chord built on it. Forwarding one would reach IRIS's
                // own recall, which is a list of every line that went through
                // the prompt, including the ones a macro or an IRIS helper
                // sent: lines nobody typed and nobody wants offered back.
                //
                // Where there is no command line at all - a full-screen routine
                // such as `^%G` - the arrows still belong to the far side and
                // are left alone.
                if matches!(key, Key::ArrowUp | Key::ArrowDown) && ctx.line.is_some() {
                    if recall_here && modifiers.is_none() {
                        action.recall = Some(if *key == Key::ArrowUp {
                            Recall::Back
                        } else {
                            Recall::Forward
                        });
                    }
                    continue;
                }
                // Ctrl+A selects the command being typed. Only where there is
                // one and it is not empty: elsewhere it stays the control code
                // IRIS would otherwise receive.
                if *key == Key::A
                    && modifiers.matches_exact(Modifiers::CTRL)
                    && ctx.line.is_some_and(|line| !line.is_empty())
                {
                    action.select_line = true;
                    continue;
                }
                // Shift drags the loose end of the selection over the command
                // line and Ctrl walks the cursor a word at a time, the way both
                // do in a text field. Extending sends nothing at all: IRIS's
                // cursor stays where it is and only the highlight moves, so a
                // selection can be built up without disturbing the line.
                if ctx.line.is_some() {
                    if let Some((motion, extends)) = motion_for(key, modifiers) {
                        if extends {
                            action.extend_selection = Some(motion);
                        } else {
                            action.move_cursor = Some(motion);
                        }
                        continue;
                    }
                }
                // A movement without Shift drops the selection, again as a text
                // field would: leaving it highlighted while the cursor walks
                // away from it would make the next Backspace erase something
                // the user is no longer looking at.
                if ctx.selected_span.is_some()
                    && modifiers.is_none()
                    && matches!(key, Key::ArrowLeft | Key::ArrowRight | Key::Home | Key::End)
                {
                    action.collapse_selection = true;
                }
                // With part of the command line selected, the erase keys act on
                // the selection - which is what makes a selection worth having
                // beyond copying it.
                if matches!(key, Key::Backspace | Key::Delete)
                    && modifiers.is_none()
                    && ctx.selected_span.is_some()
                {
                    action.erase_selection = true;
                    continue;
                }
                if *key == Key::Insert {
                    action.toggle_insert = true;
                }
                if *key == Key::Enter {
                    action.submitted = Some(action.text.clone());
                }
                if let Some(bytes) = key_bytes(*key, modifiers, ctx.reader(), ctx.app_cursor_keys) {
                    action.bytes.extend_from_slice(&bytes);
                }
            }

            // egui-winit intercepts the clipboard chords *before* building a
            // key event and returns early, so Ctrl+C/Ctrl+X never arrive as
            // `Event::Key`. They must be handled here or the control codes are
            // lost entirely — which is why Ctrl+C could not interrupt IRIS.
            Event::Copy => {
                if ctx.has_selection {
                    action.copy = true;
                } else if !ctx.insert_down {
                    // No selection: Ctrl+C is an interrupt, not a copy. Unless
                    // the chord was Ctrl+Insert, which is only ever a copy —
                    // see `chord_keys_down`.
                    action.bytes.push(0x03);
                }
            }
            Event::Cut => {
                if ctx.has_selection {
                    action.copy = true;
                } else if !ctx.delete_down {
                    action.bytes.push(0x18);
                }
            }
            Event::Paste(text) => action.paste = Some(text.clone()),
            _ => {}
        }
    }

    action
}

/// Prepares clipboard text for sending. Newlines become CR because that is
/// what IRIS treats as end-of-line, and a stray LF would submit twice.
pub fn sanitize_paste(text: &str) -> String {
    text.replace("\r\n", "\r").replace('\n', "\r")
}

#[cfg(test)]
mod surround_tests {
    use super::*;

    /// The gesture as asked for: a global name picked off the screen, quoted in
    /// one keystroke rather than retyped.
    #[test]
    fn a_quote_wraps_the_selection_instead_of_replacing_it() {
        // `set x=^GLOBAL` typed at column 8, with `^GLOBAL` selected.
        let text = "set x=^GLOBAL";
        let (line, span) = surround(text, 8, 14, 21, '"').expect("should wrap");
        assert_eq!(line, "set x=\"^GLOBAL\"");
        assert_eq!(
            span,
            (15, 22),
            "the same characters, one column right of where they were"
        );
    }

    /// And wrapping what is already wrapped, which is what keeping the
    /// selection is for.
    #[test]
    fn wrapping_twice_nests_the_pairs() {
        let (once, span) = surround("a", 0, 0, 1, '(').expect("should wrap");
        assert_eq!(once, "(a)");
        let (twice, _) = surround(&once, 0, span.0, span.1, '[').expect("should wrap");
        assert_eq!(twice, "([a])");
    }

    #[test]
    fn every_pair_closes_with_its_own_character() {
        assert_eq!(surround_pair('('), Some(')'));
        assert_eq!(surround_pair('['), Some(']'));
        assert_eq!(surround_pair('{'), Some('}'));
        assert_eq!(surround_pair('"'), Some('"'));
        assert_eq!(surround_pair('\''), Some('\''));
    }

    /// Anything else typed over a selection is a replacement, which is the
    /// behaviour every text field has and the one this must not change.
    #[test]
    fn an_ordinary_character_is_not_a_wrapping_one() {
        assert_eq!(surround_pair('x'), None);
        assert_eq!(surround_pair('<'), None, "a comparison far more often");
        assert_eq!(surround_pair(')'), None, "a closing bracket wraps nothing");
        assert!(surround("abc", 0, 0, 2, 'x').is_none());
    }

    /// A selection that is empty, inverted, or reaches past the text is not
    /// something to wrap - and must not panic on the way to saying so.
    #[test]
    fn a_span_that_is_not_a_selection_wraps_nothing() {
        assert!(surround("abc", 0, 1, 1, '(').is_none(), "empty");
        assert!(surround("abc", 0, 2, 1, '(').is_none(), "inverted");
        assert!(surround("abc", 0, 0, 9, '(').is_none(), "past the end");
        assert!(surround("abc", 8, 2, 5, '(').is_none(), "before the prompt");
    }

    /// Columns are characters, not bytes: the accented text the ERP is full of
    /// must not be cut in half.
    #[test]
    fn columns_are_counted_in_characters() {
        let (line, span) = surround("ação", 0, 1, 3, '(').expect("should wrap");
        assert_eq!(line, "a(çã)o");
        assert_eq!(span, (2, 4));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `USER>write 1` with the cursor at the end.
    fn line() -> LineEdit {
        LineEdit {
            start: 5,
            cursor: 12,
            end: 12,
        }
    }

    fn ctx() -> InputContext {
        InputContext::default()
    }

    fn press(key: Key) -> Event {
        press_with(key, Modifiers::NONE)
    }

    fn press_with(key: Key, modifiers: Modifiers) -> Event {
        Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    /// Text an input method composed elsewhere and committed has to reach IRIS
    /// like anything else typed. It arrives as its own event, not as
    /// `Event::Text`, so a terminal that only read the latter could not be
    /// typed in by a CJK method, the emoji panel or the touch keyboard at all.
    #[test]
    fn committed_input_method_text_is_typed() {
        let events = [Event::Ime(egui::ImeEvent::Commit("日本".into()))];
        let action = translate(&events, &ctx());
        assert_eq!(action.text, "日本");
    }

    /// The composition on its way to being committed is not: IRIS owns the line
    /// being edited, so a half-finished word has nowhere to go and belongs in
    /// the system's own candidate window until it is done.
    #[test]
    fn an_unfinished_composition_is_not_typed() {
        for event in [
            Event::Ime(egui::ImeEvent::Enabled),
            Event::Ime(egui::ImeEvent::Preedit("に".into())),
            Event::Ime(egui::ImeEvent::Disabled),
        ] {
            let action = translate(&[event], &ctx());
            assert!(
                action.is_empty() && action.text.is_empty(),
                "an unfinished composition reached IRIS"
            );
        }
    }

    #[test]
    fn enter_sends_carriage_return_not_line_feed() {
        assert_eq!(
            key_bytes(Key::Enter, &Modifiers::NONE, Reader::IrisRoutine, false),
            Some(vec![b'\r'])
        );
    }

    #[test]
    fn backspace_sends_del_at_a_prompt() {
        assert_eq!(
            key_bytes(
                Key::Backspace,
                &Modifiers::NONE,
                Reader::IrisPrompt(line()),
                false
            ),
            Some(vec![0x7f])
        );
    }

    #[test]
    fn arrows_send_csi_sequences() {
        assert_eq!(
            key_bytes(Key::ArrowUp, &Modifiers::NONE, Reader::IrisRoutine, false),
            Some(b"\x1b[A".to_vec())
        );
    }

    #[test]
    fn ctrl_letters_map_to_control_codes() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        };
        assert_eq!(
            key_bytes(Key::C, &ctrl, Reader::IrisRoutine, false),
            Some(vec![0x03])
        );
        assert_eq!(
            key_bytes(Key::A, &ctrl, Reader::IrisRoutine, false),
            Some(vec![0x01])
        );
        assert_eq!(
            key_bytes(Key::Z, &ctrl, Reader::IrisRoutine, false),
            Some(vec![0x1a])
        );
    }

    /// IRIS does not act on the Home/End sequences, so they are walked with the
    /// arrow keys it does act on.
    #[test]
    fn home_walks_left_to_the_start_of_the_typed_line() {
        assert_eq!(
            key_bytes(
                Key::Home,
                &Modifiers::NONE,
                Reader::IrisPrompt(line()),
                false
            ),
            Some(b"\x1b[D".repeat(7))
        );
    }

    #[test]
    fn end_walks_right_to_the_end_of_the_typed_line() {
        let mid = LineEdit {
            cursor: 8,
            ..line()
        };
        assert_eq!(
            key_bytes(Key::End, &Modifiers::NONE, Reader::IrisPrompt(mid), false),
            Some(b"\x1b[C".repeat(4))
        );
    }

    /// Already there: nothing to send, and nothing that could disturb the line.
    #[test]
    fn home_and_end_send_nothing_when_the_cursor_is_already_there() {
        let start = LineEdit {
            cursor: 5,
            ..line()
        };
        assert_eq!(
            key_bytes(
                Key::Home,
                &Modifiers::NONE,
                Reader::IrisPrompt(start),
                false
            ),
            Some(Vec::new())
        );
        assert_eq!(
            key_bytes(
                Key::End,
                &Modifiers::NONE,
                Reader::IrisPrompt(line()),
                false
            ),
            Some(Vec::new())
        );
    }

    /// Off a command line they go out in the tilde spelling
    /// `^%SM(301,"KeyTr")` lists, not the `ESC [ H` / `ESC [ F` a VT terminal
    /// sends: that table has no entry for those, so they reach a routine's
    /// `RCar` read as nothing at all.
    #[test]
    fn home_and_end_use_the_tilde_spelling_off_a_command_line() {
        assert_eq!(
            key_bytes(Key::Home, &Modifiers::NONE, Reader::IrisRoutine, false),
            Some(b"\x1b[1~".to_vec())
        );
        assert_eq!(
            key_bytes(Key::End, &Modifiers::NONE, Reader::IrisRoutine, false),
            Some(b"\x1b[4~".to_vec())
        );
    }

    /// IRIS never reports its insert/replace state, so the keystroke has to be
    /// reported alongside the bytes rather than instead of them.
    #[test]
    fn the_insert_key_is_reported_and_still_sent() {
        let action = translate(&[press(Key::Insert)], &ctx());
        assert!(action.toggle_insert, "the key press was not reported");
        assert_eq!(
            action.bytes,
            b"\x1b[2~".to_vec(),
            "the key must still reach IRIS"
        );
    }

    #[test]
    fn an_ordinary_key_does_not_touch_the_insert_state() {
        assert!(!translate(&[press(Key::Home)], &ctx()).toggle_insert);
    }

    /// The function keys as `^%SM(301,"KeyTr")` spells them: PF keys through
    /// F5, tilde forms from F6 on. F5 is the one a generic terminal gets
    /// wrong, so it is the one worth pinning.
    #[test]
    fn the_function_keys_break_from_pf_to_tilde_form_after_f5() {
        assert_eq!(
            key_bytes(Key::F5, &Modifiers::NONE, Reader::IrisRoutine, false),
            Some(b"\x1bOT".to_vec())
        );
        assert_eq!(
            key_bytes(Key::F6, &Modifiers::NONE, Reader::IrisRoutine, false),
            Some(b"\x1b[17~".to_vec())
        );
        assert_eq!(
            key_bytes(Key::F12, &Modifiers::NONE, Reader::IrisRoutine, false),
            Some(b"\x1b[24~".to_vec())
        );
    }

    /// A shell has no IRIS prompt either, so before [`Reader`] existed it was
    /// indistinguishable from a routine and got the routine's bytes. BS is
    /// Ctrl+Backspace to a Windows shell, so every backspace rubbed out the
    /// whole word; Home, End and F5 were the ERP's spellings, which a shell
    /// does not recognise.
    #[test]
    fn a_shell_gets_the_plain_vt_spellings_a_terminal_is_expected_to_send() {
        let shell = |key| key_bytes(key, &Modifiers::NONE, Reader::Shell, false);
        assert_eq!(shell(Key::Backspace), Some(vec![0x7f]));
        assert_eq!(shell(Key::Home), Some(b"[H".to_vec()));
        assert_eq!(shell(Key::End), Some(b"[F".to_vec()));
        assert_eq!(shell(Key::F5), Some(b"[15~".to_vec()));
    }

    /// The tilde forms the two worlds agree on stay the same either way, so a
    /// reader that only ever saw one of them still works.
    #[test]
    fn the_keys_both_worlds_spell_alike_do_not_depend_on_the_reader() {
        for key in [
            Key::Delete,
            Key::Insert,
            Key::PageUp,
            Key::PageDown,
            Key::F6,
            Key::F12,
        ] {
            assert_eq!(
                key_bytes(key, &Modifiers::NONE, Reader::Shell, false),
                key_bytes(key, &Modifiers::NONE, Reader::IrisRoutine, false),
                "{key:?} should not differ between readers"
            );
        }
    }

    /// DECCKM is a terminal convention, so a shell keeps it: only the ERP's
    /// keyboard table, which has no `ESC O` arrow in it, has to be spared.
    #[test]
    fn a_shell_still_honours_application_cursor_mode() {
        assert_eq!(
            key_bytes(Key::ArrowUp, &Modifiers::NONE, Reader::Shell, true),
            Some(b"OA".to_vec())
        );
    }

    /// The ERP's own field editor erases on BS and treats DEL as a printable
    /// character, so a backspace off a command line has to be BS - while at a
    /// prompt, where IRIS reads the line itself, it stays DEL.
    #[test]
    fn backspace_is_del_at_a_prompt_and_bs_inside_a_routine() {
        assert_eq!(
            key_bytes(
                Key::Backspace,
                &Modifiers::NONE,
                Reader::IrisPrompt(line()),
                false
            ),
            Some(vec![0x7f])
        );
        assert_eq!(
            key_bytes(Key::Backspace, &Modifiers::NONE, Reader::IrisRoutine, false),
            Some(vec![0x08])
        );
    }

    /// DECCKM only reaches the far side where IRIS is reading the line. A
    /// routine's `RCar` matches against a table that has no `ESC O` arrow in
    /// it, so off a prompt the ANSI spelling goes out whatever the mode says.
    #[test]
    fn an_arrow_ignores_application_cursor_mode_off_a_command_line() {
        assert_eq!(
            key_bytes(
                Key::ArrowUp,
                &Modifiers::NONE,
                Reader::IrisPrompt(line()),
                true
            ),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(
            key_bytes(Key::ArrowUp, &Modifiers::NONE, Reader::IrisRoutine, true),
            Some(b"\x1b[A".to_vec())
        );
    }

    #[test]
    fn enter_reports_that_the_line_was_submitted() {
        let action = translate(&[press(Key::Enter)], &ctx());
        assert_eq!(action.submitted.as_deref(), Some(""));
        assert_eq!(action.bytes, vec![b'\r']);
        assert!(translate(&[press(Key::Home)], &ctx()).submitted.is_none());
    }

    /// A character typed in the same frame as the Enter has not been echoed, so
    /// the screen does not show it and it has to travel with the submission.
    #[test]
    fn a_character_typed_alongside_the_enter_is_reported_with_it() {
        let events = [Event::Text("1".into()), press(Key::Enter)];
        assert_eq!(
            translate(&events, &ctx()).submitted.as_deref(),
            Some("1"),
            "the unechoed tail of the command was lost"
        );
    }

    /// The arrows over a command line never reach IRIS: its own recall lists
    /// every line that went through the prompt, macros and IRIS helpers
    /// included, and that list is not the user's command history.
    #[test]
    fn the_arrows_at_a_prompt_are_never_forwarded() {
        let at_prompt = InputContext {
            line: Some(line()),
            can_recall: true,
            ..InputContext::default()
        };
        for ctx in [
            at_prompt,
            // Nothing to recall, and nothing to forward either: an empty
            // history must not fall through to IRIS's.
            InputContext {
                can_recall: false,
                ..at_prompt
            },
        ] {
            for key in [Key::ArrowUp, Key::ArrowDown] {
                for modifiers in [Modifiers::NONE, Modifiers::CTRL] {
                    let action = translate(&[press_with(key, modifiers)], &ctx);
                    assert!(
                        action.bytes.is_empty(),
                        "{key:?} with {modifiers:?} reached IRIS"
                    );
                }
            }
        }
    }

    /// The bare arrows are the recall, whatever else is going on: no modifier
    /// turns it on, and none turns it off.
    #[test]
    fn the_bare_arrows_are_the_recall() {
        let at_prompt = InputContext {
            line: Some(line()),
            can_recall: true,
            ..InputContext::default()
        };

        assert_eq!(
            translate(&[press(Key::ArrowUp)], &at_prompt).recall,
            Some(Recall::Back)
        );
        assert_eq!(
            translate(&[press(Key::ArrowDown)], &at_prompt).recall,
            Some(Recall::Forward)
        );
        // A chord is not a second way in - and it still must not reach IRIS,
        // whose own recall is the list this keeps out of the way.
        for modifiers in [Modifiers::CTRL, Modifiers::SHIFT] {
            let action = translate(&[press_with(Key::ArrowUp, modifiers)], &at_prompt);
            assert_eq!(action.recall, None, "{modifiers:?} recalled");
            assert!(action.bytes.is_empty(), "{modifiers:?} reached IRIS");
        }
    }

    /// Away from a prompt the arrows are the far side's: a full-screen routine
    /// reads them itself.
    #[test]
    fn the_arrows_still_reach_a_full_screen_routine() {
        let no_line = InputContext::default();
        let action = translate(&[press(Key::ArrowUp)], &no_line);
        assert_eq!(action.bytes, b"\x1b[A".to_vec());
        assert_eq!(action.recall, None);
    }

    /// Ctrl+A selects what has been typed instead of reaching IRIS as SOH.
    #[test]
    fn ctrl_a_selects_the_command_being_typed() {
        let at_prompt = InputContext {
            line: Some(line()),
            ..InputContext::default()
        };
        let action = translate(&[press_with(Key::A, Modifiers::CTRL)], &at_prompt);
        assert!(action.select_line);
        assert!(action.bytes.is_empty(), "IRIS must not also receive SOH");
    }

    /// With nothing typed, or nowhere to type, it is the control code again.
    #[test]
    fn ctrl_a_stays_a_control_code_when_there_is_no_command_to_select() {
        let empty_line = InputContext {
            line: Some(LineEdit {
                start: 5,
                cursor: 5,
                end: 5,
            }),
            ..InputContext::default()
        };
        for ctx in [empty_line, InputContext::default()] {
            let action = translate(&[press_with(Key::A, Modifiers::CTRL)], &ctx);
            assert!(!action.select_line);
            assert_eq!(action.bytes, vec![0x01]);
        }
    }

    /// Shift plus a movement key builds a selection without touching the line.
    #[test]
    fn shift_and_a_movement_key_extend_the_selection() {
        let at_prompt = InputContext {
            line: Some(line()),
            ..InputContext::default()
        };
        for (key, expected) in [
            (Key::ArrowLeft, Motion::Left),
            (Key::ArrowRight, Motion::Right),
            (Key::Home, Motion::LineStart),
            (Key::End, Motion::LineEnd),
        ] {
            let action = translate(&[press_with(key, Modifiers::SHIFT)], &at_prompt);
            assert_eq!(action.extend_selection, Some(expected), "{key:?}");
            assert!(
                action.bytes.is_empty(),
                "{key:?} moved IRIS's cursor as well as the selection"
            );
        }
    }

    /// Ctrl walks the cursor a word at a time; Ctrl+Shift takes the selection
    /// with it. Both are the app's to carry out, so neither reaches IRIS as a
    /// key of its own.
    #[test]
    fn ctrl_and_an_arrow_move_by_word_and_ctrl_shift_selects_by_word() {
        let at_prompt = InputContext {
            line: Some(line()),
            ..InputContext::default()
        };
        let ctrl_shift = Modifiers {
            shift: true,
            ..Modifiers::CTRL
        };

        for (key, expected) in [
            (Key::ArrowLeft, Motion::WordLeft),
            (Key::ArrowRight, Motion::WordRight),
        ] {
            let moved = translate(&[press_with(key, Modifiers::CTRL)], &at_prompt);
            assert_eq!(moved.move_cursor, Some(expected), "Ctrl+{key:?}");
            assert_eq!(moved.extend_selection, None);
            assert!(moved.bytes.is_empty(), "Ctrl+{key:?} reached IRIS as a key");

            let selected = translate(&[press_with(key, ctrl_shift)], &at_prompt);
            assert_eq!(
                selected.extend_selection,
                Some(expected),
                "Ctrl+Shift+{key:?}"
            );
            assert_eq!(selected.move_cursor, None);
            assert!(selected.bytes.is_empty());
        }
    }

    /// Word motion is over the line being typed, so off a prompt the keys are
    /// IRIS's again.
    #[test]
    fn ctrl_and_an_arrow_reach_iris_away_from_a_prompt() {
        let action = translate(&[press_with(Key::ArrowLeft, Modifiers::CTRL)], &ctx());
        assert_eq!(action.move_cursor, None);
        assert_eq!(action.bytes, b"\x1b[D".to_vec());
    }

    /// Off a command line there is nothing to select, so the keys stay IRIS's.
    #[test]
    fn shift_and_a_movement_key_reach_iris_away_from_a_prompt() {
        let action = translate(&[press_with(Key::ArrowLeft, Modifiers::SHIFT)], &ctx());
        assert_eq!(action.extend_selection, None);
        assert_eq!(action.bytes, b"\x1b[D".to_vec());
    }

    /// Without Shift the same keys drop the selection and move the cursor, or
    /// the next Backspace would erase something no longer highlighted.
    #[test]
    fn a_movement_key_without_shift_collapses_the_selection() {
        let selected = InputContext {
            line: Some(line()),
            selected_span: Some((5, 12)),
            ..InputContext::default()
        };
        let action = translate(&[press(Key::ArrowLeft)], &selected);
        assert!(action.collapse_selection);
        assert_eq!(
            action.bytes,
            b"\x1b[D".to_vec(),
            "the cursor must still move"
        );

        // Home walks to the start of the line, and drops the selection with it.
        let action = translate(&[press(Key::Home)], &selected);
        assert!(action.collapse_selection);
        assert_eq!(action.bytes, b"\x1b[D".repeat(7));

        assert!(!translate(&[press(Key::ArrowLeft)], &ctx()).collapse_selection);
    }

    /// The point of selecting the line: the erase keys then take the selection
    /// out of IRIS's read buffer instead of nibbling at the cursor.
    #[test]
    fn backspace_and_delete_erase_a_selection_inside_the_command_line() {
        let selected = InputContext {
            line: Some(line()),
            selected_span: Some((5, 12)),
            ..InputContext::default()
        };
        for key in [Key::Backspace, Key::Delete] {
            let action = translate(&[press(key)], &selected);
            assert!(
                action.erase_selection,
                "{key:?} did not erase the selection"
            );
            assert!(
                action.bytes.is_empty(),
                "{key:?} must not also send its own key"
            );
        }
    }

    /// A selection in the scrollback is highlighted text, not something IRIS
    /// can be asked to erase.
    #[test]
    fn the_erase_keys_are_unchanged_without_a_selection_on_the_command_line() {
        let at_prompt = InputContext {
            line: Some(line()),
            ..InputContext::default()
        };
        let action = translate(&[press(Key::Backspace)], &at_prompt);
        assert!(!action.erase_selection);
        assert_eq!(action.bytes, vec![0x7f]);
    }

    /// At the end of a command line, with something to recall, Up is the app's.
    #[test]
    fn up_and_down_become_a_recall_at_the_prompt() {
        let ctx = InputContext {
            line: Some(line()),
            can_recall: true,
            ..InputContext::default()
        };
        let up = translate(&[press(Key::ArrowUp)], &ctx);
        assert_eq!(up.recall, Some(Recall::Back));
        assert!(up.bytes.is_empty(), "IRIS must not also be asked to recall");

        let down = translate(&[press(Key::ArrowDown)], &ctx);
        assert_eq!(down.recall, Some(Recall::Forward));
        assert!(down.bytes.is_empty());
    }

    /// With nothing to recall the arrow does nothing at all - it used to fall
    /// through to IRIS, whose own recall is the list this feature exists to
    /// keep out of the way. Away from a prompt it is still IRIS's key.
    #[test]
    fn up_does_nothing_when_there_is_nothing_for_the_app_to_recall() {
        let no_history = InputContext {
            line: Some(line()),
            ..InputContext::default()
        };
        let action = translate(&[press(Key::ArrowUp)], &no_history);
        assert_eq!(action.recall, None);
        assert!(action.bytes.is_empty());

        let no_prompt = InputContext {
            can_recall: true,
            ..InputContext::default()
        };
        let action = translate(&[press(Key::ArrowUp)], &no_prompt);
        assert_eq!(action.recall, None);
        assert_eq!(action.bytes, b"\x1b[A".to_vec());
    }

    /// Mid-line, when the setting allows it: the native IRIS terminal replaces
    /// the whole line wherever the cursor happens to be, and so does this - the
    /// caller walks the cursor to the end before rubbing the line out. With the
    /// setting off the line is left alone. Nothing is forwarded to IRIS either
    /// way, since its own recall is the list this feature exists to avoid.
    #[test]
    fn recall_mid_line_is_what_decides_whether_up_takes_the_line() {
        let mid_line = InputContext {
            line: Some(LineEdit {
                cursor: 8,
                ..line()
            }),
            can_recall: true,
            recall_mid_line: true,
            ..InputContext::default()
        };
        let action = translate(&[press(Key::ArrowUp)], &mid_line);
        assert_eq!(action.recall, Some(Recall::Back));
        assert!(action.bytes.is_empty());

        let action = translate(
            &[press(Key::ArrowUp)],
            &InputContext {
                recall_mid_line: false,
                ..mid_line
            },
        );
        assert_eq!(action.recall, None);
        assert!(action.bytes.is_empty(), "IRIS must not recall either");

        // The end of the line is the recall whichever way the setting is set.
        let action = translate(
            &[press(Key::ArrowUp)],
            &InputContext {
                line: Some(line()),
                recall_mid_line: false,
                ..mid_line
            },
        );
        assert_eq!(action.recall, Some(Recall::Back));
    }

    /// With the mode on, the arrows change shape: `ESC O D` instead of
    /// `ESC [ D`. IRIS 2023 asks for it, and an arrow in the wrong spelling
    /// moves its cursor nowhere at all.
    #[test]
    fn application_cursor_keys_change_the_spelling_of_an_arrow() {
        let app = InputContext {
            app_cursor_keys: true,
            // Only at a prompt does the mode mean anything - see `key_bytes`.
            line: Some(line()),
            ..InputContext::default()
        };
        assert_eq!(
            translate(&[press(Key::ArrowLeft)], &app).bytes,
            cursor_key(true, b'D')
        );
        assert_eq!(cursor_key(true, b'D'), b"\x1bOD".to_vec());
        assert_eq!(cursor_key(false, b'D'), b"\x1b[D".to_vec());

        // Home is walked with arrows, so it follows the same spelling.
        let at_prompt = InputContext {
            line: Some(line()),
            app_cursor_keys: true,
            ..InputContext::default()
        };
        assert_eq!(
            translate(&[press(Key::Home)], &at_prompt).bytes,
            b"\x1bOD".repeat(7)
        );
    }

    #[test]
    fn paste_normalises_every_newline_flavour_to_cr() {
        assert_eq!(sanitize_paste("a\r\nb\nc"), "a\rb\rc");
    }

    /// egui-winit turns Ctrl+C into `Event::Copy` and never emits a key event
    /// for it, so this is the only path that can produce an interrupt.
    #[test]
    fn ctrl_c_copies_when_text_is_selected() {
        let selected = InputContext {
            has_selection: true,
            ..InputContext::default()
        };
        let action = translate(&[Event::Copy], &selected);
        assert!(action.copy);
        assert!(action.is_empty(), "must not also send bytes to IRIS");
    }

    #[test]
    fn ctrl_c_interrupts_when_nothing_is_selected() {
        let action = translate(&[Event::Copy], &ctx());
        assert!(!action.copy);
        assert_eq!(action.bytes, vec![0x03], "Ctrl+C must reach IRIS as ETX");
    }

    /// Windows hands Ctrl+Insert over as the same event as Ctrl+C. It is only
    /// ever a copy, and must never interrupt the session.
    #[test]
    fn ctrl_insert_never_interrupts() {
        let insert = InputContext {
            insert_down: true,
            ..InputContext::default()
        };
        let action = translate(&[Event::Copy], &insert);
        assert!(!action.copy, "there was nothing selected to copy");
        assert!(
            action.bytes.is_empty(),
            "Ctrl+Insert must not send an interrupt"
        );
    }

    /// The same trap on the cut chord: Shift+Delete is not Ctrl+X.
    #[test]
    fn shift_delete_never_sends_can() {
        let delete = InputContext {
            delete_down: true,
            ..InputContext::default()
        };
        assert!(translate(&[Event::Cut], &delete).bytes.is_empty());
    }

    #[test]
    fn ctrl_x_sends_can_when_nothing_is_selected() {
        let action = translate(&[Event::Cut], &ctx());
        assert_eq!(action.bytes, vec![0x18]);

        let selected = InputContext {
            has_selection: true,
            ..InputContext::default()
        };
        assert!(translate(&[Event::Cut], &selected).copy);
    }

    /// Arrows, Tab and Escape are what egui would otherwise steal for focus
    /// navigation; the terminal claims them via a focus-lock filter, so they
    /// must survive translation intact.
    #[test]
    fn focus_navigation_keys_are_forwarded_to_iris() {
        for (key, expected) in [
            (Key::ArrowUp, b"\x1b[A".to_vec()),
            (Key::ArrowDown, b"\x1b[B".to_vec()),
            (Key::ArrowLeft, b"\x1b[D".to_vec()),
            (Key::ArrowRight, b"\x1b[C".to_vec()),
            (Key::Tab, b"\t".to_vec()),
            (Key::Escape, b"\x1b".to_vec()),
        ] {
            let action = translate(&[press(key)], &ctx());
            assert_eq!(action.bytes, expected, "{key:?} was not forwarded");
        }
    }
}
