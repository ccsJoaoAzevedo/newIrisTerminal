//! The Macros pages: every macro there is, group by group, and an editor for
//! one of them on a sub-page of its own.
//!
//! Running macros is on the terminal's right-click menu, where the output
//! they are run against is; what is here is the managing. The list is a card
//! per group, the way the right-click menu arranges them, and a macro opens
//! onto its own page - General, Commands, Parameters and what it Will send -
//! with the way back to the list above it.
//!
//! Two kinds of macro, and the difference is the whole of the read-only half
//! of these pages: the organisation's file is shared and never written from
//! the app, so those are shown and can be copied but not changed. Only the
//! personal file is edited here, and only `UiRequest::SavePersonalMacros`
//! writes it.

use egui::Ui;

use super::{
    drawn, section, subpage, untitled, Category, Ctx, Item, Page, Route, Section, MACROS_PAGE,
};
use crate::features::macros::{Macro, MacroGroup, Origin, Param, ParamUse};
use crate::i18n::{tr, tr1, tr2};
use crate::ui::file_dialog;
use crate::ui::panels::{UiRequest, WARNING};
use crate::ui::prefs::{self, Card, Row};
use crate::ui::shortcut;

/// What the Macros pages keep between frames.
#[derive(Default)]
pub struct MacroPageState {
    /// Narrows the list. Matches a name or a description, never a body: a body
    /// can carry a password, and a filter reporting a hit inside one would say
    /// so without ever showing it.
    pub filter: String,
    /// Which macro the editor is on, by (group, index).
    selected: Option<(usize, usize)>,
    /// The macro being edited, kept apart from the stored one so Revert is a
    /// real revert and a half-typed name is never a name.
    draft: Option<Macro>,
    /// Which macro `draft` belongs to. Kept apart from `selected` because it is
    /// what tells a selection change from a redraw.
    draft_of: Option<(usize, usize)>,
    /// The body exactly as it is being typed, before it is split into lines.
    ///
    /// The field's own text rather than the macro's lines rejoined every frame:
    /// rejoining put every keystroke through `trim`, and a space at the end of
    /// a line was taken away again before it could be drawn. Split into lines
    /// once, on the way to the file.
    body_draft: String,
    /// Whether a hidden body is currently shown. Deliberately not persisted,
    /// and cleared whenever the editor moves, so opening a macro never starts
    /// by putting its password on screen.
    reveal_body: bool,
    /// The editor is waiting for a key combination to be pressed, so that it
    /// can be read off the keyboard instead of typed out. Public because the
    /// app has to stop claiming shortcuts for itself while it is set, or Ctrl+T
    /// would open a tab rather than be recorded.
    pub capture_shortcut: bool,
    /// Which group the list is on, when one has been clicked.
    ///
    /// A group is a thing you can act on now - rename it, add a macro to it -
    /// so it has a selection of its own beside the macro's. Opening a macro
    /// moves it to that macro's group, which is what makes "New macro" land
    /// where the eye already is.
    selected_group: Option<usize>,
    /// The group being renamed, and the name as it is being typed.
    ///
    /// Kept out of the group itself for the same reason the macro editor keeps
    /// a draft: a half-typed name is not a name, and abandoning the rename has
    /// to leave the old one intact.
    group_rename: Option<(usize, String)>,
    /// A group being named for the first time, if New group has been pressed.
    ///
    /// Nothing exists on the list until the name is confirmed: creating the
    /// group first and then offering to rename it left a group called "New
    /// group" behind every time the field was cancelled, and an empty one at
    /// that. The group and its first macro are made together, when the name
    /// is.
    new_group: Option<String>,
    /// A macro Delete has been pressed on, waiting on the confirmation that a
    /// deleted macro is not recoverable.
    confirm_delete: Option<(usize, usize)>,
    /// The file dialog choosing the organisation's file, while it is open.
    picking_org: Option<std::sync::mpsc::Receiver<Option<String>>>,
}

impl MacroPageState {
    /// Puts the editor on one macro, loading a draft of it. Reports whether
    /// what it displaced has to be written to disk.
    ///
    /// Whatever was being edited is committed first: moving to another macro
    /// is not a gesture anyone means as "throw away what I just typed", and a
    /// modal asking about it every time would be worse than either.
    pub(super) fn select(&mut self, at: Option<(usize, usize)>, groups: &mut [MacroGroup]) -> bool {
        // The group follows the macro, so "New macro" and Rename act on the
        // group whose macro is open rather than on whatever was clicked last.
        if let Some((gi, _)) = at {
            self.selected_group = Some(gi);
        }
        if self.draft_of == at {
            self.selected = at;
            return false;
        }
        let saved = self.commit(groups);
        self.selected = at;
        self.draft_of = at;
        self.draft = at
            .and_then(|(gi, mi)| groups.get(gi)?.macros.get(mi))
            .cloned();
        self.body_draft = self
            .draft
            .as_ref()
            .map(|m| m.body.join("\n"))
            .unwrap_or_default();
        self.reveal_body = false;
        self.capture_shortcut = false;
        self.confirm_delete = None;
        saved
    }

    /// Writes the draft back into the group it came from. Reports whether
    /// anything actually changed, which is what decides if the file is
    /// rewritten.
    ///
    /// The organisation's macros are guarded here as well as in the editor that
    /// draws them disabled: this is the one place that could write to a shared
    /// file, so it is the place that must not.
    fn commit(&mut self, groups: &mut [MacroGroup]) -> bool {
        let Some((gi, mi)) = self.draft_of else {
            return false;
        };
        let Some(draft) = self.draft.as_ref() else {
            return false;
        };
        let Some(stored) = groups.get_mut(gi).and_then(|g| g.macros.get_mut(mi)) else {
            return false;
        };
        if !stored.origin.is_editable() {
            return false;
        }
        let mut edited = draft.clone();
        // Origin is never taken from the draft: an edited macro stays personal,
        // so nothing can promote itself into the shared file.
        edited.origin = Origin::Personal;
        // Where the typed text becomes lines: once, on the way to the file,
        // rather than on every keystroke.
        edited.body = crate::features::macros::body_lines(&self.body_draft);
        if &edited == stored {
            return false;
        }
        *stored = edited;
        true
    }

    /// Puts the editor down, committing what it held. Reports whether the
    /// file has to be written.
    ///
    /// The selection stays, so the list still shows which macro was open and
    /// coming back to it loads a fresh draft of what was kept.
    fn leave(&mut self, groups: &mut [MacroGroup]) -> bool {
        self.capture_shortcut = false;
        if self.draft_of.is_none() {
            return false;
        }
        let saved = self.commit(groups);
        self.draft = None;
        self.draft_of = None;
        self.reveal_body = false;
        self.confirm_delete = None;
        saved
    }

