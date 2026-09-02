//! Where the line being typed starts and ends, when IRIS is sitting at a
//! prompt.
//!
//! IRIS does the line editing, not the terminal: what is on screen is the only
//! record of what has been typed so far, and IRIS never reports its read
//! buffer. So the gestures a terminal is expected to provide — Home, End,
//! clicking to put the cursor somewhere, recalling an earlier command — are
//! built by reading the row the cursor is on and then sending the arrow keys
//! and rubouts that get IRIS's own cursor where it should be.
//!
//! The prompt is what makes that safe. A row with no IRIS prompt on it is not
//! a command line — a full-screen routine such as `^%G` paints wherever it
//! likes — and every function here returns `None` for one, which is what lets
//! those keys fall through to IRIS untouched.

use crate::term::{syntax, Grid};

/// The command line under the cursor, in columns of the grid.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LineEdit {
    /// First column of typed text: just past the prompt.
    pub start: usize,
    /// Column the cursor is at.
    pub cursor: usize,
    /// Column just past the last character typed.
    pub end: usize,
}

impl LineEdit {
    /// Whether the cursor is past everything typed, which is where a rubout
    /// erases the last character rather than one in the middle.
    pub fn at_end(self) -> bool {
        self.cursor >= self.end
    }

    /// Characters typed since the prompt.
    pub fn len(self) -> usize {
        self.end.saturating_sub(self.start)
    }

    pub fn is_empty(self) -> bool {
        self.len() == 0
    }
}

/// A movement over the command line, in the terms a text field uses.
///
/// Shared by the two things that move over it: the cursor, and the loose end of
/// a selection. Which one a key press drives is the only difference between
/// Ctrl+Left and Ctrl+Shift+Left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Motion {
    Left,
    Right,
    /// Ctrl+Left: to the start of the word before this position.
    WordLeft,
    /// Ctrl+Right: to the start of the word after it.
    WordRight,
    /// Home: the first character typed.
    LineStart,
    /// End: past the last one.
    LineEnd,
}

/// Where `motion` lands, starting from column `from`.
///
/// Always inside `line.start..=line.end`: everything here moves over what has
/// been typed, and the prompt itself is not part of that.
pub fn target(grid: &Grid, line: LineEdit, from: usize, motion: Motion) -> usize {
    let from = from.clamp(line.start, line.end);
    match motion {
        Motion::Left => from.saturating_sub(1).max(line.start),
        Motion::Right => (from + 1).min(line.end),
        Motion::LineStart => line.start,
        Motion::LineEnd => line.end,
        Motion::WordLeft => line.start + previous_word(&text_of(grid, line), from - line.start),
        Motion::WordRight => line.start + next_word(&text_of(grid, line), from - line.start),
    }
}

/// The typed part of the line, as characters.
fn text_of(grid: &Grid, line: LineEdit) -> Vec<char> {
    let Some(row) = grid.screen.get(grid.cursor.row) else {
        return Vec::new();
    };
    let end = line.end.min(row.cells.len());
    let Some(cells) = row.cells.get(line.start..end) else {
        return Vec::new();
    };
    cells.iter().map(|c| c.ch).collect()
}

/// What kind of run a character belongs to.
///
/// Three classes rather than "word or not", which is what makes Ctrl+Right stop
/// between `$SYSTEM` and `.OBJ` in a line of ObjectScript instead of skipping
/// the whole expression - the same thing an editor does with the same keys.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Class {
    Space,
    Word,
    Symbol,
}

fn class(ch: char) -> Class {
    if ch.is_whitespace() {
        Class::Space
    } else if ch.is_alphanumeric() || ch == '_' {
        Class::Word
    } else {
        Class::Symbol
    }
}

/// Start of the run before `at`, blanks in between skipped first: a word
/// boundary never lands in the middle of a gap.
fn previous_word(chars: &[char], at: usize) -> usize {
    let mut i = at.min(chars.len());
    while i > 0 && class(chars[i - 1]) == Class::Space {
        i -= 1;
    }
    let Some(kind) = i.checked_sub(1).map(|prev| class(chars[prev])) else {
        return 0;
    };
    while i > 0 && class(chars[i - 1]) == kind {
        i -= 1;
    }
    i
}

/// Start of the run after `at`: out of the run under the cursor, then over the
/// blanks that follow it.
fn next_word(chars: &[char], at: usize) -> usize {
    let mut i = at.min(chars.len());
    if let Some(kind) = chars.get(i).map(|ch| class(*ch)) {
        if kind != Class::Space {
            while i < chars.len() && class(chars[i]) == kind {
                i += 1;
            }
        }
    }
    while i < chars.len() && class(chars[i]) == Class::Space {
        i += 1;
    }
    i
}

