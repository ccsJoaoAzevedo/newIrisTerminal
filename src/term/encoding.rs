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
    /// Pass bytes through untouched and let `vte` decode them. Correct for an
    /// instance whose output translation is configured properly.
    Utf8,
    /// Western European DOS codepage, as emitted by older Caché/IRIS instances
    /// on Windows in Latin-script locales.
    Cp850,
    /// Windows Western European.
    Cp1252,
    /// ISO 8859-1.
    Latin1,
    /// Repairs text that IRIS encoded twice.
    ///
    /// Some instances translate their output to UTF-8 and then run the result
    /// through a CP850 -> UTF-8 translation a second time. Each byte of the
    /// first encoding is reinterpreted as a CP850 glyph, so `Nó` (`c3 b3`)
    /// arrives as the box-drawing characters `├│`. Undoing it means mapping
    /// each character back to its CP850 byte and decoding those bytes as
    /// UTF-8 — which is exactly what the local instance needs.
    ///
    /// This is the default: the guards in `repair_double_encoding` make it
    /// a no-op on a correctly-configured instance, while fixing a mangled
    /// one.
    #[default]
    Cp850Doubled,
}

impl Encoding {
    pub const ALL: [Encoding; 5] = [
        Encoding::Utf8,
        Encoding::Cp850Doubled,
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
            Encoding::Cp850Doubled => "UTF-8 double-encoded via CP850 (repair)",
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
            Encoding::Latin1 | Encoding::Utf8 | Encoding::Cp850Doubled => char::from(byte),
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
            Encoding::Utf8 | Encoding::Cp850Doubled => None,
        }
    }

    /// Converts incoming bytes to UTF-8 for the parser.
    ///
    /// Single-byte encodings have no multi-byte sequences, so a chunk boundary
    /// can never split a character — the caller may pass whatever the PTY read
    /// returned without buffering.
    pub fn decode(self, bytes: &[u8]) -> Vec<u8> {
        // Pure ASCII is identical in every encoding here, and is the
        // overwhelming majority of output.
        if bytes.iter().all(|b| *b < 0x80) {
            return bytes.to_vec();
        }
        if self == Encoding::Cp850Doubled {
            return repair_double_encoding(bytes);
        }
        if !self.is_single_byte() {
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
        // Input is never double-encoded: we type what IRIS should receive, and
        // the mangling happens on its output path only.
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

/// Undoes one extra CP850 -> UTF-8 translation layer.
///
/// Some instances translate output to UTF-8 and then run the result through a
/// CP850 -> UTF-8 translation a second time, so `Nó` (`c3 b3`) arrives as the
/// box-drawing pair `├│`. Repairing it means mapping characters back to their
/// CP850 bytes and reading those as UTF-8.
///
/// Applied blindly this would wreck genuine box-drawing output, which ERP
/// full-screen routines do use. Three rules keep it safe:
///
/// * ASCII is never touched, so escape sequences are untouched.
/// * Each run of non-ASCII characters is converted only if its bytes form
///   **valid UTF-8**. A real table border such as `├───┤` maps to
///   `c3 c4 c4 c4 b4`, which is not valid UTF-8, so it is left alone.
/// * The decoded result must be ordinary Latin text (Latin-1 Supplement or
///   Latin Extended-A). Anything else is not what this mangling produces.
///
/// Runs that fail any rule are emitted unchanged, so correctly-encoded UTF-8
/// passes through untouched and the mode is safe to leave switched on.
fn repair_double_encoding(bytes: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(bytes);
    let mut out = Vec::with_capacity(bytes.len());
    let mut run: Vec<char> = Vec::new();

    for ch in text.chars() {
        if (ch as u32) < 0x80 {
            flush_run(&mut run, &mut out);
            out.push(ch as u8);
        } else {
            run.push(ch);
        }
    }
    flush_run(&mut run, &mut out);
    out
}

/// Converts one run of non-ASCII characters if it looks like double-encoded
/// Latin text, otherwise emits it unchanged.
fn flush_run(run: &mut Vec<char>, out: &mut Vec<u8>) {
    if run.is_empty() {
        return;
    }

    let mapped: Option<Vec<u8>> = run
        .iter()
        .map(|ch| {
            CP850_HIGH
                .iter()
                .position(|c| c == ch)
                .map(|i| 0x80 + i as u8)
        })
        .collect();

    let repaired = mapped
        .and_then(|bytes| String::from_utf8(bytes).ok())
        .filter(|text| {
            // Only accept a result that is plausible Latin prose.
            text.chars().all(|c| {
                let code = c as u32;
                (0xa0..=0x24f).contains(&code)
            })
        });

    match repaired {
        Some(text) => out.extend_from_slice(text.as_bytes()),
        None => {
            let mut buf = [0u8; 4];
            for ch in run.iter() {
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
        }
    }
    run.clear();
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

    /// The exact bytes the local IRIS 2025.1 instance sends for its login
    /// banner: `Nó` arrives as `├│` and `ção` as `├º├úo`, because the text was
    /// encoded to UTF-8 and then run through CP850 -> UTF-8 a second time.
    #[test]
    fn repairs_the_double_encoded_banner_this_instance_sends() {
        let raw = "N├│: CCDESNOT063, Configura├º├úo: CONSISETM".as_bytes();
        let fixed = String::from_utf8_lossy(&Encoding::Cp850Doubled.decode(raw)).into_owned();
        assert_eq!(fixed, "Nó: CCDESNOT063, Configuração: CONSISETM");
    }

    /// Ordinary, correctly-encoded UTF-8 must survive the repair pass — the
    /// mode has to be safe to leave switched on.
    #[test]
    fn repair_leaves_text_it_cannot_explain_alone() {
        let text = "日本語 and plain ASCII";
        let out =
            String::from_utf8_lossy(&Encoding::Cp850Doubled.decode(text.as_bytes())).into_owned();
        assert_eq!(out, text);
    }

    #[test]
    fn repair_never_touches_ascii_or_escape_sequences() {
        let raw = b"[2J[1;1HUSER>";
        assert_eq!(Encoding::Cp850Doubled.decode(raw), raw.to_vec());
    }

    /// The whole reason the repair is run-based and guarded: an ERP screen
    /// that genuinely draws a table must not be turned into mojibake.
    #[test]
    fn repair_leaves_genuine_box_drawing_alone() {
        for border in ["├───┤", "┌──────┐", "│ x │", "╔═══╗"]
        {
            let out = String::from_utf8_lossy(&Encoding::Cp850Doubled.decode(border.as_bytes()))
                .into_owned();
            assert_eq!(out, border, "corrupted genuine box drawing: {border}");
        }
    }

    /// Correctly-encoded accented text must survive untouched, so the mode is
    /// safe to leave on for an instance that does not need it.
    #[test]
    fn repair_leaves_correct_portuguese_alone() {
        for text in ["Instância", "não", "Configuração", "ó"] {
            let out = String::from_utf8_lossy(&Encoding::Cp850Doubled.decode(text.as_bytes()))
                .into_owned();
            assert_eq!(out, text, "corrupted already-correct text: {text}");
        }
    }

    /// Typing is unaffected: only IRIS's output path is mangled.
    #[test]
    fn repair_mode_sends_input_as_plain_utf8() {
        assert_eq!(
            Encoding::Cp850Doubled.encode("Instância"),
            "Instância".as_bytes().to_vec()
        );
    }

    #[test]
    fn escape_sequences_survive_transcoding() {
        // Decoding must not disturb CSI bytes, which are ASCII everywhere.
        let bytes = b"\x1b[2J\x1b[1;1Hol\xa0";
        let out = Encoding::Cp850.decode(bytes);
        assert!(out.starts_with(b"\x1b[2J\x1b[1;1Hol"));
    }
}
