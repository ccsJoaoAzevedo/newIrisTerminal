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
//! - [`autocomplete`] suggests the rest of the word being typed at a prompt.
//! - [`snake`] is the easter egg `/snake` opens, and is a game, not a terminal.
//! - [`explorer_menu`] puts "Open newIrisTerminal here" in Explorer's
//!   right-click menu, and reads the command line it launches with.
//! - [`iris_terminal`] stands this app in for the IRIS tray's Terminal.
//! - [`update`] checks for a newer release and installs it.

pub mod analyze;
pub mod autocomplete;
pub mod autologon;
pub mod doc_lookup;
pub mod explorer_menu;
pub mod export;
pub mod history;
pub mod iris_terminal;
pub mod logging;
pub mod macros;
pub mod natives;
pub mod screensaver;
pub mod snake;
pub mod update;
