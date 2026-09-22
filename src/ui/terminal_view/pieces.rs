//! Recognising the part of a global a pointer or a selection is asking about.
//!
//! A global reaches the screen in three shapes, and this reads all three:
//!
//! * A `zwrite` row: `^NAME(subscripts)="p1^p2^p3"`, key and value on one row.
//! * A `write` of one node: the command line names the key -
//!   `USER>w ^NAME(subscripts)` - and the row under it is the bare value,
//!   unquoted, with nothing on it to say whose value it is.
//! * A `^%G` listing, which prints a key in full only when it has to: a row
//!   whose leading subscripts are the same as the row above's leaves them out
//!   and indents to where the first different one starts, `          3)="..."`.
//!   The missing part is recovered from the rows above.
//!
//! This is a character-level reader of those shapes, not a general parser: it
//! only has to answer "is this one `^`-delimited piece of the value, or one
//! subscript of the key, and if so which and of what global".

use super::Selection;
use crate::term::{syntax, Grid, Row};

/// A single piece of a global's value, selected whole.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PieceSelection {
    /// Without the leading `^`.
    pub global: String,
    /// The subscripts as written, quotes stripped. What decides *which*
    /// class describes this row: one global is mapped by as many classes as
    /// it has subscript shapes - `^FTCL` has thirty-odd - and they are told
    /// apart by how many subscripts there are and what the constant ones
    /// hold.
    pub subscripts: Vec<String>,
    /// 1-based, matching IRIS's own `$piece` numbering over `^`.
    pub piece: usize,
    /// The whole piece's text, not just what happened to be selected inside
    /// it - a formatter needs the whole thing, not a partial run of digits.
    pub piece_text: String,
    /// Where the selection starts inside `piece_text`, in characters.
    ///
    /// A piece can be subdivided again by a delimiter of its own (`;`, `,`),
    /// and only the class's own map says which pieces those are - so the
    /// sub-piece cannot be worked out here. This is what lets it be worked
    /// out later, once the map is known.
    pub offset: usize,
}

/// A single subscript of a global's key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeySelection {
    /// Without the leading `^`.
    pub global: String,
    /// Every subscript of the row, for the same reason
    /// [`PieceSelection::subscripts`] carries them: they are what pick the
    /// class that describes this row out of the several the global has.
    pub subscripts: Vec<String>,
    /// 1-based position in that list - which subscript is being asked about.
    pub position: usize,
    /// That subscript as written, quotes stripped.
    pub text: String,
}

/// What part of a global the pointer is asking about.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GlobalTarget {
    Piece(PieceSelection),
    Key(KeySelection),
}

impl GlobalTarget {
    /// The global, without its `^` - what a lookup is keyed on, whichever
    /// part of the row is being asked about.
    pub fn global(&self) -> &str {
        match self {
            GlobalTarget::Piece(piece) => &piece.global,
            GlobalTarget::Key(key) => &key.global,
        }
    }
}

/// What the selection asks about.
///
/// Three ways of asking, and they answer the same question:
///
/// * The selection sits inside one piece of the value - the ordinary case,
///   and what a double-click on a value produces.
/// * The selection is the two delimiters *around* a piece, and the answer is
///   whatever lies between them. This is the only way to ask about an **empty**
///   piece, which has nothing in it to select: `^^` in the middle, `"^` at the
///   start, `^"` at the end, and the same for a sub-piece's own delimiter.
///   Convenient rather than obscure: a double-click takes the run of like
///   characters under it and `^` is a run of its own, so double-clicking a
///   `^^` already selects exactly the pair.
/// * The selection sits inside one subscript of the key, and the answer is
///   that subscript.
///
/// `None` for a selection spanning more than one line (nothing here is
/// per-line), one that reaches outside both the key and the value, one that
/// crosses a `^` or a subscript boundary, or a row that is none of the shapes
/// the module reads.
pub fn target_at_selection(grid: &Grid, selection: &Selection) -> Option<GlobalTarget> {
    let (line, sel_start, sel_end) = single_line_span(selection)?;
    if sel_start >= sel_end {
        return None;
    }
    target_in(grid, line, sel_start, sel_end)
}

/// What the pointer alone asks about, with nothing selected - the one column
/// under it read as a selection of width one.
///
/// It answers exactly what a selection of that column would, empty pieces
/// included: pointing at either delimiter of a `^^` names the piece between
/// them. See `widened`, which is what makes one column enough.
pub fn target_at_point(grid: &Grid, line: usize, col: usize) -> Option<GlobalTarget> {
    target_in(grid, line, col, col + 1)
}

fn target_in(grid: &Grid, line: usize, from: usize, to: usize) -> Option<GlobalTarget> {
    let (text, shape) = shape_at(grid, line)?;

    // The value first: a subscript span and the value never overlap, so the
    // order is only about which of two `None`s is reached first.
    if let Some(piece) = piece_in(&text, &shape, from, to) {
        return Some(GlobalTarget::Piece(piece));
    }
    key_in(&shape, from, to).map(GlobalTarget::Key)
}

/// The row at `line` taken apart, with its text, whichever of the three shapes
/// in the module's description it is.
///
/// Every test that decides a row is none of them is made on the cells before
/// the row is copied out, not after: on hover this runs every frame the
/// pointer is anywhere in the pane, a row is as wide as `TERMINAL_COLS` lets
/// it be, and most rows are plain output that fails on its first character.
fn shape_at(grid: &Grid, line: usize) -> Option<(Vec<char>, RowShape)> {
    let row = grid.line(line)?;
    let first = row.cells.first()?.ch;
    let under_write = line
        .checked_sub(1)
        .and_then(|above| grid.line(above))
        .and_then(written_reference);
    let has_prompt = syntax::prompt_end_of(&row.cells).is_some();
    if first != '^' && first != ' ' && under_write.is_none() && !has_prompt {
        return None;
    }

    let text = text_of(row);
    let shape = if first == '^' { row_shape(&text) } else { None };
    // A written value can start with `^` itself - an empty first piece - so
    // it is tried whenever the row is not a `zwrite` one, not only when it
    // does not start with `^`. And before an indented row, being the more
    // certain of the two: a command line right above it names exactly this,
    // where an indented row is only a guess until the rows above it agree.
    let shape = shape
        .or_else(|| under_write.and_then(|reference| written_value(reference, &text, has_prompt)))
        .or_else(|| match first {
            ' ' => continued_row(grid, line, &text),
            _ => None,
        })
        .or_else(|| {
            written_reference(row).map(|reference| RowShape {
                global: reference.global,
                subscripts: reference.subscripts,
                key_spans: reference.key_spans,
                value: None,
            })
        })?;
    Some((text, shape))
}

