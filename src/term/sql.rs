//! Spotting SQL on a line typed at the IRIS SQL shell.
//!
//! The companion of [`super::syntax`] for the rows whose prompt is the SQL
//! shell's (`USER>>`). Only what follows the prompt is read: the shell prints
//! its result sets in columns of its own, and colouring a row of data as if it
//! were a statement would be noise.
//!
//! Reuses the ObjectScript [`Kind`]s instead of adding SQL ones, so every
//! theme colours SQL without a single new entry: a keyword is a command, a
//! function is a function, a table is a class - which in IRIS it literally is.
//! There is no comment kind to borrow, so a `--` comment is left in the
//! default colour; that it is the one uncoloured stretch on a coloured line is
//! what sets it apart.

use super::syntax::{Kind, Span};

/// Keywords, upper case. Matched case-insensitively.
pub(crate) const KEYWORDS: &[&str] = &[
    "ADD",
    "ALL",
    "ALTER",
    "AND",
    "ANY",
    "AS",
    "ASC",
    "BETWEEN",
    "BY",
    "CALL",
    "CASE",
    "COLUMN",
    "COMMIT",
    "CONSTRAINT",
    "CREATE",
    "CROSS",
    "DEFAULT",
    "DELETE",
    "DESC",
    "DISTINCT",
    "DROP",
    "ELSE",
    "END",
    "ESCAPE",
    "EXISTS",
    "FALSE",
    "FOREIGN",
    "FROM",
    "FULL",
    "GO",
    "GRANT",
    "GROUP",
    "HAVING",
    "IN",
    "INDEX",
    "INNER",
    "INSERT",
    "INTO",
    "IS",
    "JOIN",
    "KEY",
    "LEFT",
    "LIKE",
    "LIMIT",
    "NOT",
    "NULL",
    "OFFSET",
    "ON",
    "OR",
    "ORDER",
    "OUTER",
    "PRIMARY",
    "PROCEDURE",
    "REFERENCES",
    "REVOKE",
    "RIGHT",
    "ROLLBACK",
    "SELECT",
    "SET",
    "SOME",
    "TABLE",
    "THEN",
    "TOP",
    "TRUE",
    "TRUNCATE",
    "UNION",
    "UNIQUE",
    "UPDATE",
    "VALUES",
    "VIEW",
    "WHEN",
    "WHERE",
    "WITH",
    "%EXACT",
    "%STARTSWITH",
    "%MATCHES",
    "%PATTERN",
];

/// Functions, upper case, including the `%` collation functions and the
/// ObjectScript `$` functions IRIS SQL also accepts.
pub(crate) const FUNCTIONS: &[&str] = &[
    "ABS",
    "AVG",
    "CAST",
    "CEILING",
    "CHAR_LENGTH",
    "CHARINDEX",
    "COALESCE",
    "CONCAT",
    "CONVERT",
    "COUNT",
    "CURRENT_DATE",
    "CURRENT_TIMESTAMP",
    "DATEADD",
    "DATEDIFF",
    "DATENAME",
    "DATEPART",
    "FLOOR",
    "GETDATE",
    "IFNULL",
    "INSTR",
    "ISNULL",
    "LCASE",
    "LEFT",
    "LENGTH",
    "LIST",
    "LOWER",
    "LTRIM",
    "MAX",
    "MIN",
    "NOW",
    "NULLIF",
    "NVL",
    "POSITION",
    "REPLACE",
    "REPLICATE",
    "RIGHT",
    "ROUND",
    "RTRIM",
    "STRING",
    "SUBSTR",
    "SUBSTRING",
    "SUM",
    "TO_CHAR",
    "TO_DATE",
    "TO_NUMBER",
    "TRIM",
    "UCASE",
    "UPPER",
    "%ALPHAUP",
    "%EXTERNAL",
    "%INTERNAL",
    "%ODBCIN",
    "%ODBCOUT",
    "%SQLSTRING",
    "%SQLUPPER",
    "%UPPER",
    "$EXTRACT",
    "$FIND",
    "$LENGTH",
    "$LIST",
    "$LISTBUILD",
    "$LISTGET",
    "$PIECE",
];

/// Keywords after which the next name is a table.
const TABLE_BEFORE: &[&str] = &["FROM", "JOIN", "INTO", "UPDATE", "TABLE"];

