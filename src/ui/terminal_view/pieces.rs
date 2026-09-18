//! Recognising a global-value piece under the current selection.
//!
//! A `zwrite`-shaped row reads `^NAME(subscripts)="p1^p2^p3"`. This is a
//! character-level reader of that one shape, not a general parser: it only
//! has to answer "does the selection sit inside exactly one `^`-delimited
//! piece of the value, and if so which piece and what global".

use super::Selection;
use crate::term::Grid;

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

/// The piece the selection asks about.
///
/// Two ways of asking, and they answer the same question:
///
/// * The selection sits inside one piece - the ordinary case, and what a
///   double-click on a value produces.
/// * The selection is the two delimiters *around* a piece, and the answer is
///   whatever lies between them. This is the only way to ask about an **empty**
///   piece, which has nothing in it to select: `^^` in the middle, `"^` at the
///   start, `^"` at the end, and the same for a sub-piece's own delimiter.
///   Convenient rather than obscure: a double-click takes the run of like
///   characters under it and `^` is a run of its own, so double-clicking a
///   `^^` already selects exactly the pair.
///
/// `None` for a selection spanning more than one line (nothing here is
/// per-line), one that reaches outside the value part of the row, one that
/// crosses a `^`, or a row that is not shaped like `^NAME(...)=...` at all.
pub fn piece_at_selection(grid: &Grid, selection: &Selection) -> Option<PieceSelection> {
    let (line, sel_start, sel_end) = single_line_span(selection)?;
    if sel_start >= sel_end {
        return None;
    }
    let row = grid.line(line)?;
    let width = row.used_width();
    let text: Vec<char> = row.cells[..width].iter().map(|c| c.ch).collect();

    let (global, subscripts, value_start) = global_and_value_start(&text)?;
    let (val_from, val_to) = unquoted_span(&text, value_start);

    // What is being asked about, as a half-open range of the value.
    let (asked_from, asked_to) =
        match between_delimiters(&text, val_from, val_to, sel_start, sel_end) {
            Some(between) => between,
            None => {
                if sel_start < val_from || sel_end > val_to {
                    return None;
                }
                (sel_start - val_from, sel_end - val_from)
            }
        };

    let value = &text[val_from..val_to];
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
        global,
        subscripts,
        piece,
        piece_text: value[piece_start..piece_end].iter().collect(),
        offset: asked_from - piece_start,
    })
}

/// What lies between the two delimiters a selection is made of, as a
/// half-open range of the value - or `None` when the selection is not that
/// shape and is an ordinary selection inside a piece.
///
/// The first and last selected columns both have to be delimiters. The value's
/// own quotes count as delimiters, because the first and last pieces have
/// nothing else bounding them.
fn between_delimiters(
    text: &[char],
    val_from: usize,
    val_to: usize,
    sel_start: usize,
    sel_end: usize,
) -> Option<(usize, usize)> {
    let last = sel_end.checked_sub(1)?;
    // Two distinct characters: a lone `^` does not say which of the two pieces
    // it separates is the one being asked about.
    if last <= sel_start {
        return None;
    }
    if !is_delimiter(text, val_from, val_to, sel_start)
        || !is_delimiter(text, val_from, val_to, last)
    {
        return None;
    }
    let from = (sel_start + 1).checked_sub(val_from)?;
    let to = last.checked_sub(val_from)?;
    (from <= to).then_some((from, to))
}

/// Whether column `at` holds something that bounds a piece: a `^`, one of the
/// characters a map can subdivide a piece with, or one of the value's own
/// quotes.
///
/// The sub-delimiters are named here rather than taken from the map, which
/// this layer cannot see. Listing the two the ERP actually uses is enough:
/// picking the wrong one only ever widens what is offered as the piece's
/// text, and `crate::features::doc_lookup::describe` splits it by the
/// delimiter the map really declares.
fn is_delimiter(text: &[char], val_from: usize, val_to: usize, at: usize) -> bool {
    match text.get(at) {
        Some('"') => at + 1 == val_from || at == val_to,
        Some(&c) if at >= val_from && at < val_to => matches!(c, '^' | ';' | ','),
        _ => false,
    }
}

/// The selection's line and column span, when it sits on exactly one line and
/// is not empty.
fn single_line_span(selection: &Selection) -> Option<(usize, usize, usize)> {
    let (start, end) = selection.ordered();
    (start.0 == end.0).then_some((start.0, start.1, end.1))
}

/// The global's name (without `^`), its subscripts, and the column just past
/// the `=` that starts its value, or `None` when the row does not open with
/// `^NAME` followed, past an optional subscript list, by `=`.
fn global_and_value_start(text: &[char]) -> Option<(String, Vec<String>, usize)> {
    let mut i = 0;
    if text.first() != Some(&'^') {
        return None;
    }
    i += 1;
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
    if text.get(i) == Some(&'(') {
        let close = skip_balanced_parens(text, i)?;
        subscripts = split_subscripts(&text[i + 1..close.saturating_sub(1)]);
        i = close;
    }
    if text.get(i) == Some(&'=') {
        Some((global, subscripts, i + 1))
    } else {
        None
    }
}