/// The piece of the value the span `from..to` asks about.
///
/// Worked in columns relative to the value, signed, because the column just
/// before the value is one of its bounds and a written value starts at column
/// zero: there `-1` is a column that does not exist and still has to bound the
/// first piece.
fn piece_in(text: &[char], shape: &RowShape, from: usize, to: usize) -> Option<PieceSelection> {
    let (val_from, val_to) = shape.value?;
    // Past the row's last character there is nothing to point at. Checked
    // here because a value that runs to the end of the row - a written one,
    // an unquoted one - has that empty column as its own closing bound.
    if from >= text.len() {
        return None;
    }
    let value = &text[val_from..val_to];
    let relative = |col: usize| col as isize - val_from as isize;
    let (from, to) = widened(value, relative(from), relative(to));
    // What is being asked about, as a half-open range of the value.
    let (asked_from, asked_to) = match between_delimiters(value, from, to) {
        Some(between) => between,
        None => {
            if from < 0 || to > value.len() as isize {
                return None;
            }
            (from as usize, to as usize)
        }
    };

    if value.get(asked_from..asked_to)?.contains(&'^') {
        return None;
    }

    let piece_start = value[..asked_from]
        .iter()
        .rposition(|&c| c == '^')
        .map(|i| i + 1)
        .unwrap_or(0);
    let piece_end = value[asked_from..]
        .iter()
        .position(|&c| c == '^')
        .map(|i| asked_from + i)
        .unwrap_or(value.len());
    let piece = 1 + value[..piece_start].iter().filter(|&&c| c == '^').count();

    Some(PieceSelection {
        global: shape.global.clone(),
        subscripts: shape.subscripts.clone(),
        piece,
        piece_text: value[piece_start..piece_end].iter().collect(),
        offset: asked_from - piece_start,
    })
}

/// The subscript the span `from..to` lies wholly inside.
///
/// Wholly, so a selection running from one subscript into the next names
/// neither - the same rule a selection crossing a `^` follows. A subscript
/// with nothing in it is not reachable this way and has no description worth
/// reaching: it is an empty key, not a documented one.
fn key_in(shape: &RowShape, from: usize, to: usize) -> Option<KeySelection> {
    let at = shape
        .key_spans
        .iter()
        .position(|&(start, end)| from >= start && to <= end && start < end)?;
    Some(KeySelection {
        global: shape.global.clone(),
        subscripts: shape.subscripts.clone(),
        position: at + 1,
        text: shape.subscripts.get(at).cloned().unwrap_or_default(),
    })
}

/// One column on a delimiter, grown to the pair of delimiters around the
/// empty piece beside it. Anything else is handed back untouched.
///
/// An empty piece occupies no columns, so it cannot be pointed at - only the
/// delimiters around it can. Selecting the pair is the gesture for that, and
/// with nothing selected there is no pair to select: this is what lets the
/// same question be asked by pointing, which is the whole of what the
/// hover-only mode has to work with.
///
/// Unambiguous, which a lone delimiter otherwise is not: a `^` with an
/// ordinary piece on each side does not say which of the two is meant and is
/// left alone, but one with another delimiter beside it has an empty piece
/// between them and nothing else it could mean. The side after is tried
/// first, so the two delimiters of one `^^` both name the piece between them.
///
/// In columns relative to the value, like everything `piece_in` hands on.
fn widened(value: &[char], from: isize, to: isize) -> (isize, isize) {
    if to != from + 1 || !is_delimiter(value, from) {
        return (from, to);
    }
    if is_delimiter(value, to) {
        return (from, to + 1);
    }
    if is_delimiter(value, from - 1) {
        return (from - 1, to);
    }
    (from, to)
}

/// What lies between the two delimiters a selection is made of, as a
/// half-open range of the value - or `None` when the selection is not that
/// shape and is an ordinary selection inside a piece.
///
/// The first and last selected columns both have to be delimiters. The
/// value's own bounds count as delimiters, because the first and last pieces
/// have nothing else bounding them.
fn between_delimiters(value: &[char], sel_start: isize, sel_end: isize) -> Option<(usize, usize)> {
    let last = sel_end - 1;
    // Two distinct characters: a lone `^` does not say which of the two pieces
    // it separates is the one being asked about.
    if last <= sel_start {
        return None;
    }
    if !is_delimiter(value, sel_start) || !is_delimiter(value, last) {
        return None;
    }
    // Both in range: a delimiter is never left of `-1` or right of the
    // value's length, and `last` is right of `sel_start`.
    Some(((sel_start + 1) as usize, last as usize))
}

/// Whether column `at` of the value bounds a piece: a `^`, one of the
/// characters a map can subdivide a piece with, or one of the value's own two
/// bounds - the columns just before and just after it.
///
/// The bounds are the quotes of a quoted value. An unquoted one has nothing
/// there, or an `=` there, and they bound it all the same: this is what lets
/// the empty first piece of a written value, which starts at column zero, be
/// pointed at through the `^` that closes it.
///
/// The sub-delimiters are named here rather than taken from the map, which
/// this layer cannot see. Listing the two the ERP actually uses is enough:
/// picking the wrong one only ever widens what is offered as the piece's
/// text, and `crate::features::doc_lookup::describe` splits it by the
/// delimiter the map really declares.
fn is_delimiter(value: &[char], at: isize) -> bool {
    let len = value.len() as isize;
    if at == -1 || at == len {
        return true;
    }
    (0..len).contains(&at) && matches!(value[at as usize], '^' | ';' | ',')
}

