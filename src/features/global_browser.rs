//! A structured browser for IRIS globals.
//!
//! `^%G` is prompt-driven and paginates as free text, which is fine to read and
//! miserable to search. This builds the same view from data instead: a short
//! read-only ObjectScript walk emits one tagged line per node and per `$Piece`,
//! and the result becomes a searchable, paginated grid where every piece is a
//! labelled column.
//!
//! Two decisions are worth stating, because both are about not losing data:
//!
//! * **Pieces are split on the IRIS side.** `$Piece` is the authority on what
//!   piece *n* is; re-implementing that split here would eventually disagree
//!   with the ERP code that wrote the value.
//! * **One line per piece, not one per node.** IRIS truncates output at the
//!   terminal's right margin rather than wrapping it (see `tests/live_wrap.rs`),
//!   so a wide node written on a single line silently loses its tail. Short
//!   lines keep the data intact regardless of window width.
//!
//! Everything here is read-only: `$Query` and `$Get` only. Nothing in this
//! module can write to a database.

/// Line tags. Chosen to be improbable in real data and trivial to scan for.
const TAG_NODE: &str = "@N@";
const TAG_PIECE: &str = "@P@";
const TAG_END: &str = "@E@";
const TAG_ERROR: &str = "@X@";

/// Default cap on nodes fetched per request. A global can hold millions of
/// nodes; pulling them all through a terminal would hang the session.
pub const DEFAULT_LIMIT: usize = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Query {
    /// Global name, with or without the leading caret.
    pub global: String,
    /// Resume point: a full reference such as `^CSW1(12,3)`. Empty starts at
    /// the beginning.
    pub start: String,
    pub limit: usize,
    /// Delimiter for `$Piece`. `^` is the ObjectScript default.
    pub delimiter: String,
}

impl Default for Query {
    fn default() -> Self {
        Query {
            global: String::new(),
            start: String::new(),
            limit: DEFAULT_LIMIT,
            delimiter: "^".to_string(),
        }
    }
}

impl Query {
    /// The global reference with exactly one leading caret.
    pub fn normalized_global(&self) -> String {
        format!("^{}", self.global.trim().trim_start_matches('^'))
    }

    pub fn is_runnable(&self) -> bool {
        let name = self.global.trim().trim_start_matches('^');
        !name.is_empty()
            && name.chars().all(|c| {
                c.is_ascii_alphanumeric() || matches!(c, '%' | '.' | '(' | ')' | ',' | '"')
            })
    }
}

/// Builds the read-only walk.
///
/// Wrapped in a `Try/Catch` so a bad global name reports itself as one tagged
/// line instead of dropping the session into the `<SYNTAX>` error prompt,
/// where the next thing the user types would be swallowed.
pub fn build_script(query: &Query) -> String {
    let global = query.normalized_global();
    let start = if query.start.trim().is_empty() {
        global.clone()
    } else {
        query.start.trim().to_string()
    };
    let limit = query.limit.max(1);
    let delimiter = if query.delimiter.is_empty() {
        "^".to_string()
    } else {
        query.delimiter.clone()
    };
    let delimiter = escape_os_string(&delimiter);

    format!(
        concat!(
            "Try {{ ",
            "Set nitC=0,nitQ=\"{start}\" ",
            "For {{ ",
            "Set nitQ=$Query(@nitQ) Quit:nitQ=\"\"  ",
            "Set nitC=nitC+1,nitV=$Get(@nitQ) ",
            "Write \"{tag_node}\",nitQ,! ",
            "For nitI=1:1:$Length(nitV,\"{delim}\") {{ ",
            "Write \"{tag_piece}\",nitI,\"@\",$Piece(nitV,\"{delim}\",nitI),! }} ",
            "Quit:nitC>={limit}  }} ",
            "Write \"{tag_end}\",nitC,! }} ",
            "Catch nitE {{ Write \"{tag_error}\",nitE.DisplayString(),! }}"
        ),
        start = escape_os_string(&start),
        limit = limit,
        delim = delimiter,
        tag_node = TAG_NODE,
        tag_piece = TAG_PIECE,
        tag_end = TAG_END,
        tag_error = TAG_ERROR,
    )
}

/// Doubles quotes, the ObjectScript way of escaping them inside a literal.
fn escape_os_string(text: &str) -> String {
    text.replace('"', "\"\"")
}

