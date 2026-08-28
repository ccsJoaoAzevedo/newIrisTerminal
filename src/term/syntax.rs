//! Spotting globals and quoted strings in terminal output.
//!
//! This is a heuristic over whatever happens to be on the screen, not a parser:
//! the terminal has no idea whether a line is ObjectScript, a `zwrite` dump, or
//! a banner. It is worth having anyway, because ERP output is mostly global
//! references and delimited strings, and telling the two apart at a glance is
//! the difference between reading a `zwrite` and decoding it.
//!
//! Being a guess is exactly why the renderer only applies these colours to
//! cells the remote side left at the default foreground. Anything IRIS coloured
//! deliberately with an SGR sequence keeps the colour it asked for.

use super::cell::Cell;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A global reference, `^NAME` or `^%NAME`.
    Global,
    /// A quoted string, quotes included.
    Str,
}

/// A run of columns sharing one kind. `end` is exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

/// Finds the globals and strings in one row of cells.
///
/// Strings win over globals: a `^` inside quotes is a piece delimiter, not a
/// global, which matters because ERP data is full of `"a^b^c"`.
pub fn scan(cells: &[Cell]) -> Vec<Span> {
    let chars: Vec<char> = cells.iter().map(|c| c.ch).collect();
    let mut spans = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        match chars[i] {
            '"' => {
                let start = i;
                i += 1;
                while i < chars.len() {
                    if chars[i] == '"' {
                        // ObjectScript escapes a quote by doubling it, so a pair
                        // is content and the string carries on.
                        if chars.get(i + 1) == Some(&'"') {
                            i += 2;
                            continue;
                        }
                        i += 1;
                        break;
                    }
                    i += 1;
                }
                // An unterminated string runs to the end of the row: output is
                // routinely cut off at the right margin mid-value.
                spans.push(Span {
                    start,
                    end: i,
                    kind: Kind::Str,
                });
            }
            '^' => {
                // A letter (after an optional `%`) is what separates a global
                // from the piece delimiter. Nothing in `300^1^1^67222` matches,
                // because no `^` there is followed by a letter.
                let mut j = i + 1;
                if chars.get(j) == Some(&'%') {
                    j += 1;
                }
                if chars.get(j).is_some_and(|c| c.is_ascii_alphabetic()) {
                    j += 1;
                    while chars
                        .get(j)
                        .is_some_and(|c| c.is_ascii_alphanumeric() || *c == '.')
                    {
                        j += 1;
                    }
                    // Stops before any `(`, so the subscripts are left to be
                    // coloured on their own terms.
                    spans.push(Span {
                        start: i,
                        end: j,
                        kind: Kind::Global,
                    });
                    i = j;
                } else {
                    i += 1;
                }
            }
            _ => i += 1,
        }
    }

    spans
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(text: &str) -> Vec<Cell> {
        text.chars()
            .map(|ch| Cell {
                ch,
                ..Cell::default()
            })
            .collect()
    }

    fn kinds(text: &str) -> Vec<(Kind, String)> {
        let cells = row(text);
        scan(&cells)
            .into_iter()
            .map(|s| (s.kind, cells[s.start..s.end].iter().map(|c| c.ch).collect()))
            .collect()
    }

    #[test]
    fn a_global_reference_is_found_without_its_subscripts() {
        assert_eq!(
            kinds("zw ^CCDU(1,1)"),
            vec![(Kind::Global, "^CCDU".to_string())]
        );
    }

    #[test]
    fn a_percent_global_is_a_global() {
        assert_eq!(
            kinds("Do ^%CSW1GATCUST"),
            vec![(Kind::Global, "^%CSW1GATCUST".to_string())]
        );
        assert_eq!(
            kinds("kill ^mtempCCTTGPRG005Quim(term)"),
            vec![(Kind::Global, "^mtempCCTTGPRG005Quim".to_string())]
        );
    }

    /// The case that makes the "letter after the caret" rule necessary: `^` is
    /// also the piece delimiter, and ERP values are full of it.
    #[test]
    fn a_piece_delimiter_is_not_a_global() {
        assert!(kinds("300^1^1^67222^48220").is_empty());
    }

    #[test]
    fn a_caret_inside_a_string_stays_part_of_the_string() {
        assert_eq!(
            kinds(r#""1^350^2^2^64329""#),
            vec![(Kind::Str, r#""1^350^2^2^64329""#.to_string())]
        );
    }

    #[test]
    fn a_doubled_quote_does_not_end_the_string() {
        assert_eq!(
            kinds(r#""a""b" x"#),
            vec![(Kind::Str, r#""a""b""#.to_string())]
        );
    }

    /// Output is cut at the device margin all the time, so a value that never
    /// closes its quote must still be coloured to where the row ends.
    #[test]
    fn an_unterminated_string_runs_to_the_end_of_the_row() {
        assert_eq!(
            kinds(r#"x="1^350^2"#),
            vec![(Kind::Str, r#""1^350^2"#.to_string())]
        );
    }

    #[test]
    fn a_real_zwrite_line_splits_into_a_global_and_its_value() {
        assert_eq!(
            kinds(r#"^CCDU(1,1)="1^350^2^2^64329""#),
            vec![
                (Kind::Global, "^CCDU".to_string()),
                (Kind::Str, r#""1^350^2^2^64329""#.to_string()),
            ]
        );
    }

    #[test]
    fn a_lone_caret_is_left_alone() {
        assert!(kinds("^ ^^ ^1").is_empty());
    }
}