    /// The selected macro as it stands in the file, rather than in the draft.
    fn stored<'a>(&self, groups: &'a [MacroGroup]) -> Option<&'a Macro> {
        let (gi, mi) = self.selected?;
        groups.get(gi)?.macros.get(mi)
    }
}

/// The macro editor, which shows whichever macro is selected.
const EDITOR: Route = Route::sub(Category::Macros, "macro");

pub(super) fn pages() -> Vec<Page> {
    vec![
        super::page(Category::Macros, list()),
        subpage(Category::Macros, "macro", "Macro", editor())
            .with_heading(macro_title)
            .with_top(editor_top),
        subpage(
            Category::Macros,
            "org_macros",
            "Organization macros",
            organization_macros(),
        ),
    ]
}

/// An index into a list that has since been reloaded, or had a macro deleted,
/// would point at the wrong macro; a stale selection is dropped rather than
/// followed.
pub(super) fn forget_stale_selection(c: &mut Ctx<'_>) {
    let groups = &*c.macro_groups;
    let state = &mut c.state.macros;
    let exists = |at: Option<(usize, usize)>| {
        at.is_some_and(|(gi, mi)| groups.get(gi).and_then(|g| g.macros.get(mi)).is_some())
    };
    if state.selected.is_some() && !exists(state.selected) {
        state.selected = None;
        state.draft_of = None;
        state.draft = None;
    }
    if state.confirm_delete.is_some() && !exists(state.confirm_delete) {
        state.confirm_delete = None;
    }
    if state.selected_group.is_some_and(|gi| gi >= groups.len()) {
        state.selected_group = None;
    }
}

/// After the page is drawn: an editor that is no longer on screen is put
/// down, and what it held kept - leaving the page, or closing the window, is
/// how an edit is finished here, as on every other page.
pub(super) fn settle(c: &mut Ctx<'_>, open: bool) {
    if (!open || c.state.settings_route != EDITOR) && c.state.macros.leave(c.macro_groups) {
        c.requests.push(UiRequest::SavePersonalMacros);
    }
}

// ---------------------------------------------------------------------------
// The list
// ---------------------------------------------------------------------------

fn list() -> Vec<Section> {
    vec![
        untitled(vec![Item::control("macro_filter", "Filter", filter)
            .keys(&["search", "find", "filtrar", "procurar", "pesquisar"])]),
        drawn(group_cards),
        untitled(vec![Item::rows("macro_add", "New macro", add_buttons)
            .keys(&["new", "group", "create", "nova", "novo", "grupo", "criar"])])
        .footer("Groups are how the right-click menu is arranged, and running a macro is on that menu. The organization's macros are read-only here: duplicate one to change it."),
        section(
            "Options",
            vec![
                Item::control("macro_manager_shortcut", "Shortcut for this page", manager_shortcut)
                    .sub("Opens Settings on the macros from the terminal.")
                    .keys(&["shortcut", "hotkey", "keyboard", "atalho", "teclado", "manager", "gerenciador"]),
                Item::control("macros_folder", "Macros folder", macros_folder)
                    .keys(&["folder", "file", "pasta", "arquivo"]),
            ],
        ),
        section(
            "Shared",
            vec![Item::link(
                "org_macros_link",
                "Organization macros",
                Route::sub(Category::Macros, "org_macros"),
            )
            .sub("A file of macros shared with the team, read-only.")
            .keys(&["shared", "unc", "network", "organization", "compartilhado", "rede", "organizacao"])],
        ),
    ]
}

fn filter(ui: &mut Ui, c: &mut Ctx<'_>) {
    ui.add(
        egui::TextEdit::singleline(&mut c.state.macros.filter)
            .hint_text(tr("Name or description"))
            .desired_width(220.0),
    );
}

