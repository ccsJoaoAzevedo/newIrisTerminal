//! Looking up a global's piece structure, for the piece tooltip.
//!
//! The question is asked down a session of its own - see [`DocLookup`] - and
//! never down the one the user is looking at. Typing into that one was the
//! first design and it cannot be made safe: the decision that the prompt is
//! idle and the moment the bytes actually go out are separated by however
//! long it takes the session to next say something, and the user types in
//! between.
//!
//! The query asks for structure only - which piece is which property, its
//! size, its type, its value list - never for a specific record's value.
//! That is what makes the answer cacheable per `(namespace, global)`: the
//! first hover on a global is the only one that ever has to ask.
//!
//! `build_query`'s `ObjectScript` has been run against a live instance and
//! its shape is load-bearing - read that function's comment before
//! rearranging it.

use std::collections::HashMap;
use std::time::{Duration, Instant};

use crate::config::Profile;
use crate::features::autologon::Autologon;
use crate::pty::Session;
use crate::term::{lineedit, Grid};

/// Wraps one piece's line in the query's answer, and the line that says the
/// answer is complete. Ordinary printable text, not a control byte: a C0
/// control the parser does not recognise is silently dropped rather than
/// drawn, so it would never reach a row's cells to be found again.
const MARK_MAP: &str = "##CSWMAP##";
const MARK: &str = "##CSWTIP##";
const MARK_END: &str = "##CSWTIPEND##";

/// One piece of a global's value, as `%CSWDOCGLOBAL` documents it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PieceInfo {
    /// 1-based, matching `$piece` numbering over `^`.
    pub piece: usize,
    /// Which run of `sub_delim` inside that piece this describes, for a
    /// piece the map subdivides again - IRIS writes the pair `12,1`. `None`
    /// for a piece that is not subdivided.
    pub sub: Option<usize>,
    /// The delimiter that subdivision uses (`;`, `,`). Carried per piece
    /// because it is declared per piece, not per map.
    pub sub_delim: Option<char>,
    pub description: String,
    /// As IRIS reports it - a bare width (`"5"`) or, for a scaled numeric
    /// type, `"total,decimals"` (`"5,2"`).
    pub size: String,
    /// The data type class name, e.g. `%Date`, `DataType.Valor`.
    pub kind: String,
    /// `(raw value, label)` pairs, from the property's display list -
    /// `DataType.SimNao`'s hardcoded one included. Empty when the property
    /// has none.
    pub value_list: Vec<(String, String)>,
}

impl PieceInfo {
    /// How IRIS itself writes this piece's number: `12`, or `12,1` for one
    /// run of a subdivided piece.
    pub fn label(&self) -> String {
        match self.sub {
            Some(sub) => format!("{},{sub}", self.piece),
            None => self.piece.to_string(),
        }
    }
}

/// One class's data map for a global: the subscript shape it claims, and
/// what the pieces of a row under that shape mean.
///
/// A global is mapped by as many classes as it has subscript shapes.
/// `^FTCL` has thirty-odd, and taking the first one - which is what the
/// first version of this did - describes the wrong row every time but one.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct MapInfo {
    /// How many subscripts a row under this map has.
    pub keys: usize,
    /// The subscripts that are constants, 1-based position and value. These
    /// are what tell two maps of the same depth apart - `^FTCL(e,c,19,i)` is
    /// a different class from `^FTCL(e,c,24,s)`.
    pub fixed: Vec<(usize, String)>,
    pub pieces: Vec<PieceInfo>,
}

impl MapInfo {
    /// Whether a row with these subscripts is one this map describes.
    fn matches(&self, subscripts: &[String]) -> bool {
        subscripts.len() == self.keys
            && self
                .fixed
                .iter()
                .all(|(at, value)| subscripts.get(at - 1) == Some(value))
    }
}

/// What a hovered piece means, once the maps are known: the description, and
/// the piece's own text narrowed to the sub-piece the pointer is actually in.
pub struct Described<'a> {
    pub info: &'a PieceInfo,
    /// The value to format and show - the whole piece, or just the run of it
    /// under the pointer where the piece is subdivided.
    pub raw: String,
}

/// The piece under a selection, picked out of every map the global has.
///
/// `subscripts`, `piece`, `piece_text` and `offset` are what
/// `crate::ui::terminal_view::PieceSelection` read off the row. They are
/// passed as primitives rather than as that type because nothing under
/// `features/` may depend on the widget layer.
pub fn describe<'a>(
    maps: &'a [MapInfo],
    subscripts: &[String],
    piece: usize,
    piece_text: &str,
    offset: usize,
) -> Option<Described<'a>> {
    let map = maps.iter().find(|m| m.matches(subscripts))?;
    let candidates: Vec<&PieceInfo> = map.pieces.iter().filter(|p| p.piece == piece).collect();

    // Not subdivided: one entry, and the whole piece is the value.
    if let [only] = candidates[..] {
        if only.sub.is_none() {
            return Some(Described {
                info: only,
                raw: piece_text.to_string(),
            });
        }
    }
    let delim = candidates.iter().find_map(|p| p.sub_delim)?;
    // Which run of the delimiter the selection started in, 1-based - the
    // same counting `$piece` does, one level further down.
    let sub = 1 + piece_text
        .chars()
        .take(offset)
        .filter(|&c| c == delim)
        .count();
    let info = candidates.into_iter().find(|p| p.sub == Some(sub))?;
    let raw = piece_text.split(delim).nth(sub - 1)?.to_string();
    Some(Described { info, raw })
}

/// What the tooltip can say about a raw value beyond the value itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Formatted {
    /// No rule applies, and the raw value is all there is to show.
    Nothing,
    Value(String),
    /// A rule applies and the value is not something it can read - a `%Date`
    /// holding a word, a scaled decimal holding a letter. Worth saying out
    /// loud rather than falling silent: silence reads as "nothing to add",
    /// and this is a fault in the stored data.
    Invalid,
}

