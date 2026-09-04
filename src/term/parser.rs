//! VT/ANSI escape-sequence handling.
//!
//! `vte` does the byte-level state machine (UTF-8 decoding, CSI/OSC/DCS
//! framing); this module maps the resulting events onto [`Grid`] operations.
//! Anything unrecognised is ignored rather than panicking — IRIS and the
//! terminfo it targets emit a long tail of sequences we do not need, and a
//! terminal that dies on an unknown escape is useless.

use vte::{Params, Perform};

use super::cell::{Attrs, Color};
use super::grid::Grid;

/// Wraps a [`Grid`] so it can be handed to `vte::Parser::advance`.
pub struct Performer<'a> {
    pub grid: &'a mut Grid,
    /// Bytes the remote side asked us to send back (device status reports).
    /// Drained by the session after each parse batch.
    pub replies: Vec<u8>,
}

impl<'a> Performer<'a> {
    pub fn new(grid: &'a mut Grid) -> Self {
        Performer {
            grid,
            replies: Vec::new(),
        }
    }
}

/// First parameter, defaulting when absent or zero — the near-universal
/// convention for CSI movement/count parameters.
fn param(params: &Params, index: usize, default: u16) -> u16 {
    match params.iter().nth(index).and_then(|p| p.first().copied()) {
        Some(0) | None => default,
        Some(v) => v,
    }
}

/// Like [`param`] but treats an explicit 0 as meaningful (ED/EL modes).
fn param_raw(params: &Params, index: usize, default: u16) -> u16 {
    params
        .iter()
        .nth(index)
        .and_then(|p| p.first().copied())
        .unwrap_or(default)
}

