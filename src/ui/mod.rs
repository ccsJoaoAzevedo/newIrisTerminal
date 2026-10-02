//! Everything egui: the widgets, the chrome, and reading the keyboard.
//!
//! [`terminal_view`] is the one that matters - the character grid, its
//! selection and its scrollbars - and it is the frame's hot path. The rest is
//! the furniture around it.
//!
//! - [`terminal_view`] draws a grid and reports what the pointer did to it.
//! - [`input`] turns a frame's key presses into an intent, so that what a key
//!   means is decided in one place rather than at every call site.
//! - [`chrome`] draws the title bar, because the window has no system frame.
//! - [`panels`] holds the settings, profile and update dialogs.
//! - [`settings_view`] is the Settings window, laid out with [`prefs`]: a
//!   sidebar, pages of cards, and a search over every row - including the
//!   pages that edit themes, macros and the screen saver.
//! - [`search`] is Ctrl+F over the transcript.
//! - [`completion`] draws the autocomplete popup under the cursor.
//! - [`wrap`] decides which grid line each display row shows.
//! - [`fonts`], [`icons`], [`shading`], [`monitors`], [`shortcut`] are small
//!   helpers named for what they do.
//! - [`detach`] opens a pane in a window of its own.
//! - [`snake_view`] draws the easter egg's board, in a tab that holds no
//!   session at all.

pub mod chrome;
pub mod completion;
pub mod desktop;
pub mod detach;
pub mod dialog;
pub mod fonts;
pub mod icons;
pub mod input;
pub mod monitors;
pub mod panels;
pub mod prefs;
pub mod screensaver_image;
pub mod screensaver_logos;
pub mod screensaver_view;
pub mod search;
pub mod settings_view;
pub mod shading;
pub mod shortcut;
pub mod snake_view;
pub mod terminal_view;
pub mod tray;
pub mod wrap;