/// The run of like characters around `at`, as a half-open column range.
///
/// One run is one class: a word, a stretch of blanks, or a stretch of symbols.
/// This is what a double-click selects, and it shares `class` with the Ctrl+
/// arrow motions on purpose - a double-click and a Ctrl+Right must agree on
/// where `$SYSTEM` ends and `.OBJ` begins, or the same line reads as two
/// different sets of words depending on which hand you used.
///
/// `None` when `at` is past the end of `chars`, which is a click on nothing.
pub fn word_bounds(chars: &[char], at: usize) -> Option<(usize, usize)> {
    let kind = class(*chars.get(at)?);
    let mut start = at;
    while start > 0 && class(chars[start - 1]) == kind {
        start -= 1;
    }
    let mut end = at + 1;
    while end < chars.len() && class(chars[end]) == kind {
        end += 1;
    }
    Some((start, end))
}

/// The command line the cursor is on, or `None` when it is not on one.
pub fn current(grid: &Grid) -> Option<LineEdit> {
    let row = grid.screen.get(grid.cursor.row)?;
    let start = syntax::prompt_end_of(&row.cells)?;

    let cursor = grid.cursor.col;
    if cursor < start {
        // The cursor is in the prompt itself, so IRIS is not reading a line
        // here - it is painting something.
        return None;
    }
    // A trailing space is blank as far as the row is concerned but is still
    // something the user typed, so the cursor extends the line.
    let end = row.used_width().max(cursor);
    Some(LineEdit { start, cursor, end })
}