/// The groups, each a heading and a card of its macros.
///
/// The gestures are collected and carried out after the list is drawn: it is
/// drawn from a borrow of the groups, so nothing can be moved or renamed until
/// it has finished with them.
fn group_cards(ui: &mut Ui, c: &mut Ctx<'_>) {
    let state = &mut c.state.macros;
    let groups = &mut *c.macro_groups;
    let filter = state.filter.trim().to_owned();
    let mut to_select: Option<(usize, usize)> = None;
    let mut rename_started: Option<usize> = None;
    let mut rename_done: Option<(usize, String)> = None;
    let mut rename_cancelled = false;
    let mut add_to: Option<usize> = None;
    let mut to_select_group: Option<usize> = None;
    let mut create_group: Option<String> = None;
    let mut create_cancelled = false;

    if groups.is_empty() && state.new_group.is_none() {
        prefs::empty_state(ui, tr("No macros defined."));
    }
    // The field New group opens, at the top of the list where the group itself
    // will appear. Nothing has been created yet - see `new_group` - so it is
    // drawn here rather than against a group.
    if let Some(draft) = state.new_group.as_mut() {
        prefs::section(ui, tr("New group"));
        prefs::card(ui, "new-group", |card| {
            card.row(Row::new(tr("Group name")), |ui| {
                if prefs::button(ui, tr("Cancel")).clicked() {
                    create_cancelled = true;
                }
                let confirm = prefs::button(ui, tr("Confirm")).clicked();
                let field = ui.add(
                    egui::TextEdit::singleline(draft)
                        .desired_width(180.0)
                        .hint_text(tr("Group name")),
                );
                field.request_focus();
                let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if confirm || entered {
                    create_group = Some(draft.clone());
                }
            });
        });
    }

    for (gi, group) in groups.iter().enumerate() {
        let matching: Vec<(usize, &Macro)> = group
            .macros
            .iter()
            .enumerate()
            .filter(|(_, m)| {
                filter.is_empty()
                    || prefs::matches(&filter, &[m.name.as_str(), m.description.as_str()])
            })
            .collect();
        // An empty group is still a group: it is what New group has just made,
        // and it has to be on the list to be clicked, renamed, or given a
        // macro. Only a filter hides one, and then because it genuinely has no
        // match.
        if matching.is_empty() && !(filter.is_empty() && group.macros.is_empty()) {
            continue;
        }
        let name = if group.name.is_empty() {
            tr("Macros").to_string()
        } else {
            group.name.clone()
        };
        // Provenance on the group rather than on every row: a shared macro
        // behaving oddly is someone else's file, not something the user can
        // have broken locally.
        let title = match group.origin {
            Origin::Organization => format!("{name}  ({})", tr("org")),
            Origin::Personal => name,
        };
        // Renaming replaces the heading outright rather than editing in place
        // under it: the field is the heading for as long as it is open, so
        // there is no moment where two names for the same group are on screen.
        if let Some((at, draft)) = state.group_rename.as_mut().filter(|(at, _)| *at == gi) {
            let at = *at;
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                let field = ui.add(
                    egui::TextEdit::singleline(draft)
                        .desired_width(200.0)
                        .hint_text(tr("Group name")),
                );
                field.request_focus();
                let entered = field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                if prefs::button(ui, tr("Rename")).clicked() || entered {
                    rename_done = Some((at, draft.clone()));
                }
                if prefs::button(ui, tr("Cancel")).clicked() {
                    rename_cancelled = true;
                }
            });
            ui.add_space(6.0);
        } else {
            // Clicking the group's name is how a group is acted on: it is what
            // "New macro" then adds to, and where the right-click menu's own
            // entries land.
            let heading = prefs::section_link(ui, &title).on_hover_text(tr(
                "Click to add new macros here; right-click to rename the group.",
            ));
            if heading.clicked() {
                to_select_group = Some(gi);
            }
            let editable_group = group.origin.is_editable();
            heading.context_menu(|ui| {
                if ui
                    .add_enabled(editable_group, egui::Button::new(tr("Rename group...")))
                    .on_hover_text(tr(
                        "Only your own groups; the organization's file is never written.",
                    ))
                    .clicked()
                {
                    rename_started = Some(gi);
                    ui.close_menu();
                }
                if ui.button(tr("New macro in this group")).clicked() {
                    add_to = Some(gi);
                    ui.close_menu();
                }
            });
        }
        prefs::card(ui, ("macro-group", gi), |card| {
            if matching.is_empty() {
                card.row(Row::new(tr("No macros in this group yet.")), |_| ());
            }
            for (mi, m) in matching {
                // A binding nobody can see is a binding nobody uses, so it is
                // shown under the name.
                let mut about: Vec<&str> = Vec::new();
                if !m.description.is_empty() {
                    about.push(&m.description);
                }
                if let Some(key) = &m.key {
                    about.push(key);
                }
                if m.confirm {
                    about.push(tr("confirms"));
                }
                let about = about.join("  \u{b7}  ");
                // The macro last opened is lit, so coming back from its page
                // finds it at once.
                let lit = if state.selected == Some((gi, mi)) {
                    0.5
                } else {
                    0.0
                };
                let spec = Row::new(&m.name)
                    .subtitle((!about.is_empty()).then_some(about.as_str()))
                    .highlight(lit);
                if card.nav(spec).clicked() {
                    to_select = Some((gi, mi));
                }
            }
        });
    }

    let mut saved = false;
    if let Some(gi) = to_select_group {
        state.selected_group = Some(gi);
    }
    if let Some(gi) = rename_started {
        let current = groups.get(gi).map(|g| g.name.clone()).unwrap_or_default();
        state.selected_group = Some(gi);
        state.group_rename = Some((gi, current));
    }
    if rename_cancelled {
        state.group_rename = None;
    }
    if let Some((gi, name)) = rename_done {
        state.group_rename = None;
        if rename_group(groups, gi, &name) {
            // Renaming can merge two groups into one, which moves every index
            // after it. Nothing is left pointing into the old arrangement
            // rather than guessing where it went.
            state.select(None, groups);
            state.selected_group = None;
            saved = true;
        }
    }
    if create_cancelled {
        state.new_group = None;
    }
    if let Some(name) = create_group {
        state.new_group = None;
        // A blank name is the one the list shows as "Macros", and two of those
        // would be indistinguishable, so an unnamed group is given a name
        // nothing else has.
        let name = match name.trim() {
            "" => unused_group_name(groups),
            named => named.to_string(),
        };
        // Straight into an empty macro: a group with nothing in it is a
        // heading, and the reason anyone makes one is to put a macro in it.
        add_to = Some(personal_group(groups, &name));
    }
    if saved {
        c.requests.push(UiRequest::SavePersonalMacros);
    }
    if let Some(at) = to_select {
        open_macro(c, at);
    } else if let Some(gi) = add_to {
        new_macro(c, gi);
    }
}

/// Opens the editor on a macro of the list.
fn open_macro(c: &mut Ctx<'_>, at: (usize, usize)) {
    if c.state.macros.select(Some(at), c.macro_groups) {
        c.requests.push(UiRequest::SavePersonalMacros);
    }
    c.go = Some(EDITOR);
}

/// Adds an empty macro to the group pointed at - or to a personal one beside
/// it, for a group of the organization's - and opens the editor on it.
fn new_macro(c: &mut Ctx<'_>, gi: usize) {
    let groups = &mut *c.macro_groups;
    let gi = new_macro_group(groups, gi);
    let at = add_macro(
        groups,
        gi,
        Macro {
            origin: Origin::Personal,
            name: tr("New macro").to_string(),
            ..Macro::default()
        },
    );
    c.state.macros.select(Some(at), groups);
    c.requests.push(UiRequest::SavePersonalMacros);
    c.go = Some(EDITOR);
}

/// New macro, into the group in focus, and New group.
///
/// A group is picked by clicking it, not by typing its name. Typing it was the
/// old way in, and it was a bad one: the name had to be spelled exactly, a
/// typo silently made a second group beside the intended one, and there was
/// no way at all to correct a name once written.
fn add_buttons(card: &mut Card<'_>, c: &mut Ctx<'_>) {
    let state = &mut c.state.macros;
    // The group in focus: the one that was clicked, or the group of the macro
    // last opened.
    let target = state
        .selected_group
        .or(state.selected.map(|(gi, _)| gi))
        .and_then(|gi| Some((gi, c.macro_groups.get(gi)?.name.clone())));
    let (label, hint) = match target.as_ref() {
        Some((_, name)) if !name.is_empty() => (
            tr1("New macro in {}", name),
            tr1("A new macro in {}.", name),
        ),
        Some(_) => (
            tr("New macro").to_owned(),
            tr("A new macro in this group.").to_owned(),
        ),
        None => (
            tr("New macro").to_owned(),
            tr("Click a group first: a macro is always in one.").to_owned(),
        ),
    };
    let mut add = false;
    card.buttons(|ui| {
        if prefs::button(ui, tr("New group"))
            .on_hover_text(tr("Groups are how the right-click menu is arranged."))
            .clicked()
        {
            // Straight into the name field. Nothing is created until it is
            // confirmed, and confirming makes the group and its first macro
            // together.
            state.new_group = Some(String::new());
            state.group_rename = None;
        }
        add = ui
            .add_enabled(target.is_some(), prefs::button_widget(&label))
            .on_hover_text(hint.as_str())
            .on_disabled_hover_text(hint.as_str())
            .clicked();
    });
    if let Some((gi, _)) = target.filter(|_| add) {
        new_macro(c, gi);
    }
}