/// The selection's line and column span, when it sits on exactly one line and
/// is not empty.
fn single_line_span(selection: &Selection) -> Option<(usize, usize, usize)> {
    let (start, end) = selection.ordered();
    (start.0 == end.0).then_some((start.0, start.1, end.1))
}

/// A row taken apart: which global, what its key holds and where each
/// subscript of it sits on the row, and where its value starts and ends.
struct RowShape {
    global: String,
    subscripts: Vec<String>,
    /// Each subscript's own columns on the row, as a half-open range and in
    /// the same order as `subscripts`. Quotes included, because what is being
    /// pointed at is the text on screen and a quote is part of it.
    ///
    /// Empty for a subscript that is part of the key but not on this row - a
    /// written value's, which are on the command line, or the ones `^%G` left
    /// out - so that nothing can point into it.
    key_spans: Vec<(usize, usize)>,
    /// The value's columns, quotes stripped, or `None` for the command line of
    /// a `write`, which holds a key and no value.
    value: Option<(usize, usize)>,
}

/// A global reference as written, `^NAME(subscripts)`: the global, its
/// subscripts, and where each of them and the reference itself end.
struct Reference {
    global: String,
    subscripts: Vec<String>,
    key_spans: Vec<(usize, usize)>,
    /// The column just past the reference - past its `)`, or past the name
    /// when it has no subscripts.
    end: usize,
}

/// The reference starting at column `at`, or `None` when there is not one.
fn reference_at(text: &[char], at: usize) -> Option<Reference> {
    if text.get(at) != Some(&'^') {
        return None;
    }
    let mut i = at + 1;
    let name_from = i;
    if text.get(i) == Some(&'%') {
        i += 1;
    }
    while text.get(i).is_some_and(|c| c.is_ascii_alphanumeric()) {
        i += 1;
    }
    if i == name_from {
        return None;
    }
    let global: String = text[name_from..i].iter().collect();

    let mut subscripts = Vec::new();
    let mut key_spans = Vec::new();
    if text.get(i) == Some(&'(') {
        let close = skip_balanced_parens(text, i)?;
        for (key, from, to) in split_subscripts(text, i + 1, close.saturating_sub(1)) {
            subscripts.push(key);
            key_spans.push((from, to));
        }
        i = close;
    }
    Some(Reference {
        global,
        subscripts,
        key_spans,
        end: i,
    })
}

/// A `zwrite` row taken apart, or `None` when it does not open with `^NAME`
/// followed, past an optional subscript list, by `=`.
fn row_shape(text: &[char]) -> Option<RowShape> {
    let reference = reference_at(text, 0)?;
    if text.get(reference.end) != Some(&'=') {
        return None;
    }
    Some(RowShape {
        global: reference.global,
        subscripts: reference.subscripts,
        key_spans: reference.key_spans,
        value: Some(unquoted_span(text, reference.end + 1)),
    })
}

/// The node a command line writes, when it writes exactly one: `w ^NAME(...)`
/// or `write ^NAME(...)`, in either case, with nothing after it but the `,!`
/// that ends the line.
///
/// Anything more - two references, an expression, a function around the
/// reference - and the row under it is no longer that node's value alone, so
/// it is refused rather than half-read.
fn written_reference(row: &Row) -> Option<Reference> {
    let start = syntax::prompt_end_of(&row.cells)?;
    let text = text_of(row);
    let blanks = |from: usize| {
        text.get(from..)
            .map_or(0, |rest| rest.iter().take_while(|&&c| c == ' ').count())
    };

    let word_from = start + blanks(start);
    let word_len = text
        .get(word_from..)?
        .iter()
        .take_while(|c| c.is_ascii_alphabetic())
        .count();
    let word: String = text[word_from..word_from + word_len].iter().collect();
    if !word.eq_ignore_ascii_case("w") && !word.eq_ignore_ascii_case("write") {
        return None;
    }
    let after_word = word_from + word_len;
    // A command and its argument are separated by exactly the blank IRIS
    // requires; two would make it an argumentless `write`, which lists every
    // variable instead.
    if text.get(after_word) != Some(&' ') {
        return None;
    }
    let reference = reference_at(&text, after_word + 1)?;

    let mut i = reference.end;
    while text.get(i..i + 2) == Some(&[',', '!'][..]) {
        i += 2;
    }
    // The row is trimmed of trailing blanks, so the reference ends it.
    (i == text.len()).then_some(reference)
}

/// The value a `write` of `reference` printed, which is the whole row.
///
/// `None` when the row is the prompt that follows a write of an empty value
/// rather than a value, or the error IRIS printed instead of one.
fn written_value(reference: Reference, text: &[char], has_prompt: bool) -> Option<RowShape> {
    if has_prompt || is_error(text) {
        return None;
    }
    Some(RowShape {
        key_spans: vec![(0, 0); reference.subscripts.len()],
        global: reference.global,
        subscripts: reference.subscripts,
        value: Some((0, text.len())),
    })
}

/// Whether the row opens with an IRIS error, `<UNDEFINED>` and its kind.
fn is_error(text: &[char]) -> bool {
    let Some(rest) = text.strip_prefix(&['<']) else {
        return false;
    };
    let name = rest.iter().take_while(|c| c.is_ascii_uppercase()).count();
    name > 0 && rest.get(name) == Some(&'>')
}

/// Deep enough for any `^%G` listing a person reads by scrolling, and a bound
/// on what one hovered frame can cost: a row far down a long run of siblings
/// walks past every one of them to reach the row that names its global.
const MAX_ROWS_ABOVE: usize = 5000;