impl Perform for Performer<'_> {
    fn print(&mut self, c: char) {
        self.grid.print(c);
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            0x07 => {} // BEL — deliberately silent; IRIS rings it often.
            0x08 => self.grid.backspace(),
            0x09 => self.grid.tab(),
            0x0a..=0x0c => self.grid.line_feed(),
            0x0d => self.grid.carriage_return(),
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        // `?` intermediate marks the DEC private modes (SM/RM).
        let private = intermediates.first() == Some(&b'?');

        match action {
            'A' => self.grid.move_up(param(params, 0, 1) as usize),
            'B' | 'e' => self.grid.move_down(param(params, 0, 1) as usize),
            'C' | 'a' => self.grid.move_right(param(params, 0, 1) as usize),
            'D' => self.grid.move_left(param(params, 0, 1) as usize),
            'E' => {
                self.grid.move_down(param(params, 0, 1) as usize);
                self.grid.carriage_return();
            }
            'F' => {
                self.grid.move_up(param(params, 0, 1) as usize);
                self.grid.carriage_return();
            }
            'G' | '`' => {
                let col = param(params, 0, 1).saturating_sub(1) as usize;
                let row = self.grid.cursor.row;
                self.grid.set_cursor(row, col);
            }
            'd' => {
                let row = param(params, 0, 1).saturating_sub(1) as usize;
                let col = self.grid.cursor.col;
                self.grid.set_cursor(row, col);
            }
            'H' | 'f' => {
                let row = param(params, 0, 1).saturating_sub(1) as usize;
                let col = param(params, 1, 1).saturating_sub(1) as usize;
                self.grid.set_cursor(row, col);
            }
            'J' => self.grid.erase_in_display(param_raw(params, 0, 0)),
            'K' => self.grid.erase_in_line(param_raw(params, 0, 0)),
            'L' => self.grid.insert_lines(param(params, 0, 1) as usize),
            'M' => self.grid.delete_lines(param(params, 0, 1) as usize),
            'P' => self.grid.delete_chars(param(params, 0, 1) as usize),
            'X' => self.grid.erase_chars(param(params, 0, 1) as usize),
            '@' => self.grid.insert_chars(param(params, 0, 1) as usize),
            'S' => self.grid.scroll_up(param(params, 0, 1) as usize),
            'T' => self.grid.scroll_down(param(params, 0, 1) as usize),
            'm' => self.sgr(params),
            'r' => {
                // DECSTBM. Omitted bottom means "to the last row".
                let top = param(params, 0, 1).saturating_sub(1) as usize;
                let bottom = param(params, 1, self.grid.rows as u16).saturating_sub(1) as usize;
                self.grid.set_scroll_region(top, bottom);
            }
            's' => self.grid.save_cursor(),
            'u' => self.grid.restore_cursor(),
            'h' if private => self.set_dec_mode(params, true),
            'l' if private => self.set_dec_mode(params, false),
            'h' => self.set_ansi_mode(params, true),
            'l' => self.set_ansi_mode(params, false),
            'n' => {
                // DSR. 5 = "are you ok", 6 = report cursor position. IRIS uses
                // 6 to discover where it is after an ambiguous repaint, so a
                // missing reply leaves full-screen routines mispositioned.
                match param_raw(params, 0, 0) {
                    5 => self.replies.extend_from_slice(b"\x1b[0n"),
                    6 => {
                        let reply = format!(
                            "\x1b[{};{}R",
                            self.grid.cursor.row + 1,
                            self.grid.cursor.col + 1
                        );
                        self.replies.extend_from_slice(reply.as_bytes());
                    }
                    _ => {}
                }
            }
            'c' => {
                // DA — identify as a VT102, which every IRIS terminfo handles.
                self.replies.extend_from_slice(b"\x1b[?6c");
            }
            _ => {}
        }
    }

    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        if !intermediates.is_empty() {
            // Charset selection (ESC ( B and friends) — we are always UTF-8.
            return;
        }
        match byte {
            b'7' => self.grid.save_cursor(),
            b'8' => self.grid.restore_cursor(),
            b'D' => self.grid.line_feed(),
            // DECKPAM / DECKPNM. terminfo's `smkx` sends this together with
            // DECCKM (`ESC [ ? 1 h ESC =`), and either half on its own is the
            // far side asking for the application spelling of the arrow keys.
            b'=' => self.grid.app_cursor_keys = true,
            b'>' => self.grid.app_cursor_keys = false,
            b'E' => {
                self.grid.line_feed();
                self.grid.carriage_return();
            }
            b'M' => self.grid.reverse_line_feed(),
            b'c' => self.grid.reset(),
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        // OSC 0 (icon + title) and OSC 2 (title) both name the window.
        let Some(kind) = params.first() else { return };
        if matches!(*kind, b"0" | b"2") {
            if let Some(text) = params.get(1) {
                self.grid.title = Some(String::from_utf8_lossy(text).into_owned());
            }
        }
    }

    fn hook(&mut self, _: &Params, _: &[u8], _: bool, _: char) {}
    fn put(&mut self, _: u8) {}
    fn unhook(&mut self) {}
}