fn manager_shortcut(ui: &mut Ui, c: &mut Ctx<'_>) {
    // The same field a macro's own binding is set in, so the two behave the
    // same way - including the warning when the chord is one the app has
    // already taken.
    c.changed |= shortcut::picker(
        ui,
        &mut c.settings.macro_manager_shortcut,
        &mut c.state.capture_manager_shortcut,
    );
}

fn macros_folder(ui: &mut Ui, c: &mut Ctx<'_>) {
    let path = crate::config::personal_macros_path();
    if prefs::button(ui, tr("Open folder"))
        .on_hover_text(path.display().to_string())
        .clicked()
    {
        if let Some(folder) = path.parent() {
            c.requests.push(UiRequest::OpenFolder(folder.to_path_buf()));
        }
    }
}

fn organization_macros() -> Vec<Section> {
    vec![section(
        "Organization macro file (shared, read-only)",
        vec![Item::rows("org_macros_path", "Path", org_macros).keys(&[
            "shared",
            "unc",
            "network",
            "file",
            "compartilhado",
            "rede",
            "arquivo",
        ])],
    )]
}

/// The path, and under it whether it can be reached - which changes as the
/// path is typed, so it is a row of its own kind.
fn org_macros(card: &mut Card<'_>, c: &mut Ctx<'_>) {
    let status = c.settings.org_macros().map(|path| path.exists());
    let subtitle = match status {
        None => tr("A UNC share, mapped drive, or local copy. Leave empty for none."),
        Some(true) => tr("Found."),
        Some(false) => tr("Not reachable right now - personal macros will still load."),
    };
    let picking = &mut c.state.macros.picking_org;
    let settings = &mut *c.settings;
    let mut changed = false;
    card.row(Row::new(tr("Path")).subtitle(subtitle), |ui| {
        if file_dialog::AVAILABLE
            && ui
                .add_enabled(picking.is_none(), prefs::button_widget(tr("Browse...")))
                .clicked()
        {
            *picking = Some(file_dialog::pick(
                ui.ctx(),
                tr("Choose the organization macro file"),
                file_dialog::Kind {
                    name: tr("Macro files"),
                    patterns: "*.xml",
                },
                &settings.org_macros_path.display().to_string(),
            ));
        }
        let mut org = settings.org_macros_path.display().to_string();
        let edit = egui::TextEdit::singleline(&mut org).desired_width(260.0);
        let edit = if status == Some(false) {
            edit.text_color(WARNING)
        } else {
            edit
        };
        if ui.add(edit).changed() {
            settings.org_macros_path = std::path::PathBuf::from(org.trim());
            changed = true;
        }
    });
    c.changed |= changed;
}

/// The answer of the organisation file's dialog, whatever page is on screen
/// when it comes.
pub(super) fn collect_picked_org_file(ctx: &egui::Context, c: &mut Ctx<'_>) {
    if let Some(path) = file_dialog::collect(ctx, &mut c.state.macros.picking_org) {
        c.settings.org_macros_path = std::path::PathBuf::from(path);
        c.changed = true;
        // The organisation's groups come first in the list, so loading a
        // different file moves every personal macro's place in it: the editor
        // lets go of the one it is on - keeping what was typed - before the
        // indices it holds come to mean another macro.
        let state = &mut c.state.macros;
        if state.select(None, c.macro_groups) {
            c.requests.push(UiRequest::SavePersonalMacros);
        }
        state.selected_group = None;
        state.group_rename = None;
        state.confirm_delete = None;
        c.requests.push(UiRequest::ReloadMacros);
    }
}

/// A copy of `source`, under a name that says it is one.
fn copy_of(source: &Macro) -> Macro {
    let mut copy = source.clone();
    copy.origin = Origin::Personal;
    copy.name = tr1("{} copy", &source.name);
    // A shortcut belongs to one macro: two answering the same chord means the
    // one that fires is whichever the loop happens to reach last.
    copy.key = None;
    copy
}

/// A name no personal group is using yet, for a group about to be created.
///
/// Numbered rather than left blank: an empty name is what the list shows as
/// "Macros", and two of those side by side would be indistinguishable.
fn unused_group_name(groups: &[MacroGroup]) -> String {
    let base = tr("New group").to_string();
    if !groups.iter().any(|g| g.name == base) {
        return base;
    }
    (2..)
        .map(|n| format!("{base} {n}"))
        .find(|name| !groups.iter().any(|g| &g.name == name))
        .unwrap_or(base)
}

/// The group a new macro should actually go into, given the one that was
/// pointed at.
///
/// Its own index when the group is the user's; a personal group of the same
/// name otherwise. The organisation's file is never written, so a macro cannot
/// be added to one of its groups - it gets a group of its own beside it, which
/// is exactly what Duplicate does.
fn new_macro_group(groups: &mut Vec<MacroGroup>, gi: usize) -> usize {
    match groups.get(gi) {
        Some(group) if group.origin.is_editable() => gi,
        Some(group) => {
            let name = group.name.clone();
            personal_group(groups, &name)
        }
        None => personal_group(groups, &unused_group_name(groups)),
    }
}

/// Renames a personal group, reporting whether anything changed.
///
/// A name already taken by another personal group merges the two rather than
/// leaving a duplicate: two groups under one name are one group as far as the
/// right-click menu is concerned, and keeping them apart in the file only
/// means the menu shows the heading twice.
fn rename_group(groups: &mut Vec<MacroGroup>, gi: usize, name: &str) -> bool {
    let name = name.trim().to_string();
    let Some(group) = groups.get(gi) else {
        return false;
    };
    // The organisation's file is never written, and a blank name is the one
    // the list already shows as "Macros" - which would be a rename to nothing.
    if !group.origin.is_editable() || name.is_empty() || group.name == name {
        return false;
    }
    let existing = groups
        .iter()
        .position(|g| g.name == name && g.origin.is_editable());
    match existing {
        Some(into) if into != gi => {
            let moved = groups.remove(gi);
            // `remove` shifts everything after it down by one.
            let into = if into > gi { into - 1 } else { into };
            groups[into].macros.extend(moved.macros);
        }
        _ => groups[gi].name = name,
    }
    true
}

