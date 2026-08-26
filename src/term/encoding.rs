//! Character-set handling for the byte stream to and from IRIS.
//!
//! `vte` decodes UTF-8, which is what current IRIS builds emit: a probe of the
//! local 2025.1 instance (`tests/encoding_probe.rs`) confirmed the session
//! stream is valid UTF-8 end to end, so [`Encoding::Utf8`] is the default.
//!
//! The single-byte codepages remain because older Caché and IRIS instances,
//! and instances with a non-UTF-8 I/O translation table configured, do emit
//! CP850 or Windows-1252. Getting this wrong is not an error — it silently
//! mangles accented characters — so it is a per-profile setting rather than
//! something guessed at runtime.
//!
//! Escape sequences are pure ASCII in every encoding handled here, so
//! transcoding before parsing never disturbs them.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Encoding {
    /// Pass bytes through untouched and let `vte` decode them. What current
    /// IRIS builds emit, and the default.
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

    /// Whether this encoding needs transcoding at all.
    fn is_single_byte(self) -> bool {
        !matches!(self, Encoding::Utf8)
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
    /// Single-byte encodings have no multi-byte sequences, so a chunk boundary
    /// can never split a character — the caller may pass whatever the PTY read
    /// returned without buffering.
    pub fn decode(self, bytes: &[u8]) -> Vec<u8> {
        // Fast path: UTF-8 needs nothing, and pure ASCII is identical in every
        // encoding here — together the overwhelming majority of output.
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
        }
    }

    /// Covers the legacy case: an instance whose I/O translation emits CP850.
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

    /// The default must not touch the stream: the live instance sends UTF-8,
    /// and transcoding it would corrupt every non-ASCII character.
    #[test]
    fn utf8_passes_through_unchanged() {
        let bytes = "Instância".as_bytes();
        assert_eq!(Encoding::Utf8.decode(bytes), bytes.to_vec());
    }

    #[test]
    fn encoding_round_trips_through_each_single_byte_codepage() {
        for enc in [Encoding::Cp850, Encoding::Cp1252, Encoding::Latin1] {
            let text = "Instância não";
            let encoded = enc.encode(text);
            let decoded = String::from_utf8(enc.decode(&encoded)).unwrap();
            assert_eq!(decoded, text, "{enc:?}");
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
}