impl Performer<'_> {
    /// DEC private mode set/reset. Only the modes that change what we draw are
    /// honoured; the rest (bracketed paste, mouse reporting) are accepted
    /// silently so the remote side does not retry.
    /// SM/RM without the `?` intermediate: the ANSI modes.
    ///
    /// Only IRM (4) is of any interest, and only as a display hint - see
    /// [`crate::term::Grid::insert_mode`]. The insert *behaviour* is
    /// deliberately not implemented: IRIS does not use IRM to edit, so shifting
    /// characters on its behalf would only ever corrupt a line.
    fn set_ansi_mode(&mut self, params: &Params, enable: bool) {
        for p in params.iter() {
            if p.first().copied() == Some(4) {
                self.grid.insert_mode = enable;
                self.grid.touch();
            }
        }
    }

    fn set_dec_mode(&mut self, params: &Params, enable: bool) {
        for p in params.iter() {
            // DECTCEM (25) is the only private mode that changes what we draw;
            // DECCKM (1) changes what we *send*. The rest — bracketed paste,
            // mouse reporting, alt-screen — are accepted silently so the
            // remote side does not keep retrying.
            match p.first().copied() {
                Some(25) => {
                    self.grid.cursor.visible = enable;
                    self.grid.touch();
                }
                Some(1) => self.grid.app_cursor_keys = enable,
                _ => {}
            }
        }
    }

    /// SGR — select graphic rendition. Walks the parameter list because
    /// 38/48 consume following parameters for extended colour.
    fn sgr(&mut self, params: &Params) {
        let flat: Vec<u16> = params.iter().flat_map(|p| p.iter().copied()).collect();
        if flat.is_empty() {
            self.grid.pen.reset();
            self.grid.touch();
            return;
        }

        let mut i = 0;
        while i < flat.len() {
            let pen = &mut self.grid.pen;
            match flat[i] {
                0 => pen.reset(),
                1 => pen.attrs.insert(Attrs::BOLD),
                2 => pen.attrs.insert(Attrs::DIM),
                3 => pen.attrs.insert(Attrs::ITALIC),
                4 => pen.attrs.insert(Attrs::UNDERLINE),
                5 | 6 => pen.attrs.insert(Attrs::BLINK),
                7 => pen.attrs.insert(Attrs::REVERSE),
                8 => pen.attrs.insert(Attrs::HIDDEN),
                9 => pen.attrs.insert(Attrs::STRIKE),
                21 | 22 => {
                    pen.attrs.remove(Attrs::BOLD);
                    pen.attrs.remove(Attrs::DIM);
                }
                23 => pen.attrs.remove(Attrs::ITALIC),
                24 => pen.attrs.remove(Attrs::UNDERLINE),
                25 => pen.attrs.remove(Attrs::BLINK),
                27 => pen.attrs.remove(Attrs::REVERSE),
                28 => pen.attrs.remove(Attrs::HIDDEN),
                29 => pen.attrs.remove(Attrs::STRIKE),
                n @ 30..=37 => pen.fg = Color::Indexed((n - 30) as u8),
                38 => {
                    if let Some((color, consumed)) = parse_extended_color(&flat[i..]) {
                        pen.fg = color;
                        i += consumed;
                        continue;
                    }
                }
                39 => pen.fg = Color::Default,
                n @ 40..=47 => pen.bg = Color::Indexed((n - 40) as u8),
                48 => {
                    if let Some((color, consumed)) = parse_extended_color(&flat[i..]) {
                        pen.bg = color;
                        i += consumed;
                        continue;
                    }
                }
                49 => pen.bg = Color::Default,
                // Bright variants, as emitted by 16-colour terminfo.
                n @ 90..=97 => pen.fg = Color::Indexed((n - 90 + 8) as u8),
                n @ 100..=107 => pen.bg = Color::Indexed((n - 100 + 8) as u8),
                _ => {}
            }
            i += 1;
        }
        self.grid.touch();
    }
}

/// Parses `38;5;N` (256-colour) or `38;2;R;G;B` (truecolour) starting at the
/// 38/48 selector. Returns the colour and how many parameters it consumed.
fn parse_extended_color(rest: &[u16]) -> Option<(Color, usize)> {
    match rest.get(1)? {
        5 => Some((Color::Indexed(*rest.get(2)? as u8), 3)),
        2 => {
            let r = *rest.get(2)? as u8;
            let g = *rest.get(3)? as u8;
            let b = *rest.get(4)? as u8;
            Some((Color::Rgb(r, g, b), 5))
        }
        _ => None,
    }
}