/// The index of the personal group called `name`, creating it if there is none.
fn personal_group(groups: &mut Vec<MacroGroup>, name: &str) -> usize {
    match groups
        .iter()
        .position(|g| g.name == name && g.origin.is_editable())
    {
        Some(gi) => gi,
        None => {
            groups.push(MacroGroup {
                name: name.to_string(),
                origin: Origin::Personal,
                macros: Vec::new(),
            });
            groups.len() - 1
        }
    }
}

/// Adds `m` to a group and reports where it landed.
fn add_macro(groups: &mut [MacroGroup], gi: usize, m: Macro) -> (usize, usize) {
    groups[gi].macros.push(m);
    (gi, groups[gi].macros.len() - 1)
}

// ---------------------------------------------------------------------------
// The editor
// ---------------------------------------------------------------------------

fn editable(c: &Ctx<'_>) -> bool {
    c.state
        .macros
        .stored(c.macro_groups)
        .is_some_and(|m| m.origin.is_editable())
}

/// Whether the body is being kept off the screen just now.
fn hidden(c: &Ctx<'_>) -> bool {
    let state = &c.state.macros;
    state.draft.as_ref().is_some_and(|m| m.hide_command) && !state.reveal_body
}

fn editor() -> Vec<Section> {
    vec![
        section(
            "General",
            vec![
                Item::control("macro_name", "Name", |ui, c| {
                    if let Some(draft) = c.state.macros.draft.as_mut() {
                        ui.add(egui::TextEdit::singleline(&mut draft.name).desired_width(260.0));
                    }
                })
                .contextual()
                .keys(&["macro", "nome", "rename", "renomear"]),
                Item::control("macro_description", "Description", |ui, c| {
                    if let Some(draft) = c.state.macros.draft.as_mut() {
                        ui.add(
                            egui::TextEdit::singleline(&mut draft.description).desired_width(260.0),
                        );
                    }
                })
                .contextual()
                .keys(&["macro", "descricao", "tooltip"]),
                Item::control("macro_shortcut", "Shortcut", |ui, c| {
                    let state = &mut c.state.macros;
                    if let Some(draft) = state.draft.as_mut() {
                        // The same field the page's own shortcut is set in.
                        shortcut::picker(ui, &mut draft.key, &mut state.capture_shortcut);
                    }
                })
                .contextual()
                .keys(&["macro", "hotkey", "keyboard", "atalho", "teclado"]),
                Item::rows(
                    "macro_confirm",
                    "Confirm before sending (use for anything that writes)",
                    confirm_row,
                )
                .contextual()
                .keys(&["macro", "confirm", "confirmar", "write", "gravar"]),
                Item::rows("macro_hide", "Hide command", hide_row)
                    .contextual()
                    .keys(&["macro", "password", "secret", "senha", "segredo", "ocultar"]),
            ],
        )
        .enabled(editable),
        section(
            "Commands",
            vec![Item::rows(
                "macro_body",
                "Body - one command per line, {{param}} is substituted",
                body_row,
            )
            .contextual()
            .keys(&["macro", "body", "command", "corpo", "comando"])],
        )
        .enabled(editable),
        section(
            "Parameters",
            vec![Item::rows("macro_params", "Parameters", params_rows)
                .contextual()
                .keys(&[
                    "macro",
                    "param",
                    "placeholder",
                    "prompt",
                    "default",
                    "parametro",
                    "padrao",
                ])],
        )
        .enabled(editable)
        .footer(
            "Each one becomes a field in the right-click menu, filled in before the macro is sent.",
        ),
        // What it would send, with the defaults filled in. Not decoration: it
        // is where a substituted value pointing at the wrong global gets
        // noticed, before the text reaches a shared database.
        section(
            "Will send",
            vec![Item::rows("macro_preview", "Will send", will_send)
                .contextual()
                .keys(&["macro", "preview", "previa", "expand"])],
        ),
        untitled(vec![Item::rows("macro_actions", "Run", action_rows)
            .contextual()
            .keys(&[
                "macro",
                "save",
                "revert",
                "duplicate",
                "delete",
                "salvar",
                "reverter",
                "executar",
                "duplicar",
                "excluir",
            ])])
        .footer("Leaving this page keeps what was typed, as Save does."),
    ]
}

fn macro_title(c: &Ctx<'_>) -> String {
    let state = &c.state.macros;
    state
        .draft
        .as_ref()
        .or(state.stored(c.macro_groups))
        .map(|m| m.name.clone())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| tr("Macro").to_owned())
}

/// Above the editor's cards: the draft loaded, and the warning that an
/// organization macro is only being shown.
///
/// With no macro to edit - the one selected has gone - the page goes back to
/// the list rather than drawing an editor for nothing.
fn editor_top(ui: &mut Ui, c: &mut Ctx<'_>) {
    let state = &mut c.state.macros;
    let Some(at) = state.selected else {
        c.go = Some(MACROS_PAGE);
        return;
    };
    if state.draft_of != Some(at) && state.select(Some(at), c.macro_groups) {
        c.requests.push(UiRequest::SavePersonalMacros);
    }
    if !editable(c) {
        ui.add_space(6.0);
        ui.colored_label(
            WARNING,
            tr("Provided by the organization; read-only here. Duplicate it to make changes."),
        );
    }
}

fn confirm_row(card: &mut Card<'_>, c: &mut Ctx<'_>) {
    if let Some(draft) = c.state.macros.draft.as_mut() {
        let spec = Row::new(tr("Confirm before sending (use for anything that writes)"));
        card.toggle(spec, &mut draft.confirm);
    }
}

fn hide_row(card: &mut Card<'_>, c: &mut Ctx<'_>) {
    let state = &mut c.state.macros;
    if let Some(draft) = state.draft.as_mut() {
        let spec = Row::new(tr("Hide command")).hint(tr(
            "For a body that carries a password. Keeps it out of the menus; IRIS still echoes what it is sent.",
        ));
        if card.toggle(spec, &mut draft.hide_command).inner {
            // Switching it on hides the body again immediately, so the secret
            // is not left on screen by the very act of protecting it.
            state.reveal_body = false;
        }
    }
}

fn body_row(card: &mut Card<'_>, c: &mut Ctx<'_>) {
    let hidden = hidden(c);
    let state = &mut c.state.macros;
    if state.draft.is_none() {
        return;
    }
    card.custom(|ui| {
        ui.label(tr("Body - one command per line, {{param}} is substituted"));
        if hidden {
            ui.horizontal(|ui| {
                ui.weak(hidden_lines_note(
                    crate::features::macros::body_lines(&state.body_draft).len(),
                ));
                if prefs::button(ui, tr("Reveal")).clicked() {
                    state.reveal_body = true;
                }
            });
        } else {
            // The field owns the text and nothing rewrites it between
            // keystrokes - see `body_draft`.
            ui.add(
                egui::TextEdit::multiline(&mut state.body_draft)
                    .desired_rows(6)
                    .desired_width(f32::INFINITY)
                    .code_editor(),
            );
        }
    });
}

