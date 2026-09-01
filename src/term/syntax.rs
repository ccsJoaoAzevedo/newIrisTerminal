//! Spotting ObjectScript in terminal output.
//!
//! This is a heuristic over whatever happens to be on the screen, not a parser:
//! the terminal has no idea whether a line is ObjectScript, a `zwrite` dump, or
//! a banner. It is worth having anyway, because ERP output is mostly global
//! references, delimited strings and numbers, and telling them apart at a
//! glance is the difference between reading a `zwrite` and decoding it.
//!
//! The kinds mirror the semantic token scopes of the InterSystems VS Code
//! extension (`COS_Command`, `COS_Globalvariable`, ...), so a theme can be
//! filled in straight from an editor colour customisation. Three of those
//! scopes are deliberately absent: `COS_Localundeclared`, `COS_Objectname` and
//! `COS_Routine`-as-a-declaration need to know what has been declared and what
//! type it has, which is exactly the information a terminal does not have.
//!
//! Being a guess is why the renderer only applies these colours to cells the
//! remote side left at the default foreground. Anything IRIS coloured
//! deliberately with an SGR sequence keeps the colour it asked for.
//!
//! Two tiers of confidence, because the same rule is not safe everywhere:
//!
//! * Everywhere on the row: strings, numbers, globals, `$`-constructs, class
//!   names. These are shapes, not words, so they cannot fire on prose.
//! * Only after an IRIS prompt: commands, labels, operators and delimiters.
//!   `set` is a command on a command line and an ordinary word in a sentence,
//!   and a comma is punctuation far more often than it is a subscript
//!   separator.

use super::cell::Cell;

/// One highlighted token kind, named after the scope it mirrors in the
/// InterSystems VS Code extension.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// A tag, as in the `Foo` of `Foo^ROUTINE`. `COS_Label`.
    Label,
    /// A command word: `set`, `w`, `zwrite`. `COS_Command`.
    Command,
    /// A quoted string, quotes included. `COS_String`.
    Str,
    /// A numeric literal. `COS_Number`.
    Number,
    /// Parentheses, commas, colons. `COS_Delimiter`.
    Delimiter,
    /// `=`, `_`, `'`, and the rest. `COS_Operator`.
    Operator,
    /// `#dim`, `#define`, and `$$$MACRO`. `COS_PreProcessorCommand`.
    PreProcessor,
    /// A system function — a `$name` that is being called. `COS_Function`.
    Function,
    /// A global reference, `^NAME` or `^%NAME`. `COS_Globalvariable`.
    Global,
    /// A system variable — a `$name` that is not. `COS_Systemvariable`.
    SystemVariable,
    /// The class in `##class(Pkg.Cls)`. `COS_ObjectClass`.
    ObjectClass,
    /// The name in `obj.Method(` or `..Method(`, without the dots.
    /// `COS_Objectmethod`.
    ObjectMethod,
    /// The name in `..Property`, the relative reference used inside a method.
    /// `COS_Objectattribute`.
    ObjectAttribute,
    /// The name in `object.Property`. `COS_Objectmember`.
    ObjectMember,
    /// The routine half of `Tag^ROUTINE`, and the target of `do ^ROUTINE`.
    /// `COS_Routine`.
    Routine,
    /// `$$Tag^ROUTINE`. `COS_Extrinsicfunction`.
    Extrinsic,
}

/// A run of columns sharing one kind. `end` is exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: usize,
    pub end: usize,
    pub kind: Kind,
}