/// Feeds bytes through the parser into the grid, returning any bytes that must
/// be written back to the PTY (cursor-position and device-attribute replies).
pub fn advance(parser: &mut vte::Parser, grid: &mut Grid, bytes: &[u8]) -> Vec<u8> {
    let mut performer = Performer::new(grid);
    for &byte in bytes {
        parser.advance(&mut performer, byte);
    }
    performer.replies
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Drives a fresh grid with the given bytes and hands it back for assertions.
    fn run(cols: usize, rows: usize, input: &[u8]) -> Grid {
        let mut grid = Grid::new(cols, rows, 100);
        let mut parser = vte::Parser::new();
        advance(&mut parser, &mut grid, input);
        grid
    }

    /// terminfo's `smkx` is `ESC [ ? 1 h ESC =`, and it is the far side saying
    /// it wants `ESC O A` back rather than `ESC [ A`. IRIS 2023 sends it, which
    /// is why the arrow keys stopped moving its cursor.
    #[test]
    fn application_cursor_keys_are_tracked() {
        assert!(run(20, 5, b"\x1b[?1h").app_cursor_keys);
        assert!(!run(20, 5, b"\x1b[?1h\x1b[?1l").app_cursor_keys);
        // Either half of the pair on its own means the same thing.
        assert!(run(20, 5, b"\x1b=").app_cursor_keys);
        assert!(!run(20, 5, b"\x1b=\x1b>").app_cursor_keys);
        // A reset is the far side forgetting its own modes.
        assert!(!run(20, 5, b"\x1b[?1h\x1bc").app_cursor_keys);
        // And the mode nothing here reads is still accepted quietly.
        assert!(!run(20, 5, b"\x1b[?2004h").app_cursor_keys);
    }

    #[test]
    fn plain_text_lands_on_the_first_row() {
        let grid = run(20, 5, b"USER>");
        assert_eq!(grid.screen[0].to_text(), "USER>");
        assert_eq!(grid.cursor.col, 5);
    }

    #[test]
    fn cup_is_one_based() {
        let grid = run(20, 5, b"\x1b[3;7HX");
        assert_eq!(grid.cursor.row, 2);
        assert_eq!(grid.screen[2].cells[6].ch, 'X');
    }

    #[test]
    fn wrap_is_deferred_until_the_next_character() {
        // Exactly `cols` characters must not scroll; the 5th char sits in the
        // last column with the cursor still on row 0.
        let grid = run(5, 3, b"abcde");
        assert_eq!(grid.cursor.row, 0);
        assert_eq!(grid.screen[0].to_text(), "abcde");

        let grid = run(5, 3, b"abcdef");
        assert_eq!(grid.cursor.row, 1);
        assert_eq!(grid.screen[1].to_text(), "f");
        assert!(grid.screen[0].wrapped);
    }

    #[test]
    fn ed_modes_clear_the_right_regions() {
        // Mode 0: cursor to end.
        let grid = run(10, 3, b"aaaa\x1b[2;1Hbbbb\x1b[1;3H\x1b[0J");
        assert_eq!(grid.screen[0].to_text(), "aa");
        assert_eq!(grid.screen[1].to_text(), "");

        // Mode 2: everything.
        let grid = run(10, 3, b"aaaa\x1b[2;1Hbbbb\x1b[2J");
        assert_eq!(grid.screen[0].to_text(), "");
        assert_eq!(grid.screen[1].to_text(), "");
    }

    /// The regression behind "`W #` loses everything", byte for byte as a live
    /// IRIS sends it: home, then erase-to-end-of-line and a newline for every
    /// row, with the new prompt printed straight over one of them. Not an ED
    /// sequence in sight, so the transcript survives only if the clear is
    /// recognised while it is being carried out.
    #[test]
    fn the_clear_iris_actually_sends_keeps_the_screen_in_the_scrollback() {
        let mut grid = Grid::new(20, 4, 100);
        let mut parser = vte::Parser::new();
        advance(&mut parser, &mut grid, b"one\r\ntwo\r\nthree\r\nUSER>W #");
        assert_eq!(grid.screen[0].to_text(), "one");

        advance(
            &mut parser,
            &mut grid,
            b"\x1b[?25l\x1b[H\x1b[K\r\nUSER>\x1b[K\r\n\x1b[K\r\n\x1b[K\x1b[2;7H\x1b[?25h",
        );

        let history: Vec<String> = grid.scrollback.iter().map(|r| r.to_text()).collect();
        assert_eq!(
            history,
            vec![
                "one".to_string(),
                "two".to_string(),
                "three".to_string(),
                "USER>W #".to_string(),
            ],
            "the cleared screen never reached the scrollback"
        );
        assert_eq!(grid.screen[0].to_text(), "");
        assert_eq!(grid.screen[1].to_text(), "USER>");
    }

    /// A Windows pseudoconsole answers every resize by repainting the screen,
    /// and its repaint is character for character the shape of the `W #` above:
    /// hide the cursor, home, then erase-to-end-of-line and a newline for every
    /// row, with the content written back on the way down.
    ///
    /// So it used to be filed into the transcript as a clear - and the repaint
    /// then landed underneath the copy it had just archived. Dragging the window
    /// taller duplicated everything on screen, once per resize. The bytes here
    /// are the ones a real pseudoconsole sent, captured on a resize from 24 rows
    /// to 40.
    #[test]
    fn the_repaint_a_pseudoconsole_sends_on_resize_is_not_a_clear() {
        let mut grid = Grid::new(20, 4, 100);
        let mut parser = vte::Parser::new();
        advance(&mut parser, &mut grid, b"one\r\ntwo\r\nUSER>");
        assert!(grid.scrollback.is_empty());

        // The resize itself, and then what comes back because of it.
        grid.resize(20, 6);
        advance(
            &mut parser,
            &mut grid,
            b"\x1b[?25l\x1b[H\x1b[Kone\x1b[K\r\ntwo\x1b[K\r\nUSER>\x1b[K\r\n\x1b[K\r\n\x1b[K\r\n\x1b[K\x1b[3;6H\x1b[?25h",
        );

        assert!(
            grid.scrollback.is_empty(),
            "the repaint was filed as a clear: {:?}",
            grid.scrollback
                .iter()
                .map(|r| r.to_text())
                .collect::<Vec<_>>()
        );
        let screen: Vec<String> = grid.screen.iter().map(|r| r.to_text()).collect();
        assert_eq!(
            screen,
            vec![
                "one".to_string(),
                "two".to_string(),
                "USER>".to_string(),
                String::new(),
                String::new(),
                String::new(),
            ],
            "the screen should hold one copy of itself"
        );
    }

    /// And the window it is read in has to lapse, or a resize on a connection
    /// that does not repaint - Telnet, where the far side is told the new size
    /// and says nothing back - would leave the next real clear-screen unable to
    /// file its screen away.
    #[test]
    fn a_clear_long_after_a_resize_still_files_the_screen_away() {
        let mut grid = Grid::new(20, 4, 100);
        let mut parser = vte::Parser::new();
        grid.resize(20, 4);
        advance(&mut parser, &mut grid, b"one\r\ntwo\r\nthree\r\nUSER>W #");

        // Past the window the repaint would have arrived in. A real one comes
        // back in the same read as the resize goes out, so this is the only
        // place the wait is ever paid.
        std::thread::sleep(
            crate::term::grid::REPAINT_WINDOW + std::time::Duration::from_millis(50),
        );
        advance(
            &mut parser,
            &mut grid,
            b"\x1b[?25l\x1b[H\x1b[K\r\nUSER>\x1b[K\r\n\x1b[K\r\n\x1b[K\x1b[2;7H\x1b[?25h",
        );

        let history: Vec<String> = grid.scrollback.iter().map(|r| r.to_text()).collect();
        assert_eq!(
            history,
            vec![
                "one".to_string(),
                "two".to_string(),
                "three".to_string(),
                "USER>W #".to_string(),
            ],
            "a clear that is not a resize repaint must still be archived"
        );
    }

    /// Ctrl+Delete asks IRIS for the clear, because only IRIS can reset its own
    /// idea of where the cursor is - so the clear that comes back must drop the
    /// history rather than archive it, echoed command and all.
    #[test]
    fn a_requested_clear_purges_the_history_instead_of_filing_it() {
        let mut grid = Grid::new(20, 4, 100);
        let mut parser = vte::Parser::new();
        advance(
            &mut parser,
            &mut grid,
            b"one\r\ntwo\r\nthree\r\nfour\r\nUSER>",
        );
        assert!(!grid.scrollback.is_empty(), "nothing to purge");

        // What the gesture does: ask IRIS to clear, and arrange for the clear
        // that comes back to drop the history rather than add to it.
        grid.purge_history_on_next_clear();
        // IRIS echoes the command it was sent, then clears the screen.
        advance(
            &mut parser,
            &mut grid,
            b"W #\x1b[H\x1b[K\r\nUSER>\x1b[K\r\n\x1b[K\r\n\x1b[K\x1b[2;7H",
        );

        assert!(
            grid.scrollback.is_empty(),
            "the requested clear left something behind: {:?}",
            grid.scrollback
                .iter()
                .map(|r| r.to_text())
                .collect::<Vec<_>>()
        );
        // And the prompt is back at the top, which is the whole reason the
        // clear is asked of IRIS rather than done here.
        assert_eq!(grid.screen[1].to_text(), "USER>");
        assert_eq!(grid.cursor.row, 1);
    }

    /// A purge that is armed and never used must not ambush the next ordinary
    /// clear-screen.
    #[test]
    fn a_cancelled_purge_leaves_the_next_clear_alone() {
        let mut grid = Grid::new(20, 4, 100);
        let mut parser = vte::Parser::new();
        advance(&mut parser, &mut grid, b"one\r\ntwo\r\nthree\r\nUSER>W #");

        grid.purge_history_on_next_clear();
        grid.cancel_purge();
        advance(
            &mut parser,
            &mut grid,
            b"\x1b[H\x1b[K\r\nUSER>\x1b[K\r\n\x1b[K\r\n\x1b[K\x1b[2;7H",
        );

        assert_eq!(grid.scrollback.len(), 4, "the clear archived nothing");
    }

    /// The other half of the deal: an app repainting its screen from the top
    /// must not push a copy of the old one into the history every time.
    #[test]
    fn a_repaint_that_jumps_around_files_nothing() {
        let mut grid = Grid::new(20, 4, 100);
        let mut parser = vte::Parser::new();
        advance(&mut parser, &mut grid, b"a\r\nb\r\nc\r\nd");
        // Home, redraw the top line, then jump back up the screen - which is
        // what a form does and a clear never does.
        advance(
            &mut parser,
            &mut grid,
            b"\x1b[H\x1b[KMenu\x1b[4;1H\x1b[Kfooter\x1b[2;1H\x1b[Kbody",
        );
        assert!(
            grid.scrollback.is_empty(),
            "a repaint was mistaken for a clear: {:?}",
            grid.scrollback
                .iter()
                .map(|r| r.to_text())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn el_mode_1_clears_through_the_cursor_inclusive() {
        let grid = run(10, 2, b"abcdef\x1b[1;4H\x1b[1K");
        assert_eq!(grid.screen[0].to_text(), "    ef");
    }

    #[test]
    fn sgr_sets_and_resets_attributes() {
        let grid = run(10, 2, b"\x1b[1;31mR\x1b[0mN");
        assert_eq!(grid.screen[0].cells[0].fg, Color::Indexed(1));
        assert!(grid.screen[0].cells[0].attrs.contains(Attrs::BOLD));
        assert_eq!(grid.screen[0].cells[1].fg, Color::Default);
        assert!(grid.screen[0].cells[1].attrs.is_empty());
    }

    #[test]
    fn sgr_truecolour_consumes_its_parameters() {
        // The trailing 1 must still be read as BOLD, not swallowed by 38;2.
        let grid = run(10, 2, b"\x1b[38;2;10;20;30;1mX");
        assert_eq!(grid.screen[0].cells[0].fg, Color::Rgb(10, 20, 30));
        assert!(grid.screen[0].cells[0].attrs.contains(Attrs::BOLD));
    }

    #[test]
    fn scroll_region_confines_line_feeds() {
        // Region rows 2..4 (1-based). Filling past its bottom scrolls only
        // inside it, leaving row 1 untouched.
        let mut grid = Grid::new(10, 5, 100);
        let mut parser = vte::Parser::new();
        advance(&mut parser, &mut grid, b"top\x1b[2;4r\x1b[2;1H");
        advance(&mut parser, &mut grid, b"a\nb\nc\nd");

        assert_eq!(grid.screen[0].to_text(), "top");
        assert_eq!(grid.screen[3].to_text(), "   d");
        // An inner-region scroll must not reach scrollback.
        assert!(grid.scrollback.is_empty());
    }

    #[test]
    fn full_screen_scroll_feeds_scrollback() {
        let mut grid = Grid::new(10, 2, 100);
        let mut parser = vte::Parser::new();
        advance(&mut parser, &mut grid, b"one\r\ntwo\r\nthree");
        assert_eq!(grid.scrollback.len(), 1);
        assert_eq!(grid.scrollback[0].to_text(), "one");
        assert_eq!(grid.screen[1].to_text(), "three");
    }

    #[test]
    fn dsr_6_reports_the_cursor_position() {
        let mut grid = Grid::new(20, 5, 100);
        let mut parser = vte::Parser::new();
        let replies = advance(&mut parser, &mut grid, b"\x1b[3;7H\x1b[6n");
        assert_eq!(replies, b"\x1b[3;7R");
    }

    /// IRM is the standard way a host announces insert mode. It is tracked even
    /// though the insert *behaviour* is not implemented, because it is the
    /// authoritative answer when it does arrive.
    #[test]
    fn irm_sets_and_clears_insert_mode() {
        let grid = run(10, 2, b"\x1b[4h");
        assert!(grid.insert_mode);
        let grid = run(10, 2, b"\x1b[4h\x1b[4l");
        assert!(!grid.insert_mode);
    }

    /// `ESC [ ? 4 h` is DECSCLM, a different mode entirely, and must not be
    /// mistaken for IRM.
    #[test]
    fn the_private_form_of_mode_four_is_not_irm() {
        let grid = run(10, 2, b"\x1b[?4h");
        assert!(!grid.insert_mode);
    }

    #[test]
    fn dectcem_toggles_cursor_visibility() {
        let grid = run(10, 2, b"\x1b[?25l");
        assert!(!grid.cursor.visible);
        let grid = run(10, 2, b"\x1b[?25l\x1b[?25h");
        assert!(grid.cursor.visible);
    }

    #[test]
    fn osc_2_sets_the_title() {
        let grid = run(10, 2, b"\x1b]2;IRIS USER\x07");
        assert_eq!(grid.title.as_deref(), Some("IRIS USER"));
    }

    #[test]
    fn unknown_sequences_are_ignored_not_fatal() {
        let grid = run(10, 2, b"\x1b[>4;2m\x1b[?2004hok");
        assert_eq!(grid.screen[0].to_text(), "ok");
    }

    #[test]
    fn growing_taller_pulls_lines_back_from_scrollback() {
        let mut grid = Grid::new(10, 2, 100);
        let mut parser = vte::Parser::new();
        advance(&mut parser, &mut grid, b"one\r\ntwo\r\nthree");
        assert_eq!(grid.scrollback.len(), 1);

        grid.resize(10, 3);
        assert!(grid.scrollback.is_empty());
        assert_eq!(grid.screen[0].to_text(), "one");
        assert_eq!(grid.screen[2].to_text(), "three");
    }
}
