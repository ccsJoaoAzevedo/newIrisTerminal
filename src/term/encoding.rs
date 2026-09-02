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
    /// Undoes the codepage translation a Windows pseudo-console puts in the
    /// way, in both directions.
    ///
    /// The instance speaks UTF-8, but the console between it and this terminal
    /// does not: it reads the instance's bytes as CP850 and re-encodes them,
    /// so `Nó` (`c3 b3`) arrives as the box-drawing characters `├│`. Undoing it
    /// means mapping each character back to its CP850 byte and reading those
    /// bytes as UTF-8.
    ///
    /// Typing travels the same road in reverse. The console takes what this
    /// terminal writes and converts it to the codepage before IRIS reads it, so
    /// a `ó` sent as plain UTF-8 reaches IRIS as `?` — character 63, which is
    /// what a probe of the live instance reports. Sending the CP850 glyph for
    /// each byte IRIS should receive is what gets `ó` there intact.
    ///
    /// This is the default: the guards in `repair_double_encoding` make the
    /// output half a no-op on a console that is not translating, and a session
    /// reached without one — Telnet — is opened as [`Encoding::Utf8`].
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
            Encoding::Cp850Doubled => "Windows console (CP850 round trip)",
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

    /// [`Encoding::decode`] for a stream that arrives one PTY read at a time.
    ///
    /// The repair reads a whole run of characters at a time, and a read can end
    /// in the middle of one: `carry` holds the few bytes that are waiting for
    /// the rest of it. Without this, a boundary landing inside `Nó` - which
    /// travels as `├│` - leaves the box-drawing characters on screen instead of
    /// the accent. Rare, and exactly the sort of thing that only shows up on a
    /// long screen of output. Every other encoding here is single-byte and
    /// cannot be split, so it passes straight through.
    pub fn decode_chunk(self, bytes: &[u8], carry: &mut Vec<u8>) -> Vec<u8> {
        if self != Encoding::Cp850Doubled {
            return self.decode(bytes);
        }
        let mut buffered = std::mem::take(carry);
        buffered.extend_from_slice(bytes);
        let split = buffered.len() - held_back(&buffered);
        carry.extend_from_slice(&buffered[split..]);
        self.decode(&buffered[..split])
    }

    /// Converts incoming bytes to UTF-8 for the parser.
    ///
    /// Single-byte encodings have no multi-byte sequences, so a chunk boundary
    /// can never split a character. The one that can is the repair, which reads
    /// UTF-8 — see [`Encoding::decode_chunk`] for the streaming form.
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
        if self == Encoding::Cp850Doubled {
            return double_encode(text);
        }
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

/// Each byte IRIS should receive, carried as the CP850 glyph the console will
/// turn back into that byte.
///
/// The mirror image of [`repair_double_encoding`]: the console converts what it
/// is given to its codepage before IRIS reads it, so the way to hand IRIS the
/// UTF-8 byte `c3` is to send the character CP850 keeps at `c3`. ASCII is
/// already itself in both, which is what leaves every escape sequence and
/// control code alone.
fn double_encode(text: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len());
    let mut buf = [0u8; 4];
    for &byte in text.as_bytes() {
        if byte < 0x80 {
            out.push(byte);
        } else {
            let ch = CP850_HIGH[(byte - 0x80) as usize];
            out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        }
    }
    out
}

/// How many bytes at the end of `bytes` have to wait for the next read.
///
/// Two things can be cut in half by a read boundary. The UTF-8 encoding of a
/// character is one. The other is the run of characters that carries a repaired
/// one: `├` is a whole character and a whole run, but the byte it stands for -
/// `c3` - is only the first half of `ó`, so it has to wait for the `│` that
/// finishes it.
///
/// At most a character and three of those, since the widest thing being
/// reassembled is four bytes.
fn held_back(bytes: &[u8]) -> usize {
    let partial = incomplete_tail(bytes);
    let text = String::from_utf8_lossy(&bytes[..bytes.len() - partial]);

    // The trailing run of non-ASCII characters, in order, at most three.
    let mut tail: Vec<char> = Vec::new();
    for ch in text.chars().rev() {
        if (ch as u32) < 0x80 || tail.len() == 3 {
            break;
        }
        tail.push(ch);
    }
    tail.reverse();

    // The bytes they stand for. A character with no CP850 byte - a replacement
    // character, or genuine box drawing this is not going to repair - means
    // there is nothing here waiting to be finished.
    let mapped: Option<Vec<u8>> = tail
        .iter()
        .map(|ch| {
            CP850_HIGH
                .iter()
                .position(|c| c == ch)
                .map(|i| 0x80 + i as u8)
        })
        .collect();
    let keep = mapped.map_or(0, |bytes| incomplete_tail(&bytes));

    let held: usize = tail[tail.len() - keep..]
        .iter()
        .map(|ch| ch.len_utf8())
        .sum();
    partial + held
}