/// How `raw` reads under `piece`'s rules.
///
/// Order matters: a value list is the most specific thing a property can
/// carry and wins over a type-based rule, even where both happen to apply.
pub fn format_value(piece: &PieceInfo, raw: &str) -> Formatted {
    // An empty piece is unset, not wrong. Every rule below would refuse it,
    // and calling every unset date in a row invalid would be noise.
    if raw.trim().is_empty() {
        return Formatted::Nothing;
    }
    if let Some(label) = piece
        .value_list
        .iter()
        .find(|(value, _)| value == raw)
        .map(|(_, label)| label.clone())
    {
        return Formatted::Value(label);
    }
    // A value the list does not mention falls through rather than being
    // called invalid: these lists are often partial, and a property that
    // documents two of its five codes is the normal case.
    if names_type(&piece.kind, "%Date") {
        return or_invalid(format_date(raw));
    }
    if names_type(&piece.kind, "%Time") {
        return or_invalid(format_time(raw));
    }
    if let Some(decimals) = decimal_places(&piece.size) {
        return or_invalid(format_decimal(raw, decimals));
    }
    Formatted::Nothing
}

fn or_invalid(formatted: Option<String>) -> Formatted {
    match formatted {
        Some(value) => Formatted::Value(value),
        None => Formatted::Invalid,
    }
}

/// Whether `kind` names the data type `name`.
///
/// Not a substring test: `%Date` must not match `%DateTime` and `%Time` must
/// not match `%TimeStamp`, which store different things and would be read as
/// nonsense by the other's rule. A match has to end the class name.
fn names_type(kind: &str, name: &str) -> bool {
    kind.match_indices(name).any(|(at, _)| {
        kind[at + name.len()..]
            .chars()
            .next()
            .is_none_or(|c| !c.is_ascii_alphanumeric())
    })
}

/// IRIS's logical `%Date`: whole days since 1840-12-31, the `$H` epoch.
fn format_date(raw: &str) -> Option<String> {
    let days: i64 = raw.trim().parse().ok()?;
    // Day 0 is the epoch itself; there is no negative logical date.
    if days < 0 {
        return None;
    }
    let epoch = chrono::NaiveDate::from_ymd_opt(1840, 12, 31)?;
    let date = epoch.checked_add_signed(chrono::Duration::days(days))?;
    Some(date.format("%d/%m/%Y").to_string())
}

/// IRIS's logical `%Time`: whole seconds since midnight, the second half of
/// `$H`.
fn format_time(raw: &str) -> Option<String> {
    let seconds: u32 = raw.trim().parse().ok()?;
    // 86400 is the following midnight, which no time-of-day ever is.
    if seconds >= 86_400 {
        return None;
    }
    Some(format!(
        "{:02}:{:02}:{:02}",
        seconds / 3600,
        (seconds % 3600) / 60,
        seconds % 60
    ))
}

/// The decimal count out of a `"total,decimals"` size, or `None` for a bare
/// width - which is every type that is not stored pre-scaled.
fn decimal_places(size: &str) -> Option<u32> {
    let (_, decimals) = size.split_once(',')?;
    decimals.trim().parse().ok()
}

/// `raw` divided by `10^decimals` and rendered pt-BR: `,` for the decimal
/// point, `.` grouping the integer part in thousands.
///
/// Integer arithmetic throughout - `raw` is a scaled integer straight off the
/// wire, and there is no reason to let a float anywhere near a currency
/// figure.
fn format_decimal(raw: &str, decimals: u32) -> Option<String> {
    let raw = raw.trim();
    let (negative, digits) = match raw.strip_prefix('-') {
        Some(rest) => (true, rest),
        None => (false, raw),
    };
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let scale = 10u64.checked_pow(decimals)?;
    let value: u64 = digits.parse().ok()?;
    let (whole, frac) = (value / scale, value % scale);

    let mut out = String::new();
    if negative && (whole != 0 || frac != 0) {
        out.push('-');
    }
    out.push_str(&group_thousands(whole));
    if decimals > 0 {
        out.push(',');
        out.push_str(&format!("{frac:0width$}", width = decimals as usize));
    }
    Some(out)
}

/// `n` with a `.` every three digits from the right, the way a pt-BR reader
/// expects a whole number's worth of a formatted value to look.
fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push('.');
        }
        out.push(c);
    }
    out
}

/// `dadosProp` pieces 6/7 - `",0,1"` / `",Não,Sim"` - into pairs. Piece 1 of
/// each is a placeholder both sides skip, mirroring the ERP's own
/// `MontarDisplayList`, which starts at `cont=2`.
fn parse_value_list(values: &str, labels: &str) -> Vec<(String, String)> {
    let values = values.split(',').skip(1);
    let labels = labels.split(',').skip(1);
    values
        .zip(labels)
        .filter(|(v, _)| !v.is_empty())
        .map(|(v, l)| (v.to_string(), l.to_string()))
        .collect()
}

/// Whether the structure of a global is known, being asked for, or beyond
/// asking.
#[derive(Debug, PartialEq, Eq)]
pub enum Lookup {
    /// Known, from this call or an earlier one.
    Ready(Vec<MapInfo>),
    /// Asked before and got nothing back for it - not retried.
    NotFound,
    /// A query is in flight (for this global or another); try again once it
    /// settles.
    Pending,
    /// Nothing will be asked: this profile has nothing to ask. A shell has no
    /// namespaces and no globals, and a session that could not be opened is
    /// not opened again for a tooltip.
    Unavailable,
}

/// Where the sidecar is in answering.
enum Step {
    /// Opened, but not yet sitting at a prompt we can type at - logging in,
    /// or still printing its banner.
    Waking,
    /// Asked, watching its own screen for the end marker.
    Asking {
        key: (String, String),
        since: Instant,
    },
}

/// A second session, opened on the same profile, that only this module ever
/// writes to or reads from.
///
/// It carries a whole terminal of its own - parser, grid, autologon - because
/// the answer arrives as terminal output and there is no cheaper way to read
/// it back than the one the rest of the app already uses. It is deliberately
/// never given a [`crate::features::logging::SessionLog`]: it types a
/// password during autologon, and the invariant is that a password reaches no
/// file.
struct Sidecar {
    session: Session,
    parser: vte::Parser,
    grid: Grid,
    autologon: Autologon,
    step: Step,
    /// When the sidecar last had anything to do. It holds an IRIS licence
    /// slot open for as long as it lives, so it does not outlive its
    /// usefulness by more than [`IDLE_TIMEOUT`].
    idle_since: Instant,
}

