//! Translating egui input events into the bytes a VT terminal would send.
//!
//! The one platform-dependent thing here is which modifier means "terminal
//! shortcut": Ctrl everywhere except macOS, where Cmd is idiomatic for
//! copy/paste while Ctrl must stay available for control characters.

use egui::{Event, Key, Modifiers};

/// True when the modifier used for app shortcuts (copy, paste, new tab) is
/// held. `Modifiers::command` is already Cmd on macOS and Ctrl elsewhere.
pub fn is_command(m: &Modifiers) -> bool {
    m.command
}

/// Bytes for a key press, or `None` when the key is not something we send.
pub fn key_bytes(key: Key, modifiers: &Modifiers) -> Option<Vec<u8>> {
    // Application shortcuts are handled by the caller, not sent to IRIS.
    let ctrl = modifiers.ctrl;

    let bytes: Vec<u8> = match key {
        // IRIS expects CR for "line entered"; sending LF leaves it waiting.
        Key::Enter => vec![b'\r'],
        Key::Tab => vec![b'\t'],
        Key::Escape => vec![0x1b],
        // DEL, not BS — this is what terminfo's kbs maps to and what IRIS
        // erase handling expects.
        Key::Backspace => vec![0x7f],
        Key::Delete => b"\x1b[3~".to_vec(),
        Key::Insert => b"\x1b[2~".to_vec(),
        Key::Home => b"\x1b[H".to_vec(),
        Key::End => b"\x1b[F".to_vec(),
        Key::PageUp => b"\x1b[5~".to_vec(),
        Key::PageDown => b"\x1b[6~".to_vec(),
        Key::ArrowUp => b"\x1b[A".to_vec(),
        Key::ArrowDown => b"\x1b[B".to_vec(),
        Key::ArrowRight => b"\x1b[C".to_vec(),
        Key::ArrowLeft => b"\x1b[D".to_vec(),
        Key::F1 => b"\x1bOP".to_vec(),
        Key::F2 => b"\x1bOQ".to_vec(),
        Key::F3 => b"\x1bOR".to_vec(),
        Key::F4 => b"\x1bOS".to_vec(),
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

/// Turns a frame's worth of egui events into bytes for the PTY.
///
/// `has_selection` decides the Ctrl+C ambiguity: with text selected it copies,
/// otherwise it interrupts the running routine — the behaviour every terminal
/// converges on.
pub struct InputAction {
    /// Key sequences that are ASCII/control bytes by definition — arrows,
    /// Enter, Ctrl-codes. These go to the PTY verbatim.
    pub bytes: Vec<u8>,
    /// Characters the user typed. Kept as text, not bytes, because they must
    /// be encoded into the instance's codepage before being sent.
    pub text: String,
    pub copy: bool,
    pub paste: Option<String>,
}

impl InputAction {
    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty() && self.text.is_empty()
    }
}

pub fn translate(events: &[Event], has_selection: bool) -> InputAction {
    let mut action = InputAction {
        bytes: Vec::new(),
        text: String::new(),
        copy: false,
        paste: None,
    };

    for event in events {
        match event {
            Event::Text(text) => {
                // egui already filters out text produced while a command
                // modifier is held, so this is genuine typed input.
                action.text.push_str(text);
            }
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                if let Some(bytes) = key_bytes(*key, modifiers) {
                    action.bytes.extend_from_slice(&bytes);
                }
            }

            // egui-winit intercepts the clipboard chords *before* building a
            // key event and returns early, so Ctrl+C/Ctrl+X never arrive as
            // `Event::Key`. They must be handled here or the control codes are
            // lost entirely — which is why Ctrl+C could not interrupt IRIS.
            Event::Copy => {
                if has_selection {
                    action.copy = true;
                } else {
                    // No selection: Ctrl+C is an interrupt, not a copy.
                    action.bytes.push(0x03);
                }
            }
            Event::Cut => {
                if has_selection {
                    action.copy = true;
                } else {
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
mod tests {
    use super::*;

    #[test]
    fn enter_sends_carriage_return_not_line_feed() {
        assert_eq!(key_bytes(Key::Enter, &Modifiers::NONE), Some(vec![b'\r']));
    }

    #[test]
    fn backspace_sends_del() {
        assert_eq!(
            key_bytes(Key::Backspace, &Modifiers::NONE),
            Some(vec![0x7f])
        );
    }

    #[test]
    fn arrows_send_csi_sequences() {
        assert_eq!(
            key_bytes(Key::ArrowUp, &Modifiers::NONE),
            Some(b"\x1b[A".to_vec())
        );
    }

    #[test]
    fn ctrl_letters_map_to_control_codes() {
        let ctrl = Modifiers {
            ctrl: true,
            ..Modifiers::NONE
        };
        assert_eq!(key_bytes(Key::C, &ctrl), Some(vec![0x03]));
        assert_eq!(key_bytes(Key::A, &ctrl), Some(vec![0x01]));
        assert_eq!(key_bytes(Key::Z, &ctrl), Some(vec![0x1a]));
    }

    #[test]
    fn paste_normalises_every_newline_flavour_to_cr() {
        assert_eq!(sanitize_paste("a\r\nb\nc"), "a\rb\rc");
    }

    /// egui-winit turns Ctrl+C into `Event::Copy` and never emits a key event
    /// for it, so this is the only path that can produce an interrupt.
    #[test]
    fn ctrl_c_copies_when_text_is_selected() {
        let action = translate(&[Event::Copy], true);
        assert!(action.copy);
        assert!(action.is_empty(), "must not also send bytes to IRIS");
    }

    #[test]
    fn ctrl_c_interrupts_when_nothing_is_selected() {
        let action = translate(&[Event::Copy], false);
        assert!(!action.copy);
        assert_eq!(action.bytes, vec![0x03], "Ctrl+C must reach IRIS as ETX");
    }

    #[test]
    fn ctrl_x_sends_can_when_nothing_is_selected() {
        let action = translate(&[Event::Cut], false);
        assert_eq!(action.bytes, vec![0x18]);
        assert!(translate(&[Event::Cut], true).copy);
    }

    /// Arrows, Tab and Escape are what egui would otherwise steal for focus
    /// navigation; the terminal claims them via a focus-lock filter, so they
    /// must survive translation intact.
    #[test]
    fn focus_navigation_keys_are_forwarded_to_iris() {
        for (key, expected) in [
            (Key::ArrowUp, b"[A".to_vec()),
            (Key::ArrowDown, b"[B".to_vec()),
            (Key::ArrowLeft, b"[D".to_vec()),
            (Key::ArrowRight, b"[C".to_vec()),
            (Key::Tab, b"	".to_vec()),
            (Key::Escape, b"".to_vec()),
        ] {
            let event = Event::Key {
                key,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Modifiers::NONE,
            };
            let action = translate(std::slice::from_ref(&event), false);
            assert_eq!(action.bytes, expected, "{key:?} was not forwarded");
        }
    }
}
