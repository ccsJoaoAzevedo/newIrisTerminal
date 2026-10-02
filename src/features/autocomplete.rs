//! Suggesting the rest of the word being typed at an IRIS prompt.
//!
//! The far side owns the line, so a suggestion is never put into it by this
//! side. What is typed is read back off the screen, the word under the cursor
//! is matched against what could follow it, and accepting a suggestion sends
//! only the characters that are missing - plus, for a name whose case matters,
//! the rubouts that take back the ones typed in the wrong case first. Nothing
//! else is ever sent, and nothing at all while the cursor is anywhere but the
//! end of the line: inserting mid-line would mean trusting IRIS to be in insert
//! rather than replace mode, which it never reports.
//!
//! Where the names come from:
//!
//! - fixed lists: the commands [`crate::term::syntax`] already knows, the `$`
//!   functions and system variables, the `$SYSTEM` classes, and the SQL
//!   keywords and functions of [`crate::term::sql`];
//! - the commands this app has seen run, from the history and from every line
//!   recorded since - which is where `^GLOBAL`s, routines, `$$Tag^ROUTINE`
//!   entry points, `##class(...)` names and SQL tables come from;
//! - what is on the screen right now, which catches a name that has just been
//!   printed by a `zwrite` or a listing.
//!
//! The server is never asked. A side session in the way of
//! [`crate::features::doc_lookup`] could list every class and global, but it
//! costs a licence slot and a login, and a name that has never once appeared
//! in front of the user is rarely the one they are reaching for.
//!
//! Recomputed only when the line on screen changes, and only after the user
//! has typed: a frame that draws nothing new asks for nothing here.

use std::collections::{BTreeSet, HashMap};

use crate::term::syntax::{self, Kind};
use crate::term::{lineedit, sql, Grid};

/// How many suggestions the popup holds. Enough to choose between; more is a
/// list to read rather than a hint.
pub const MAX_SHOWN: usize = 10;

/// How many names of each sort are remembered. A session that prints a large
/// listing must not grow this without bound.
const MAX_NAMES: usize = 4_000;

/// What the word under the cursor is, judged from what comes before it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Context {
    /// A bare word where a command goes: the start of the line, or after a
    /// space.
    Command,
    /// After a single `$`: a function or a system variable.
    Dollar,
    /// After `$SYSTEM.`: one of its classes.
    System,
    /// After `$$`: an entry point, `Tag^ROUTINE`.
    Extrinsic,
    /// After `^`. `routine` when the caret names one - after a tag, or as the
    /// argument of `do`, `goto` or `job` - and a global otherwise.
    Caret { routine: bool },
    /// Inside `##class(`.
    Class,
    /// Anything at the SQL shell's prompt.
    Sql,
}

/// The word being completed: what it is, and the part of it a suggestion
/// replaces - which leaves out the sigil, so `$pi` is `pi`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Token {
    pub context: Context,
    pub text: String,
}

/// What a suggestion is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Command,
    Function,
    SystemVariable,
    SystemClass,
    Class,
    Global,
    Routine,
    /// `Tag^ROUTINE`, as `$$` calls it.
    Entry,
    SqlKeyword,
    SqlFunction,
    Table,
}

impl Category {
    /// Whether IRIS reads this name whatever its case. A command, a `$`
    /// function and an SQL keyword are; a global, a routine and a class are
    /// not, and `^csw` is a different global from `^CSW`.
    pub fn case_insensitive(self) -> bool {
        matches!(
            self,
            Category::Command
                | Category::Function
                | Category::SystemVariable
                | Category::SqlKeyword
                | Category::SqlFunction
        )
    }

    /// What goes in front of the name when it is shown, so the popup reads the
    /// way the line will.
    pub fn sigil(self) -> &'static str {
        match self {
            Category::Function | Category::SystemVariable => "$",
            Category::SystemClass => "$SYSTEM.",
            Category::Global | Category::Routine => "^",
            Category::Entry => "$$",
            _ => "",
        }
    }

    /// The small word beside each suggestion. English, for `tr`.
    pub fn label(self) -> &'static str {
        match self {
            Category::Command => "command",
            Category::Function | Category::SqlFunction => "function",
            Category::SystemVariable => "system variable",
            Category::SystemClass | Category::Class => "class",
            Category::Global => "global",
            Category::Routine => "routine",
            Category::Entry => "entry point",
            Category::SqlKeyword => "keyword",
            Category::Table => "table",
        }
    }
}

/// One suggestion.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Candidate {
    /// The whole word the token becomes, without its sigil.
    pub text: String,
    pub category: Category,
}

impl Candidate {
    /// As shown in the popup.
    pub fn display(&self) -> String {
        format!("{}{}", self.category.sigil(), self.text)
    }
}