/// One line per parameter - its name, its prompt, its default - each with the
/// checker's verdict under it, and the placeholders nothing declares.
fn params_rows(card: &mut Card<'_>, c: &mut Ctx<'_>) {
    let hidden = hidden(c);
    let state = &mut c.state.macros;
    let Some(draft) = state.draft.as_mut() else {
        return;
    };
    let body = &state.body_draft;
    let mut drop_param = None;
    card.custom(|ui| {
        // Checked against the text as it is typed, blank lines kept, so a
        // line number is the one the field shows. Blank lines carry no
        // placeholder, so the verdict is the one the split body sent by Run
        // would get.
        let mut check = Macro {
            params: draft.params.clone(),
            body: body.lines().map(str::to_string).collect(),
            ..Macro::default()
        };
        if draft.params.is_empty() {
            ui.weak(tr("No parameters."));
        }
        for pi in 0..draft.params.len() {
            let param = &mut draft.params[pi];
            ui.horizontal(|ui| {
                ui.add(egui::TextEdit::singleline(&mut param.name).desired_width(90.0))
                    .on_hover_text(tr("Name used as {{name}} in the body"));
                ui.add(egui::TextEdit::singleline(&mut param.prompt).desired_width(150.0))
                    .on_hover_text(tr("Prompt shown when running"));
                ui.add(egui::TextEdit::singleline(&mut param.default).desired_width(110.0))
                    .on_hover_text(tr("Default value"));
                if ui
                    .small_button("x")
                    .on_hover_text(tr("Remove this parameter"))
                    .clicked()
                {
                    drop_param = Some(pi);
                }
            });
            // Updated row by row rather than cloned once above, so the verdict
            // follows the keystroke that changed it. Only earlier rows bear on
            // this one's, and they are already up to date.
            check.params[pi] = param.clone();
            param_status(ui, &check, pi);
        }
        undeclared_status(ui, &check, hidden);
    });
    if let Some(pi) = drop_param {
        draft.params.remove(pi);
    }
    card.buttons(|ui| {
        if prefs::button(ui, tr("Add parameter")).clicked() {
            draft.params.push(Param::default());
        }
    });
}

fn will_send(card: &mut Card<'_>, c: &mut Ctx<'_>) {
    let hidden = hidden(c);
    let state = &c.state.macros;
    let Some(draft) = state.draft.as_ref() else {
        return;
    };
    let lines = crate::features::macros::body_lines(&state.body_draft);
    card.custom(|ui| {
        if hidden {
            ui.weak(hidden_lines_note(lines.len()));
            return;
        }
        let mut preview = draft.clone();
        preview.body = lines;
        let expanded = preview.expand(&preview.default_values());
        if expanded.is_empty() {
            ui.weak(tr("Nothing yet."));
        }
        for line in expanded {
            ui.code(line);
        }
    });
}

/// Run, Duplicate and Delete for any macro; Save and Revert for one that can
/// be changed.
fn action_rows(card: &mut Card<'_>, c: &mut Ctx<'_>) {
    let editable = editable(c);
    let state = &mut c.state.macros;
    let (Some(at), Some(draft)) = (state.selected, state.draft.as_ref()) else {
        return;
    };
    let (mut save, mut revert, mut run, mut copy, mut delete) = (false, false, false, false, false);
    // Right to left: Save ends the row, where the eye finishes reading it.
    card.buttons(|ui| {
        if editable {
            save = prefs::button(ui, tr("Save"))
                .on_hover_text(tr("Writes your personal macro file."))
                .clicked();
            revert = prefs::button(ui, tr("Revert"))
                .on_hover_text(tr("Back to what is in the file."))
                .clicked();
        }
        run = prefs::button(ui, tr("Run"))
            .on_hover_text(tr(
                "Sends it to the active session, asking for parameters and confirmation exactly as the right-click menu does.",
            ))
            .clicked();
        // Duplicating is the only way to get an organisation macro that can
        // be changed.
        copy = prefs::button(ui, tr("Duplicate"))
            .on_hover_text(tr1("A copy of {} in your own macros.", &draft.name))
            .clicked();
        delete = ui
            .add_enabled(editable, prefs::button_widget(tr("Delete")))
            .on_hover_text(tr(
                "Only your own macros; the organization's file is never written.",
            ))
            .on_disabled_hover_text(tr(
                "Only your own macros; the organization's file is never written.",
            ))
            .clicked();
    });
    if delete {
        state.confirm_delete = Some(at);
    }
    let mut confirmed = false;
    if state.confirm_delete == Some(at) {
        let question = tr1("Delete {}?", &draft.name);
        let spec = Row::new(&question).subtitle(tr("A deleted macro cannot be recovered."));
        let mut keep = false;
        card.row(spec, |ui| {
            confirmed = prefs::button(ui, tr("Delete")).clicked();
            keep = prefs::button(ui, tr("Keep")).clicked();
        });
        if keep {
            state.confirm_delete = None;
        }
    }

    let groups = &mut *c.macro_groups;
    if run {
        // Through the ordinary request path, so `confirm` and the parameter
        // prompt apply exactly as they do to the menu.
        let mut m = draft.clone();
        m.body = crate::features::macros::body_lines(&state.body_draft);
        c.requests.push(UiRequest::RunMacro(m));
    }
    if save && state.commit(groups) {
        c.requests.push(UiRequest::SavePersonalMacros);
    }
    if revert {
        // Loading the draft again from the file is exactly what selecting the
        // macro afresh does.
        state.draft_of = None;
        state.select(Some(at), groups);
    }
    if copy {
        if let Some(source) = state.stored(groups).cloned() {
            let mut source = source;
            // The copy is of what is on screen, edits and all, the way the
            // name in the hint promises.
            if let Some(draft) = state.draft.as_ref() {
                source = Macro {
                    origin: source.origin,
                    body: crate::features::macros::body_lines(&state.body_draft),
                    ..draft.clone()
                };
            }
            // Into a personal group of the same name: an organisation group is
            // never written to, so a copy out of one gets a group of its own
            // beside it rather than landing in the shared list.
            let group = groups[at.0].name.clone();
            let gi = personal_group(groups, &group);
            let new = add_macro(groups, gi, copy_of(&source));
            state.select(Some(new), groups);
            c.requests.push(UiRequest::SavePersonalMacros);
        }
    }
    if confirmed {
        let (gi, mi) = at;
        // Guarded by construction - Delete only lights up on a personal macro
        // - but checked again so a later change here cannot destroy shared
        // data.
        if groups[gi].macros[mi].origin.is_editable() {
            groups[gi].macros.remove(mi);
            if groups[gi].macros.is_empty() {
                groups.remove(gi);
            }
            // Straight out of the state rather than through `select`, which
            // would try to commit the draft of the macro that has just gone.
            state.selected = None;
            state.selected_group = None;
            state.draft_of = None;
            state.draft = None;
            c.requests.push(UiRequest::SavePersonalMacros);
            c.go = Some(MACROS_PAGE);
        }
        state.confirm_delete = None;
    }
}

