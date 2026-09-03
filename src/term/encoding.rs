//! Character-set handling for the byte stream to and from IRIS.
//!
//! # One charset, both directions, applied once
//!
//! This is the rule the whole module exists to keep, and it is the rule PuTTY
//! and IRISTerm keep too: a session has exactly one character set, the bytes
//! arriving are decoded with it, the bytes typed are encoded with it, and
//! nothing translates a second time anywhere in between. Break it in either
//! direction and accented text does not merely look wrong — the terminal and
//! the far side stop agreeing on how many *columns* a line occupies, and every
//! gesture built on reading the line back off the screen (recall, Home, End,
//! rubbing out a selection) starts landing in the wrong place. See
//! [`crate::term::lineedit`] for why that column count is load-bearing.
//!
//! # A local session is UTF-8, and the console is what makes it so
//!
//! On Windows a local session runs inside a pseudo-console, and a pseudo-console
//! is not a pipe: it decodes the child's bytes with its own codepage and
//! re-encodes them as UTF-8 for the terminal, then decodes the terminal's UTF-8
//! and re-encodes it into that codepage for the child. `tests/live_codepage.rs`
//! measures the whole path against a live instance, and two facts fall out:
//!
//! * **The wire is always UTF-8, whatever codepage the console is on.** A raw
//!   high byte from IRIS never reaches this terminal. So no decoding on this
//!   side can rescue a local session — by the time the bytes arrive the console
//!   has already had its way with them.
//! * **Only a UTF-8 console carries a typed accent intact.** On the machine's
//!   OEM codepage a typed `ó` reaches IRIS as `?` — character 63 — whatever
//!   bytes are put on the wire for it.
//!
//! Which is why sessions are opened with `chcp 65001` in front of them (see
//! [`crate::pty::launcher`]), and why a local session's charset is not a choice:
//! it is UTF-8, and [`crate::config::Profile::wire_encoding`] answers UTF-8 for
//! one however the profile is configured. A console left on the OEM codepage is
//! what produced the original bug — the banner arriving as `N├│`, a typed `ó`
//! arriving as `?`, and an accent costing a column that only IRIS counted.
//!
//! There was briefly a fifth encoding here, `Cp850Doubled`, which tried to
//! repair such a console from this side. It is gone, and why is worth keeping:
//! it could put the *characters* back but never the column, and its input half
//! sent two characters (`├│`) where IRIS should receive one — so IRIS stored the
//! box-drawing pair, while the terminal, having folded it back into `ó` for
//! display, was a column short for every accent on the line. Half a translation
//! is worse than none.
//!
//! # Where the codepages are real
//!
//! A Telnet session has no console in the path: the bytes on the socket are the
//! instance's own. An older Caché or IRIS, or one with a non-UTF-8 I/O
//! translation table configured, really does emit CP850 or Windows-1252 there,
//! and the single-byte encodings below decode and encode it symmetrically.
//! Getting it wrong is not an error — it silently mangles accented characters —
//! so it is a per-profile setting rather than something guessed at runtime.
//!
//! Escape sequences are pure ASCII in every encoding handled here, so
//! transcoding never disturbs them.

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Encoding {
    /// Pass bytes through untouched and let `vte` decode them. What a local
    /// session always speaks, what a current instance speaks over Telnet, and
    /// the default.
    #[default]
    Utf8,
    /// Western European DOS codepage, as emitted by older Caché/IRIS instances
    /// on Windows in Latin-script locales.
    Cp850,
    /// Windows Western European.
    Cp1252,
    /// ISO 8859-1.
    Latin1,
}

