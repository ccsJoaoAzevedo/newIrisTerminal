//! The terminal itself: bytes in, a screen of characters out.
//!
//! Nothing here knows about egui, the window or the user. A [`Grid`] is the
//! screen and its scrollback; [`parser`] drives one from a byte stream through
//! `vte`; [`Cell`] is one character with its colour and attributes. The whole
//! module is drivable headlessly, which is what the integration tests do.
//!
//! The pieces, in the order bytes pass through them:
//!
//! - [`encoding`] turns wire bytes into a `str`. A Telnet server may speak a
//!   codepage; a local session is UTF-8.
//! - [`parser`] interprets escape sequences and writes into the grid. It also
//!   answers the queries that expect a reply, which the caller must send back.
//! - [`grid`] holds the rows, the cursor, the scrollback and the wrapping.
//! - [`palette`] resolves a cell's colour against the theme.
//! - [`syntax`] colours ObjectScript on top of that, and never overrules a
//!   colour the far side asked for.
//! - [`lineedit`] finds the prompt and the line being typed on it, which is how
//!   history, recall and the namespace in the tab name are all read off the
//!   screen rather than tracked.
//!
//! [`Cell`]: cell::Cell

pub mod cell;
pub mod encoding;
pub mod grid;
pub mod lineedit;
pub mod palette;
pub mod parser;
pub mod syntax;

pub use cell::Attrs;
pub use encoding::Encoding;
pub use grid::{Grid, Row};
pub use lineedit::{LineEdit, Motion};