/// What accepting a suggestion sends: rubouts first, then the characters.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Edit {
    pub rubouts: usize,
    pub insert: String,
}

/// The commands offered: the full names, plus the two that are already as
/// short as anyone types them. An abbreviation is something typed, not
/// something worth suggesting.
fn commands() -> impl Iterator<Item = &'static str> {
    syntax::COMMANDS
        .iter()
        .copied()
        .filter(|c| c.len() > 2 || matches!(*c, "do" | "if"))
}

const DOLLAR_FUNCTIONS: &[&str] = &[
    "ASCII",
    "BIT",
    "BITCOUNT",
    "BITFIND",
    "BITLOGIC",
    "CASE",
    "CHAR",
    "CLASSMETHOD",
    "CLASSNAME",
    "DATA",
    "DECIMAL",
    "DOUBLE",
    "EXTRACT",
    "FACTOR",
    "FIND",
    "FNUMBER",
    "GET",
    "INCREMENT",
    "INUMBER",
    "ISOBJECT",
    "ISVALIDNUM",
    "JUSTIFY",
    "LENGTH",
    "LIST",
    "LISTBUILD",
    "LISTDATA",
    "LISTFIND",
    "LISTFROMSTRING",
    "LISTGET",
    "LISTLENGTH",
    "LISTNEXT",
    "LISTSAME",
    "LISTTOSTRING",
    "LISTUPDATE",
    "LISTVALID",
    "LOCATE",
    "MATCH",
    "METHOD",
    "NAME",
    "NCONVERT",
    "NEXT",
    "NORMALIZE",
    "NOW",
    "NUMBER",
    "ORDER",
    "PARAMETER",
    "PIECE",
    "PROPERTY",
    "QLENGTH",
    "QSUBSCRIPT",
    "QUERY",
    "RANDOM",
    "REVERSE",
    "SCONVERT",
    "SELECT",
    "SEQUENCE",
    "SORTBEGIN",
    "SORTEND",
    "TEXT",
    "TRANSLATE",
    "VIEW",
    "WASCII",
    "WCHAR",
    "WEXTRACT",
    "WFIND",
    "WLENGTH",
    "WREVERSE",
    "XECUTE",
    "ZABS",
    "ZCONVERT",
    "ZCRC",
    "ZDATE",
    "ZDATEH",
    "ZDATETIME",
    "ZDATETIMEH",
    "ZHEX",
    "ZPOWER",
    "ZSTRIP",
    "ZTIME",
    "ZTIMEH",
];

const DOLLAR_VARIABLES: &[&str] = &[
    "DEVICE",
    "ECODE",
    "ESTACK",
    "ETRAP",
    "HALT",
    "HOROLOG",
    "IO",
    "JOB",
    "KEY",
    "NAMESPACE",
    "PRINCIPAL",
    "QUIT",
    "ROLES",
    "STACK",
    "STORAGE",
    "SYSTEM",
    "TEST",
    "THIS",
    "THROWOBJ",
    "TLEVEL",
    "USERNAME",
    "ZA",
    "ZB",
    "ZCHILD",
    "ZEOF",
    "ZERROR",
    "ZHOROLOG",
    "ZIO",
    "ZJOB",
    "ZMODE",
    "ZNAME",
    "ZNSPACE",
    "ZPARENT",
    "ZPI",
    "ZREFERENCE",
    "ZSTORAGE",
    "ZTIMESTAMP",
    "ZTIMEZONE",
    "ZTRAP",
    "ZVERSION",
];

const SYSTEM_CLASSES: &[&str] = &[
    "Backup",
    "CSP",
    "Config",
    "Encryption",
    "Error",
    "Event",
    "ICU",
    "INetInfo",
    "License",
    "Mirror",
    "Monitor",
    "OBJ",
    "Process",
    "Python",
    "SQL",
    "SYS",
    "Security",
    "Semaphore",
    "Status",
    "Task",
    "Util",
    "Version",
    "WorkMgr",
];

/// System classes worth offering inside `##class(` before the session has
/// shown any of its own.
const CLASSES: &[&str] = &[
    "%Dictionary.ClassDefinition",
    "%Dictionary.CompiledClass",
    "%DynamicArray",
    "%DynamicObject",
    "%File",
    "%Library.File",
    "%Net.HttpRequest",
    "%Regex.Matcher",
    "%SQL.Statement",
    "%Stream.FileCharacter",
    "%Stream.GlobalCharacter",
    "%SYS.Namespace",
    "%SYSTEM.Process",
    "%SYSTEM.SQL",
    "%SYSTEM.Status",
];

