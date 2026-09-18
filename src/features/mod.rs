//! What the terminal does besides being a terminal.
//!
//! Each of these is independent of the others and of the UI: they take a grid,
//! a profile or a path, and answer. The shell wires them to menu items and
//! shortcuts.
//!
//! - [`autologon`] watches the screen for a login prompt and answers it.
//! - [`doc_lookup`] watches for a safe moment to ask a session what a
//!   global's pieces mean, for the piece tooltip.
//! - [`history`] remembers commands across runs; Up and Down walk it.
//! - [`logging`] writes a session transcript, muted across a password.
//! - [`export`] saves the screen or the scrollback as text or HTML.
//! - [`macros`] runs saved command sequences, with parameters.
//! - [`natives`] is the built-in set of those.
//! - [`analyze`] hands a transcript to Claude Code.
//! - [`update`] checks for a newer release and installs it.

pub mod analyze;
pub mod autologon;
pub mod doc_lookup;
pub mod export;
pub mod history;
pub mod logging;
pub mod macros;
pub mod natives;
pub mod update;