/// Removes ANSI/VT escape sequences from captured output.
///
/// The capture buffer is the raw session stream, so it carries the cursor and
/// erase sequences ConPTY emits between writes. Left in place they would end
/// up inside piece text, and a value shown in the grid would silently differ
/// from what is in the database.
pub fn strip_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '\u{1b}' {
            // Other C0 controls (BEL, and the SO/SI charset toggles) are not
            // data either.
            if !matches!(ch, '\u{7}' | '\u{e}' | '\u{f}') {
                out.push(ch);
            }
            continue;
        }

        match chars.next() {
            // CSI: parameters and intermediates, then a final byte in @..~.
            Some('[') => {
                for c in chars.by_ref() {
                    if ('\u{40}'..='\u{7e}').contains(&c) {
                        break;
                    }
                }
            }
            // OSC: runs to BEL, or to ST (ESC backslash).
            Some(']') => {
                while let Some(c) = chars.next() {
                    if c == '\u{7}' {
                        break;
                    }
                    if c == '\u{1b}' && chars.peek() == Some(&'\\') {
                        chars.next();
                        break;
                    }
                }
            }
            // Charset selection and similar two-character sequences.
            Some('(') | Some(')') | Some('#') => {
                chars.next();
            }
            _ => {}
        }
    }
    out
}

/// One global node.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Node {
    /// Full reference as IRIS reported it, e.g. `^CSW1(12,3)`.
    pub reference: String,
    /// Subscripts split out of the reference, in order.
    pub subscripts: Vec<String>,
    /// `$Piece` values, piece 1 first.
    pub pieces: Vec<String>,
}

impl Node {
    /// The node's value, reassembled. Useful for search and for copying.
    pub fn value(&self, delimiter: &str) -> String {
        self.pieces.join(delimiter)
    }

