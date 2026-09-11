//! Parsing, formatting and setting the keyboard shortcuts the app can be told
//! about.
//!
//! Shortcuts live in the macro XML as text (`key="Ctrl+Shift+G"`), because the
//! file is shared between people and hand-edited, and the one for the macro
//! manager lives in `settings.toml` the same way. That text is the source of
//! truth; this module is the only thing that decides what it means, so a value
//! that cannot be understood simply never fires rather than breaking the file.
//!
//! [`picker`] is the field it is set in, shared by the two places that set one.

use egui::{Key, Modifiers, Ui};

use crate::i18n::{tr, tr1};
use crate::ui::panels::WARNING;

/// Modifiers the app keeps for itself, and the keys they are used with.
///
/// Binding a macro to one of these would shadow a shortcut the user cannot
/// otherwise reach, so the editor warns instead of silently losing the tab
/// shortcut.
const RESERVED: &[(&str, Key)] = &[
    ("Ctrl+T", Key::T),
    ("Ctrl+W", Key::W),
    ("Ctrl+F", Key::F),
    ("Ctrl+Tab", Key::Tab),
    ("Ctrl+Plus", Key::Plus),
    ("Ctrl+Equals", Key::Equals),
    ("Ctrl+Minus", Key::Minus),
];

/// Turns `"Ctrl+Shift+G"` into the modifiers and key it names.
///
/// Segments are split on `+` and may appear in any order, so both
/// `Ctrl+Shift+G` and `shift+ctrl+g` work. Returns `None` when there is no key,
/// more than one key, or a segment nothing recognises.
pub fn parse(text: &str) -> Option<(Modifiers, Key)> {
    let mut modifiers = Modifiers::NONE;
    let mut key = None;

    for part in text.split('+') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        match part.to_lowercase().as_str() {
            "ctrl" | "control" => modifiers.ctrl = true,
            "shift" => modifiers.shift = true,
            "alt" | "option" => modifiers.alt = true,
            // `command` is what egui matches on: Cmd on macOS, Ctrl elsewhere.
            // A file written on one platform therefore still binds on another.
            "cmd" | "command" | "super" | "win" | "meta" => modifiers.command = true,
            _ => {
                if key.replace(key_from(part)?).is_some() {
                    // Two keys in one shortcut is a typo, not a chord.
                    return None;
                }
            }
        }
    }

    // `Ctrl` implies `command` on every platform but macOS, and egui compares
    // the two separately; setting both keeps `matches_exact` satisfied by a
    // real Ctrl press.
    if modifiers.ctrl {
        modifiers.command = true;
    }

    let key = key?;
    // A bare letter would fire while the user was typing into the terminal.
    if modifiers.is_none() {
        return None;
    }
    Some((modifiers, key))
}

/// A key name, being forgiving about how it was written.
///
/// egui's own table is case-sensitive and knows a digit only as `7`, `Digit7`
/// or `Numpad7`. This text is hand-edited in a shared XML file, so `g`, `G`,
/// `Num7` and `7` all have to mean what they obviously mean. Multi-word names
/// keep egui's spelling (`PageUp`, not `pageup`), since guessing where the
/// second word starts would accept more typos than it fixed.
fn key_from(part: &str) -> Option<Key> {
    let lower = part.to_lowercase();

    let digit = ["numpad", "digit", "num"]
        .iter()
        .find_map(|prefix| lower.strip_prefix(prefix))
        .unwrap_or("");

    [part, &part.to_uppercase(), &capitalized(&lower), digit]
        .iter()
        .find_map(|candidate| Key::from_name(candidate))
}

/// `home` -> `Home`. Only useful for single-word names, which is all it claims.
fn capitalized(lower: &str) -> String {
    let mut chars = lower.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().chain(chars).collect(),
        None => String::new(),
    }
}

/// Canonical text for a shortcut, in the order the parser prints it.
pub fn format(modifiers: Modifiers, key: Key) -> String {
    let mut out = String::new();
    if modifiers.ctrl || modifiers.command {
        out.push_str("Ctrl+");
    }
    if modifiers.alt {
        out.push_str("Alt+");
    }
    if modifiers.shift {
        out.push_str("Shift+");
    }
    out.push_str(key.name());
    out
}