/// `{{name}}`, spelled out for a message.
fn braced(name: &str) -> String {
    format!("{{{{{name}}}}}")
}

/// What the parameter at `index` does to the command, in words.
///
/// True with the good news, false with a warning; kept apart from the
/// drawing so the wording can be tested without a `Ui`.
fn param_status_text(m: &Macro, index: usize) -> (bool, String) {
    let name = m.params.get(index).map_or("", |p| p.name.as_str());
    match m.param_use(index) {
        ParamUse::Used { lines, count } => {
            let text = match lines.as_slice() {
                [line] if count == 1 => tr1("Used in line {}", &line.to_string()),
                [line] => tr2(
                    "Used {} times in line {}",
                    &count.to_string(),
                    &line.to_string(),
                ),
                _ => {
                    let lines: Vec<_> = lines.iter().map(usize::to_string).collect();
                    tr2(
                        "Used {} times, in lines {}",
                        &count.to_string(),
                        &lines.join(", "),
                    )
                }
            };
            (true, text)
        }
        ParamUse::Blank => (
            false,
            tr("Blank name: this parameter is ignored and not asked for.").to_string(),
        ),
        ParamUse::Duplicate => (
            false,
            tr1(
                "Duplicate: an earlier parameter fills {}, so this one is not asked for.",
                &braced(name),
            ),
        ),
        ParamUse::Unused => (
            false,
            tr1(
                "Not used: no {} in the command, so it is not asked for.",
                &braced(name),
            ),
        ),
    }
}

/// The checker's line under one parameter row.
fn param_status(ui: &mut Ui, m: &Macro, index: usize) {
    let (ok, text) = param_status_text(m, index);
    let colour = if !ok {
        ui.visuals().warn_fg_color
    } else if ui.visuals().dark_mode {
        egui::Color32::LIGHT_GREEN
    } else {
        egui::Color32::DARK_GREEN
    };
    ui.label(egui::RichText::new(text).small().color(colour));
}

/// Placeholders in the body that no parameter fills, one warning each.
///
/// The name is left out while the body is hidden: it is a piece of the text
/// the flag is there to keep off the screen.
fn undeclared_text(m: &Macro, hidden: bool) -> Vec<String> {
    m.undeclared_placeholders()
        .into_iter()
        .map(|(line, name)| {
            if hidden {
                tr1(
                    "Line {}: a placeholder has no parameter and is sent as typed.",
                    &line.to_string(),
                )
            } else {
                tr2(
                    "Line {}: {} has no parameter and is sent as typed.",
                    &line.to_string(),
                    &braced(name),
                )
            }
        })
        .collect()
}

fn undeclared_status(ui: &mut Ui, m: &Macro, hidden: bool) {
    let colour = ui.visuals().warn_fg_color;
    for text in undeclared_text(m, hidden) {
        ui.label(egui::RichText::new(text).small().color(colour));
    }
}