/// Command words and the abbreviations IRIS accepts for them. Matched
/// case-insensitively, and only on a command line — see the module docs.
const COMMANDS: &[&str] = &[
    "b",
    "break",
    "c",
    "close",
    "catch",
    "continue",
    "d",
    "do",
    "e",
    "else",
    "elseif",
    "f",
    "for",
    "g",
    "goto",
    "h",
    "halt",
    "hang",
    "i",
    "if",
    "j",
    "job",
    "k",
    "kill",
    "l",
    "lock",
    "m",
    "merge",
    "n",
    "new",
    "o",
    "open",
    "q",
    "quit",
    "r",
    "read",
    "return",
    "s",
    "set",
    "tc",
    "tcommit",
    "throw",
    "tro",
    "trollback",
    "try",
    "ts",
    "tstart",
    "u",
    "use",
    "v",
    "view",
    "w",
    "while",
    "write",
    "x",
    "xecute",
    "zb",
    "zbreak",
    "zi",
    "zinsert",
    "zk",
    "zkill",
    "zl",
    "zload",
    "zn",
    "znspace",
    "zp",
    "zprint",
    "zr",
    "zremove",
    "zs",
    "zsave",
    "zt",
    "ztrap",
    "zw",
    "zwrite",
    "zzdump",
    "zzprint",
    "zzwrite",
];

/// Commands whose argument is a routine, so the `^NAME` after one is a routine
/// call rather than a global.
const CALL_COMMANDS: &[&str] = &["d", "do", "g", "goto", "j", "job"];

/// Finds the ObjectScript tokens in one row of cells.
///
/// Strings win over everything: a `^` inside quotes is a piece delimiter, not a
/// global, which matters because ERP data is full of `"a^b^c"`.
pub fn scan(cells: &[Cell]) -> Vec<Span> {
    let chars: Vec<char> = cells.iter().map(|c| c.ch).collect();
    scan_chars(&chars)
}