    /// True when any subscript or piece contains `needle`, compared
    /// case-insensitively.
    pub fn matches(&self, needle: &str) -> bool {
        if needle.is_empty() {
            return true;
        }
        let needle = needle.to_lowercase();
        self.reference.to_lowercase().contains(&needle)
            || self
                .pieces
                .iter()
                .any(|p| p.to_lowercase().contains(&needle))
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Page {
    pub nodes: Vec<Node>,
    /// Node count IRIS reported, present once the walk finished cleanly.
    pub reported: Option<usize>,
    /// An IRIS error, if the walk failed.
    pub error: Option<String>,
    /// True when the limit was reached, so more nodes remain.
    pub truncated: bool,
}

impl Page {
    /// Reference to resume from, for fetching the next page.
    pub fn resume_from(&self) -> Option<String> {
        self.truncated
            .then(|| self.nodes.last().map(|n| n.reference.clone()))
            .flatten()
    }

    /// Widest piece count, which is how many piece columns the grid needs.
    pub fn piece_columns(&self) -> usize {
        self.nodes.iter().map(|n| n.pieces.len()).max().unwrap_or(0)
    }

    /// Deepest subscript count, likewise for subscript columns.
    pub fn subscript_columns(&self) -> usize {
        self.nodes
            .iter()
            .map(|n| n.subscripts.len())
            .max()
            .unwrap_or(0)
    }
}

/// Parses the tagged output back into nodes.
///
/// Tolerant by design: it reads whatever tagged lines it finds and ignores
/// everything else, so the command echo, the prompt, and any banner IRIS
/// decides to print do not need special handling.
pub fn parse(output: &str, limit: usize) -> Page {
    let output = &strip_ansi(output);
    let mut page = Page::default();
    let mut current: Option<Node> = None;

    for line in output.lines() {
        let line = line.trim_end();

        if let Some(rest) = line.find(TAG_ERROR).map(|i| &line[i + TAG_ERROR.len()..]) {
            page.error = Some(rest.trim().to_string());
            continue;
        }

        if let Some(rest) = line.find(TAG_END).map(|i| &line[i + TAG_END.len()..]) {
            page.reported = rest.trim().parse::<usize>().ok();
            continue;
        }

        if let Some(rest) = line.find(TAG_NODE).map(|i| &line[i + TAG_NODE.len()..]) {
            if let Some(node) = current.take() {
                page.nodes.push(node);
            }
            let reference = rest.trim().to_string();
            current = Some(Node {
                subscripts: split_subscripts(&reference),
                reference,
                pieces: Vec::new(),
            });
            continue;
        }

        if let Some(rest) = line.find(TAG_PIECE).map(|i| &line[i + TAG_PIECE.len()..]) {
            // `<index>@<text>`; the text may itself contain '@'.
            let Some((index, text)) = rest.split_once('@') else {
                continue;
            };
            let Ok(index) = index.trim().parse::<usize>() else {
                continue;
            };
            if let Some(node) = current.as_mut() {
                // Index is 1-based and arrives in order, but pad rather than
                // assume: a dropped line must not shift every later piece.
                if node.pieces.len() < index {
                    node.pieces.resize(index, String::new());
                }
                node.pieces[index - 1] = text.to_string();
            }
        }
    }

    if let Some(node) = current.take() {
        page.nodes.push(node);
    }

    page.truncated = page.nodes.len() >= limit;
    page
}

/// Splits `^CSW1(12,"ab",3)` into `["12", "\"ab\"", "3"]`.
///
/// Quoted subscripts may contain commas and parentheses, so this tracks quote
/// state rather than splitting naively.
pub fn split_subscripts(reference: &str) -> Vec<String> {
    let Some(open) = reference.find('(') else {
        return Vec::new();
    };
    let inner = &reference[open + 1..];
    let inner = inner.strip_suffix(')').unwrap_or(inner);

    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut chars = inner.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            '"' => {
                // A doubled quote inside a quoted string is a literal quote.
                if in_quotes && chars.peek() == Some(&'"') {
                    current.push('"');
                    chars.next();
                } else {
                    in_quotes = !in_quotes;
                    current.push('"');
                }
            }
            ',' if !in_quotes => {
                out.push(current.trim().to_string());
                current = String::new();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        out.push(current.trim().to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn query(global: &str) -> Query {
        Query {
            global: global.to_string(),
            ..Query::default()
        }
    }

    #[test]
    fn the_global_is_normalised_to_one_caret() {
        assert_eq!(query("CSW1").normalized_global(), "^CSW1");
        assert_eq!(query("^CSW1").normalized_global(), "^CSW1");
        assert_eq!(query("  CSW1 ").normalized_global(), "^CSW1");
    }

    #[test]
    fn an_empty_or_hostile_global_is_not_runnable() {
        assert!(!query("").is_runnable());
        assert!(!query("   ").is_runnable());
        assert!(query("CSW1").is_runnable());
        // No room for command injection through the name.
        assert!(!query("X Set ^EVIL=1").is_runnable());
    }

    /// The script must never contain anything that writes.
    #[test]
    fn the_script_is_read_only() {
        let script = build_script(&query("CSW1"));
        for forbidden in [" Set ^", "Kill", "Merge ", "TSTART", "Do ^"] {
            assert!(
                !script.contains(forbidden),
                "script contains {forbidden:?}: {script}"
            );
        }
        assert!(script.contains("$Query("));
        assert!(script.contains("$Get("));
    }

    #[test]
    fn the_script_starts_at_the_global_or_the_resume_point() {
        assert!(build_script(&query("CSW1")).contains("nitQ=\"^CSW1\""));

        let resumed = Query {
            start: "^CSW1(12,3)".into(),
            ..query("CSW1")
        };
        assert!(build_script(&resumed).contains("nitQ=\"^CSW1(12,3)\""));
    }

    #[test]
    fn the_script_honours_the_limit_and_delimiter() {
        let q = Query {
            limit: 42,
            delimiter: "|".into(),
            ..query("CSW1")
        };
        let script = build_script(&q);
        assert!(script.contains("nitC>=42"));
        assert!(script.contains("$Piece(nitV,\"|\""));
    }

    /// A quote in a resume reference must not break out of the literal.
    #[test]
    fn quotes_in_the_resume_point_are_escaped() {
        let q = Query {
            start: "^CSW1(\"a\")".into(),
            ..query("CSW1")
        };
        assert!(build_script(&q).contains("^CSW1(\"\"a\"\")"));
    }

    const OUTPUT: &str = concat!(
        "COMP80>Try { Set nitC=0 ... }\n",
        "@N@^CSW1(1)\n",
        "@P@1@ABC\n",
        "@P@2@2024\n",
        "@P@3@12.50\n",
        "@N@^CSW1(2)\n",
        "@P@1@DEF\n",
        "@P@2@2025\n",
        "@E@2\n",
        "COMP80>",
    );

    #[test]
    fn parses_nodes_and_pieces() {
        let page = parse(OUTPUT, 500);
        assert!(page.error.is_none());
        assert_eq!(page.reported, Some(2));
        assert_eq!(page.nodes.len(), 2);

        assert_eq!(page.nodes[0].reference, "^CSW1(1)");
        assert_eq!(page.nodes[0].pieces, vec!["ABC", "2024", "12.50"]);
        assert_eq!(page.nodes[1].pieces, vec!["DEF", "2025"]);
    }

    #[test]
    fn ignores_the_echo_and_the_prompt() {
        let page = parse(OUTPUT, 500);
        assert!(page.nodes.iter().all(|n| !n.reference.contains("COMP80")));
    }

    #[test]
    fn column_counts_come_from_the_widest_row() {
        let page = parse(OUTPUT, 500);
        assert_eq!(page.piece_columns(), 3);
        assert_eq!(page.subscript_columns(), 1);
    }

    #[test]
    fn a_piece_containing_an_at_sign_survives() {
        let page = parse("@N@^X(1)\n@P@1@a@b@c\n@E@1\n", 500);
        assert_eq!(page.nodes[0].pieces, vec!["a@b@c"]);
    }

    #[test]
    fn an_empty_piece_is_preserved_in_position() {
        let page = parse("@N@^X(1)\n@P@1@a\n@P@2@\n@P@3@c\n@E@1\n", 500);
        assert_eq!(page.nodes[0].pieces, vec!["a", "", "c"]);
    }

    /// A dropped line must not shift every later piece into the wrong column,
    /// which would silently misattribute ERP data.
    #[test]
    fn a_missing_piece_line_leaves_a_gap_rather_than_shifting() {
        let page = parse("@N@^X(1)\n@P@1@a\n@P@3@c\n@E@1\n", 500);
        assert_eq!(page.nodes[0].pieces, vec!["a", "", "c"]);
    }

    #[test]
    fn an_iris_error_is_reported_not_swallowed() {
        let page = parse("@X@<SYNTAX>zTest+1^%G\n", 500);
        assert_eq!(page.error.as_deref(), Some("<SYNTAX>zTest+1^%G"));
        assert!(page.nodes.is_empty());
    }

    #[test]
    fn reaching_the_limit_marks_the_page_truncated_and_gives_a_resume_point() {
        let page = parse("@N@^X(1)\n@P@1@a\n@N@^X(2)\n@P@1@b\n", 2);
        assert!(page.truncated);
        assert_eq!(page.resume_from().as_deref(), Some("^X(2)"));

        let complete = parse("@N@^X(1)\n@P@1@a\n@E@1\n", 500);
        assert!(!complete.truncated);
        assert!(complete.resume_from().is_none());
    }

    #[test]
    fn escape_sequences_are_stripped_from_captured_output() {
        let raw = "\u{1b}[2J\u{1b}[1;1H@N@^X(1)\r\n\u{1b}[K@P@1@abc\u{1b}[0m\r\n@E@1\r\n";
        let page = parse(raw, 500);
        assert_eq!(page.nodes.len(), 1);
        assert_eq!(page.nodes[0].reference, "^X(1)");
        assert_eq!(
            page.nodes[0].pieces,
            vec!["abc"],
            "an escape sequence leaked into the value"
        );
    }

    #[test]
    fn stripping_leaves_ordinary_text_intact() {
        assert_eq!(strip_ansi("plain text 123"), "plain text 123");
        assert_eq!(strip_ansi("acentuacao especial"), "acentuacao especial");
    }

    #[test]
    fn stripping_removes_osc_titles() {
        assert_eq!(strip_ansi("\u{1b}]0;a title\u{7}data"), "data");
    }

    #[test]
    fn subscripts_split_on_commas() {
        assert_eq!(split_subscripts("^CSW1(12,3)"), vec!["12", "3"]);
        assert_eq!(split_subscripts("^CSW1"), Vec::<String>::new());
    }

    /// A quoted subscript may contain commas and parentheses; splitting on the
    /// raw comma would break the reference apart in the wrong place.
    #[test]
    fn subscripts_respect_quoting() {
        assert_eq!(
            split_subscripts("^CSW1(1,\"a,b\",3)"),
            vec!["1", "\"a,b\"", "3"]
        );
        assert_eq!(split_subscripts("^CSW1(\"x(y)\")"), vec!["\"x(y)\""]);
    }

    #[test]
    fn search_matches_reference_and_pieces_case_insensitively() {
        let page = parse(OUTPUT, 500);
        let node = &page.nodes[0];
        assert!(node.matches("abc"));
        assert!(node.matches("ABC"));
        assert!(node.matches("csw1(1)"));
        assert!(!node.matches("zzz"));
        assert!(node.matches(""), "an empty search matches everything");
    }

    #[test]
    fn the_value_is_reassembled_with_the_delimiter() {
        let page = parse(OUTPUT, 500);
        assert_eq!(page.nodes[0].value("^"), "ABC^2024^12.50");
    }
}