/// The word the cursor is at the end of, and what kind of word it is.
///
/// `before` is what was typed up to the cursor. `None` for anything not worth
/// suggesting for: inside a string, after a member's dot, a number, or a word
/// too short to narrow anything down.
pub fn token_at(before: &[char], sql: bool) -> Option<Token> {
    if sql {
        return sql_token(before);
    }
    if inside_quotes(before, '"') {
        return None;
    }
    let name_start = run_start(before, |c| c.is_ascii_alphanumeric() || c == '%');
    let name: String = before[name_start..].iter().collect();
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        return None;
    }
    let head = &before[..name_start];
    let token = |context, text: String| Some(Token { context, text });

    // A class name is dotted, so it is measured again with the dots in.
    let class_start = run_start(before, |c| {
        c.is_ascii_alphanumeric() || matches!(c, '%' | '.')
    });
    if ends_with_ci(&before[..class_start], "##class(") {
        return token(Context::Class, before[class_start..].iter().collect());
    }
    if ends_with_ci(head, "$system.") {
        return token(Context::System, name);
    }
    if name.is_empty() {
        return None;
    }
    if ends_with_ci(head, "$$$") {
        // A macro: defined in an include file this side has never seen.
        return None;
    }
    if ends_with_ci(head, "$$") {
        return token(Context::Extrinsic, name);
    }
    if ends_with_ci(head, "$") {
        return token(Context::Dollar, name);
    }
    if ends_with_ci(head, "^") {
        let caret = head.len() - 1;
        let after_tag = caret > 0 && (head[caret - 1].is_ascii_alphanumeric());
        return token(
            Context::Caret {
                routine: after_tag || after_call_command(&head[..caret]),
            },
            name,
        );
    }
    let command_position = head.last().is_none_or(|c| *c == ' ');
    if command_position && name.chars().count() >= 2 {
        return token(Context::Command, name);
    }
    None
}

/// The SQL shell's word: a name, which may be qualified - `Sample.Per`.
fn sql_token(before: &[char]) -> Option<Token> {
    if inside_quotes(before, '\'') || inside_quotes(before, '"') {
        return None;
    }
    let start = run_start(before, |c| sql::is_word(c) || matches!(c, '%' | '$' | '.'));
    let word = &before[start..];
    if !word.first().copied().is_some_and(sql::is_word_start) {
        return None;
    }
    // A host variable is the caller's name, not the shell's.
    if start > 0 && before[start - 1] == ':' {
        return None;
    }
    (word.len() >= 2).then(|| Token {
        context: Context::Sql,
        text: word.iter().collect(),
    })
}

fn inside_quotes(chars: &[char], quote: char) -> bool {
    chars.iter().filter(|c| **c == quote).count() % 2 == 1
}

/// Where the trailing run of characters satisfying `keep` begins.
fn run_start(chars: &[char], keep: impl Fn(char) -> bool) -> usize {
    let mut i = chars.len();
    while i > 0 && keep(chars[i - 1]) {
        i -= 1;
    }
    i
}

fn ends_with_ci(chars: &[char], tail: &str) -> bool {
    let tail: Vec<char> = tail.chars().collect();
    chars.len() >= tail.len()
        && chars[chars.len() - tail.len()..]
            .iter()
            .zip(&tail)
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
}

/// Whether what precedes a `^` is `do`, `goto` or `job`, which make what
/// follows the caret a routine.
fn after_call_command(head: &[char]) -> bool {
    let end = run_start(head, |c| c == ' ');
    if end == head.len() {
        return false;
    }
    let start = run_start(&head[..end], |c| c.is_ascii_alphabetic());
    let word: String = head[start..end].iter().collect::<String>().to_lowercase();
    matches!(word.as_str(), "d" | "do" | "g" | "goto" | "j" | "job")
}

/// Names this side has seen, for the sorts of word no fixed list can cover.
#[derive(Debug, Default)]
pub struct Vocabulary {
    globals: BTreeSet<String>,
    routines: BTreeSet<String>,
    classes: BTreeSet<String>,
    entries: BTreeSet<String>,
    tables: BTreeSet<String>,
    /// How often each name has been seen, by its displayed spelling in lower
    /// case - which is what lifts the `$PIECE` everybody types above the
    /// `$PARAMETER` nobody does.
    uses: HashMap<String, u32>,
}

impl Vocabulary {
    /// Takes the names out of one command, as it was run.
    pub fn harvest_line(&mut self, text: &str) {
        let chars: Vec<char> = text.chars().collect();
        self.harvest(&chars);
    }