/// The piece tooltip's lookups, and the private session that answers them.
///
/// The session the user is looking at is never written to. An earlier version
/// typed the query at the user's own prompt when it looked idle, and the gap
/// between deciding it was idle and actually sending - which could be
/// arbitrarily long, because nothing sends until the session next produces
/// output - was enough for the user to start typing, so the query landed
/// concatenated onto their half-typed line. A session nobody else types into
/// has no such gap to lose.
#[derive(Default)]
pub struct DocLookup {
    cache: HashMap<(String, String), Result<Vec<MapInfo>, ()>>,
    /// Asked for by a hover, not yet sent, and when the hover asked. The
    /// clock is what keeps a sidecar that never reaches a prompt - one whose
    /// autologon is sitting on a password prompt with no password to give -
    /// from spinning the tooltip for ever.
    want: Option<((String, String), Instant)>,
    sidecar: Option<Sidecar>,
    /// Opening one failed, or this profile has none to open. Not retried:
    /// a tooltip is not worth a reconnect attempt per frame.
    unavailable: bool,
}

/// How long the answer may take before the global is written off. Generous:
/// it covers opening the session and logging in as well as the query itself.
const ANSWER_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the sidecar may sit doing nothing before it is closed and its
/// licence slot given back.
const IDLE_TIMEOUT: Duration = Duration::from_secs(120);

/// Wide enough that IRIS never truncates one of the answer's lines at the
/// margin, small enough to cost nothing. Nobody ever looks at this grid.
///
/// It also has to be wider than the query's own longest line, which the
/// session echoes back: a line wrapped by the device could put a row boundary
/// mid-string and leave a continuation row starting with the very marker
/// [`parse_answer`] scans for.
const SIDECAR_COLS: u16 = 1024;
const SIDECAR_ROWS: u16 = 100;

impl DocLookup {
    /// Asks for `global`'s piece structure in `namespace`, or returns what is
    /// already known.
    ///
    /// Cheap and side-effect-free enough to call from a hover every frame:
    /// it only ever records what is wanted. [`DocLookup::pump`] is what opens
    /// a session and types.
    pub fn request(&mut self, namespace: &str, global: &str) -> Lookup {
        let key = (namespace.to_string(), global.to_string());
        if let Some(cached) = self.cache.get(&key) {
            return match cached {
                Ok(maps) => Lookup::Ready(maps.clone()),
                Err(()) => Lookup::NotFound,
            };
        }
        if self.unavailable {
            return Lookup::Unavailable;
        }
        // A query already in flight is left to finish. Overwriting `want`
        // would be harmless, but a hover that drifts across a `zw` dump would
        // then queue a different global every frame and never settle on one.
        if self.want.is_none() && !self.asking() {
            self.want = Some((key, Instant::now()));
        }
        Lookup::Pending
    }

    fn asking(&self) -> bool {
        matches!(
            self.sidecar.as_ref().map(|s| &s.step),
            Some(Step::Asking { .. })
        )
    }

    /// Drives the sidecar: reads whatever it has said, asks the next question
    /// when it is ready for one, and closes it once it has been idle a while.
    ///
    /// Called from [`crate::app::Tab::pump`]. Returns true when something
    /// moved, which is the caller's cue that a frame is worth asking for -
    /// the sidecar's own reader thread wakes the loop when bytes arrive, but
    /// the steps between them have nothing else to schedule them.
    pub fn pump(&mut self, profile: &Profile) -> bool {
        if self.want.is_some() && self.sidecar.is_none() {
            self.open(profile);
        }
        let Some(sidecar) = self.sidecar.as_mut() else {
            return false;
        };

        let mut moved = false;
        let (bytes, ended) = sidecar.session.drain();
        if !bytes.is_empty() {
            moved = true;
            let decoded = profile.wire_encoding().decode(&bytes);
            let replies =
                crate::term::parser::advance(&mut sidecar.parser, &mut sidecar.grid, &decoded);
            if !replies.is_empty() {
                let _ = sidecar.session.write(&replies);
            }
            if let Some(to_send) = sidecar.autologon.observe(&sidecar.grid) {
                let _ = sidecar
                    .session
                    .write(&profile.wire_encoding().encode(&to_send));
            }
        }

        match &sidecar.step {
            // The answer is read off the sidecar's own scrollback rather than
            // its screen: a global with more pieces than the grid has rows
            // scrolls the start of its own answer away before the end of it
            // arrives.
            Step::Asking { key, since } => {
                if let Some(pieces) = parse_answer(&sidecar.grid.all_text()) {
                    let key = key.clone();
                    self.finish(key, Ok(pieces));
                    moved = true;
                } else if since.elapsed() > ANSWER_TIMEOUT {
                    let key = key.clone();
                    self.finish(key, Err(()));
                    moved = true;
                }
            }
            Step::Waking => {
                let ready = ready_to_ask(&sidecar.grid);
                match self.want.take() {
                    Some((key, _)) if ready => {
                        self.ask(key, profile);
                        moved = true;
                    }
                    // Still starting up, or logging in. Given long enough
                    // that it plainly never will, the global is written off
                    // like any other unanswered one, and the tooltip falls
                    // back to the piece number rather than saying "Looking
                    // up…" until the tab is closed.
                    Some((key, since)) if since.elapsed() > ANSWER_TIMEOUT => {
                        self.cache.insert(key, Err(()));
                        moved = true;
                    }
                    other => self.want = other,
                }
            }
        }

        if ended {
            // It died on us. Whatever it was asked stays unanswered rather
            // than hanging a tooltip forever.
            if let Some(Step::Asking { key, .. }) = self.sidecar.as_ref().map(|s| &s.step) {
                let key = key.clone();
                self.finish(key, Err(()));
            }
            self.sidecar = None;
            return true;
        }
        if let Some(sidecar) = self.sidecar.as_ref() {
            if self.want.is_none()
                && matches!(sidecar.step, Step::Waking)
                && sidecar.idle_since.elapsed() > IDLE_TIMEOUT
            {
                self.sidecar = None;
                return true;
            }
        }
        moved
    }