/// How many bytes at the end of `bytes` are the start of a UTF-8 sequence that
/// has not all arrived yet.
///
/// At most three: a sequence is four bytes at the widest. Anything that is not
/// a truncated sequence counts as nothing to hold back, so malformed input is
/// handed to the decoder rather than accumulating here forever.
fn incomplete_tail(bytes: &[u8]) -> usize {
    for back in 1..=3.min(bytes.len()) {
        let byte = bytes[bytes.len() - back];
        // Continuation bytes are 10xxxxxx; anything else is a lead byte and
        // decides how long its sequence is.
        if byte & 0b1100_0000 == 0b1000_0000 {
            continue;
        }
        let width = match byte {
            0x00..=0x7f => 1,
            0xc0..=0xdf => 2,
            0xe0..=0xef => 3,
            0xf0..=0xf7 => 4,
            // A stray continuation or an invalid lead: not a truncated
            // sequence, so there is nothing to wait for.
            _ => return 0,
        };
        return if back < width { back } else { 0 };
    }
    0
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

    /// Typing travels the same road as the output, in reverse. A probe of the
    /// live instance settled it: `ó` sent as plain UTF-8 arrives as character
    /// 63 (`?`), and sent as the CP850 glyphs for its two UTF-8 bytes arrives
    /// as character 243, which is `ó`.
    #[test]
    fn typing_is_encoded_the_way_the_console_will_read_it() {
        // `ó` is c3 b3 in UTF-8; CP850 keeps `├` at c3 and `│` at b3.
        assert_eq!(Encoding::Cp850Doubled.encode("ó"), "├│".as_bytes().to_vec());
        // ASCII is itself, so commands and escape sequences are untouched.
        assert_eq!(
            Encoding::Cp850Doubled.encode("write 1\r"),
            b"write 1\r".to_vec()
        );
    }

    /// The two halves are one channel: what the terminal sends comes back the
    /// way it went out.
    #[test]
    fn the_console_round_trip_returns_what_was_typed() {
        for text in ["Instância", "não", "Configuração", "ó", "ASCII only"] {
            let sent = Encoding::Cp850Doubled.encode(text);
            let back = Encoding::Cp850Doubled.decode(&sent);
            assert_eq!(String::from_utf8_lossy(&back), text, "round trip of {text}");
        }
    }

    /// Every byte has exactly one glyph to travel as, or the round trip would
    /// be ambiguous.
    #[test]
    fn the_cp850_table_maps_each_byte_to_a_character_of_its_own() {
        let mut seen: Vec<char> = CP850_HIGH.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), CP850_HIGH.len(), "two bytes share a character");
    }

    /// A PTY read can end anywhere, including inside the three bytes of `├`.
    /// The repair reads whole characters, so the tail waits for the rest.
    #[test]
    fn a_character_split_across_two_reads_survives() {
        let whole = "N├│: ok".as_bytes();
        for split in 1..whole.len() {
            let mut carry = Vec::new();
            let mut out = Encoding::Cp850Doubled.decode_chunk(&whole[..split], &mut carry);
            out.extend(Encoding::Cp850Doubled.decode_chunk(&whole[split..], &mut carry));
            assert!(carry.is_empty(), "bytes left behind at split {split}");
            assert_eq!(
                String::from_utf8_lossy(&out),
                "Nó: ok",
                "split after {split} bytes"
            );
        }
    }

    /// Malformed input must not sit in the carry buffer waiting for a
    /// continuation that is never coming.
    #[test]
    fn a_stray_byte_is_never_held_back() {
        let mut carry = Vec::new();
        let out = Encoding::Cp850Doubled.decode_chunk(&[b'a', 0xff], &mut carry);
        assert!(carry.is_empty());
        assert_eq!(out.first(), Some(&b'a'));
    }

    /// Nor may a run that is already complete: output that ends on an accent
    /// has to reach the screen without waiting for whatever comes next.
    #[test]
    fn a_finished_run_is_not_held_back() {
        let mut carry = Vec::new();
        let out = Encoding::Cp850Doubled.decode_chunk("N├│".as_bytes(), &mut carry);
        assert!(carry.is_empty(), "the accent was left waiting");
        assert_eq!(String::from_utf8_lossy(&out), "Nó");

        // Genuine box drawing is not half of anything either.
        let mut carry = Vec::new();
        let out = Encoding::Cp850Doubled.decode_chunk("┌──┐".as_bytes(), &mut carry);
        assert!(carry.is_empty());
        assert_eq!(String::from_utf8_lossy(&out), "┌──┐");
    }

    #[test]
    fn escape_sequences_survive_transcoding() {
        // Decoding must not disturb CSI bytes, which are ASCII everywhere.
        let bytes = b"\x1b[2J\x1b[1;1Hol\xa0";
        let out = Encoding::Cp850.decode(bytes);
        assert!(out.starts_with(b"\x1b[2J\x1b[1;1Hol"));
    }
}