/// Whether this shortcut is one the app already uses for itself.
pub fn is_reserved(modifiers: Modifiers, key: Key) -> Option<&'static str> {
    if !(modifiers.ctrl || modifiers.command) || modifiers.alt {
        return None;
    }
    // Ctrl+1..9 switch tabs.
    const TAB_DIGITS: [Key; 9] = [
        Key::Num1,
        Key::Num2,
        Key::Num3,
        Key::Num4,
        Key::Num5,
        Key::Num6,
        Key::Num7,
        Key::Num8,
        Key::Num9,
    ];
    if !modifiers.shift && TAB_DIGITS.contains(&key) {
        return Some("switching tabs");
    }
    if modifiers.shift {
        return None;
    }
    RESERVED
        .iter()
        .find(|(_, reserved)| *reserved == key)
        .map(|(name, _)| *name)
}

/// The field a shortcut is set in: the text, a button that reads the chord off
/// the keyboard, and the warnings for a value that will not do what it says.
///
/// Shared by the macro editor and the settings window, which is the whole
/// reason it is here rather than in either of them: the two were the same forty
/// lines, and the second copy would have been the one that stopped warning
/// about a chord the app has already taken.
///
/// `capture` is the caller's "listening right now" flag. It has to be the
/// caller's, because while it is set the app must stop claiming shortcuts for
/// itself - otherwise Ctrl+T opens a tab instead of being recorded.
///
/// Reports whether the binding changed.
pub fn picker(ui: &mut Ui, binding: &mut Option<String>, capture: &mut bool) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        let mut text = binding.clone().unwrap_or_default();
        if ui
            .add(
                egui::TextEdit::singleline(&mut text)
                    .hint_text("Ctrl+Shift+G")
                    .desired_width(140.0),
            )
            .changed()
        {
            let text = text.trim().to_string();
            *binding = (!text.is_empty()).then_some(text);
            changed = true;
        }
        // Typing the name of a chord is fiddly and easy to get subtly wrong -
        // `Num7` against `7`, `Option` against `Alt` - so the other way in is
        // to press it. What lands in the field is what the parser produced,
        // which is the value that will fire.
        let label = if *capture {
            tr("Press the keys...")
        } else {
            tr("Detect")
        };
        if ui
            .selectable_label(*capture, label)
            .on_hover_text(tr(
                "Press the combination and it is filled in here. Esc cancels, Backspace clears it.",
            ))
            .clicked()
        {
            *capture = !*capture;
        }
    });

    if *capture {
        match captured(ui) {
            Capture::Waiting => {}
            Capture::Cancelled => *capture = false,
            Capture::Cleared => {
                *binding = None;
                *capture = false;
                changed = true;
            }
            Capture::Chord(text) => {
                *binding = Some(text);
                *capture = false;
                changed = true;
            }
        }
        ui.small(tr(
            "A modifier is required: Ctrl, Alt, or both, with or without Shift.",
        ));
    }

    // Reported rather than rejected: a macro's binding is also edited by hand
    // in the shared XML, and a value we do not understand has to survive a
    // round trip through here instead of being erased.
    if let Some(key) = binding.as_deref() {
        match parse(key) {
            None => {
                ui.colored_label(
                    WARNING,
                    tr("Not understood, so it will not fire. Needs a modifier, like Ctrl+Shift+G."),
                );
            }
            Some((modifiers, parsed)) => {
                if let Some(used_for) = is_reserved(modifiers, parsed) {
                    ui.colored_label(
                        WARNING,
                        tr1("The app already uses this for {}; add Shift.", used_for),
                    );
                }
            }
        }
    }
    changed
}

/// What a frame of key presses meant while [`picker`] was listening.
enum Capture {
    /// Nothing usable yet, so keep listening.
    Waiting,
    /// Escape: leave the binding as it was.
    Cancelled,
    /// Backspace or Delete: no shortcut at all.
    Cleared,
    /// A chord, in the parser's own spelling.
    Chord(String),
}