fn scan_chars(chars: &[char]) -> Vec<Span> {
    let code_from = code_start(chars);
    let mut spans: Vec<Span> = Vec::new();
    let mut i = 0;

    while i < chars.len() {
        let ch = chars[i];
        // Whether this column is part of something the user typed at a prompt.
        let code = i >= code_from;

        match ch {
            '"' => {
                let end = string_end(chars, i);
                push(&mut spans, i, end, Kind::Str);
                i = end;
            }
            '^' => {
                let end = name_end(chars, i + 1);
                if end > i + 1 {
                    // `Tag^ROU` and `do ^ROU` name a routine; a bare `^ABC`
                    // sitting in output is a global.
                    let kind = if follows_name(chars, i) || follows_call_command(chars, i) {
                        Kind::Routine
                    } else {
                        Kind::Global
                    };
                    push(&mut spans, i, end, kind);
                    i = end;
                } else {
                    // A lone `^` is the piece delimiter, or the `^` operator.
                    if code {
                        push(&mut spans, i, i + 1, Kind::Operator);
                    }
                    i += 1;
                }
            }
            '$' => i = scan_dollar(chars, i, &mut spans),
            '#' => i = scan_hash(chars, i, code, &mut spans),
            '.' if code => {
                let attribute = chars.get(i + 1) == Some(&'.');
                let name_at = if attribute { i + 2 } else { i + 1 };
                let end = name_end(chars, name_at);
                // `..Prop` stands on its own; `obj.Prop` needs the object.
                let anchored = attribute
                    || (i > 0 && (is_name(chars[i - 1]) || matches!(chars[i - 1], ')' | '%')));
                if end > name_at && anchored {
                    let kind = if chars.get(end) == Some(&'(') {
                        Kind::ObjectMethod
                    } else if attribute {
                        Kind::ObjectAttribute
                    } else {
                        Kind::ObjectMember
                    };
                    // The name only. The dots are punctuation between the
                    // object and what is being reached for, and colouring them
                    // as part of the name makes `obj.Prop` read as one word.
                    push(&mut spans, name_at, end, kind);
                    i = end;
                } else {
                    i += 1;
                }
            }
            c if c.is_ascii_digit() => {
                let end = number_end(chars, i);
                push(&mut spans, i, end, Kind::Number);
                i = end;
            }
            c if is_name_start(c) => {
                let end = name_end(chars, i);
                if chars.get(end) == Some(&'^') {
                    // The tag of `Tag^ROUTINE`.
                    push(&mut spans, i, end, Kind::Label);
                } else if code && is_command(&chars[i..end]) && stands_alone(chars, end) {
                    push(&mut spans, i, end, Kind::Command);
                }
                i = end;
            }
            '(' | ')' | ',' | ':' | ';' if code => {
                push(&mut spans, i, i + 1, Kind::Delimiter);
                i += 1;
            }
            c if code && is_operator(c) => {
                let mut end = i + 1;
                while end < chars.len() && is_operator(chars[end]) {
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

/// Column at which typed code begins: everything up to and including an IRIS
/// prompt is output, and colouring `set` in a sentence is noise.
///
/// Returns the row's length when there is no prompt on it, which switches the
/// command-line rules off for that row rather than guessing.
fn code_start(chars: &[char]) -> usize {
    prompt_end(chars).unwrap_or(chars.len())
}

/// Column just past an IRIS prompt on this row, or `None` when there is not
/// one.
///
/// Shared with [`crate::term::lineedit`]: the same test decides whether a row
/// is a command line for colouring and whether Home, End and Up belong to the
/// line being typed rather than to IRIS.
pub fn prompt_end(chars: &[char]) -> Option<usize> {
    prompt_end_by(chars.len(), |i| chars[i])
}

/// The same test over a row of cells, so a caller holding those does not have
/// to collect them into a `Vec<char>` to ask.
pub fn prompt_end_of(cells: &[Cell]) -> Option<usize> {
    prompt_end_by(cells.len(), |i| cells[i].ch)
}

/// One forward pass: the head must be entirely made of prompt characters and
/// end on an alphanumeric, both of which can be carried along as it goes.
///
/// The first `>` decides the row either way. `>` is not itself a prompt
/// character, so a head that fails here still contains whatever spoiled it when
/// a later `>` looks back over the same columns.
fn prompt_end_by(len: usize, at: impl Fn(usize) -> char) -> Option<usize> {
    let mut prev_alphanumeric = false;
    for i in 0..len {
        let ch = at(i);
        // `USER>`, `%SYS>`, and the `TL1:USER>` a transaction adds.
        if ch == '>' {
            return prev_alphanumeric.then_some(i + 1);
        }
        if !(ch.is_ascii_alphanumeric() || matches!(ch, '%' | '^' | '_' | '-' | '.' | ':')) {
            return None;
        }
        prev_alphanumeric = ch.is_ascii_alphanumeric();
    }
    None
}

/// End of the string starting at `open`, quote included.
///
/// An unterminated string runs to the end of the row: output is routinely cut
/// off at the right margin mid-value.
fn string_end(chars: &[char], open: usize) -> usize {
    let mut i = open + 1;
    while i < chars.len() {
        if chars[i] == '"' {
            // ObjectScript escapes a quote by doubling it, so a pair is
            // content and the string carries on.
            if chars.get(i + 1) == Some(&'"') {
                i += 2;
                continue;
            }
            return i + 1;
        }
        i += 1;
    }
    chars.len()
}

fn is_name_start(c: char) -> bool {
    c.is_ascii_alphabetic() || c == '%'
}

fn is_name(c: char) -> bool {
    c.is_ascii_alphanumeric()
}

fn is_operator(c: char) -> bool {
    matches!(
        c,
        '=' | '+'
            | '-'
            | '*'
            | '/'
            | '\\'
            | '\''
            | '!'
            | '&'
            | '_'
            | '<'
            | '>'
            | '?'
            | '@'
            | '['
            | ']'
    )
}

/// End of the identifier starting at `from`, or `from` itself when there is
/// none. A leading `%` is part of the name; a later one is not.
fn name_end(chars: &[char], from: usize) -> usize {
    if !chars.get(from).copied().is_some_and(is_name_start) {
        return from;
    }
    let mut i = from + 1;
    while chars.get(i).copied().is_some_and(is_name) {
        i += 1;
    }
    i
}

/// End of the number starting at `from`, decimal point included.
fn number_end(chars: &[char], from: usize) -> usize {
    let mut i = from;
    while chars.get(i).is_some_and(|c| c.is_ascii_digit()) {
        i += 1;
    }
    // A `.` only continues the number when a digit follows it, so the full stop
    // ending a sentence is not swallowed.
    if chars.get(i) == Some(&'.') && chars.get(i + 1).is_some_and(|c| c.is_ascii_digit()) {
        i += 1;
        while chars.get(i).is_some_and(|c| c.is_ascii_digit()) {
            i += 1;
        }
    }
    i
}

fn follows_name(chars: &[char], at: usize) -> bool {
    at > 0 && (is_name(chars[at - 1]) || chars[at - 1] == '%')
}

/// Whether the `^` at `at` is the argument of `do`, `goto` or `job`.
fn follows_call_command(chars: &[char], at: usize) -> bool {
    let mut end = at;
    while end > 0 && chars[end - 1] == ' ' {
        end -= 1;
    }
    if end == at {
        return false;
    }
    let mut start = end;
    while start > 0 && is_name(chars[start - 1]) {
        start -= 1;
    }
    word_is(&chars[start..end], CALL_COMMANDS)
}

fn is_command(word: &[char]) -> bool {
    word_is(word, COMMANDS)
}

fn word_is(word: &[char], list: &[&str]) -> bool {
    if word.is_empty() || word.len() > 9 {
        return false;
    }
    // Compared char by char rather than by lowercasing into a `String` first:
    // this runs on every identifier of every row of every frame, and the list
    // is pure ASCII, so a non-ASCII char simply matches nothing.
    list.iter().any(|candidate| {
        candidate.len() == word.len()
            && candidate
                .bytes()
                .zip(word)
                .all(|(b, c)| char::from(b) == c.to_ascii_lowercase())
    })
}

/// Whether a word ending at `end` is being used as a command rather than as
/// part of an expression. `set` is a command; the `s` of `s=1` is a variable,
/// and `write(x)` is a function call someone wrote in another language.
fn stands_alone(chars: &[char], end: usize) -> bool {
    match chars.get(end) {
        // `w "hi"`, `w"hi"`, `i:x=1 ...` — a command may butt straight up
        // against its argument.
        None => true,
        Some(c) => matches!(c, ' ' | ':' | '"' | '^' | '$' | '#' | '@' | '!'),
    }
}

/// `$$$MACRO`, `$$Tag`, `$Function(` and `$SystemVariable`.
fn scan_dollar(chars: &[char], at: usize, spans: &mut Vec<Span>) -> usize {
    if chars.get(at + 1) == Some(&'$') && chars.get(at + 2) == Some(&'$') {
        let end = name_end(chars, at + 3);
        if end > at + 3 {
            push(spans, at, end, Kind::PreProcessor);
            return end;
        }
        return at + 1;
    }
    if chars.get(at + 1) == Some(&'$') {
        let end = name_end(chars, at + 2);
        if end > at + 2 {
            push(spans, at, end, Kind::Extrinsic);
            return end;
        }
        return at + 1;
    }
    let end = name_end(chars, at + 1);
    if end > at + 1 {
        // Being called is what separates `$piece(x,"^")` from `$horolog`.
        let kind = if chars.get(end) == Some(&'(') {
            Kind::Function
        } else {
            Kind::SystemVariable
        };
        push(spans, at, end, kind);
        return end;
    }
    at + 1
}

/// `##class(Pkg.Cls)` and the preprocessor directives.
fn scan_hash(chars: &[char], at: usize, code: bool, spans: &mut Vec<Span>) -> usize {
    if chars.get(at + 1) == Some(&'#') {
        let word_at = at + 2;
        let word_end = name_end(chars, word_at);
        if word_end > word_at {
            // `##class` and `##super` are keywords; the class name they take is
            // dotted, so it is read separately from an ordinary identifier.
            push(spans, at, word_end, Kind::Command);
            if chars.get(word_end) == Some(&'(') {
                let start = word_end + 1;
                let mut i = start;
                while chars
                    .get(i)
                    .is_some_and(|c| is_name(*c) || matches!(c, '.' | '%'))
                {
                    i += 1;
                }
                push(spans, start, i, Kind::ObjectClass);
                return i;
            }
            return word_end;
        }
    }
    if code {
        let end = name_end(chars, at + 1);
        if end > at + 1 {
            // `#dim`, `#define`, `#include`.
            push(spans, at, end, Kind::PreProcessor);
            return end;
        }
        // `#` on its own is the modulo operator — and the argument of `w #`.
        push(spans, at, at + 1, Kind::Operator);
    }
    at + 1
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(text: &str) -> Vec<(Kind, String)> {
        let chars: Vec<char> = text.chars().collect();
        scan_chars(&chars)
            .into_iter()
            .map(|s| (s.kind, chars[s.start..s.end].iter().collect()))
            .collect()
    }

    /// The renderer's entry point takes cells, so it has to keep working too.
    #[test]
    fn scanning_cells_matches_scanning_text() {
        let cells: Vec<Cell> = r#"zw ^CCDU"#
            .chars()
            .map(|ch| Cell {
                ch,
                ..Cell::default()
            })
            .collect();
        assert_eq!(
            scan(&cells),
            vec![Span {
                start: 3,
                end: 8,
                kind: Kind::Global
            }]
        );
    }

    #[test]
    fn a_global_reference_is_found_without_its_subscripts() {
        assert_eq!(kinds("zw ^CCDU"), vec![(Kind::Global, "^CCDU".to_string())]);
    }

    #[test]
    fn a_percent_global_is_a_global() {
        assert_eq!(
            kinds("^%CSW1GATCUST"),
            vec![(Kind::Global, "^%CSW1GATCUST".to_string())]
        );
    }

    /// The case that makes the "letter after the caret" rule necessary: `^` is
    /// also the piece delimiter, and ERP values are full of it.
    #[test]
    fn a_piece_delimiter_is_not_a_global() {
        let spans = kinds("300^1^1^67222^48220");
        assert!(
            spans.iter().all(|(kind, _)| *kind == Kind::Number),
            "{spans:?}"
        );
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
                (Kind::Number, "1".to_string()),
                (Kind::Number, "1".to_string()),
                (Kind::Str, r#""1^350^2^2^64329""#.to_string()),
            ]
        );
    }

    /// A caret with no name after it is the delimiter or the operator, never a
    /// global - the digit is the only thing coloured here.
    #[test]
    fn a_lone_caret_is_left_alone() {
        assert_eq!(kinds("^ ^^ ^1"), vec![(Kind::Number, "1".to_string())]);
    }

    /// Command words only count on a command line, which is what the prompt
    /// marks. Prose that happens to contain one is left alone.
    #[test]
    fn a_command_is_only_a_command_after_a_prompt() {
        assert_eq!(
            kinds("USER>set x=1"),
            vec![
                (Kind::Command, "set".to_string()),
                (Kind::Operator, "=".to_string()),
                (Kind::Number, "1".to_string()),
            ]
        );
        assert!(kinds("Please set the flag").is_empty());
    }

    /// The abbreviations are the whole point of a terminal, and `w #` - the
    /// clear-screen - is the shortest command line there is.
    #[test]
    fn abbreviations_and_their_arguments_are_recognised() {
        assert_eq!(
            kinds("%SYS>w #"),
            vec![
                (Kind::Command, "w".to_string()),
                (Kind::Operator, "#".to_string()),
            ]
        );
    }

    /// A variable that happens to be spelled like an abbreviation is not a
    /// command: `s=1` assigns, `s x=1` sets.
    #[test]
    fn a_word_in_an_expression_is_not_a_command() {
        assert_eq!(
            kinds("USER>w s=1"),
            vec![
                (Kind::Command, "w".to_string()),
                (Kind::Operator, "=".to_string()),
                (Kind::Number, "1".to_string()),
            ]
        );
    }

    #[test]
    fn a_tag_and_its_routine_are_told_apart() {
        assert_eq!(
            kinds("GetScrollableRS^%CSW1APICONTROLLER"),
            vec![
                (Kind::Label, "GetScrollableRS".to_string()),
                (Kind::Routine, "^%CSW1APICONTROLLER".to_string()),
            ]
        );
    }

    /// `do ^ROU` runs a routine; the same text without the command is a global
    /// being written or read.
    #[test]
    fn a_call_argument_is_a_routine_rather_than_a_global() {
        assert_eq!(
            kinds("USER>do ^%CSW1GATCUST")
                .into_iter()
                .map(|(k, _)| k)
                .collect::<Vec<_>>(),
            vec![Kind::Command, Kind::Routine]
        );
        assert_eq!(
            kinds("USER>zw ^%CSW1GATCUST")
                .into_iter()
                .map(|(k, _)| k)
                .collect::<Vec<_>>(),
            vec![Kind::Command, Kind::Global]
        );
    }

    #[test]
    fn dollar_constructs_split_four_ways() {
        assert_eq!(
            kinds("$$$OK $horolog $piece(x) $$Tag^ROU"),
            vec![
                (Kind::PreProcessor, "$$$OK".to_string()),
                (Kind::SystemVariable, "$horolog".to_string()),
                (Kind::Function, "$piece".to_string()),
                (Kind::Extrinsic, "$$Tag".to_string()),
                (Kind::Routine, "^ROU".to_string()),
            ]
        );
    }

    #[test]
    fn a_class_reference_names_its_class() {
        let spans = kinds("USER>w ##class(Src2.Classe).%New()");
        assert!(
            spans.contains(&(Kind::Command, "##class".to_string())),
            "{spans:?}"
        );
        assert!(
            spans.contains(&(Kind::ObjectClass, "Src2.Classe".to_string())),
            "{spans:?}"
        );
        // The dot is punctuation, not part of the method name.
        assert!(
            spans.contains(&(Kind::ObjectMethod, "%New".to_string())),
            "{spans:?}"
        );
    }

    #[test]
    fn members_methods_and_attributes_are_told_apart() {
        let spans = kinds("USER>w obj.Nome_..Codigo_obj.Calcular()");
        assert!(
            spans.contains(&(Kind::ObjectMember, "Nome".to_string())),
            "{spans:?}"
        );
        assert!(
            spans.contains(&(Kind::ObjectAttribute, "Codigo".to_string())),
            "{spans:?}"
        );
        assert!(
            spans.contains(&(Kind::ObjectMethod, "Calcular".to_string())),
            "{spans:?}"
        );
        // And nothing coloured starts on a dot.
        assert!(
            !spans.iter().any(|(_, text)| text.starts_with('.')),
            "a dot was coloured: {spans:?}"
        );
    }

    /// A dotted name in ordinary output is not an object reference: there is no
    /// prompt on the row, so the member rules never run.
    #[test]
    fn a_file_name_in_output_is_not_an_object_member() {
        assert!(kinds("Loaded config.toml").is_empty());
    }

    #[test]
    fn a_decimal_is_one_number_and_a_full_stop_is_not_part_of_it() {
        assert_eq!(
            kinds("Total 1234.56 done."),
            vec![(Kind::Number, "1234.56".to_string())]
        );
    }

    /// A number inside an identifier belongs to the identifier.
    #[test]
    fn digits_inside_a_name_are_not_a_number() {
        assert_eq!(kinds("CCTTGPRG005"), Vec::new());
        assert_eq!(
            kinds("^CCTTGPRG005"),
            vec![(Kind::Global, "^CCTTGPRG005".to_string())]
        );
    }

    #[test]
    fn a_preprocessor_directive_is_found_on_a_command_line() {
        assert_eq!(
            kinds("USER>#dim conta"),
            vec![(Kind::PreProcessor, "#dim".to_string())]
        );
    }

    /// The prompt itself is output, not code: nothing before the `>` may be
    /// coloured as a command.
    #[test]
    fn the_prompt_is_not_part_of_the_command_line() {
        let spans = kinds("TL1:USER>w 1");
        assert_eq!(
            spans,
            vec![
                (Kind::Command, "w".to_string()),
                (Kind::Number, "1".to_string()),
            ]
        );
    }
}