/// The subscript list between the parens, split on the commas that separate
/// *this* level - not the ones inside a quoted key or a nested expression.
///
/// A quoted subscript is handed back unquoted, doubled quotes collapsed, so
/// it compares equal to the literal a class's map declares for it.
fn split_subscripts(text: &[char]) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut depth = 0usize;
    let mut i = 0;
    while i < text.len() {
        match text[i] {
            '"' => {
                let end = string_end(text, i);
                let inner: String = text[i + 1..end.saturating_sub(1)].iter().collect();
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
                out.push(std::mem::take(&mut current));
                i += 1;
            }
            c => {
                current.push(c);
                i += 1;
            }
        }
    }
    if !text.is_empty() {
        out.push(current);
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

    /// The running example: a real `zwrite` line, one piece selected whole -
    /// the shape a double-click already produces, since `^` is its own
    /// symbol run and the digits are a word run of their own.
    #[test]
    fn a_selected_piece_is_named_and_numbered() {
        let grid = grid_with(&[r#"^CCDU(1,1)="1^350^2^2^64329^67043^^1000000""#]);
        // The value starts at column 12 (`"1^350^...`); "67043" is the 6th
        // piece, columns 28..33.
        let selection = Selection::across(0, 28, 33);
        let piece = piece_at_selection(&grid, &selection).expect("a piece");
        assert_eq!(piece.global, "CCDU");
        assert_eq!(piece.subscripts, vec!["1".to_string(), "1".to_string()]);
        assert_eq!(piece.piece, 6);
        assert_eq!(piece.piece_text, "67043");
    }

    #[test]
    fn the_first_piece_is_found_too() {
        let grid = grid_with(&[r#"^CCDU(1,1)="1^350^2""#]);
        let selection = Selection::across(0, 12, 13);
        let piece = piece_at_selection(&grid, &selection).expect("a piece");
        assert_eq!(piece.piece, 1);
        assert_eq!(piece.piece_text, "1");
    }

    /// Selecting only part of a piece's digits still resolves to the whole
    /// piece - a formatter needs the whole number, not half of it.
    #[test]
    fn a_partial_selection_inside_a_piece_yields_the_whole_piece() {
        let grid = grid_with(&[r#"^CCDU(1,1)="1^350^67043""#]);
        let selection = Selection::across(0, 19, 21); // "70" inside "67043"
        let piece = piece_at_selection(&grid, &selection).expect("a piece");
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
        let piece = piece_at_selection(&grid, &selection).expect("a piece");
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
        let piece = piece_at_selection(&grid, &selection).expect("a piece");
        assert_eq!(piece.piece, 1);
        assert_eq!(piece.piece_text, "");
    }

    /// And the last by the closing quote.
    #[test]
    fn selecting_the_last_caret_and_the_quote_names_an_empty_last_piece() {
        let grid = grid_with(&[r#"^CCDU(1,1)="1^350^""#]);
        // `1^350^` runs 12..18, so the last `^` is at 17 and the quote at 18.
        let selection = Selection::across(0, 17, 19);
        let piece = piece_at_selection(&grid, &selection).expect("a piece");
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
        let piece = piece_at_selection(&grid, &selection).expect("a piece");
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
        let piece = piece_at_selection(&grid, &selection).expect("a piece");
        assert_eq!(piece.piece, 2);
        assert_eq!(piece.piece_text, "350");
    }

    /// A lone `^` does not say which of the two pieces it separates is meant.
    #[test]
    fn selecting_a_single_delimiter_names_nothing() {
        let grid = grid_with(&[r#"^CCDU(1,1)="1^^350""#]);
        let selection = Selection::across(0, 13, 14);
        assert_eq!(piece_at_selection(&grid, &selection), None);
    }

    /// Two delimiters with a `^` between them span more than one piece.
    #[test]
    fn two_delimiters_with_a_piece_boundary_between_them_name_nothing() {
        let grid = grid_with(&[r#"^CCDU(1,1)="1^350^67^2""#]);
        let selection = Selection::across(0, 13, 21); // `^350^67^`
        assert_eq!(piece_at_selection(&grid, &selection), None);
    }

    #[test]
    fn a_selection_crossing_a_caret_is_not_one_piece() {
        let grid = grid_with(&[r#"^CCDU(1,1)="1^350^67043""#]);
        // Spans the `^` between "350" and "67043".
        let selection = Selection::across(0, 16, 19);
        assert_eq!(piece_at_selection(&grid, &selection), None);
    }

    #[test]
    fn a_row_that_is_not_a_global_assignment_yields_nothing() {
        let grid = grid_with(&["USER>write 1"]);
        let selection = Selection::across(0, 10, 11);
        assert_eq!(piece_at_selection(&grid, &selection), None);
    }

    #[test]
    fn a_selection_spanning_two_lines_yields_nothing() {
        let grid = grid_with(&[r#"^CCDU(1,1)="1^2""#, r#"^CCDU(1,2)="1^2""#]);
        let selection = Selection {
            start: (0, 12),
            end: (1, 13),
        };
        assert_eq!(piece_at_selection(&grid, &selection), None);
    }

    /// The subscripts may themselves hold a quoted key with parens or carets
    /// in it, which must not be mistaken for the end of the subscript list or
    /// for a piece delimiter.
    #[test]
    fn a_quoted_subscript_does_not_confuse_the_subscript_scan() {
        let grid = grid_with(&[r#"^CCDU(1,"4000164C")="31399^1^99""#]);
        let selection = Selection::across(0, 21, 26);
        let piece = piece_at_selection(&grid, &selection).expect("a piece");
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
        let piece = piece_at_selection(&grid, &selection).expect("a piece");
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
        let piece = piece_at_selection(&grid, &selection).expect("a piece");
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
        let piece = piece_at_selection(&grid, &selection).expect("a piece");
        assert_eq!(piece.piece, 1);
        assert_eq!(piece.piece_text, "1000000");
    }
}