impl Encoding {
    pub const ALL: [Encoding; 4] = [
        Encoding::Utf8,
        Encoding::Cp850,
        Encoding::Cp1252,
        Encoding::Latin1,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Cp850 => "CP850 (DOS Western)",
            Encoding::Cp1252 => "Windows-1252",
            Encoding::Latin1 => "ISO 8859-1",
        }
    }

    /// The encoding a configuration file names, if it names one this build has.
    ///
    /// `None` covers a hand-edited typo and `cp850-doubled`, the retired
    /// console repair described in the module docs. Both have to load as
    /// *something*: a profile is one table in the middle of the settings file,
    /// and refusing the value would take the user's themes, window size and
    /// every other profile down with it.
    fn from_name(name: &str) -> Option<Encoding> {
        match name.trim().to_ascii_lowercase().as_str() {
            "utf8" | "utf-8" => Some(Encoding::Utf8),
            "cp850" | "ibm850" | "850" => Some(Encoding::Cp850),
            "cp1252" | "windows-1252" | "1252" => Some(Encoding::Cp1252),
            "latin1" | "iso-8859-1" | "iso8859-1" => Some(Encoding::Latin1),
            _ => None,
        }
    }

    /// Whether this encoding is a plain single-byte codepage.
    fn is_single_byte(self) -> bool {
        matches!(self, Encoding::Cp850 | Encoding::Cp1252 | Encoding::Latin1)
    }

    /// The character a high byte (0x80..0xFF) stands for.
    fn high_char(self, byte: u8) -> char {
        match self {
            // ISO 8859-1 maps its high half straight onto U+0080..U+00FF, so
            // it needs no table.
            Encoding::Latin1 | Encoding::Utf8 => char::from(byte),
            Encoding::Cp850 => CP850_HIGH[(byte - 0x80) as usize],
            Encoding::Cp1252 => CP1252_HIGH[(byte - 0x80) as usize],
        }
    }

    /// The high byte representing `ch`, if this encoding has one.
    fn high_byte(self, ch: char) -> Option<u8> {
        match self {
            Encoding::Latin1 => {
                let code = ch as u32;
                (0x80..=0xff).contains(&code).then_some(code as u8)
            }
            Encoding::Cp850 => CP850_HIGH
                .iter()
                .position(|c| *c == ch)
                .map(|i| 0x80 + i as u8),
            Encoding::Cp1252 => CP1252_HIGH
                .iter()
                .position(|c| *c == ch)
                .map(|i| 0x80 + i as u8),
            Encoding::Utf8 => None,
        }
    }

    /// Converts incoming bytes to UTF-8 for the parser.
    ///
    /// Safe to call one PTY read at a time, which is how it is called: every
    /// encoding here is either a passthrough or single-byte, so a read boundary
    /// has no multi-byte sequence *of this module's making* to fall inside.
    /// UTF-8 that arrives split across two reads is reassembled by `vte`, which
    /// keeps its own decoder state across calls.
    pub fn decode(self, bytes: &[u8]) -> Vec<u8> {
        // Pure ASCII is identical in every encoding here, and is the
        // overwhelming majority of output.
        if !self.is_single_byte() || bytes.iter().all(|b| *b < 0x80) {
            return bytes.to_vec();
        }

        let mut out = Vec::with_capacity(bytes.len());
        let mut buf = [0u8; 4];
        for &byte in bytes {
            if byte < 0x80 {
                out.push(byte);
            } else {
                out.extend_from_slice(self.high_char(byte).encode_utf8(&mut buf).as_bytes());
            }
        }
        out
    }

    /// Converts text the user typed into the bytes IRIS expects.
    ///
    /// A character with no representation in the target codepage becomes `?`,
    /// matching what a real terminal does rather than dropping it silently.
    pub fn encode(self, text: &str) -> Vec<u8> {
        if !self.is_single_byte() {
            return text.as_bytes().to_vec();
        }

        let mut out = Vec::with_capacity(text.len());
        for ch in text.chars() {
            if (ch as u32) < 0x80 {
                out.push(ch as u8);
            } else {
                out.push(self.high_byte(ch).unwrap_or(b'?'));
            }
        }
        out
    }
}

/// Lenient on the way in, exact on the way out.
///
/// See [`Encoding::from_name`]: a name this build does not have loads as the
/// default and is written back as the default the next time the settings are
/// saved, which is what retires `cp850-doubled` from the profiles that still
/// carry it.
impl<'de> Deserialize<'de> for Encoding {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let name = String::deserialize(deserializer)?;
        Ok(Encoding::from_name(&name).unwrap_or_else(|| {
            log::warn!("unknown encoding {name:?} in the settings; using UTF-8");
            Encoding::default()
        }))
    }
}