    /// Records an answer and stands the sidecar down to wait for the next one.
    ///
    /// Deliberately does not touch the grid. The prompt IRIS printed after the
    /// answer is the only thing that says the sidecar is free again, and
    /// wiping the screen here threw it away: the session was then idle with a
    /// blank grid, nothing would make it print another prompt, and every
    /// later hover sat on "Looking up…" for ever. Clearing belongs at the
    /// moment of asking - see [`DocLookup::ask`].
    fn finish(&mut self, key: (String, String), answer: Result<Vec<MapInfo>, ()>) {
        self.cache.insert(key, answer);
        if let Some(sidecar) = self.sidecar.as_mut() {
            sidecar.step = Step::Waking;
            sidecar.idle_since = Instant::now();
        }
    }

    fn ask(&mut self, key: (String, String), profile: &Profile) {
        let Some(sidecar) = self.sidecar.as_mut() else {
            return;
        };
        let query = build_query(&key.0, &key.1);
        // Everything this session has ever said, gone, so the previous
        // answer's markers cannot be read as this one's. Both halves matter:
        // `reset` files the screen into the scrollback rather than dropping
        // it - which is right for a session someone is reading and wrong for
        // this one, where the scrollback is searched - so the scrollback has
        // to go too.
        sidecar.grid.reset();
        sidecar.grid.scrollback.clear();
        if sidecar
            .session
            .write(&profile.wire_encoding().encode(&query))
            .is_err()
        {
            self.cache.insert(key, Err(()));
            self.sidecar = None;
            return;
        }
        sidecar.step = Step::Asking {
            key,
            since: Instant::now(),
        };
        sidecar.idle_since = Instant::now();
    }

    /// Opens the second session, the same three ways [`crate::app::Tab::start`]
    /// opens the first - except for a shell, which has no globals to describe
    /// and so gets no sidecar at all.
    fn open(&mut self, profile: &Profile) {
        let opened = match (profile.shell.as_ref(), profile.remote.as_ref()) {
            (Some(_), _) => {
                self.unavailable = true;
                self.want = None;
                return;
            }
            (None, Some(remote)) => {
                Session::telnet(&remote.address, remote.port, SIDECAR_COLS, SIDECAR_ROWS)
            }
            (None, None) => {
                let launcher = crate::pty::launcher::launcher();
                Session::local(
                    launcher.as_ref(),
                    &profile.launch_spec(),
                    SIDECAR_COLS,
                    SIDECAR_ROWS,
                )
            }
        };
        match opened {
            Ok(session) => {
                self.sidecar = Some(Sidecar {
                    session,
                    parser: vte::Parser::new(),
                    grid: Grid::new(SIDECAR_COLS as usize, SIDECAR_ROWS as usize, 4000),
                    autologon: Autologon::new(profile),
                    step: Step::Waking,
                    idle_since: Instant::now(),
                });
            }
            Err(_) => {
                self.unavailable = true;
                self.want = None;
            }
        }
    }
}

/// Whether the sidecar is sitting at a bare prompt and can be typed at.
///
/// The same "is there a prompt" test the command line uses. It answers twice:
/// once when the session has finished starting up, and again after every
/// answer - because the prompt IRIS prints at the end of one is what says it
/// is free for the next. Anything that clears the grid between answers
/// therefore wedges the sidecar for good.
fn ready_to_ask(grid: &Grid) -> bool {
    lineedit::current(grid).is_some_and(|line| line.is_empty())
}

/// The maps out of the answer, once the end marker has arrived - `None`
/// while it is still coming in.
///
/// Every line names the class and map it belongs to rather than piece lines
/// being grouped under a preceding header: the two passes that emit them are
/// two separate commands, so the answer is all the headers and then all the
/// pieces, never a header followed by its own.
fn parse_answer(lines: &[String]) -> Option<Vec<MapInfo>> {
    let mut maps: Vec<(String, MapInfo)> = Vec::new();
    let mut complete = false;
    for line in lines {
        let line = line.trim();
        if line == MARK_END {
            complete = true;
            continue;
        }
        if let Some(rest) = marked(line, MARK_MAP) {
            let mut fields = rest.split('|');
            let (Some(class), Some(map), Some(keys)) = (
                fields.next(),
                fields.next(),
                fields.next().and_then(|s| s.trim().parse().ok()),
            ) else {
                continue;
            };
            let fixed = fields.next().map(parse_fixed).unwrap_or_default();
            maps.push((
                format!("{class}|{map}"),
                MapInfo {
                    keys,
                    fixed,
                    pieces: Vec::new(),
                },
            ));
            continue;
        }
        let Some(rest) = marked(line, MARK) else {
            continue;
        };
        let mut fields = rest.split('|');
        let (
            Some(class),
            Some(map),
            Some(seq),
            Some(delim),
            Some(description),
            Some(size),
            Some(kind),
        ) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        )
        else {
            continue;
        };
        let Some((piece, sub)) = parse_sequence(seq) else {
            continue;
        };
        let owner = format!("{class}|{map}");
        let Some((_, map)) = maps.iter_mut().find(|(id, _)| *id == owner) else {
            continue;
        };
        let values = fields.next().unwrap_or_default();
        let labels = fields.next().unwrap_or_default();
        map.pieces.push(PieceInfo {
            piece,
            sub,
            sub_delim: delim.chars().next(),
            description: description.to_string(),
            size: size.to_string(),
            kind: kind.to_string(),
            value_list: parse_value_list(values, labels),
        });
    }
    complete.then(|| maps.into_iter().map(|(_, map)| map).collect())
}

/// A marked line's payload: what sits between the marker and the trailing
/// `##`.
fn marked<'a>(line: &'a str, mark: &str) -> Option<&'a str> {
    line.strip_prefix(mark)?.strip_suffix("##")
}

/// `3:3,4:1,` into the positions and values of a map's constant subscripts.
fn parse_fixed(spec: &str) -> Vec<(usize, String)> {
    spec.split(',')
        .filter(|entry| !entry.is_empty())
        .filter_map(|entry| {
            let (at, value) = entry.split_once(':')?;
            Some((at.trim().parse().ok()?, value.to_string()))
        })
        .collect()
}

/// IRIS's own spelling of a piece number: `12`, or `12,1` for one run of a
/// subdivided piece.
fn parse_sequence(seq: &str) -> Option<(usize, Option<usize>)> {
    match seq.split_once(',') {
        Some((piece, sub)) => Some((piece.trim().parse().ok()?, Some(sub.trim().parse().ok()?))),
        None => Some((seq.trim().parse().ok()?, None)),
    }
}