/// An indented `^%G` row, with the subscripts it leaves out recovered from the
/// rows above it.
///
/// `^%G` indents a row to the column its first new subscript would have had,
/// and every row above it is laid out on the same columns - so what is left of
/// that column on the nearest row that has something there is exactly what
/// this one leaves out. That row may be indented too, and then what *it*
/// leaves out comes from further up: the walk narrows the column it still
/// needs as it goes, and ends at the first row that is written in full.
///
/// `None` unless every step of that agrees. A row that is neither kind, or a
/// blank one, is the end of the listing, and a row reached through it would
/// belong to another global - so nothing is guessed across it.
fn continued_row(grid: &Grid, line: usize, text: &[char]) -> Option<RowShape> {
    let own = indented_row(text)?;
    // Collected nearest first, so the chunks come out in reverse order.
    let mut left_out: Vec<Vec<String>> = Vec::new();
    let mut need = own.indent;
    for above in (line.saturating_sub(MAX_ROWS_ABOVE)..line).rev() {
        let row = grid.line(above)?;
        let first = row.cells.iter().position(|c| !c.is_blank())?;
        if first >= need && first > 0 {
            // A sibling, or a row deeper than the one being read: it has
            // nothing left of `need` that is its own.
            continue;
        }
        let row_text = text_of(row);
        if first == 0 {
            let full = row_shape(&row_text)?;
            left_out.push(
                full.subscripts
                    .into_iter()
                    .zip(&full.key_spans)
                    .filter(|&(_, &(from, _))| from < need)
                    .map(|(key, _)| key)
                    .collect(),
            );
            let mut subscripts: Vec<String> = left_out.into_iter().rev().flatten().collect();
            let mut key_spans = vec![(0, 0); subscripts.len()];
            for (key, from, to) in own.keys {
                subscripts.push(key);
                key_spans.push((from, to));
            }
            return Some(RowShape {
                global: full.global,
                subscripts,
                key_spans,
                value: Some(own.value),
            });
        }
        let parent = indented_row(&row_text)?;
        left_out.push(
            parent
                .keys
                .into_iter()
                .filter(|&(_, from, _)| from < need)
                .map(|(key, ..)| key)
                .collect(),
        );
        need = parent.indent;
    }
    None
}

/// A row as `^%G` prints one whose key starts like the row above's: blanks,
/// then the rest of the subscript list and its `)`, then `=` and the value.
struct Indented {
    /// The column the row's first subscript starts at.
    indent: usize,
    /// The subscripts that are on the row, with their columns.
    keys: Vec<(String, usize, usize)>,
    value: (usize, usize),
}

fn indented_row(text: &[char]) -> Option<Indented> {
    let indent = text.iter().position(|&c| c != ' ')?;
    if indent == 0 {
        return None;
    }
    // The list's `(` is on a row above; the column before the indent stands
    // in for it, which is all `skip_balanced_parens` looks at it for.
    let close = skip_balanced_parens(text, indent - 1)?;
    let list_end = close - 1;
    if list_end <= indent || text.get(close) != Some(&'=') {
        return None;
    }
    Some(Indented {
        indent,
        keys: split_subscripts(text, indent, list_end),
        value: unquoted_span(text, close + 1),
    })
}

/// The row's characters, up to its last non-blank one.
fn text_of(row: &Row) -> Vec<char> {
    row.cells[..row.used_width()].iter().map(|c| c.ch).collect()
}

/// The subscript list in `text[from..to]`, split on the commas that separate
/// *this* level - not the ones inside a quoted key or a nested expression.
///
/// Each entry comes back with the columns it occupies on the row, so that a
/// pointer can be matched against it. The columns are of the row, not of the
/// list, because that is what every caller compares against.
///
/// A quoted subscript is handed back unquoted, doubled quotes collapsed, so
/// it compares equal to the literal a class's map declares for it - but its
/// span still covers the quotes, which are on screen like anything else.
fn split_subscripts(text: &[char], from: usize, to: usize) -> Vec<(String, usize, usize)> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut start = from;
    let mut depth = 0usize;
    let mut i = from;
    while i < to {
        match text[i] {
            '"' => {
                let end = string_end(text, i).min(to);
                let inner: String = text[i + 1..end.saturating_sub(1).max(i + 1)]
                    .iter()
                    .collect();
                current.push_str(&inner.replace("\"\"", "\""));
                i = end;
            }
            '(' => {
                depth += 1;
                current.push('(');
                i += 1;
            }
            ')' => {
                depth = depth.saturating_sub(1);
                current.push(')');
                i += 1;
            }
            ',' if depth == 0 => {
                out.push((std::mem::take(&mut current), start, i));
                i += 1;
                start = i;
            }
            c => {
                current.push(c);
                i += 1;
            }
        }
    }
    if to > from {
        out.push((current, start, to));
    }
    out
}