/// CP850 (DOS Latin-1), bytes 0x80..0xFF.
const CP850_HIGH: [char; 128] = [
    'Ç', 'ü', 'é', 'â', 'ä', 'à', 'å', 'ç', 'ê', 'ë', 'è', 'ï', 'î', 'ì', 'Ä', 'Å', 'É', 'æ', 'Æ',
    'ô', 'ö', 'ò', 'û', 'ù', 'ÿ', 'Ö', 'Ü', 'ø', '£', 'Ø', '×', 'ƒ', 'á', 'í', 'ó', 'ú', 'ñ', 'Ñ',
    'ª', 'º', '¿', '®', '¬', '½', '¼', '¡', '«', '»', '░', '▒', '▓', '│', '┤', 'Á', 'Â', 'À', '©',
    '╣', '║', '╗', '╝', '¢', '¥', '┐', '└', '┴', '┬', '├', '─', '┼', 'ã', 'Ã', '╚', '╔', '╩', '╦',
    '╠', '═', '╬', '¤', 'ð', 'Ð', 'Ê', 'Ë', 'È', 'ı', 'Í', 'Î', 'Ï', '┘', '┌', '█', '▄', '¦', 'Ì',
    '▀', 'Ó', 'ß', 'Ô', 'Ò', 'õ', 'Õ', 'µ', 'þ', 'Þ', 'Ú', 'Û', 'Ù', 'ý', 'Ý', '¯', '´',
    '\u{00ad}', '±', '‗', '¾', '¶', '§', '÷', '¸', '°', '¨', '·', '¹', '³', '²', '■', '\u{00a0}',
];