/// An IRIS string literal, quotes doubled the way ObjectScript escapes them.
fn quoted(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

/// The lines typed into the sidecar: switch namespace, read the structure,
/// print one marked line per data map, one per piece, then the end marker.
///
/// Not a `{` in sight, and the end marker on a line of its own, because
/// neither shortcut survives contact with a terminal's command line:
///
/// * An argumentless `FOR` takes *the rest of the line* as its body, so an
///   end marker written after the loop on the same line is written once per
///   iteration and never after the loop has finished - which is to say,
///   never, since the loop only ends when `$ORDER` runs out and quits first.
///   Nesting `FOR`s on one line does work, and is how both passes walk class,
///   map and then key or piece: each level's body is simply the rest of the
///   line. Nothing can follow the innermost loop, though, which is why the
///   one line per map is written from *inside* the key loop, on its last
///   turn (`W:$O(...)=""`).
/// * A `DO { ... }` block is compiled per invocation in direct mode and
///   raises `<SYNTAX>` the second time round the loop. It runs exactly one
///   iteration and then fails, which is what made this look like a quoting
///   problem the first time.
///
/// Both passes name the class and map on every line they write, because they
/// are separate commands and their output does not interleave.
///
/// The namespace is re-checked after the `ZN` rather than assumed: a `ZN` to
/// a namespace that does not exist leaves the session where it was, and
/// answering out of the wrong namespace would be cached as though it were
/// right.
fn build_query(namespace: &str, global: &str) -> String {
    let g = quoted(global);
    let ns = quoted(namespace);
    let lines = [
        "K cswT,cc,mm,kk,pp".to_string(),
        format!("ZN {ns}"),
        format!(
            "I $ZCVT($ZNSPACE,\"U\")=$ZCVT({ns},\"U\") S cswG={g},cswSc=##class(Src2.Classe).ObterInfoClassesGlobalCache(cswG,.cswT)"
        ),
        // The ERP's own fallback when a global is not described where it
        // lives: look again in the configured development namespace. Mirrors
        // `GerarGlobalTrabalho^%CSWDOCGLOBALRG`.
        "I '$D(cswT),$D(cswG) S cswNs2=\"\",cswSc=##class(Src2.Config).ObterNamespaceDesenv(,.cswNs2) ZN:cswNs2'=\"\" cswNs2 S cswSc=##class(Src2.Classe).ObterInfoClassesGlobalCache(cswG,.cswT)".to_string(),
        // Pass one: the subscript shape of every map, so the caller can pick
        // the one whose shape the hovered row actually has.
        format!(
            "S cc=\"\" F  S cc=$O(cswT(cc)) Q:cc=\"\"  S mm=\"\" F  S mm=$O(cswT(cc,\"maps\",\"data\",mm)) Q:mm=\"\"  S kk=\"\",nn=0,ff=\"\" F  S kk=$O(cswT(cc,\"maps\",\"data\",mm,\"chaves\",kk)) Q:kk=\"\"  S nn=nn+1 S:$D(cswT(cc,\"maps\",\"data\",mm,\"chaves\",kk,\"fixa\")) ff=ff_kk_\":\"_$P(cswT(cc,\"maps\",\"data\",mm,\"chaves\",kk,\"fixa\"),\"^\",1)_\",\" W:$O(cswT(cc,\"maps\",\"data\",mm,\"chaves\",kk))=\"\" \"{MARK_MAP}\"_cc_\"|\"_mm_\"|\"_nn_\"|\"_ff_\"##\",!"
        ),
        // Pass two: every piece of every map, each line saying which map it
        // belongs to and which delimiter, if any, subdivides it further.
        format!(
            "S cc=\"\" F  S cc=$O(cswT(cc)) Q:cc=\"\"  S mm=\"\" F  S mm=$O(cswT(cc,\"maps\",\"data\",mm)) Q:mm=\"\"  S pp=\"\" F  S pp=$O(cswT(cc,\"maps\",\"data\",mm,\"pieces\",pp)) Q:pp=\"\"  S dp=cswT(cc,\"maps\",\"data\",mm,\"pieces\",pp),pr=$P(dp,\"^\",1),sq=$P(dp,\"^\",3),d2=$G(cswT(cc,\"maps\",\"data\",mm,\"pieces\",pp,2)),dd=$G(cswT(cc,\"props\",pr)),cswSc=$$EditarDadosPropriedade^%CSWDOCGLOBALRG(.dd) W:pr'=\"\" \"{MARK}\"_cc_\"|\"_mm_\"|\"_sq_\"|\"_d2_\"|\"_$P(dd,\"^\",1)_\"|\"_$P(dd,\"^\",5)_\"|\"_$P(dd,\"^\",3)_\"|\"_$P(dd,\"^\",6)_\"|\"_$P(dd,\"^\",7)_\"##\",!"
        ),
        format!("W \"{MARK_END}\",!"),
    ];
    lines.join("\r") + "\r"
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The lines of an answer, end marker included.
    fn answer(lines: &[&str]) -> Vec<String> {
        lines
            .iter()
            .map(|s| s.to_string())
            .chain(std::iter::once(MARK_END.to_string()))
            .collect()
    }

    /// Not an assertion: prints the exact text the sidecar would type, so it
    /// can be replayed against a live instance. Ignored because it proves
    /// nothing on its own - see the module doc.
    #[test]
    #[ignore]
    fn prints_the_query_for_manual_replay() {
        print!("{}", build_query("COMP80", "FTCL").replace('\r', "\n"));
    }

    // --- formatting precedence -------------------------------------------

    fn piece(kind: &str, size: &str, value_list: &[(&str, &str)]) -> PieceInfo {
        PieceInfo {
            piece: 1,
            sub: None,
            sub_delim: None,
            description: "Test".into(),
            size: size.into(),
            kind: kind.into(),
            value_list: value_list
                .iter()
                .map(|(v, l)| (v.to_string(), l.to_string()))
                .collect(),
        }
    }

    /// `DataType.SimNao`'s hardcoded default: 0 is "Não", 1 is "Sim".
    #[test]
    fn a_sim_nao_value_list_formats_as_nao_or_sim() {
        let p = piece("DataType.SimNao", "1", &[("0", "Não"), ("1", "Sim")]);
        assert_eq!(format_value(&p, "0"), Formatted::Value("Não".into()));
        assert_eq!(format_value(&p, "1"), Formatted::Value("Sim".into()));
    }

    /// A real class-level display list works exactly the same way - nothing
    /// SimNao-specific in the rule itself.
    #[test]
    fn a_property_level_display_list_is_used_the_same_way() {
        let p = piece(
            "%Integer",
            "1",
            &[("0", "Sem Comissão"), ("1", "Com Comissão")],
        );
        assert_eq!(
            format_value(&p, "0"),
            Formatted::Value("Sem Comissão".into())
        );
    }

    /// A code the list does not mention is not called invalid: these lists
    /// are often partial, and there is nothing wrong with the stored value.
    #[test]
    fn a_code_outside_the_display_list_is_not_called_invalid() {
        let p = piece("%Integer", "1", &[("0", "Não"), ("1", "Sim")]);
        assert_eq!(format_value(&p, "7"), Formatted::Nothing);
    }

    #[test]
    fn a_date_piece_converts_from_the_1840_epoch() {
        let p = piece("%Date", "5", &[]);
        // 67043 days after 1840-12-31.
        assert_eq!(
            format_value(&p, "67043"),
            Formatted::Value("22/07/2024".into())
        );
    }

    /// `%Time` is whole seconds since midnight - the second half of `$H`.
    #[test]
    fn a_time_piece_converts_from_seconds_since_midnight() {
        let p = piece("%Time", "5", &[]);
        assert_eq!(
            format_value(&p, "29376"),
            Formatted::Value("08:09:36".into())
        );
        assert_eq!(format_value(&p, "0"), Formatted::Value("00:00:00".into()));
        assert_eq!(
            format_value(&p, "86399"),
            Formatted::Value("23:59:59".into())
        );
    }

    /// The whole point of the type rules is that they say when the data is
    /// wrong. A `%Date` holding a word is a fault worth naming, not a gap in
    /// what this knows how to format.
    #[test]
    fn a_value_a_rule_cannot_read_is_reported_as_invalid() {
        assert_eq!(
            format_value(&piece("%Date", "5", &[]), "ABC"),
            Formatted::Invalid
        );
        assert_eq!(
            format_value(&piece("%Time", "5", &[]), "ABC"),
            Formatted::Invalid
        );
        assert_eq!(
            format_value(&piece("DataType.Valor", "5,2", &[]), "ABC"),
            Formatted::Invalid
        );
    }

    /// Out of range counts as unreadable too: there is no day before the
    /// epoch and no time of day at or past the following midnight.
    #[test]
    fn a_value_outside_its_types_range_is_invalid() {
        assert_eq!(
            format_value(&piece("%Date", "5", &[]), "-1"),
            Formatted::Invalid
        );
        assert_eq!(
            format_value(&piece("%Time", "5", &[]), "86400"),
            Formatted::Invalid
        );
    }

    /// An unset piece is not a fault. Every rule would refuse it, and calling
    /// each empty date in a row invalid would bury the ones that are.
    #[test]
    fn an_empty_piece_is_not_called_invalid() {
        assert_eq!(
            format_value(&piece("%Date", "5", &[]), ""),
            Formatted::Nothing
        );
        assert_eq!(
            format_value(&piece("%Time", "5", &[]), "   "),
            Formatted::Nothing
        );
        assert_eq!(
            format_value(&piece("DataType.Valor", "5,2", &[]), ""),
            Formatted::Nothing
        );
    }

    /// `%DateTime` and `%TimeStamp` hold something else entirely, and reading
    /// either through the day-count or seconds-since-midnight rule would turn
    /// a good value into a wrong one.
    #[test]
    fn a_longer_type_name_is_not_mistaken_for_the_one_it_starts_with() {
        assert_eq!(
            format_value(&piece("%DateTime", "", &[]), "67043"),
            Formatted::Nothing
        );
        assert_eq!(
            format_value(&piece("%TimeStamp", "", &[]), "29376"),
            Formatted::Nothing
        );
    }

    #[test]
    fn a_decimal_scaled_size_groups_thousands_and_uses_a_comma() {
        let p = piece("DataType.Valor", "5,2", &[]);
        assert_eq!(
            format_value(&p, "100000"),
            Formatted::Value("1.000,00".into())
        );
        assert_eq!(format_value(&p, "0"), Formatted::Value("0,00".into()));
    }

    #[test]
    fn a_bare_size_with_no_comma_is_not_treated_as_decimal() {
        let p = piece("%Integer", "5", &[]);
        assert_eq!(format_value(&p, "100000"), Formatted::Nothing);
    }

    /// A value list wins even where a type-based rule would also match -
    /// the most specific thing a property can carry.
    #[test]
    fn a_value_list_takes_precedence_over_a_type_rule() {
        let p = piece("%Date", "5", &[("67043", "Feriado")]);
        assert_eq!(
            format_value(&p, "67043"),
            Formatted::Value("Feriado".into())
        );
    }

    #[test]
    fn nothing_formats_a_value_with_no_rule_that_applies() {
        let p = piece("%String", "40", &[]);
        assert_eq!(format_value(&p, "anything"), Formatted::Nothing);
    }

    // --- sentinel-line parsing ---------------------------------------------

    /// The two-pass answer for a global mapped by one class, exactly as a
    /// live instance writes it.
    #[test]
    fn a_complete_answer_parses_a_map_and_its_pieces() {
        let maps = parse_answer(&answer(&[
            "##CSWMAP##Cre.DuplicataAberta|CCDUMap|2|##",
            "##CSWTIP##Cre.DuplicataAberta|CCDUMap|1||Código do cliente|5|%Integer||##",
            "##CSWTIP##Cre.DuplicataAberta|CCDUMap|6||Data de Vencimento|5|%Date||##",
            "##CSWTIP##Cre.DuplicataAberta|CCDUMap|12,1|;|Primeira Nota Fiscal|10|%String||##",
        ]))
        .expect("a complete answer");
        assert_eq!(maps.len(), 1);
        assert_eq!(maps[0].keys, 2);
        assert!(maps[0].fixed.is_empty());
        assert_eq!(maps[0].pieces.len(), 3);
        assert_eq!(maps[0].pieces[1].piece, 6);
        assert_eq!(maps[0].pieces[1].kind, "%Date");
        assert_eq!(maps[0].pieces[2].piece, 12);
        assert_eq!(maps[0].pieces[2].sub, Some(1));
        assert_eq!(maps[0].pieces[2].sub_delim, Some(';'));
        assert_eq!(maps[0].pieces[2].label(), "12,1");
    }

    /// The bug this whole shape exists for: one global, several classes, told
    /// apart by how many subscripts a row has and what the constant ones
    /// hold. Taking the first map described every `^FTCL` row as the first
    /// class alphabetically.
    #[test]
    fn the_map_is_chosen_by_the_rows_own_subscript_shape() {
        let maps = parse_answer(&answer(&[
            "##CSWMAP##Fat.Cliente|FTCLMap|2|##",
            "##CSWMAP##Fat.CliInscEst|FTCLMap|4|3:19,##",
            "##CSWMAP##Fat.CliComplemento31|FTCLMap|6|3:3,4:1,##",
            "##CSWTIP##Fat.Cliente|FTCLMap|1||Nome do cliente|40|%String||##",
            "##CSWTIP##Fat.CliInscEst|FTCLMap|1||Endereco|60|%String||##",
            "##CSWTIP##Fat.CliComplemento31|FTCLMap|1||Código do Operador||%Integer||##",
        ]))
        .expect("a complete answer");

        let subs = |s: &str| -> Vec<String> { s.split(',').map(str::to_string).collect() };
        let named = |subs: Vec<String>| {
            describe(&maps, &subs, 1, "99", 0)
                .map(|d| d.info.description.clone())
                .unwrap_or_default()
        };
        assert_eq!(named(subs("1,1")), "Nome do cliente");
        assert_eq!(named(subs("1,1,19,7")), "Endereco");
        assert_eq!(
            named(subs("1,1,3,1,67774,29376")),
            "Código do Operador",
            "the row from the screenshot: six subscripts, two of them constants"
        );
    }

    /// Same depth, different constant: the fixed subscript is the only thing
    /// telling these two apart.
    #[test]
    fn two_maps_of_the_same_depth_are_told_apart_by_their_constant() {
        let maps = parse_answer(&answer(&[
            "##CSWMAP##Fat.CliInscEst|FTCLMap|4|3:19,##",
            "##CSWMAP##Fat.ClienteHistCadastral|FTCLMap|4|3:24,##",
            "##CSWTIP##Fat.CliInscEst|FTCLMap|1||Endereco|60|%String||##",
            "##CSWTIP##Fat.ClienteHistCadastral|FTCLMap|1||Histórico|60|%String||##",
        ]))
        .expect("a complete answer");
        let at = |k: &str| {
            let subs: Vec<String> = k.split(',').map(str::to_string).collect();
            describe(&maps, &subs, 1, "x", 0).map(|d| d.info.description.clone())
        };
        assert_eq!(at("1,1,19,7").as_deref(), Some("Endereco"));
        assert_eq!(at("1,1,24,7").as_deref(), Some("Histórico"));
        assert_eq!(at("1,1,99,7"), None, "no map claims this shape");
    }

    /// A piece the map subdivides again: which run of its own delimiter the
    /// pointer is in decides both the description and the value to format.
    #[test]
    fn a_subdivided_piece_resolves_by_where_the_pointer_sits_inside_it() {
        let maps = parse_answer(&answer(&[
            "##CSWMAP##Cre.DuplicataAberta|CCDUMap|2|##",
            "##CSWTIP##Cre.DuplicataAberta|CCDUMap|12,1|;|Primeira Nota Fiscal|10|%String||##",
            "##CSWTIP##Cre.DuplicataAberta|CCDUMap|12,2|;|Quantidade de Notas|3|%Integer||##",
        ]))
        .expect("a complete answer");
        let subs = vec!["1".to_string(), "1".to_string()];

        let first = describe(&maps, &subs, 12, "1234;7", 0).expect("the first run");
        assert_eq!(first.info.description, "Primeira Nota Fiscal");
        assert_eq!(first.raw, "1234");

        let second = describe(&maps, &subs, 12, "1234;7", 5).expect("the second run");
        assert_eq!(second.info.description, "Quantidade de Notas");
        assert_eq!(
            second.raw, "7",
            "the value shown is the sub-piece, not all of it"
        );
    }

    /// A piece nothing describes - the gaps in a map's numbering are real -
    /// is not guessed at.
    #[test]
    fn an_undescribed_piece_resolves_to_nothing() {
        let maps = parse_answer(&answer(&[
            "##CSWMAP##Cre.DuplicataAberta|CCDUMap|2|##",
            "##CSWTIP##Cre.DuplicataAberta|CCDUMap|1||Empresa|4|%Integer||##",
        ]))
        .expect("a complete answer");
        let subs = vec!["1".to_string(), "1".to_string()];
        assert!(describe(&maps, &subs, 16, "x", 0).is_none());
    }

    /// A piece line naming a map no header introduced is dropped rather than
    /// inventing a map for it: a truncated answer must not become a wrong one.
    #[test]
    fn a_piece_with_no_map_of_its_own_is_dropped() {
        let maps = parse_answer(&answer(&[
            "##CSWMAP##Fat.Cliente|FTCLMap|2|##",
            "##CSWTIP##Fat.Orphan|FTCLMap|1||Nowhere|4|%Integer||##",
        ]))
        .expect("a complete answer");
        assert_eq!(maps.len(), 1);
        assert!(maps[0].pieces.is_empty());
    }

    #[test]
    fn an_answer_missing_its_end_marker_is_not_ready_yet() {
        let lines = vec!["##CSWMAP##Fat.Cliente|FTCLMap|2|##".to_string()];
        assert_eq!(parse_answer(&lines), None);
    }

    /// A global with nothing to say for itself still ends cleanly - no maps,
    /// but a definite answer rather than an eternal wait.
    #[test]
    fn an_answer_with_no_maps_at_all_is_still_complete() {
        let lines = vec!["##CSWTIPEND##".to_string()];
        assert_eq!(parse_answer(&lines), Some(Vec::new()));
    }

    // --- the query's shape --------------------------------------------------

    /// The two ways a terminal's command line refuses to carry this query.
    /// Both cost a full debugging session the first time, and neither shows
    /// up as anything but silence or a `<SYNTAX>` on the second iteration.
    #[test]
    fn the_query_puts_the_end_marker_on_a_line_of_its_own_and_uses_no_braces() {
        let query = build_query("RDB81-TR", "CCDU");
        assert!(
            !query.contains('{'),
            "a DO block only compiles once in direct mode: {query}"
        );
        let last = query
            .trim_end_matches('\r')
            .rsplit('\r')
            .next()
            .expect("a last line");
        assert_eq!(
            last,
            format!("W \"{MARK_END}\",!"),
            "an argumentless FOR swallows the rest of its line, so the end \
             marker cannot share one with the loop"
        );
    }

    #[test]
    fn the_query_switches_namespace_and_checks_it_got_there() {
        let query = build_query("RDB81-TR", "CCDU");
        assert!(query.contains("ZN \"RDB81-TR\""), "{query}");
        assert!(query.contains("$ZNSPACE"), "{query}");
        assert!(query.contains("\"CCDU\""), "{query}");
    }

    // --- when the sidecar is free to be asked ------------------------------

    /// Builds a grid holding `lines`, cursor at the end of the last one.
    fn grid_showing(lines: &[&str]) -> Grid {
        let mut grid = Grid::new(120, lines.len().max(1), 100);
        for (row, line) in lines.iter().enumerate() {
            grid.screen[row].set_text(line);
        }
        grid.cursor.row = lines.len().saturating_sub(1);
        grid.cursor.col = lines.last().map_or(0, |l| l.chars().count());
        grid
    }

    /// The prompt IRIS prints after an answer is what says the sidecar is free
    /// for the next question. Wiping the grid on the way out of one answer
    /// threw it away, and since an idle session prints nothing unprompted,
    /// every later hover then sat on "Looking up…" for ever.
    #[test]
    fn the_prompt_left_by_an_answer_frees_the_sidecar_for_the_next_one() {
        let answered = grid_showing(&[
            "COMP80>K cswT,cc,mm,kk,pp",
            "##CSWMAP##Cre.DuplicataAberta|CCDUMap|2|##",
            MARK_END,
            "COMP80>",
        ]);
        assert!(ready_to_ask(&answered));

        let mut wiped = answered;
        wiped.reset();
        assert!(
            !ready_to_ask(&wiped),
            "a cleared grid has no prompt, and nothing will print another one"
        );
    }

    /// While the answer is still arriving the cursor is not on a prompt, so
    /// nothing else gets asked over the top of it.
    #[test]
    fn a_sidecar_mid_answer_is_not_free() {
        let grid = grid_showing(&["COMP80>K cswT,cc,mm,kk,pp", "##CSWMAP##X|XMap|2|##"]);
        assert!(!ready_to_ask(&grid));
    }

    /// A prompt with something already typed at it is not free either.
    #[test]
    fn a_prompt_with_something_typed_at_it_is_not_free() {
        assert!(!ready_to_ask(&grid_showing(&["COMP80>zw ^CCDU"])));
    }

    /// The question waiting to be asked, without the clock beside it.
    fn wanted(lookup: &DocLookup) -> Option<(String, String)> {
        lookup.want.as_ref().map(|(key, _)| key.clone())
    }

    // --- what a hover asks for ----------------------------------------------

    #[test]
    fn a_hover_records_what_it_wants_and_reports_it_as_pending() {
        let mut lookup = DocLookup::default();
        assert_eq!(lookup.request("RDB81-TR", "CCDU"), Lookup::Pending);
        assert_eq!(
            wanted(&lookup),
            Some(("RDB81-TR".into(), "CCDU".into())),
            "the hover has to leave something for `pump` to act on"
        );
    }

    /// A hover drifting across a `zw` dump must not replace the question
    /// already waiting to be asked, or nothing is ever asked at all.
    #[test]
    fn a_second_hover_does_not_displace_the_question_already_waiting() {
        let mut lookup = DocLookup::default();
        lookup.request("RDB81-TR", "CCDU");
        lookup.request("RDB81-TR", "OTHER");
        assert_eq!(wanted(&lookup), Some(("RDB81-TR".into(), "CCDU".into())));
    }

    #[test]
    fn a_known_answer_is_returned_without_asking_for_anything() {
        let mut lookup = DocLookup::default();
        lookup
            .cache
            .insert(("RDB81-TR".into(), "CCDU".into()), Ok(Vec::new()));
        assert_eq!(
            lookup.request("RDB81-TR", "CCDU"),
            Lookup::Ready(Vec::new())
        );
        assert_eq!(wanted(&lookup), None, "nothing should have been queued");
    }

    #[test]
    fn a_global_that_failed_once_is_not_asked_about_again() {
        let mut lookup = DocLookup::default();
        lookup
            .cache
            .insert(("RDB81-TR".into(), "NOPE".into()), Err(()));
        assert_eq!(lookup.request("RDB81-TR", "NOPE"), Lookup::NotFound);
        assert_eq!(wanted(&lookup), None);
    }

    #[test]
    fn different_namespaces_are_cached_separately() {
        let mut lookup = DocLookup::default();
        lookup
            .cache
            .insert(("USER".into(), "CCDU".into()), Ok(Vec::new()));
        assert_eq!(
            lookup.request("RDB81-TR", "CCDU"),
            Lookup::Pending,
            "a different namespace must not hit the other one's cache"
        );
    }

    /// A profile with no instance behind it - a shell - has no globals to
    /// describe, and asking again every frame would try to start one.
    #[test]
    fn a_shell_profile_is_written_off_once_and_never_retried() {
        let mut lookup = DocLookup::default();
        let mut profile = Profile::default();
        profile.shell = Some(crate::config::profile::ShellCommand {
            program: std::path::PathBuf::from("cmd.exe"),
            args: Vec::new(),
            cwd: None,
        });
        lookup.request("USER", "CCDU");
        lookup.pump(&profile);
        assert!(lookup.sidecar.is_none());
        assert_eq!(lookup.request("USER", "CCDU"), Lookup::Unavailable);
    }
}