    /// Takes the names out of what is on the screen, other than the row being
    /// typed on - which would otherwise teach it every prefix of the word on
    /// its way to being typed.
    pub fn harvest_screen(&mut self, grid: &Grid) {
        let mut chars = Vec::new();
        for (index, row) in grid.screen.iter().enumerate() {
            let used = row.used_width();
            if index == grid.cursor.row || used == 0 {
                continue;
            }
            chars.clear();
            chars.extend(row.cells[..used.min(row.cells.len())].iter().map(|c| c.ch));
            self.harvest(&chars);
        }
    }

    fn harvest(&mut self, chars: &[char]) {
        // A row at the SQL shell's prompt is a statement, and a history line
        // that starts like one is one too: the prompt is not recorded.
        let (sql_from, is_sql) = match syntax::prompt_in(chars) {
            Some(prompt) => (prompt.end, prompt.sql),
            None => (0, starts_like_sql(chars)),
        };
        if is_sql {
            for span in sql::scan(chars, sql_from) {
                if span.kind == Kind::ObjectClass {
                    let name: String = chars[span.start..span.end].iter().collect();
                    self.count(&name);
                    insert(&mut self.tables, name);
                }
            }
            return;
        }

        let spans = syntax::scan_text(chars);
        for (n, span) in spans.iter().enumerate() {
            let text: String = chars[span.start..span.end].iter().collect();
            match span.kind {
                Kind::Global => {
                    self.count(&text);
                    insert(&mut self.globals, text[1..].to_string());
                }
                Kind::Routine => {
                    self.count(&text);
                    if let Some(tag) = n
                        .checked_sub(1)
                        .map(|p| spans[p])
                        .filter(|p| p.end == span.start)
                        .filter(|p| matches!(p.kind, Kind::Label | Kind::Extrinsic))
                    {
                        let tag: String = chars[tag.start..tag.end].iter().collect();
                        let entry = format!("{}{text}", tag.trim_start_matches('$'));
                        self.count(&format!("$${entry}"));
                        insert(&mut self.entries, entry);
                    }
                    insert(&mut self.routines, text[1..].to_string());
                }
                Kind::ObjectClass => {
                    self.count(&text);
                    insert(&mut self.classes, text);
                }
                Kind::Function | Kind::SystemVariable => self.count(&text),
                _ => {}
            }
        }
        // The command a line starts with. Commands are only recognised after a
        // prompt, and a recorded line has none, so it is read here instead.
        let from = syntax::prompt_end(chars).unwrap_or(0);
        let first: String = chars[from.min(chars.len())..]
            .iter()
            .skip_while(|c| **c == ' ')
            .take_while(|c| c.is_ascii_alphabetic())
            .collect();
        if !first.is_empty() && commands().any(|c| c.eq_ignore_ascii_case(&first)) {
            self.count(&first);
        }
    }

    fn count(&mut self, display: &str) {
        let key = display.to_lowercase();
        if self.uses.len() < MAX_NAMES || self.uses.contains_key(&key) {
            *self.uses.entry(key).or_default() += 1;
        }
    }

    fn uses(&self, candidate: &Candidate) -> u32 {
        self.uses
            .get(&candidate.display().to_lowercase())
            .copied()
            .unwrap_or(0)
    }
}

fn insert(set: &mut BTreeSet<String>, name: String) {
    if set.len() < MAX_NAMES {
        set.insert(name);
    }
}

fn starts_like_sql(chars: &[char]) -> bool {
    let first: String = chars
        .iter()
        .skip_while(|c| **c == ' ')
        .take_while(|c| c.is_ascii_alphabetic())
        .collect();
    ["select", "insert", "update", "delete", "with"]
        .iter()
        .any(|w| first.eq_ignore_ascii_case(w))
}