/// Windows-1252, bytes 0x80..0xFF.
const CP1252_HIGH: [char; 128] = [
    '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž', '\u{8f}',
    '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}', 'ž', 'Ÿ',
    '\u{00a0}', '¡', '¢', '£', '¤', '¥', '¦', '§', '¨', '©', 'ª', '«', '¬', '\u{00ad}', '®', '¯',
    '°', '±', '²', '³', '´', 'µ', '¶', '·', '¸', '¹', 'º', '»', '¼', '½', '¾', '¿', 'À', 'Á', 'Â',
    'Ã', 'Ä', 'Å', 'Æ', 'Ç', 'È', 'É', 'Ê', 'Ë', 'Ì', 'Í', 'Î', 'Ï', 'Ð', 'Ñ', 'Ò', 'Ó', 'Ô', 'Õ',
    'Ö', '×', 'Ø', 'Ù', 'Ú', 'Û', 'Ü', 'Ý', 'Þ', 'ß', 'à', 'á', 'â', 'ã', 'ä', 'å', 'æ', 'ç', 'è',
    'é', 'ê', 'ë', 'ì', 'í', 'î', 'ï', 'ð', 'ñ', 'ò', 'ó', 'ô', 'õ', 'ö', '÷', 'ø', 'ù', 'ú', 'û',
    'ü', 'ý', 'þ', 'ÿ',
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ascii_is_untouched_by_every_encoding() {
        for enc in Encoding::ALL {
            assert_eq!(enc.decode(b"USER>"), b"USER>".to_vec(), "{enc:?}");
            assert_eq!(enc.encode("USER>"), b"USER>".to_vec(), "{enc:?}");
        }
    }

    /// Covers the legacy case: an instance whose I/O translation emits CP850
    /// over Telnet, where there is no console to have re-encoded it.
    #[test]
    fn cp850_decodes_accented_text_from_a_legacy_instance() {
        // 'â' is 0x83 in CP850.
        let bytes = b"Inst\x83ncia do IRIS desconhecida";
        let decoded = String::from_utf8(Encoding::Cp850.decode(bytes)).unwrap();
        assert_eq!(decoded, "Instância do IRIS desconhecida");
    }

    #[test]
    fn cp1252_and_latin1_agree_on_shared_characters() {
        assert_eq!(
            Encoding::Cp1252.decode(b"\xe7\xe3"),
            Encoding::Latin1.decode(b"\xe7\xe3")
        );
        assert_eq!(
            String::from_utf8(Encoding::Latin1.decode(b"\xe7\xe3")).unwrap(),
            "çã"
        );
    }

    /// The default must not touch the stream: a session sends UTF-8, and
    /// transcoding it would corrupt every non-ASCII character.
    #[test]
    fn utf8_passes_through_unchanged() {
        let bytes = "Instância".as_bytes();
        assert_eq!(Encoding::Utf8.decode(bytes), bytes.to_vec());
        assert_eq!(Encoding::Utf8.encode("Instância"), bytes.to_vec());
    }

    /// The rule the module exists to keep: what goes out comes back the same,
    /// through every encoding, in one translation each way.
    #[test]
    fn every_encoding_round_trips_symmetrically() {
        for enc in Encoding::ALL {
            let text = "Instância não Configuração";
            let encoded = enc.encode(text);
            let decoded = String::from_utf8(enc.decode(&encoded)).unwrap();
            assert_eq!(decoded, text, "{enc:?}");
        }
    }

    /// The other half of that rule, and the one the retired console repair
    /// broke: a character typed has to be one character on the far side too, or
    /// every gesture that reads the line back off the screen is out by one per
    /// accent.
    #[test]
    fn a_character_typed_is_one_character_on_the_wire() {
        for enc in Encoding::ALL {
            let sent = enc.encode("ó");
            let back = String::from_utf8(enc.decode(&sent)).unwrap();
            assert_eq!(back.chars().count(), 1, "{enc:?} sent {sent:02x?}");
        }
    }

    #[test]
    fn unrepresentable_characters_become_question_marks() {
        // A CJK character has no CP850 slot.
        assert_eq!(Encoding::Cp850.encode("a漢b"), b"a?b".to_vec());
    }

    #[test]
    fn escape_sequences_survive_transcoding() {
        // Decoding must not disturb CSI bytes, which are ASCII everywhere.
        let bytes = b"\x1b[2J\x1b[1;1Hol\xa0";
        let out = Encoding::Cp850.decode(bytes);
        assert!(out.starts_with(b"\x1b[2J\x1b[1;1Hol"));
    }

    /// Genuine box drawing has to survive, because ERP full-screen routines
    /// draw with it. The retired repair is what put that at risk.
    #[test]
    fn box_drawing_passes_through_untouched() {
        for border in ["├───┤", "┌──────┐", "│ x │", "╔═══╗"]
        {
            let out = String::from_utf8(Encoding::Utf8.decode(border.as_bytes())).unwrap();
            assert_eq!(out, border, "corrupted box drawing: {border}");
        }
    }

    /// Every name a settings file can carry, including the ones this build no
    /// longer writes, resolves without taking the file down with it.
    #[test]
    fn a_name_this_build_does_not_have_loads_as_utf8() {
        #[derive(Deserialize, Serialize)]
        struct Held {
            encoding: Encoding,
        }
        let held = |name: &str| {
            toml::from_str::<Held>(&format!("encoding = \"{name}\""))
                .unwrap_or_else(|e| panic!("{name}: {e}"))
                .encoding
        };

        // The retired console repair, which a profile written by 0.3.0 still
        // carries: it must not refuse to load, and it must not stay.
        assert_eq!(held("cp850-doubled"), Encoding::Utf8);
        assert_eq!(held("nonsense"), Encoding::Utf8);
        assert_eq!(held(""), Encoding::Utf8);

        // And the names this build does write survive a round trip.
        for enc in Encoding::ALL {
            let written = toml::to_string(&Held { encoding: enc }).unwrap();
            let name = written
                .trim()
                .trim_start_matches("encoding = ")
                .trim_matches('"');
            assert_eq!(held(name), enc, "{enc:?} was written as {name:?}");
        }
    }
}