/// Namespace named by the prompt the cursor is on, if there is one.
///
/// `USER>` is the namespace on its own, and a transaction or an instance
/// prefixes it - `TL1:USER>`, `IRIS:USER>` - so the last colon-separated piece
/// is the namespace in every spelling of the prompt IRIS uses. `None` off a
/// prompt row, which is what keeps a full-screen routine from being read as
/// one.
pub fn namespace(grid: &Grid) -> Option<String> {
    let row = grid.screen.get(grid.cursor.row)?;
    let end = syntax::prompt_end_of(&row.cells)?;
    // `end` is just past the `>`, which is not part of the name.
    let text: String = row
        .cells
        .get(..end.saturating_sub(1))?
        .iter()
        .map(|c| c.ch)
        .collect();
    let name = text.rsplit(':').next().unwrap_or_default().trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Text typed at the prompt the cursor is on, trailing blanks trimmed.
///
/// This is what a command is recorded from. Reading it back off the screen
/// rather than accumulating keystrokes means a line IRIS recalled itself, or
/// one that arrived by paste, counts exactly like a typed one.
pub fn typed_text(grid: &Grid) -> Option<String> {
    let line = current(grid)?;
    let row = grid.screen.get(grid.cursor.row)?;
    let mut text: String = row
        .cells
        .get(line.start..line.end.min(row.cells.len()))?
        .iter()
        .map(|c| c.ch)
        .collect();
    text.truncate(text.trim_end().len());
    Some(text)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a one-row grid holding `text`, with the cursor at `col`.
    fn grid_with(text: &str, col: usize) -> Grid {
        let mut grid = Grid::new(40, 1, 10);
        for (i, ch) in text.chars().enumerate() {
            grid.screen[0].cells[i].ch = ch;
        }
        grid.cursor.col = col;
        grid
    }

    /// Walks the whole line one Ctrl+Right at a time, from the prompt to the
    /// end, and reports every column it stopped at.
    fn stops_walking_right(text: &str) -> Vec<usize> {
        let grid = grid_with(text, text.chars().count());
        let line = current(&grid).expect("a command line");
        let mut at = line.start;
        let mut stops = Vec::new();
        while at < line.end {
            at = target(&grid, line, at, Motion::WordRight);
            stops.push(at - line.start);
        }
        stops
    }

    fn stops_walking_left(text: &str) -> Vec<usize> {
        let grid = grid_with(text, text.chars().count());
        let line = current(&grid).expect("a command line");
        let mut at = line.end;
        let mut stops = Vec::new();
        while at > line.start {
            at = target(&grid, line, at, Motion::WordLeft);
            stops.push(at - line.start);
        }
        stops
    }

    /// Ctrl+Right lands on the start of each following word, as it does in an
    /// editor - and a run of punctuation is a stop of its own, which is what
    /// makes it useful on a line of ObjectScript.
    #[test]
    fn word_right_stops_at_the_start_of_each_run() {
        // `set x = 1`: the words, and the `=` between them.
        assert_eq!(stops_walking_right("USER>set x = 1"), vec![4, 6, 8, 9]);
        // `do ^%CSW1GEN("X")`: `^%`, the routine name, `("`, `X` and `")` are
        // each a stop, which is the point of counting punctuation as a run.
        assert_eq!(
            stops_walking_right("USER>do ^%CSW1GEN(\"X\")"),
            vec![3, 5, 12, 14, 15, 17]
        );
    }

    /// Ctrl+Left lands on the same boundaries, coming the other way.
    #[test]
    fn word_left_stops_at_the_start_of_each_run() {
        assert_eq!(stops_walking_left("USER>set x = 1"), vec![8, 6, 4, 0]);
    }

    /// Trailing and repeated blanks are crossed rather than stopped in.
    #[test]
    fn blanks_are_skipped_rather_than_landed_on() {
        let grid = grid_with("USER>a   b", 10);
        let line = current(&grid).expect("a command line");
        assert_eq!(target(&grid, line, 5, Motion::WordRight), 9, "a -> b");
        assert_eq!(target(&grid, line, 10, Motion::WordLeft), 9);
        assert_eq!(target(&grid, line, 9, Motion::WordLeft), 5, "b -> a");
    }

    /// Nothing may walk off either end of what was typed.
    #[test]
    fn word_motion_stays_inside_the_typed_line() {
        let grid = grid_with("USER>write 1", 12);
        let line = current(&grid).expect("a command line");
        assert_eq!(target(&grid, line, 5, Motion::WordLeft), 5);
        assert_eq!(target(&grid, line, 12, Motion::WordRight), 12);
        // And an empty line has nowhere to go at all.
        let grid = grid_with("USER>", 5);
        let line = current(&grid).expect("a command line");
        for motion in [Motion::WordLeft, Motion::WordRight, Motion::Left] {
            assert_eq!(target(&grid, line, 5, motion), 5, "{motion:?}");
        }
    }

    /// The single-column and whole-line motions share the same clamping.
    #[test]
    fn the_simple_motions_move_one_column_and_stop_at_the_edges() {
        let grid = grid_with("USER>write 1", 8);
        let line = current(&grid).expect("a command line");
        assert_eq!(target(&grid, line, 8, Motion::Left), 7);
        assert_eq!(target(&grid, line, 8, Motion::Right), 9);
        assert_eq!(target(&grid, line, 8, Motion::LineStart), 5);
        assert_eq!(target(&grid, line, 8, Motion::LineEnd), 12);
        assert_eq!(target(&grid, line, 5, Motion::Left), 5);
        assert_eq!(target(&grid, line, 12, Motion::Right), 12);
    }

    #[test]
    fn the_line_starts_just_past_the_prompt() {
        let grid = grid_with("USER>write 1", 12);
        let line = current(&grid).expect("a prompt row is a command line");
        assert_eq!(line.start, 5);
        assert_eq!(line.cursor, 12);
        assert_eq!(line.end, 12);
        assert!(line.at_end());
        assert_eq!(line.len(), 7);
    }

    #[test]
    fn a_transaction_prompt_is_still_a_prompt() {
        let grid = grid_with("TL1:USER>write 1", 16);
        assert_eq!(current(&grid).map(|l| l.start), Some(9));
    }

    /// A full-screen routine paints rows that are not command lines, and its
    /// arrow keys have to reach IRIS untouched.
    #[test]
    fn a_row_without_a_prompt_is_not_a_command_line() {
        assert!(current(&grid_with("Global ^CSW1 selected", 8)).is_none());
        assert!(current(&grid_with("", 0)).is_none());
    }

    /// Between the start of the row and the prompt IRIS is writing, not
    /// reading.
    #[test]
    fn a_cursor_inside_the_prompt_is_not_editing() {
        assert!(current(&grid_with("USER>write 1", 2)).is_none());
    }

    /// The cursor mid-line is the case a rubout must not be used on.
    #[test]
    fn the_cursor_can_sit_before_the_end_of_the_line() {
        let line = current(&grid_with("USER>write 1", 8)).expect("still a command line");
        assert_eq!(line.cursor, 8);
        assert_eq!(line.end, 12);
        assert!(!line.at_end());
    }

    /// A trailing space is part of what was typed even though the row counts
    /// it as blank.
    #[test]
    fn a_trailing_space_counts_as_typed() {
        let line = current(&grid_with("USER>write 1 ", 13)).expect("a command line");
        assert_eq!(line.end, 13);
        assert!(line.at_end());
    }

    /// The tab name can carry the namespace, which is read off the prompt: the
    /// only place the session says which one it is in.
    #[test]
    fn the_namespace_is_read_off_the_prompt() {
        assert_eq!(
            namespace(&grid_with("RDB76-TR>write 1", 16)).as_deref(),
            Some("RDB76-TR")
        );
        assert_eq!(
            namespace(&grid_with("TL1:USER>", 9)).as_deref(),
            Some("USER"),
            "a transaction prefix is not part of the namespace"
        );
        // Not a prompt row, so nothing to read.
        assert_eq!(namespace(&grid_with("Global ^CSW1 selected", 8)), None);
    }

    #[test]
    fn the_typed_text_excludes_the_prompt() {
        assert_eq!(
            typed_text(&grid_with("USER>write 1", 12)).as_deref(),
            Some("write 1")
        );
        assert_eq!(
            typed_text(&grid_with("USER>write 1 ", 13)).as_deref(),
            Some("write 1"),
            "a trailing space must not be recorded"
        );
        assert_eq!(typed_text(&grid_with("USER>", 5)).as_deref(), Some(""));
        assert_eq!(typed_text(&grid_with("no prompt", 3)), None);
    }
}