/// Index just past the `)` matching the `(` at `open`, skipping over quoted
/// strings so a `)` or `^` inside one is not mistaken for structure.
fn skip_balanced_parens(text: &[char], open: usize) -> Option<usize> {
    let mut i = open + 1;
    let mut depth = 1usize;
    while depth > 0 {
        match text.get(i)? {
            '"' => i = string_end(text, i),
            '(' => {
                depth += 1;
                i += 1;
            }
            ')' => {
                depth -= 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    Some(i)
}

/// Index just past the closing quote of the string starting at `open`, or the
/// end of `text` for one a device margin cut off before it closed.
fn string_end(text: &[char], open: usize) -> usize {
    let mut i = open + 1;
    while i < text.len() {
        if text[i] == '"' {
            if text.get(i + 1) == Some(&'"') {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    text.len()
}

/// The value's own span, quotes stripped when it has them.
///
/// `zwrite` always quotes a value with a `^` in it - which is every value
/// worth looking at a piece of - but an unquoted one (a bare number, say) is
/// read as itself rather than refused.
fn unquoted_span(text: &[char], from: usize) -> (usize, usize) {
    if text.get(from) == Some(&'"') {
        (from + 1, string_end(text, from).saturating_sub(1))
    } else {
        (from, text.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::terminal_view::Selection;

    fn grid_with(lines: &[&str]) -> Grid {
        let mut grid = Grid::new(80, lines.len().max(1), 100);
        for (r, line) in lines.iter().enumerate() {
            grid.screen[r].set_text(line);
        }
        grid
    }

    /// The selection's answer, when it is a piece of the value. Most of what
    /// is below is about the piece reader, and saying so once keeps every one
    /// of those tests to the assertion it is actually making.
    fn piece_of(grid: &Grid, selection: &Selection) -> Option<PieceSelection> {
        match target_at_selection(grid, selection)? {
            GlobalTarget::Piece(piece) => Some(piece),
            GlobalTarget::Key(key) => panic!("expected a piece, got subscript {}", key.position),
        }
    }

    /// The same for a subscript.
    fn key_of(grid: &Grid, selection: &Selection) -> Option<KeySelection> {
        match target_at_selection(grid, selection)? {
            GlobalTarget::Key(key) => Some(key),
            GlobalTarget::Piece(piece) => panic!("expected a key, got piece {}", piece.piece),
        }
    }

    /// The running example: a real `zwrite` line, one piece selected whole -
    /// the shape a double-click already produces, since `^` is its own
    /// symbol run and the digits are a word run of their own.
    #[test]
    fn a_selected_piece_is_named_and_numbered() {
        let grid = grid_with(&[r#"^CCDU(1,1)="1^350^2^2^64329^67043^^1000000""#]);
        // The value starts at column 12 (`"1^350^...`); "67043" is the 6th
        // piece, columns 28..33.
        let selection = Selection::across(0, 28, 33);
        let piece = piece_of(&grid, &selection).expect("a piece");
        assert_eq!(piece.global, "CCDU");
        assert_eq!(piece.subscripts, vec!["1".to_string(), "1".to_string()]);
        assert_eq!(piece.piece, 6);
        assert_eq!(piece.piece_text, "67043");
    }

    #[test]
    fn the_first_piece_is_found_too() {
        let grid = grid_with(&[r#"^CCDU(1,1)="1^350^2""#]);
        let selection = Selection::across(0, 12, 13);
        let piece = piece_of(&grid, &selection).expect("a piece");
        assert_eq!(piece.piece, 1);
        assert_eq!(piece.piece_text, "1");
    }

    /// Selecting only part of a piece's digits still resolves to the whole
    /// piece - a formatter needs the whole number, not half of it.
    #[test]
    fn a_partial_selection_inside_a_piece_yields_the_whole_piece() {
        let grid = grid_with(&[r#"^CCDU(1,1)="1^350^67043""#]);
        let selection = Selection::across(0, 19, 21); // "70" inside "67043"
        let piece = piece_of(&grid, &selection).expect("a piece");
        assert_eq!(piece.piece_text, "67043");
    }

    /// An empty piece has nothing in it to select, so the way to ask about one
    /// is to select the two delimiters around it. Here piece 2 is empty and
    /// the selection is the `^^`.
    #[test]
    fn selecting_the_two_delimiters_around_an_empty_piece_names_it() {
        let grid = grid_with(&[r#"^CCDU(1,1)="1^^350""#]);
        // The value starts at column 12: `1` at 12, then `^^` at 13 and 14.
        let selection = Selection::across(0, 13, 15);
        let piece = piece_of(&grid, &selection).expect("a piece");
        assert_eq!(piece.piece, 2);
        assert_eq!(piece.piece_text, "");
        assert_eq!(piece.offset, 0);
    }

    /// The first piece is bounded by the opening quote rather than by a `^`,
    /// so the quote has to count as a delimiter too.
    #[test]
    fn selecting_the_quote_and_the_first_caret_names_an_empty_first_piece() {
        let grid = grid_with(&[r#"^CCDU(1,1)="^350^2""#]);
        // The opening quote is at column 11 and the first `^` at 12.
        let selection = Selection::across(0, 11, 13);
        let piece = piece_of(&grid, &selection).expect("a piece");
        assert_eq!(piece.piece, 1);
        assert_eq!(piece.piece_text, "");
    }

    /// And the last by the closing quote.
    #[test]
    fn selecting_the_last_caret_and_the_quote_names_an_empty_last_piece() {
        let grid = grid_with(&[r#"^CCDU(1,1)="1^350^""#]);
        // `1^350^` runs 12..18, so the last `^` is at 17 and the quote at 18.
        let selection = Selection::across(0, 17, 19);
        let piece = piece_of(&grid, &selection).expect("a piece");
        assert_eq!(piece.piece, 3);
        assert_eq!(piece.piece_text, "");
    }

    /// The same gesture one level down: a sub-piece is bounded by the piece's
    /// own `^` on one side and its sub-delimiter on the other. The sub-piece
    /// itself is resolved later, from `offset`, once the map says what the
    /// delimiter is.
    #[test]
    fn selecting_a_caret_and_a_sub_delimiter_names_the_first_sub_piece() {
        let grid = grid_with(&[r#"^CCDU(1,1)="1^12;7^2""#]);
        // The piece `12;7` runs 14..18, so its leading `^` is at 13 and the
        // `;` at 16.
        let selection = Selection::across(0, 13, 17);
        let piece = piece_of(&grid, &selection).expect("a piece");
        assert_eq!(piece.piece, 2);
        assert_eq!(
            piece.piece_text, "12;7",
            "the whole piece, for the map to split"
        );
        assert_eq!(piece.offset, 0, "which puts the pointer in the first run");
    }

    /// Two delimiters with a whole piece between them name that piece, the
    /// same as selecting the piece itself would.
    #[test]
    fn selecting_both_delimiters_of_a_filled_piece_names_that_piece() {
        let grid = grid_with(&[r#"^CCDU(1,1)="1^350^2""#]);
        let selection = Selection::across(0, 13, 18); // `^350^`
        let piece = piece_of(&grid, &selection).expect("a piece");
        assert_eq!(piece.piece, 2);
        assert_eq!(piece.piece_text, "350");
    }

    /// A `^` with an ordinary piece on either side does not say which of the
    /// two it separates is meant.
    #[test]
    fn selecting_a_single_delimiter_between_two_filled_pieces_names_nothing() {
        let grid = grid_with(&[r#"^CCDU(1,1)="1^350""#]);
        // The `^` between `1` and `350`.
        assert_eq!(
            target_at_selection(&grid, &Selection::across(0, 13, 14)),
            None
        );
    }

    /// One with another delimiter beside it does: there is an empty piece
    /// between the two and nothing else it could mean. This is what lets an
    /// empty piece be asked about by pointing at it, which is all the
    /// hover-only mode ever has - it has no selection to work from.
    #[test]
    fn one_delimiter_of_a_pair_names_the_empty_piece_between_them() {
        let grid = grid_with(&[r#"^CCDU(1,1)="1^^350""#]);
        // `1` at 12, then the two `^` at 13 and 14. Either one answers.
        for col in [13, 14] {
            let piece = piece_of(&grid, &Selection::across(0, col, col + 1))
                .unwrap_or_else(|| panic!("a piece at column {col}"));
            assert_eq!((piece.piece, piece.piece_text.as_str()), (2, ""));
        }
    }

    /// The same by pointing, with nothing selected at all - the three shapes
    /// an empty piece comes in: first, middle and last.
    #[test]
    fn pointing_at_a_delimiter_names_an_empty_piece_at_either_end_too() {
        let empty_piece_at =
            |row: &str, col: usize| match target_at_point(&grid_with(&[row]), 0, col) {
                Some(GlobalTarget::Piece(piece)) => (piece.piece, piece.piece_text),
                other => panic!("expected a piece in {row} at {col}, got {other:?}"),
            };
        // `"^350"` - the opening quote at 11 bounds the empty first piece.
        assert_eq!(
            empty_piece_at(r#"^CCDU(1,1)="^350""#, 12),
            (1, String::new())
        );
        // `"1^^350"` - between the two carets.
        assert_eq!(
            empty_piece_at(r#"^CCDU(1,1)="1^^350""#, 13),
            (2, String::new())
        );
        // `"1^350^"` - the closing quote bounds the empty last piece.
        assert_eq!(
            empty_piece_at(r#"^CCDU(1,1)="1^350^""#, 17),
            (3, String::new())
        );
    }

    /// Two delimiters with a `^` between them span more than one piece.
    #[test]
    fn two_delimiters_with_a_piece_boundary_between_them_name_nothing() {
        let grid = grid_with(&[r#"^CCDU(1,1)="1^350^67^2""#]);
        let selection = Selection::across(0, 13, 21); // `^350^67^`
        assert_eq!(target_at_selection(&grid, &selection), None);
    }

    #[test]
    fn a_selection_crossing_a_caret_is_not_one_piece() {
        let grid = grid_with(&[r#"^CCDU(1,1)="1^350^67043""#]);
        // Spans the `^` between "350" and "67043".
        let selection = Selection::across(0, 16, 19);
        assert_eq!(target_at_selection(&grid, &selection), None);
    }

    #[test]
    fn a_row_that_is_not_a_global_assignment_yields_nothing() {
        let grid = grid_with(&["USER>write 1"]);
        let selection = Selection::across(0, 10, 11);
        assert_eq!(target_at_selection(&grid, &selection), None);
    }

    #[test]
    fn a_selection_spanning_two_lines_yields_nothing() {
        let grid = grid_with(&[r#"^CCDU(1,1)="1^2""#, r#"^CCDU(1,2)="1^2""#]);
        let selection = Selection {
            start: (0, 12),
            end: (1, 13),
        };
        assert_eq!(target_at_selection(&grid, &selection), None);
    }

    /// The subscripts may themselves hold a quoted key with parens or carets
    /// in it, which must not be mistaken for the end of the subscript list or
    /// for a piece delimiter.
    #[test]
    fn a_quoted_subscript_does_not_confuse_the_subscript_scan() {
        let grid = grid_with(&[r#"^CCDU(1,"4000164C")="31399^1^99""#]);
        let selection = Selection::across(0, 21, 26);
        let piece = piece_of(&grid, &selection).expect("a piece");
        assert_eq!(piece.global, "CCDU");
        assert_eq!(
            piece.subscripts,
            vec!["1".to_string(), "4000164C".to_string()],
            "a quoted subscript is unquoted, so it compares equal to the              literal a map declares for that position"
        );
        assert_eq!(piece.piece, 1);
        assert_eq!(piece.piece_text, "31399");
    }

    /// Where the selection sits inside the piece is what later resolves a
    /// sub-piece, once the class's map says the piece has a delimiter of its
    /// own. Nothing here can know that yet.
    #[test]
    fn the_offset_within_the_piece_is_reported() {
        let grid = grid_with(&[r#"^CCDU(1,1)="1^350^ABC;DEF""#]);
        // "DEF" - the second run of the sixth column's own `;` split.
        let selection = Selection::across(0, 22, 25);
        let piece = piece_of(&grid, &selection).expect("a piece");
        assert_eq!(piece.piece_text, "ABC;DEF");
        assert_eq!(piece.offset, 4);
    }

    /// The shape that was resolving to the wrong class: six subscripts, two
    /// of them constants that pick the map out of the thirty-odd sharing
    /// this global.
    #[test]
    fn every_subscript_of_a_deep_row_is_captured() {
        let grid = grid_with(&[r#"^FTCL(1,1,3,1,67774,29376)="99^ccs.evandro.ochner^200000""#]);
        let selection = Selection::across(0, 28, 30);
        let piece = piece_of(&grid, &selection).expect("a piece");
        assert_eq!(piece.global, "FTCL");
        assert_eq!(
            piece.subscripts,
            vec!["1", "1", "3", "1", "67774", "29376"]
                .into_iter()
                .map(String::from)
                .collect::<Vec<_>>()
        );
        assert_eq!(piece.piece, 1);
        assert_eq!(piece.piece_text, "99");
    }

    #[test]
    fn an_unquoted_value_is_read_as_itself() {
        let grid = grid_with(&[r#"^CCDU(1,1,1,167236,1,3307,211002)=1000000"#]);
        let selection = Selection::across(0, 34, 41);
        let piece = piece_of(&grid, &selection).expect("a piece");
        assert_eq!(piece.piece, 1);
        assert_eq!(piece.piece_text, "1000000");
    }

    // --- the key ----------------------------------------------------------

    /// A subscript is asked about the same way a piece is, and answers with
    /// its position in the key rather than a piece number.
    #[test]
    fn a_selected_subscript_is_named_and_numbered() {
        let grid = grid_with(&[r#"^CCDU(1,4711)="1^350""#]);
        // `4711` runs 8..12.
        let key = key_of(&grid, &Selection::across(0, 8, 12)).expect("a key");
        assert_eq!(key.global, "CCDU");
        assert_eq!(key.position, 2);
        assert_eq!(key.text, "4711");
        assert_eq!(key.subscripts, vec!["1".to_string(), "4711".to_string()]);
    }

    #[test]
    fn the_first_subscript_is_found_too() {
        let grid = grid_with(&[r#"^CCDU(1,4711)="1^350""#]);
        let key = key_of(&grid, &Selection::across(0, 6, 7)).expect("a key");
        assert_eq!(key.position, 1);
        assert_eq!(key.text, "1");
    }

    /// A double-click inside a quoted key takes the word without its quotes,
    /// and that is still inside the subscript's own span.
    #[test]
    fn a_selection_inside_a_quoted_subscript_names_it() {
        let grid = grid_with(&[r#"^CCDU(1,"4000164C")="31399^1^99""#]);
        let key = key_of(&grid, &Selection::across(0, 9, 17)).expect("a key");
        assert_eq!(key.position, 2);
        assert_eq!(
            key.text, "4000164C",
            "unquoted, so it compares equal to what a map declares"
        );
    }

    /// The same rule a selection crossing a `^` follows: a selection running
    /// from one subscript into the next names neither.
    #[test]
    fn a_selection_crossing_a_comma_names_no_subscript() {
        let grid = grid_with(&[r#"^CCDU(1,4711)="1^350""#]);
        assert_eq!(
            target_at_selection(&grid, &Selection::across(0, 6, 11)),
            None
        );
    }

    /// Everything outside the key and the value - the global's own name, the
    /// `=` - names nothing at all.
    #[test]
    fn the_globals_name_is_not_a_subscript() {
        let grid = grid_with(&[r#"^CCDU(1,4711)="1^350""#]);
        assert_eq!(
            target_at_selection(&grid, &Selection::across(0, 1, 5)),
            None
        );
    }

    // --- hovering with nothing selected -----------------------------------

    /// The hover-only mode: one column under the pointer, and no selection
    /// anywhere. It answers about both halves of the row.
    #[test]
    fn a_bare_pointer_resolves_the_piece_and_the_subscript_under_it() {
        let grid = grid_with(&[r#"^CCDU(1,4711)="1^350^67043""#]);
        match target_at_point(&grid, 0, 9).expect("a key") {
            GlobalTarget::Key(key) => assert_eq!((key.position, key.text.as_str()), (2, "4711")),
            other => panic!("expected a key, got {other:?}"),
        }
        match target_at_point(&grid, 0, 22).expect("a piece") {
            GlobalTarget::Piece(piece) => {
                assert_eq!((piece.piece, piece.piece_text.as_str()), (3, "67043"));
            }
            other => panic!("expected a piece, got {other:?}"),
        }
    }

    /// Past the end of the row there is nothing to describe, and the margin
    /// is mostly what a pointer is over.
    #[test]
    fn a_pointer_past_the_end_of_the_row_resolves_to_nothing() {
        let grid = grid_with(&[r#"^CCDU(1,4711)="1^350""#]);
        assert_eq!(target_at_point(&grid, 0, 60), None);
    }

    /// The piece under a bare pointer, for the tests below that are only
    /// about which piece of which node a row is read as.
    fn piece_at(grid: &Grid, line: usize, col: usize) -> PieceSelection {
        match target_at_point(grid, line, col) {
            Some(GlobalTarget::Piece(piece)) => piece,
            other => panic!("expected a piece at {line}:{col}, got {other:?}"),
        }
    }

    fn strings(keys: &[&str]) -> Vec<String> {
        keys.iter().map(|k| k.to_string()).collect()
    }

    // --- the value of a `write` -------------------------------------------

    /// The row under `w ^NODE` is that node's value, bare: no key, no quotes.
    /// Which node is read off the command line above it.
    #[test]
    fn the_row_under_a_write_of_one_node_is_that_nodes_value() {
        let grid = grid_with(&[
            "RDB81-TR>w ^FTCL(1,1)",
            "350^DESCRICAO CLIENTE 1^RUA PINHEIRO MACHADO",
            "RDB81-TR>",
        ]);
        // `DESCRICAO CLIENTE 1` runs 4..23.
        let piece = piece_at(&grid, 1, 8);
        assert_eq!(piece.global, "FTCL");
        assert_eq!(piece.subscripts, strings(&["1", "1"]));
        assert_eq!(piece.piece, 2);
        assert_eq!(piece.piece_text, "DESCRICAO CLIENTE 1");
        assert_eq!(piece_at(&grid, 1, 0).piece_text, "350");
    }

    /// `write` spelled out, in capitals, and with the `,!` that ends the line
    /// is the same command.
    #[test]
    fn every_spelling_of_the_write_is_read() {
        for command in [
            "USER>write ^CCDU(1,1)",
            "USER>W ^CCDU(1,1)",
            "USER>w ^CCDU(1,1),!",
            "USER> w ^CCDU(1,1)",
        ] {
            let grid = grid_with(&[command, "1^350^2"]);
            let piece = piece_at(&grid, 1, 3);
            assert_eq!(
                (piece.piece, piece.piece_text.as_str()),
                (2, "350"),
                "{command}"
            );
        }
    }

    /// A written value has no quotes to bound its first and last pieces, and
    /// they can still be empty: the `^` that closes one is enough to point at.
    #[test]
    fn the_empty_pieces_at_either_end_of_a_written_value_can_be_pointed_at() {
        let grid = grid_with(&["USER>w ^CCDU(1,1)", "^350^"]);
        let first = piece_at(&grid, 1, 0);
        assert_eq!((first.piece, first.piece_text.as_str()), (1, ""));
        let last = piece_at(&grid, 1, 4);
        assert_eq!((last.piece, last.piece_text.as_str()), (3, ""));
        assert_eq!(piece_at(&grid, 1, 2).piece_text, "350");
        assert_eq!(
            target_at_point(&grid, 1, 5),
            None,
            "past the end of the row"
        );
    }

    /// The command line itself holds the key, and its subscripts answer like
    /// a `zwrite` row's do.
    #[test]
    fn the_subscripts_on_the_write_command_line_are_its_key() {
        let grid = grid_with(&["USER>w ^CCDU(1,4711)", "1^350"]);
        // `4711` runs 15..19.
        match target_at_point(&grid, 0, 16) {
            Some(GlobalTarget::Key(key)) => {
                assert_eq!(key.global, "CCDU");
                assert_eq!((key.position, key.text.as_str()), (2, "4711"));
            }
            other => panic!("expected a key, got {other:?}"),
        }
        assert_eq!(
            target_at_point(&grid, 0, 2),
            None,
            "the prompt is not the key"
        );
    }

    /// Anything but one node alone, and the row under it is not that node's
    /// value - so it names nothing rather than being read as one.
    #[test]
    fn a_write_of_anything_but_one_node_names_nothing() {
        for command in [
            "USER>w ^CCDU(1,1),^CCDU(1,2)",
            "USER>w ^CCDU(1,1)_\"x\"",
            "USER>w $g(^CCDU(1,1))",
            "USER>w x",
            "USER>s x=^CCDU(1,1)",
            "USER>zw ^CCDU(1,1)",
        ] {
            let grid = grid_with(&[command, "1^350^2"]);
            assert_eq!(target_at_point(&grid, 1, 3), None, "{command}");
        }
    }

    /// What follows a write is not always a value: an empty one prints
    /// nothing and the next prompt is right there, and an undefined one
    /// prints an error instead.
    #[test]
    fn the_prompt_or_error_after_a_write_is_not_its_value() {
        let grid = grid_with(&["USER>w ^CCDU(1,1)", "USER>"]);
        assert_eq!(target_at_point(&grid, 1, 1), None);
        let grid = grid_with(&["USER>w ^CCDU(9,9)", "<UNDEFINED> *^CCDU(9,9)"]);
        assert_eq!(target_at_point(&grid, 1, 3), None);
    }

    // --- a `^%G` listing --------------------------------------------------

    /// The listing from the screenshot this was written against, one row of
    /// each shape it produces.
    fn percent_g_listing() -> Grid {
        grid_with(&[
            r#"^FTCL(1,1,2)="4799^teste""#,
            r#"          3)="7899^27852""#,
            r#"^FTCL(1,1,3,1,67774,29330)="99^ccs.evandro.ochner^100000""#,
            r#"                    29356)="99^ccs.evandro.ochner^200000""#,
            r#"                    29376)="99^ccs.evandro.ochner^600000""#,
            r#"^FTCL(1,1,24,1)="67109^53099""#,
            r#"             2)="67108^54906""#,
            r#"             3)="67130^55796""#,
        ])
    }

    /// A row that leaves out the subscripts it shares with the row above is
    /// read with them put back.
    #[test]
    fn an_indented_percent_g_row_gets_back_the_subscripts_it_left_out() {
        let grid = percent_g_listing();
        // `3)="7899^27852"`: the value starts at column 14.
        let piece = piece_at(&grid, 1, 20);
        assert_eq!(piece.global, "FTCL");
        assert_eq!(piece.subscripts, strings(&["1", "1", "3"]));
        assert_eq!((piece.piece, piece.piece_text.as_str()), (2, "27852"));
    }

    /// The row above may be indented too, and then the full row is further up
    /// still, past every sibling in between.
    #[test]
    fn a_run_of_siblings_all_take_their_key_from_the_row_that_opened_it() {
        let grid = percent_g_listing();
        let piece = piece_at(&grid, 4, 28);
        assert_eq!(
            piece.subscripts,
            strings(&["1", "1", "3", "1", "67774", "29376"])
        );
        assert_eq!(piece.piece_text, "99");
        let piece = piece_at(&grid, 7, 18);
        assert_eq!(piece.subscripts, strings(&["1", "1", "24", "3"]));
        assert_eq!(piece.piece_text, "67130");
    }

    /// A row whose first new subscript is further left than the row above's
    /// starts takes less from it, and the rest from further up.
    #[test]
    fn an_indent_left_of_the_row_above_takes_its_key_from_further_up() {
        let grid = grid_with(&[
            r#"^X(1,2,3)="a""#,
            r#"       4)="b""#,
            r#"     5,1)="c""#,
            r#"       2)="d""#,
        ]);
        let subscripts_at = |line: usize| piece_at(&grid, line, 11).subscripts;
        assert_eq!(subscripts_at(1), strings(&["1", "2", "4"]));
        assert_eq!(subscripts_at(2), strings(&["1", "5", "1"]));
        assert_eq!(subscripts_at(3), strings(&["1", "5", "2"]));
    }

    /// The subscripts that are on the row answer with their place in the
    /// whole key; the ones left out are not on screen to point at.
    #[test]
    fn a_visible_subscript_of_an_indented_row_answers_with_its_whole_key_position() {
        let grid = percent_g_listing();
        match target_at_point(&grid, 4, 22) {
            Some(GlobalTarget::Key(key)) => {
                assert_eq!((key.position, key.text.as_str()), (6, "29376"));
                assert_eq!(key.subscripts.len(), 6);
            }
            other => panic!("expected a key, got {other:?}"),
        }
        assert_eq!(target_at_point(&grid, 4, 5), None, "the indent is blank");
    }

    /// An indented row with no listing above it, or with a blank row between
    /// it and one, is not recovered from anything.
    #[test]
    fn an_indented_row_outside_a_listing_names_nothing() {
        let grid = grid_with(&["USER>d ^%G", r#"          3)="7899^27852""#]);
        assert_eq!(target_at_point(&grid, 1, 20), None);
        let grid = grid_with(&[
            r#"^FTCL(1,1,2)="4799^teste""#,
            "",
            r#"          3)="7899^27852""#,
        ]);
        assert_eq!(target_at_point(&grid, 2, 20), None);
    }
}