/// Takes the pressed chord out of this frame's events.
///
/// Every key press is consumed while listening, and so is the text they would
/// have produced: a key pressed here is the shortcut being named, not typing,
/// and leaving it in the stream would put a letter in the field beside it or
/// fire the very shortcut being recorded. A chord without Ctrl or Alt is
/// ignored rather than accepted - a bare letter, or Shift plus one, would fire
/// while the user was typing at the prompt.
fn captured(ui: &Ui) -> Capture {
    ui.input_mut(|input| {
        let mut result = Capture::Waiting;
        input.events.retain(|event| match event {
            egui::Event::Text(_) => false,
            egui::Event::Key {
                key,
                modifiers,
                pressed: true,
                ..
            } => {
                match key {
                    Key::Escape => result = Capture::Cancelled,
                    Key::Backspace | Key::Delete => result = Capture::Cleared,
                    key if modifiers.ctrl || modifiers.alt || modifiers.command => {
                        result = Capture::Chord(format(*modifiers, *key));
                    }
                    _ => {}
                }
                false
            }
            _ => true,
        });
        result
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctrl_shift() -> Modifiers {
        Modifiers {
            ctrl: true,
            command: true,
            shift: true,
            ..Modifiers::NONE
        }
    }

    #[test]
    fn a_chord_parses_regardless_of_case_or_order() {
        let wanted = (ctrl_shift(), Key::G);
        assert_eq!(parse("Ctrl+Shift+G"), Some(wanted));
        assert_eq!(parse("shift+ctrl+g"), Some(wanted));
        assert_eq!(parse(" CTRL + SHIFT + G "), Some(wanted));
    }

    #[test]
    fn named_keys_and_digits_are_understood() {
        assert!(matches!(parse("Ctrl+Shift+F5"), Some((_, Key::F5))));
        assert!(matches!(parse("Alt+Home"), Some((_, Key::Home))));
        // Single-word names are case-insensitive.
        assert!(matches!(parse("alt+home"), Some((_, Key::Home))));
        assert!(matches!(parse("Ctrl+Shift+g"), Some((_, Key::G))));
    }

    /// Every spelling of a digit somebody might reasonably write into the XML.
    #[test]
    fn a_digit_is_understood_however_it_is_written() {
        for text in [
            "Ctrl+Shift+7",
            "Ctrl+Shift+Num7",
            "Ctrl+Shift+num7",
            "Ctrl+Shift+Digit7",
            "Ctrl+Shift+Numpad7",
        ] {
            assert!(
                matches!(parse(text), Some((_, Key::Num7))),
                "{text} was not understood"
            );
        }
    }

    /// Without a modifier the shortcut would fire on every keystroke aimed at
    /// the terminal.
    #[test]
    fn a_bare_key_is_rejected() {
        assert_eq!(parse("G"), None);
        assert_eq!(parse(""), None);
    }

    #[test]
    fn nonsense_is_rejected_rather_than_half_understood() {
        assert_eq!(parse("Ctrl+Shift"), None, "no key at all");
        assert_eq!(parse("Ctrl+G+H"), None, "two keys");
        assert_eq!(parse("Hyper+G"), None, "unknown modifier");
    }

    #[test]
    fn formatting_round_trips_back_through_the_parser() {
        for text in ["Ctrl+Shift+G", "Alt+Home", "Ctrl+F5"] {
            let (modifiers, key) = parse(text).expect(text);
            let printed = format(modifiers, key);
            assert_eq!(
                parse(&printed),
                Some((modifiers, key)),
                "{text} printed as {printed}"
            );
        }
    }

    #[test]
    fn the_apps_own_shortcuts_are_reported_as_taken() {
        let (m, k) = parse("Ctrl+T").expect("parse");
        assert_eq!(is_reserved(m, k), Some("Ctrl+T"));

        let (m, k) = parse("Ctrl+3").expect("parse");
        assert_eq!(is_reserved(m, k), Some("switching tabs"));

        // Adding Shift is enough to get out of the app's way.
        let (m, k) = parse("Ctrl+Shift+T").expect("parse");
        assert_eq!(is_reserved(m, k), None);

        let (m, k) = parse("Ctrl+Shift+G").expect("parse");
        assert_eq!(is_reserved(m, k), None);
    }
}