/// How many body lines a hidden macro has, without saying what they are.
fn hidden_lines_note(count: usize) -> String {
    match count {
        1 => tr("1 command hidden").to_string(),
        n => tr1("{} commands hidden", &n.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn groups() -> Vec<MacroGroup> {
        vec![
            MacroGroup {
                name: "Shared".into(),
                origin: Origin::Organization,
                macros: vec![Macro {
                    origin: Origin::Organization,
                    name: "Org one".into(),
                    key: Some("Ctrl+Shift+O".into()),
                    body: vec!["W 1".into()],
                    ..Macro::default()
                }],
            },
            MacroGroup {
                name: "Mine".into(),
                origin: Origin::Personal,
                macros: vec![Macro {
                    origin: Origin::Personal,
                    name: "Mine one".into(),
                    body: vec!["W 2".into()],
                    ..Macro::default()
                }],
            },
        ]
    }

    /// The organisation's file is never written, so a group of its own cannot
    /// be renamed - the list greys the entry out, and this is the guard behind
    /// that.
    #[test]
    fn an_organization_group_cannot_be_renamed() {
        let mut groups = groups();
        assert!(!rename_group(&mut groups, 0, "Anything"));
        assert_eq!(groups[0].name, "Shared");
    }

    #[test]
    fn renaming_a_personal_group_renames_it() {
        let mut groups = groups();
        assert!(rename_group(&mut groups, 1, "  Ours  "));
        assert_eq!(groups[1].name, "Ours");
    }

    /// A name already in use is a merge rather than a second group under the
    /// same heading: the right-click menu groups by name, so two would show as
    /// one anyway - with the heading drawn twice.
    #[test]
    fn renaming_onto_an_existing_personal_group_merges_the_two() {
        let mut groups = groups();
        groups.push(MacroGroup {
            name: "Other".into(),
            origin: Origin::Personal,
            macros: vec![Macro {
                origin: Origin::Personal,
                name: "Other one".into(),
                ..Macro::default()
            }],
        });

        assert!(rename_group(&mut groups, 2, "Mine"));
        assert_eq!(groups.len(), 2, "the emptied group is gone");
        let mine = groups.iter().find(|g| g.name == "Mine").unwrap();
        assert_eq!(mine.macros.len(), 2, "both macros are in the one group");
    }

    /// Nothing is written to a shared group: a macro added to one lands in a
    /// personal group of the same name, exactly as Duplicate does.
    #[test]
    fn a_new_macro_in_an_organization_group_gets_a_personal_one() {
        let mut groups = groups();
        let gi = new_macro_group(&mut groups, 0);
        assert_ne!(gi, 0);
        assert_eq!(groups[gi].name, "Shared");
        assert!(groups[gi].origin.is_editable());
    }

    #[test]
    fn a_new_macro_in_a_personal_group_stays_in_it() {
        let mut groups = groups();
        assert_eq!(new_macro_group(&mut groups, 1), 1);
        assert_eq!(groups.len(), 2, "no group was made");
    }

    /// Two groups called "New group" would be indistinguishable in the list,
    /// so the second one is numbered.
    #[test]
    fn a_new_group_gets_a_name_nothing_else_has() {
        let mut groups = groups();
        let first = unused_group_name(&groups);
        groups.push(MacroGroup {
            name: first.clone(),
            origin: Origin::Personal,
            macros: Vec::new(),
        });
        let second = unused_group_name(&groups);
        assert_ne!(first, second);
    }

    /// Selecting one macro and then another writes the first one's edits back,
    /// which is what makes moving between them safe without a modal.
    #[test]
    fn moving_off_an_edited_macro_keeps_the_edit() {
        let mut groups = groups();
        let mut state = MacroPageState::default();
        state.select(Some((1, 0)), &mut groups);
        state.draft.as_mut().unwrap().name = "Renamed".into();

        assert!(
            state.select(Some((0, 0)), &mut groups),
            "the displaced edit has to be written"
        );
        assert_eq!(groups[1].macros[0].name, "Renamed");
    }

    /// And selecting away from an untouched one writes nothing: the personal
    /// file is not rewritten just for having been looked at.
    #[test]
    fn moving_off_an_untouched_macro_writes_nothing() {
        let mut groups = groups();
        let mut state = MacroPageState::default();
        state.select(Some((1, 0)), &mut groups);
        assert!(!state.select(Some((0, 0)), &mut groups));
    }

    /// Going back to the list from an edited macro keeps the edit, and keeps
    /// the macro selected so the list can show which one it was.
    #[test]
    fn leaving_the_editor_keeps_the_edit_and_the_selection() {
        let mut groups = groups();
        let mut state = MacroPageState::default();
        state.select(Some((1, 0)), &mut groups);
        state.draft.as_mut().unwrap().description = "Says two".into();
        state.capture_shortcut = true;

        assert!(state.leave(&mut groups), "the edit has to be written");
        assert_eq!(groups[1].macros[0].description, "Says two");
        assert_eq!(state.selected, Some((1, 0)));
        assert!(state.draft.is_none());
        assert!(
            !state.capture_shortcut,
            "nothing is left listening for keys"
        );
        assert!(!state.leave(&mut groups), "and only once");
    }

    /// The shared file is never written, whatever the draft says - the guard
    /// that matters most here, since these macros come from a share the whole
    /// team reads.
    #[test]
    fn an_organization_macro_is_never_committed() {
        let mut groups = groups();
        let mut state = MacroPageState::default();
        state.select(Some((0, 0)), &mut groups);
        state.draft.as_mut().unwrap().name = "Tampered".into();

        assert!(!state.commit(&mut groups));
        assert!(!state.leave(&mut groups));
        assert_eq!(groups[0].macros[0].name, "Org one");
    }

    /// A copy is the user's own, in a group that can be written, and does not
    /// take the original's shortcut with it.
    #[test]
    fn a_copy_of_a_shared_macro_is_personal_and_unbound() {
        let mut groups = groups();
        let source = groups[0].macros[0].clone();
        let group = groups[0].name.clone();
        let gi = personal_group(&mut groups, &group);
        let (gi, mi) = add_macro(&mut groups, gi, copy_of(&source));

        assert_ne!(gi, 0, "the shared group is not written to");
        assert_eq!(
            groups[gi].name, group,
            "but the copy keeps its group's name"
        );
        assert_eq!(groups[gi].origin, Origin::Personal);
        assert_eq!(groups[gi].macros[mi].origin, Origin::Personal);
        assert_eq!(groups[gi].macros[mi].key, None);
        assert_eq!(groups[gi].macros[mi].body, source.body);
    }

    /// The body reaches the file as lines, split once on the way there rather
    /// than on every keystroke.
    #[test]
    fn the_typed_body_is_split_into_lines_on_the_way_to_the_file() {
        let mut groups = groups();
        let mut state = MacroPageState::default();
        state.select(Some((1, 0)), &mut groups);
        state.body_draft = "  W 1\n\n W 2  ".into();

        assert!(state.commit(&mut groups));
        assert_eq!(groups[1].macros[0].body, vec!["W 1", "W 2"]);
    }

    fn checked(names: &[&str], body: &[&str]) -> Macro {
        Macro {
            params: names
                .iter()
                .map(|n| Param {
                    name: n.to_string(),
                    ..Param::default()
                })
                .collect(),
            body: body.iter().map(|l| l.to_string()).collect(),
            ..Macro::default()
        }
    }

    // The wording is checked by what it carries - the numbers and the
    // placeholder - and not by the sentence: the language is a global, and the
    // i18n tests switch it while these run.

    #[test]
    fn the_checker_says_where_a_parameter_is_used() {
        let m = checked(
            &["g", "n"],
            &["ZWRITE ^{{g}}", "", "Set ^{{g}}({{n}}) = {{n}}"],
        );
        let (ok, text) = param_status_text(&m, 0);
        assert!(ok);
        assert!(text.contains('2') && text.contains("1, 3"), "{text}");
        let (ok, text) = param_status_text(&m, 1);
        assert!(ok);
        assert!(text.contains('2') && text.contains('3'), "{text}");
        let once = checked(&["g"], &["ZWRITE ^{{g}}"]);
        assert!(param_status_text(&once, 0).0);
    }

    #[test]
    fn the_checker_warns_about_a_parameter_that_is_never_asked_for() {
        let m = checked(&["", "x", "g", "g"], &["ZWRITE ^{{g}}"]);
        assert!(!param_status_text(&m, 0).0);
        let (ok, text) = param_status_text(&m, 1);
        assert!(!ok);
        assert!(text.contains("{{x}}"), "{text}");
        assert!(param_status_text(&m, 2).0);
        assert!(!param_status_text(&m, 3).0);
    }

    #[test]
    fn the_checker_finds_a_placeholder_nothing_declares() {
        let m = checked(&["g"], &["ZWRITE ^{{g}}", "Write {{typo}}"]);
        let warnings = undeclared_text(&m, false);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("{{typo}}") && warnings[0].contains('2'));
    }

    /// A hidden body's placeholder names are part of what is being hidden.
    #[test]
    fn a_hidden_body_is_checked_without_naming_its_placeholders() {
        let m = checked(&[], &["Set pw = \"{{secret}}\""]);
        let warnings = undeclared_text(&m, true);
        assert_eq!(warnings.len(), 1);
        assert!(!warnings[0].contains("secret"));
    }
}