/// Finds the SQL tokens in `chars`, from column `from` on - the column just
/// past the shell's prompt.
pub fn scan(chars: &[char], from: usize) -> Vec<Span> {
    let mut spans = Vec::new();
    let mut i = from;
    // The next name is a table: it follows `FROM`, `JOIN`, ... or a comma in a
    // `FROM` list.
    let mut want_table = false;
    let mut in_table_list = false;

    while i < chars.len() {
        let ch = chars[i];
        match ch {
            '-' if chars.get(i + 1) == Some(&'-') => break,
            '\'' => {
                let end = quoted_end(chars, i, '\'');
                push(&mut spans, i, end, Kind::Str);
                i = end;
            }
            // A delimited identifier: a name, whatever it contains, so nothing
            // inside it is read as anything else.
            '"' => i = quoted_end(chars, i, '"'),
            c if c.is_ascii_digit()
                || (c == '.' && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit())) =>
            {
                let end = number_end(chars, i);
                push(&mut spans, i, end, Kind::Number);
                i = end;
            }
            c if is_word_start(c) => {
                let end = word_end(chars, i);
                let word = &chars[i..end];
                let called = next_non_blank(chars, end) == Some('(');
                if is_in(word, KEYWORDS) {
                    push(&mut spans, i, end, Kind::Command);
                    want_table = is_in(word, TABLE_BEFORE);
                    in_table_list = word_eq(word, "FROM");
                } else if called || (matches!(c, '$' | '%') && is_in(word, FUNCTIONS)) {
                    push(&mut spans, i, end, Kind::Function);
                } else if want_table {
                    push(&mut spans, i, end, Kind::ObjectClass);
                    want_table = false;
                }
                i = end;
            }
            ',' => {
                push(&mut spans, i, i + 1, Kind::Delimiter);
                want_table = in_table_list;
                i += 1;
            }
            '(' | ')' | ';' | '.' => {
                push(&mut spans, i, i + 1, Kind::Delimiter);
                i += 1;
            }
            // A host variable or a parameter marker: something supplied from
            // outside the statement, which is what a system variable is too.
            '?' => {
                push(&mut spans, i, i + 1, Kind::SystemVariable);
                i += 1;
            }
            ':' if chars.get(i + 1).copied().is_some_and(is_word_start) => {
                let end = word_end(chars, i + 1);
                push(&mut spans, i, end, Kind::SystemVariable);
                i = end;
            }
            c if is_operator(c) => {
                let mut end = i + 1;
                while end < chars.len()
                    && is_operator(chars[end])
                    && !(chars[end] == '-' && chars.get(end + 1) == Some(&'-'))
                {
                    end += 1;
                }
                push(&mut spans, i, end, Kind::Operator);
                i = end;
            }
            _ => i += 1,
        }
    }
    spans
}

fn push(spans: &mut Vec<Span>, start: usize, end: usize, kind: Kind) {
    if end > start {
        spans.push(Span { start, end, kind });
    }
}

