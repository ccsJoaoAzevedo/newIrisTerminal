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
//! - [`theme_manager`], [`macro_manager`] are the editors for those.
//! - [`search`] is Ctrl+F over the transcript.
//! - [`wrap`] decides which grid line each display row shows.
//! - [`fonts`], [`icons`], [`shading`], [`monitors`], [`shortcut`] are small
//!   helpers named for what they do.
//! - [`detach`] opens a pane in a window of its own.

pub mod chrome;
pub mod detach;
pub mod fonts;
pub mod icons;
pub mod input;
pub mod macro_manager;
pub mod monitors;
pub mod panels;
pub mod search;
pub mod shading;
pub mod shortcut;
pub mod terminal_view;
pub mod theme_manager;
pub mod wrap;