/// What could complete `token`, best first, at most [`MAX_SHOWN`] of them.
///
/// Matched as a prefix, ignoring case. Best is, in order: a name typed so far
/// in the right case, for the names where case matters; the sort the context
/// most likely means; the name seen most often; the shortest; the first
/// alphabetically. A name the token already spells in full is not offered:
/// there is nothing left to complete.
pub fn candidates<'a>(token: &Token, vocabulary: &'a Vocabulary) -> Vec<Candidate> {
    let typed: Vec<char> = token.text.chars().collect();
    type Pool<'a> = Vec<(u8, Category, &'a str)>;
    // Typed in the wrong case sorts last; then preference, most used, shortest.
    type Rank = (bool, u8, std::cmp::Reverse<u32>, usize);
    fn add<'a, 'b: 'a>(
        pool: &mut Pool<'a>,
        preference: u8,
        category: Category,
        names: impl Iterator<Item = &'b str>,
    ) {
        pool.extend(names.map(|name| (preference, category, name as &'a str)));
    }
    let mut pool: Pool<'a> = Vec::new();
    let p = &mut pool;
    let v = vocabulary;
    match token.context {
        Context::Command => add(p, 0, Category::Command, commands()),
        Context::Dollar => {
            add(p, 0, Category::Function, DOLLAR_FUNCTIONS.iter().copied());
            add(
                p,
                0,
                Category::SystemVariable,
                DOLLAR_VARIABLES.iter().copied(),
            );
        }
        Context::System => add(p, 0, Category::SystemClass, SYSTEM_CLASSES.iter().copied()),
        Context::Extrinsic => add(p, 0, Category::Entry, v.entries.iter().map(String::as_str)),
        Context::Caret { routine } => {
            let (routines, globals) = if routine { (0, 1) } else { (1, 0) };
            add(
                p,
                routines,
                Category::Routine,
                v.routines.iter().map(String::as_str),
            );
            add(
                p,
                globals,
                Category::Global,
                v.globals.iter().map(String::as_str),
            );
        }
        Context::Class => {
            add(p, 0, Category::Class, v.classes.iter().map(String::as_str));
            add(p, 1, Category::Class, CLASSES.iter().copied());
        }
        Context::Sql => {
            add(p, 0, Category::SqlKeyword, sql::KEYWORDS.iter().copied());
            add(p, 0, Category::SqlFunction, sql::FUNCTIONS.iter().copied());
            add(p, 0, Category::Table, v.tables.iter().map(String::as_str));
        }
    }

    let mut ranked: Vec<(Rank, Candidate)> = pool
        .into_iter()
        .filter(|(_, _, name)| {
            let name: Vec<char> = name.chars().collect();
            name.len() > typed.len() && prefix_ci(&typed, &name)
        })
        .map(|(preference, category, name)| {
            let candidate = Candidate {
                text: name.to_string(),
                category,
            };
            let wrong_case = !category.case_insensitive() && !name.starts_with(&token.text);
            let uses = vocabulary.uses(&candidate);
            (
                (
                    wrong_case,
                    preference,
                    std::cmp::Reverse(uses),
                    name.chars().count(),
                ),
                candidate,
            )
        })
        .collect();
    ranked.sort_by(|(a, x), (b, y)| a.cmp(b).then_with(|| x.text.cmp(&y.text)));

    let mut out: Vec<Candidate> = Vec::new();
    for (_, candidate) in ranked {
        // `SELECT` is both a keyword and a `$` function, and a class can be
        // both seen and listed; one line in the popup each.
        if out.iter().any(|c| c.display() == candidate.display()) {
            continue;
        }
        out.push(candidate);
        if out.len() == MAX_SHOWN {
            break;
        }
    }
    out
}

fn prefix_ci(typed: &[char], name: &[char]) -> bool {
    typed.len() <= name.len()
        && typed
            .iter()
            .zip(name)
            .all(|(a, b)| a.eq_ignore_ascii_case(b))
}

/// What to send so that `typed` becomes `candidate`.
///
/// Only the missing part, where the case does not matter - in the case the
/// user is typing in, judged by their last letter, so `sel` becomes `select`
/// and `SEL` becomes `SELECT`. Where it does matter, the characters typed in
/// the wrong case are rubbed out first and typed again: `^csw` is not `^CSW`,
/// and appending to it would name a global nobody asked for.
///
/// `None` when the candidate does not extend what was typed.
pub fn edit_for(typed: &str, candidate: &Candidate) -> Option<Edit> {
    let typed: Vec<char> = typed.chars().collect();
    let name: Vec<char> = candidate.text.chars().collect();
    if name.len() <= typed.len() || !prefix_ci(&typed, &name) {
        return None;
    }
    if candidate.category.case_insensitive() {
        let tail: String = name[typed.len()..].iter().collect();
        let insert = match typed.iter().rev().find(|c| c.is_alphabetic()) {
            Some(c) if c.is_lowercase() => tail.to_lowercase(),
            Some(_) => tail.to_uppercase(),
            None => tail,
        };
        return Some(Edit { rubouts: 0, insert });
    }
    let same = typed.iter().zip(&name).take_while(|(a, b)| a == b).count();
    Some(Edit {
        rubouts: typed.len() - same,
        insert: name[same..].iter().collect(),
    })
}

/// The suggestions open over the cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Popup {
    pub token: Token,
    pub items: Vec<Candidate>,
    pub selected: usize,
    /// The user has moved through the list, which is what lets Enter accept
    /// rather than run the line. Without it, Enter on a word that happens to
    /// have suggestions would complete it instead of submitting what was
    /// typed.
    pub navigated: bool,
}

impl Popup {
    /// Moves the selection down, or up, wrapping at either end.
    pub fn step(&mut self, forward: bool) {
        let len = self.items.len().max(1);
        self.selected = if forward {
            (self.selected + 1) % len
        } else {
            (self.selected + len - 1) % len
        };
        self.navigated = true;
    }

    pub fn chosen(&self) -> Option<&Candidate> {
        self.items.get(self.selected)
    }

    /// What accepting the selected suggestion sends.
    pub fn edit(&self) -> Option<Edit> {
        edit_for(&self.token.text, self.chosen()?)
    }
}

/// The suggestions for the line the cursor is on, if any are worth offering.
///
/// `None` off a prompt - a full-screen routine paints wherever it likes, and a
/// suggestion over it would be over somebody else's screen - and anywhere but
/// the end of the line.
pub fn suggest(grid: &Grid, vocabulary: &mut Vocabulary) -> Option<Popup> {
    let line = lineedit::current(grid)?;
    if !line.at_end() {
        return None;
    }
    let prompt = lineedit::prompt(grid)?;
    let before = typed_before_cursor(grid, line);
    let token = token_at(&before, prompt.sql)?;
    // The fixed lists need no help; the rest are only as good as what has
    // been seen, and what is on screen right now is the freshest of it.
    if !matches!(
        token.context,
        Context::Command | Context::Dollar | Context::System
    ) {
        vocabulary.harvest_screen(grid);
    }
    let items = candidates(&token, vocabulary);
    (!items.is_empty()).then_some(Popup {
        token,
        items,
        selected: 0,
        navigated: false,
    })
}

/// What was typed from the prompt to the cursor. A cursor past the end of the
/// row's cells is past trailing blanks the row does not store.
fn typed_before_cursor(grid: &Grid, line: lineedit::LineEdit) -> Vec<char> {
    let Some(row) = grid.screen.get(grid.cursor.row) else {
        return Vec::new();
    };
    (line.start..line.cursor)
        .map(|col| row.cells.get(col).map_or(' ', |c| c.ch))
        .collect()
}

/// Where on screen the line was when it was last looked at, and what it said.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Seen {
    line: usize,
    col: usize,
    text: Vec<char>,
}