/// End of the quoted run opening at `open`, closing quote included. A doubled
/// quote is the quote itself, as in ObjectScript; an unclosed one runs to the
/// end of the row.
fn quoted_end(chars: &[char], open: usize, quote: char) -> usize {
    let mut i = open + 1;
    while i < chars.len() {
        if chars[i] == quote {
            if chars.get(i + 1) == Some(&quote) {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    chars.len()
}

fn number_end(chars: &[char], from: usize) -> usize {
    let mut i = from;
    let mut seen_point = false;
    while let Some(&c) = chars.get(i) {
        if c.is_ascii_digit() {
            i += 1;
        } else if c == '.' && !seen_point && chars.get(i + 1).is_some_and(|d| d.is_ascii_digit()) {
            seen_point = true;
            i += 1;
        } else {
            break;
        }
    }
    i
}

/// A name starts with a letter, `_`, or the `%` and `$` IRIS prefixes its own
/// functions with.
pub(crate) fn is_word_start(c: char) -> bool {
    c.is_ascii_alphabetic() || matches!(c, '_' | '%' | '$')
}

pub(crate) fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// End of the name at `from`. A qualified name, `Sample.Person`, is one name:
/// it is the table, and colouring half of it would say otherwise.
fn word_end(chars: &[char], from: usize) -> usize {
    let mut i = from + 1;
    loop {
        while chars.get(i).copied().is_some_and(is_word) {
            i += 1;
        }
        let qualified = chars.get(i) == Some(&'.')
            && chars
                .get(i + 1)
                .copied()
                .is_some_and(|c| c.is_ascii_alphabetic() || matches!(c, '_' | '%'));
        if !qualified {
            return i;
        }
        i += 2;
    }
}

fn next_non_blank(chars: &[char], from: usize) -> Option<char> {
    chars[from.min(chars.len())..]
        .iter()
        .copied()
        .find(|c| *c != ' ')
}

fn is_operator(c: char) -> bool {
    matches!(
        c,
        '=' | '<' | '>' | '!' | '+' | '-' | '*' | '/' | '|' | '&' | '#' | '^'
    )
}

fn word_eq(word: &[char], upper: &str) -> bool {
    word.len() == upper.len()
        && upper
            .bytes()
            .zip(word)
            .all(|(b, c)| char::from(b) == c.to_ascii_uppercase())
}

/// Compared char by char rather than by uppercasing into a `String`: this runs
/// for every name on every SQL row of every frame.
fn is_in(word: &[char], list: &[&str]) -> bool {
    list.iter().any(|candidate| word_eq(word, candidate))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(Kind, String)> {
        let chars: Vec<char> = text.chars().collect();
        scan(&chars, 0)
            .into_iter()
            .map(|s| (s.kind, chars[s.start..s.end].iter().collect()))
            .collect()
    }

    #[test]
    fn a_select_splits_into_keywords_a_function_a_table_and_a_literal() {
        assert_eq!(
            kinds("select count(*) from Sample.Person where Age > 30"),
            vec![
                (Kind::Command, "select".to_string()),
                (Kind::Function, "count".to_string()),
                (Kind::Delimiter, "(".to_string()),
                (Kind::Operator, "*".to_string()),
                (Kind::Delimiter, ")".to_string()),
                (Kind::Command, "from".to_string()),
                (Kind::ObjectClass, "Sample.Person".to_string()),
                (Kind::Command, "where".to_string()),
                (Kind::Operator, ">".to_string()),
                (Kind::Number, "30".to_string()),
            ]
        );
    }

    /// SQL quotes strings with `'`, and doubles it to mean the quote itself.
    #[test]
    fn a_string_is_single_quoted_and_a_doubled_quote_stays_inside_it() {
        assert_eq!(kinds("'it''s' x"), vec![(Kind::Str, "'it''s'".to_string())]);
        // Unclosed, it runs to the end of the row, the way a cut-off value
        // does everywhere else.
        assert_eq!(kinds("'abc"), vec![(Kind::Str, "'abc".to_string())]);
    }

    /// Nothing after `--` is a token, however much it looks like one.
    #[test]
    fn a_comment_ends_the_scan() {
        assert_eq!(
            kinds("select 1 -- from t 'x'"),
            vec![
                (Kind::Command, "select".to_string()),
                (Kind::Number, "1".to_string()),
            ]
        );
    }

    /// Every name in a `FROM` list is a table, and an alias is not.
    #[test]
    fn every_table_in_a_from_list_is_a_table() {
        let spans = kinds("select * from A.B x, C join D on x.k = D.k");
        let tables: Vec<String> = spans
            .into_iter()
            .filter(|(k, _)| *k == Kind::ObjectClass)
            .map(|(_, t)| t)
            .collect();
        assert_eq!(tables, vec!["A.B", "C", "D"]);
    }

    #[test]
    fn host_variables_and_parameters_are_system_variables() {
        assert_eq!(
            kinds("where id = ? or id = :code"),
            vec![
                (Kind::Command, "where".to_string()),
                (Kind::Operator, "=".to_string()),
                (Kind::SystemVariable, "?".to_string()),
                (Kind::Command, "or".to_string()),
                (Kind::Operator, "=".to_string()),
                (Kind::SystemVariable, ":code".to_string()),
            ]
        );
    }

    #[test]
    fn iris_functions_are_functions_even_without_a_bracket_yet() {
        assert_eq!(
            kinds("%SQLUPPER name"),
            vec![(Kind::Function, "%SQLUPPER".to_string())]
        );
        assert_eq!(
            kinds("$piece(x,'^',2)")[0],
            (Kind::Function, "$piece".to_string())
        );
    }

    /// Only what follows the prompt is a statement.
    #[test]
    fn the_scan_starts_at_the_column_it_is_given() {
        let chars: Vec<char> = "USER>>select 1".chars().collect();
        let spans = scan(&chars, 6);
        assert_eq!(spans[0].start, 6);
        assert!(spans.iter().all(|s| s.start >= 6));
    }

    #[test]
    fn a_decimal_is_one_number() {
        assert_eq!(
            kinds("1.5 .25"),
            vec![
                (Kind::Number, "1.5".to_string()),
                (Kind::Number, ".25".to_string()),
            ]
        );
    }
}
