//! The right-click menu over the terminal.
//!
//! Every entry raises a [`ContextAction`] rather than acting, so that the
//! shell decides what a gesture means and a confirmation only has to be
//! written once. See `App::handle_request`.

use super::*;

/// What the right-click menu asked for, if anything.
///
/// `Clone` rather than `Copy`: two of these carry a macro, and a macro carries
/// its body.
#[derive(Clone, Debug, PartialEq)]
pub enum ContextAction {
    CopySelection,
    /// Copy the selection and type it straight back into the session. What
    /// picking a global off the screen and putting it on the command line
    /// takes, in one entry instead of two.
    CopyAndPaste,
    Paste,
    SelectAll,
    ClearSelection,
    /// Write this much of the output to a file, in this format.
    Export(export::Format, export::Range),
    /// Put this much of the output on the clipboard.
    CopyRange(export::Range),
    /// Run a macro chosen from the menu.
    ///
    /// Nothing is filled in here: one that takes parameters, or that asks to be
    /// confirmed, opens a dialog over the terminal, and one that does neither
    /// goes straight out. The menu's job is to say *which* macro.
    RunMacro(Macro),
    /// Run an IRIS helper chosen from the menu. Always opens its dialog: every
    /// helper has at least one field to fill in.
    RunNative(Native),
    /// Hand this much of the output to a Claude Code session.
    Analyze(analyze::Scope, analyze::Panes),
    /// Layout of the tab this pane is in. Carried out by the app, which owns
    /// the tabs; the pane only says which one was asked for.
    SplitRight,
    SplitBottom,
    Unsplit,
    /// Close the session in this pane, and nothing else: the other pane of a
    /// split tab stays, unsplit.
    ClosePane,
    /// Reset the terminal and drop the scrollback. The same thing Ctrl+Delete
    /// does, put where it can be found.
    ClearTerminal,
}

/// The scope entries of the "Analyze with Claude" menu, all reporting the same
/// choice of panes.
///
/// A function rather than a closure because it is called from two arms of the
/// menu and a closure would have to borrow the answer twice.
/// The Export submenu: the six things the old Export dialog offered.
///
/// Every label is short, for the reason given at the Analyze submenu: a popup
/// is as wide as its widest entry and can only ever grow.
pub(super) fn export_menu(ui: &mut Ui) -> Option<ContextAction> {
    let mut chosen = None;
    ui.label(tr("Save to a file"));
    for (label, format, range) in [
        (
            "Screen as text",
            export::Format::Text,
            export::Range::Screen,
        ),
        (
            "Everything as text",
            export::Format::Text,
            export::Range::All,
        ),
        (
            "Screen as HTML",
            export::Format::Html,
            export::Range::Screen,
        ),
        (
            "Everything as HTML",
            export::Format::Html,
            export::Range::All,
        ),
    ] {
        if ui.button(tr(label)).clicked() {
            chosen = Some(ContextAction::Export(format, range));
            ui.close_menu();
        }
    }
    ui.separator();
    ui.label(tr("Copy to the clipboard"));
    for (label, range) in [
        ("Screen", export::Range::Screen),
        ("Everything", export::Range::All),
    ] {
        if ui.button(tr(label)).clicked() {
            chosen = Some(ContextAction::CopyRange(range));
            ui.close_menu();
        }
    }
    chosen
}

/// The Macros submenu: a level per group, then the macros in it.
pub(super) fn macro_menu(ui: &mut Ui, groups: &[MacroGroup]) -> Option<ContextAction> {
    let mut chosen = None;
    if groups.iter().all(|g| g.macros.is_empty()) {
        ui.weak(tr("No macros defined."));
        return chosen;
    }
    for group in groups {
        if group.macros.is_empty() {
            continue;
        }
        // A group with no name is not a level worth walking through: its macros
        // are offered directly, the way a single ungrouped file reads.
        if group.name.is_empty() {
            if let Some(picked) = group_entries(ui, group) {
                chosen = Some(picked);
            }
            continue;
        }
        ui.menu_button(&group.name, |ui| {
            if let Some(picked) = group_entries(ui, group) {
                chosen = Some(picked);
            }
        });
    }
    chosen
}

/// One group's macros, as menu entries.
///
/// Every entry is a plain press. What follows it depends on the macro: one
/// with parameters, or one marked `confirm`, opens a dialog over the terminal;
/// anything else is sent there and then.
fn group_entries(ui: &mut Ui, group: &MacroGroup) -> Option<ContextAction> {
    let mut chosen = None;
    for m in &group.macros {
        // Said on the entry rather than only in the dialog that follows: which
        // macros write is worth knowing *before* picking one. An ellipsis for
        // the ones that stop to ask, the way a menu entry opening a dialog is
        // spelled everywhere else.
        let label = match (m.confirm, m.needs_input()) {
            (true, _) => format!("{}...  ({})", m.name, tr("confirms")),
            (false, true) => format!("{}...", m.name),
            (false, false) => m.name.clone(),
        };
        let entry = ui.button(label);
        let entry = if m.description.is_empty() {
            entry
        } else {
            entry.on_hover_text(&m.description)
        };
        if entry.clicked() {
            chosen = Some(ContextAction::RunMacro(m.clone()));
            ui.close_menu();
        }
    }
    chosen
}

/// The IRIS utilities submenu: one entry per helper.
///
/// Every one of them opens its dialog - they all take at least one field, so
/// there is no such thing here as a helper that can be run by picking it. The
/// fields used to be a flyout hanging off the entry, which put text fields
/// inside a menu: a menu closes when the pointer wanders onto a sibling, and a
/// half-typed package name went with it. A dialog over the terminal stays until
/// it is answered.
pub(super) fn natives_menu(ui: &mut Ui) -> Option<ContextAction> {
    let mut chosen = None;
    for native in Native::ALL {
        if ui.button(tr(native.label())).clicked() {
            chosen = Some(ContextAction::RunNative(native));
            ui.close_menu();
        }
    }
    chosen
}

pub(super) fn analyze_scopes(
    ui: &mut Ui,
    has_selection: bool,
    panes: analyze::Panes,
) -> Option<ContextAction> {
    let mut chosen = None;
    for scope in analyze::Scope::ALL {
        // Asking about the selection needs one, so the entry says as much
        // rather than opening a session on nothing.
        let usable = has_selection || !scope.is_selection();
        if ui
            .add_enabled(usable, egui::Button::new(tr(scope.label())))
            .clicked()
        {
            chosen = Some(ContextAction::Analyze(scope, panes));
            ui.close_menu();
        }
    }
    chosen
}