/// One session's autocomplete: whether to look, and what was found.
///
/// Armed by typing and disarmed by anything else - a recall, an Enter, a
/// cursor key - so the popup follows the user's typing and never appears
/// because IRIS printed something.
#[derive(Debug, Default)]
pub struct Completion {
    armed: bool,
    seen: Option<Seen>,
    popup: Option<Popup>,
}

impl Completion {
    /// The user typed or rubbed out a character.
    pub fn arm(&mut self) {
        self.armed = true;
    }

    /// Closes the popup and stops looking until the next keystroke that types.
    pub fn close(&mut self) {
        self.armed = false;
        self.seen = None;
        self.popup = None;
    }

    pub fn popup(&self) -> Option<&Popup> {
        self.popup.as_ref()
    }

    pub fn popup_mut(&mut self) -> Option<&mut Popup> {
        self.popup.as_mut()
    }

    /// Reads the line again, and works the suggestions out again if it has
    /// changed since it was last read.
    ///
    /// Costs nothing while disarmed and closed, which is every frame but the
    /// ones a user is typing in.
    pub fn refresh(&mut self, grid: &Grid, vocabulary: &mut Vocabulary) {
        if !self.armed && self.popup.is_none() {
            return;
        }
        let Some(line) = lineedit::current(grid).filter(|line| line.at_end()) else {
            // The prompt has gone, or the cursor has left the end of the line:
            // nothing here is a word being typed any more.
            self.close();
            return;
        };
        let seen = Seen {
            line: grid.scrollback.len() + grid.cursor.row,
            col: line.cursor,
            text: typed_before_cursor(grid, line),
        };
        if self.seen.as_ref() == Some(&seen) {
            return;
        }
        self.seen = Some(seen);
        self.popup = suggest(grid, vocabulary);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chars(text: &str) -> Vec<char> {
        text.chars().collect()
    }

    fn token(text: &str) -> Option<Token> {
        token_at(&chars(text), false)
    }

    fn grid_with(text: &str) -> Grid {
        let mut grid = Grid::new(80, 3, 10);
        grid.screen[2].set_text(text);
        grid.cursor.row = 2;
        grid.cursor.col = text.chars().count();
        grid
    }

    fn shown(popup: &Popup) -> Vec<String> {
        popup.items.iter().map(Candidate::display).collect()
    }

    #[test]
    fn the_word_under_the_cursor_is_read_by_what_comes_before_it() {
        let context = |text| token(text).map(|t| t.context);
        assert_eq!(context("wr"), Some(Context::Command));
        assert_eq!(context("s x=1 wr"), Some(Context::Command));
        assert_eq!(context("w $pi"), Some(Context::Dollar));
        assert_eq!(context("w $system.sq"), Some(Context::System));
        assert_eq!(context("w $$Get"), Some(Context::Extrinsic));
        assert_eq!(context("zw ^CC"), Some(Context::Caret { routine: false }));
        assert_eq!(context("d ^CS"), Some(Context::Caret { routine: true }));
        assert_eq!(
            context("w $$Get^CS"),
            Some(Context::Caret { routine: true })
        );
        assert_eq!(context("w ##class(Src2.Cl"), Some(Context::Class));
        assert_eq!(
            token("w ##class(Src2.Cl").map(|t| t.text).as_deref(),
            Some("Src2.Cl")
        );
        assert_eq!(token("w $pi").map(|t| t.text).as_deref(), Some("pi"));
    }

    #[test]
    fn nothing_is_suggested_inside_a_string_a_macro_or_a_number() {
        assert_eq!(token("w \"wr"), None);
        assert_eq!(token("w $$$OK"), None);
        assert_eq!(token("s x=12"), None);
        assert_eq!(token("w"), None, "one letter narrows nothing");
        assert_eq!(token("s x=ab"), None, "not where a command goes");
        assert_eq!(token("w obj.Na"), None);
    }

    #[test]
    fn an_sql_word_may_be_qualified_and_never_sits_in_a_string() {
        let sql = |text: &str| token_at(&chars(text), true);
        assert_eq!(
            sql("select * from Sample.Pe").map(|t| t.text).as_deref(),
            Some("Sample.Pe")
        );
        assert_eq!(sql("sel").map(|t| t.context), Some(Context::Sql));
        assert_eq!(sql("where a = 'sel"), None);
        assert_eq!(sql("where a = :co"), None);
    }

    #[test]
    fn candidates_match_by_prefix_ignoring_case_shortest_first() {
        let vocabulary = Vocabulary::default();
        let found = candidates(
            &Token {
                context: Context::Dollar,
                text: "zd".into(),
            },
            &vocabulary,
        );
        let names: Vec<String> = found.iter().map(Candidate::display).collect();
        assert_eq!(
            names,
            vec!["$ZDATE", "$ZDATEH", "$ZDATETIME", "$ZDATETIMEH"]
        );
    }

    /// What has been seen before outranks what merely exists.
    #[test]
    fn a_name_seen_more_often_is_offered_first() {
        let mut vocabulary = Vocabulary::default();
        let first = |v: &Vocabulary| {
            candidates(
                &Token {
                    context: Context::Dollar,
                    text: "p".into(),
                },
                v,
            )[0]
            .display()
        };
        assert_eq!(first(&vocabulary), "$PIECE", "shortest, with no history");
        vocabulary.harvest_line("w $property(o,\"Name\")");
        assert_eq!(first(&vocabulary), "$PROPERTY");
    }

    #[test]
    fn globals_routines_entries_classes_and_tables_are_learned_from_commands() {
        let mut v = Vocabulary::default();
        v.harvest_line("zw ^CCDU(1)");
        v.harvest_line("d ^%CSW1GEN");
        v.harvest_line("w $$GetScrollableRS^%CSW1APICONTROLLER()");
        v.harvest_line("w ##class(Src2.Classe).%New()");
        v.harvest_line("select * from Sample.Person");
        assert!(v.globals.contains("CCDU"));
        assert!(v.routines.contains("%CSW1GEN"));
        assert!(v.entries.contains("GetScrollableRS^%CSW1APICONTROLLER"));
        assert!(v.classes.contains("Src2.Classe"));
        assert!(v.tables.contains("Sample.Person"));
    }

    /// A caret after `do` means a routine, so routines lead; anywhere else a
    /// global does.
    #[test]
    fn the_context_decides_whether_a_global_or_a_routine_comes_first() {
        let mut v = Vocabulary::default();
        v.harvest_line("zw ^CSWX");
        v.harvest_line("d ^CSWA");
        let first = |routine| {
            candidates(
                &Token {
                    context: Context::Caret { routine },
                    text: "CSW".into(),
                },
                &v,
            )[0]
            .clone()
        };
        assert_eq!(first(true).category, Category::Routine);
        assert_eq!(first(false).category, Category::Global);
    }

    #[test]
    fn a_word_already_spelled_in_full_has_nothing_to_complete() {
        let found = candidates(
            &Token {
                context: Context::Command,
                text: "write".into(),
            },
            &Vocabulary::default(),
        );
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn only_the_missing_part_is_sent_in_the_case_being_typed() {
        let command = Candidate {
            text: "write".into(),
            category: Category::Command,
        };
        assert_eq!(
            edit_for("wr", &command),
            Some(Edit {
                rubouts: 0,
                insert: "ite".into()
            })
        );
        assert_eq!(
            edit_for("WR", &command).map(|e| e.insert).as_deref(),
            Some("ITE")
        );
        let keyword = Candidate {
            text: "SELECT".into(),
            category: Category::SqlKeyword,
        };
        assert_eq!(
            edit_for("sel", &keyword).map(|e| e.insert).as_deref(),
            Some("ect")
        );
        assert_eq!(edit_for("x", &keyword), None, "not an extension of it");
    }

    /// `^csw` and `^CSW` are two globals, so the wrongly cased letters go and
    /// come back right.
    #[test]
    fn a_case_sensitive_name_rubs_out_what_was_typed_in_the_wrong_case() {
        let global = Candidate {
            text: "CSW1GEN".into(),
            category: Category::Global,
        };
        assert_eq!(
            edit_for("CSw", &global),
            Some(Edit {
                rubouts: 1,
                insert: "W1GEN".into()
            })
        );
        assert_eq!(
            edit_for("CSW", &global),
            Some(Edit {
                rubouts: 0,
                insert: "1GEN".into()
            })
        );
    }

    /// A full-screen routine paints rows with no prompt on them, and a popup
    /// there would be over somebody else's screen.
    #[test]
    fn no_popup_is_offered_without_a_prompt() {
        let mut v = Vocabulary::default();
        assert!(suggest(&grid_with("Global ^CSW1 selected wr"), &mut v).is_none());
        assert!(suggest(&grid_with("wr"), &mut v).is_none());
        let at_prompt = suggest(&grid_with("USER>wr"), &mut v).expect("a popup");
        assert_eq!(shown(&at_prompt), vec!["write"]);
    }

    #[test]
    fn no_popup_is_offered_mid_line() {
        let mut grid = grid_with("USER>wr x");
        grid.cursor.col = 7;
        assert!(suggest(&grid, &mut Vocabulary::default()).is_none());
    }

    #[test]
    fn the_sql_shell_is_offered_sql() {
        let popup = suggest(&grid_with("USER>>sel"), &mut Vocabulary::default()).expect("a popup");
        assert_eq!(shown(&popup), vec!["SELECT"]);
        let popup = suggest(
            &grid_with("[SQL]USER>>select * fr"),
            &mut Vocabulary::default(),
        )
        .expect("a popup");
        assert_eq!(shown(&popup), vec!["FROM"]);
    }

    /// A name printed a moment ago is offered; the half-typed word on the
    /// prompt row is not learned from.
    #[test]
    fn names_on_screen_are_offered_but_the_line_being_typed_is_not_learned() {
        let mut grid = Grid::new(80, 3, 10);
        grid.screen[0].set_text("^CCDUPLA(1)=\"x\"");
        grid.screen[2].set_text("USER>zw ^CCD");
        grid.cursor.row = 2;
        grid.cursor.col = 12;
        let mut v = Vocabulary::default();
        let popup = suggest(&grid, &mut v).expect("a popup");
        assert_eq!(shown(&popup), vec!["^CCDUPLA"]);
        assert!(!v.globals.contains("CCD"));
    }

    #[test]
    fn the_selection_wraps_and_moving_it_is_what_lets_enter_accept() {
        let mut popup = Popup {
            token: Token {
                context: Context::Dollar,
                text: "zd".into(),
            },
            items: candidates(
                &Token {
                    context: Context::Dollar,
                    text: "zd".into(),
                },
                &Vocabulary::default(),
            ),
            selected: 0,
            navigated: false,
        };
        popup.step(false);
        assert_eq!(popup.selected, popup.items.len() - 1);
        assert!(popup.navigated);
        popup.step(true);
        assert_eq!(popup.selected, 0);
        assert_eq!(popup.edit().map(|e| e.insert).as_deref(), Some("ate"));
    }

    /// Closed and disarmed, a frame reads nothing; typing arms it, and the
    /// popup follows the line from then on.
    #[test]
    fn the_popup_follows_typing_and_closes_with_the_prompt() {
        let mut completion = Completion::default();
        let mut v = Vocabulary::default();
        let grid = grid_with("USER>wr");
        completion.refresh(&grid, &mut v);
        assert!(completion.popup().is_none(), "not armed: nothing typed");

        completion.arm();
        completion.refresh(&grid, &mut v);
        assert!(completion.popup().is_some());

        completion.refresh(&grid_with("<BREAK>"), &mut v);
        assert!(completion.popup().is_none(), "the prompt went away");
    }
}
