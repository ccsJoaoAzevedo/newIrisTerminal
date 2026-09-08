//! The macro manager: pick a macro, edit everything it carries, and make new
//! ones.
//!
//! Macros used to live in a side panel that was three things at once — the list
//! you ran them from, the details of the selected one, and the door to an
//! editor in a window of its own. Running them has moved to the terminal's own
//! right-click menu, where the output you are running them against is; what is
//! left here is the managing, in the same two-pane shape as
//! [`crate::ui::theme_manager`], because it is the same job on a different kind
//! of file.
//!
//! Two kinds of macro, and the difference is the whole of the read-only half of
//! this window: the organisation's file is shared and never written from the
//! app, so those are shown and can be copied but not changed. Only the personal
//! file is edited here, and only [`MacroAction::Save`] writes it.

use egui::{Context, Ui};

use crate::features::macros::{Macro, MacroGroup, Origin, Param};
use crate::i18n::{tr, tr1};
use crate::ui::panels::WARNING;
use crate::ui::shortcut;

/// Something the manager asked for. `app.rs` carries it out, so nothing is
/// written to disk or sent to a session from inside a paint pass.
#[derive(Clone, Debug, PartialEq)]
pub enum MacroAction {
    /// Write the personal macro file back to disk.
    Save,
    /// Send this macro to the active session, parameters and confirmation
    /// applying exactly as they do to the right-click menu.
    Run(Macro),
}

/// State the manager keeps between frames.
#[derive(Default)]
pub struct MacroManagerState {
    pub open: bool,
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
    /// Group name for a macro about to be created.
    new_group: String,
    /// A macro Delete has been pressed on, waiting on the confirmation that a
    /// deleted macro is not recoverable.
    confirm_delete: Option<(usize, usize)>,
}

impl MacroManagerState {
    /// Puts the editor on one macro, loading a draft of it. Reports whether
    /// what it displaced has to be written to disk.
    ///
    /// Whatever was being edited is committed first: moving down the list is
    /// not a gesture anyone means as "throw away what I just typed", and a
    /// modal asking about it every time would be worse than either.
    fn select(&mut self, at: Option<(usize, usize)>, groups: &mut [MacroGroup]) -> bool {
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

    /// The selected macro as it stands in the file, rather than in the draft.
    fn stored<'a>(&self, groups: &'a [MacroGroup]) -> Option<&'a Macro> {
        let (gi, mi) = self.selected?;
        groups.get(gi)?.macros.get(mi)
    }
}

/// The macro manager window.
pub fn macro_manager(
    ctx: &Context,
    state: &mut MacroManagerState,
    groups: &mut Vec<MacroGroup>,
    buttons: &crate::config::theme::WindowButtons,
    native_decorations: bool,
) -> Vec<MacroAction> {
    let mut actions: Vec<MacroAction> = Vec::new();
    if !state.open {
        return actions;
    }
    let mut open = true;

    // An index into a list that has since been reloaded, or had a macro
    // deleted, would point at the wrong macro; a stale selection is dropped
    // rather than followed.
    if state
        .selected
        .is_some_and(|(gi, mi)| groups.get(gi).and_then(|g| g.macros.get(mi)).is_none())
    {
        state.selected = None;
        state.draft_of = None;
        state.draft = None;
    }

    crate::ui::detach::shell(
        ctx,
        "nit-macro-manager",
        tr("Macros"),
        &mut open,
        // Wide enough for the list and the editor side by side, and tall enough
        // for the body and the preview of what it would send.
        [960.0, 660.0],
        buttons,
        native_decorations,
        // Nothing to remember across runs: the manager is opened to make one
        // change and closed again.
        None,
        |ui| {
            ui.horizontal_top(|ui| {
                macro_list(ui, state, groups, &mut actions);
                ui.separator();
                ui.vertical(|ui| {
                    // Bounded so the hints under the fields wrap rather than
                    // stretching the window out.
                    ui.set_min_width(520.0);
                    editor(ui, state, groups, &mut actions);
                });
            });
        },
    );

    if !open {
        state.open = false;
        // Closing with an edit in flight still keeps it: this is a window with
        // a list in it, not a dialog with an OK button, and nothing else would
        // ever write it.
        if state.commit(groups) {
            actions.push(MacroAction::Save);
        }
        state.capture_shortcut = false;
    }
    actions
}

/// The left-hand pane: every macro there is, and the buttons that make and
/// unmake them.
fn macro_list(
    ui: &mut Ui,
    state: &mut MacroManagerState,
    groups: &mut Vec<MacroGroup>,
    actions: &mut Vec<MacroAction>,
) {
    ui.vertical(|ui| {
        ui.set_min_width(260.0);
        ui.set_max_width(300.0);

        ui.horizontal(|ui| {
            ui.label(tr("Filter"));
            ui.add(egui::TextEdit::singleline(&mut state.filter).desired_width(150.0));
        });
        ui.separator();

        let filter = state.filter.to_lowercase();
        let mut to_select: Option<(usize, usize)> = None;

        egui::ScrollArea::vertical()
            .id_source("macro-list")
            .max_height(380.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if groups.is_empty() {
                    ui.weak(tr("No macros defined."));
                }
                for (gi, group) in groups.iter().enumerate() {
                    let matching: Vec<(usize, &Macro)> = group
                        .macros
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| {
                            filter.is_empty()
                                || m.name.to_lowercase().contains(&filter)
                                || m.description.to_lowercase().contains(&filter)
                        })
                        .collect();
                    if matching.is_empty() {
                        continue;
                    }
                    let name = if group.name.is_empty() {
                        tr("Macros").to_string()
                    } else {
                        group.name.clone()
                    };
                    // Provenance on the group rather than on every row: a
                    // shared macro behaving oddly is someone else's file, not
                    // something the user can have broken locally.
                    let title = match group.origin {
                        Origin::Organization => format!("{name}  ({})", tr("org")),
                        Origin::Personal => name,
                    };
                    egui::CollapsingHeader::new(title)
                        .id_source(("macro-group", gi))
                        .default_open(true)
                        .show(ui, |ui| {
                            for (mi, m) in matching {
                                let label = if m.confirm {
                                    format!("{}  ({})", m.name, tr("confirms"))
                                } else {
                                    m.name.clone()
                                };
                                let selected = state.selected == Some((gi, mi));
                                let row = ui.selectable_label(selected, label);
                                let row = if m.description.is_empty() {
                                    row
                                } else {
                                    row.on_hover_text(&m.description)
                                };
                                if row.clicked() {
                                    to_select = Some((gi, mi));
                                }
                                // A binding nobody can see is a binding nobody
                                // uses, so it is shown under the name.
                                if let Some(key) = &m.key {
                                    ui.weak(key);
                                }
                            }
                        });
                }
            });

        if let Some(at) = to_select {
            if state.select(Some(at), groups) {
                actions.push(MacroAction::Save);
            }
        }

        ui.add_space(8.0);
        ui.separator();

        // Duplicating is the only way to get an organisation macro that can be
        // changed, so it comes first and acts on whatever is selected.
        let source = state
            .selected
            .and_then(|(gi, mi)| Some((groups.get(gi)?.name.clone(), groups[gi].macros.get(mi)?)))
            .map(|(group, m)| (group, m.clone()));
        if let Some((group, source)) = source {
            if ui
                .button(tr("Duplicate"))
                .on_hover_text(tr1("A copy of {} in your own macros.", &source.name))
                .clicked()
            {
                // Into a personal group of the same name: an organisation group
                // is never written to, so a copy out of one gets a group of its
                // own beside it rather than landing in the shared list.
                let gi = personal_group(groups, &group);
                let at = add_macro(groups, gi, copy_of(&source));
                state.select(Some(at), groups);
                actions.push(MacroAction::Save);
            }
        }

        ui.horizontal(|ui| {
            ui.label(tr("New in group"));
            ui.add(egui::TextEdit::singleline(&mut state.new_group).desired_width(90.0));
        });
        let named = !state.new_group.trim().is_empty();
        if ui
            .add_enabled(named, egui::Button::new(tr("New macro")))
            .on_hover_text(tr("Groups are how the right-click menu is arranged."))
            .clicked()
        {
            let name = state.new_group.trim().to_string();
            let gi = personal_group(groups, &name);
            let at = add_macro(
                groups,
                gi,
                Macro {
                    origin: Origin::Personal,
                    name: tr("New macro").to_string(),
                    ..Macro::default()
                },
            );
            state.select(Some(at), groups);
            actions.push(MacroAction::Save);
        }

        let deletable = state.stored(groups).is_some_and(|m| m.origin.is_editable());
        ui.add_enabled_ui(deletable, |ui| {
            if ui
                .button(tr("Delete"))
                .on_hover_text(tr(
                    "Only your own macros; the organization's file is never written.",
                ))
                .clicked()
            {
                state.confirm_delete = state.selected;
            }
        });

        if let Some((gi, mi)) = state.confirm_delete {
            let name = groups
                .get(gi)
                .and_then(|g| g.macros.get(mi))
                .map(|m| m.name.clone())
                .unwrap_or_default();
            ui.add_space(4.0);
            ui.label(tr1("Delete {}?", &name));
            ui.horizontal(|ui| {
                if ui.button(tr("Delete")).clicked() {
                    // Guarded by construction - Delete only lights up on a
                    // personal macro - but checked again so a later change here
                    // cannot destroy shared data.
                    if groups[gi].macros[mi].origin.is_editable() {
                        groups[gi].macros.remove(mi);
                        if groups[gi].macros.is_empty() {
                            groups.remove(gi);
                        }
                        // Straight out of the state rather than through
                        // `select`, which would try to commit the draft of the
                        // macro that has just gone.
                        state.selected = None;
                        state.draft_of = None;
                        state.draft = None;
                        actions.push(MacroAction::Save);
                    }
                    state.confirm_delete = None;
                }
                if ui.button(tr("Keep")).clicked() {
                    state.confirm_delete = None;
                }
            });
        }
    });
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

/// The right-hand pane: everything one macro carries.
fn editor(
    ui: &mut Ui,
    state: &mut MacroManagerState,
    groups: &mut [MacroGroup],
    actions: &mut Vec<MacroAction>,
) {
    let Some((gi, mi)) = state.selected else {
        ui.label(tr("Pick a macro on the left, or make a new one."));
        return;
    };
    let Some(origin) = groups
        .get(gi)
        .and_then(|g| g.macros.get(mi))
        .map(|m| m.origin)
    else {
        return;
    };
    let editable = origin.is_editable();

    // Taken out of the state for the duration of the pane, which borrows the
    // rest of it to reach the draft.
    let mut body = std::mem::take(&mut state.body_draft);
    let mut reveal = state.reveal_body;
    let mut capture = state.capture_shortcut;
    let mut save = false;
    let mut revert = false;
    let mut run = None;

    if let Some(draft) = state.draft.as_mut() {
        ui.horizontal(|ui| {
            ui.heading(&draft.name);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(tr("Run"))
                    .on_hover_text(tr(
                        "Sends it to the active session, asking for parameters and confirmation exactly as the right-click menu does.",
                    ))
                    .clicked()
                {
                    let mut m = draft.clone();
                    m.body = crate::features::macros::body_lines(&body);
                    run = Some(m);
                }
            });
        });
        if !editable {
            ui.colored_label(
                WARNING,
                tr("Provided by the organization; read-only here. Duplicate it to make changes."),
            );
        }
        ui.separator();

        egui::ScrollArea::vertical()
            .id_source("macro-editor")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_enabled_ui(editable, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(tr("Name"));
                        ui.text_edit_singleline(&mut draft.name);
                    });
                    ui.horizontal(|ui| {
                        ui.label(tr("Description"));
                        ui.text_edit_singleline(&mut draft.description);
                    });

                    ui.horizontal(|ui| {
                        ui.label(tr("Shortcut"));
                        // The same field the macro manager's hotkey is set in.
                        // See [`shortcut::picker`].
                        shortcut::picker(ui, &mut draft.key, &mut capture);
                    });

                    ui.checkbox(
                        &mut draft.confirm,
                        tr("Confirm before sending (use for anything that writes)"),
                    );
                    if ui
                        .checkbox(&mut draft.hide_command, tr("Hide command"))
                        .on_hover_text(tr(
                            "For a body that carries a password. Keeps it out of the menus; IRIS still echoes what it is sent.",
                        ))
                        .changed()
                    {
                        // Ticking the box hides the body again immediately, so
                        // the secret is not left on screen by the very act of
                        // protecting it.
                        reveal = false;
                    }

                    ui.label(tr("Body - one command per line, {{param}} is substituted"));
                    if draft.hide_command && !reveal {
                        ui.horizontal(|ui| {
                            ui.weak(hidden_lines_note(
                                crate::features::macros::body_lines(&body).len(),
                            ));
                            if ui.button(tr("Reveal")).clicked() {
                                reveal = true;
                            }
                        });
                    } else {
                        // The field owns the text and nothing rewrites it
                        // between keystrokes - see `body_draft`.
                        ui.add(
                            egui::TextEdit::multiline(&mut body)
                                .desired_rows(6)
                                .desired_width(f32::INFINITY)
                                .code_editor(),
                        );
                    }

                    ui.label(tr("Parameters"));
                    ui.small(tr(
                        "Each one becomes a field in the right-click menu, filled in before the macro is sent.",
                    ));
                    let mut drop_param = None;
                    for (pi, param) in draft.params.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.add(egui::TextEdit::singleline(&mut param.name).desired_width(70.0))
                                .on_hover_text(tr("Name used as {{name}} in the body"));
                            ui.add(
                                egui::TextEdit::singleline(&mut param.prompt).desired_width(120.0),
                            )
                            .on_hover_text(tr("Prompt shown when running"));
                            ui.add(
                                egui::TextEdit::singleline(&mut param.default).desired_width(90.0),
                            )
                            .on_hover_text(tr("Default value"));
                            if ui.small_button("x").clicked() {
                                drop_param = Some(pi);
                            }
                        });
                    }
                    if let Some(pi) = drop_param {
                        draft.params.remove(pi);
                    }
                    if ui.small_button(tr("Add parameter")).clicked() {
                        draft.params.push(Param::default());
                    }
                });

                // What it would send, with the defaults filled in. Not
                // decoration: it is where a substituted value pointing at the
                // wrong global gets noticed, before the text reaches a shared
                // database.
                ui.add_space(6.0);
                ui.separator();
                ui.label(tr("Will send:"));
                if draft.hide_command && !reveal {
                    ui.weak(hidden_lines_note(
                        crate::features::macros::body_lines(&body).len(),
                    ));
                } else {
                    let mut preview = draft.clone();
                    preview.body = crate::features::macros::body_lines(&body);
                    for line in preview.expand(&preview.default_values()) {
                        ui.code(line);
                    }
                }

                if editable {
                    ui.add_space(6.0);
                    ui.separator();
                    ui.horizontal(|ui| {
                        if ui
                            .button(tr("Save"))
                            .on_hover_text(tr("Writes your personal macro file."))
                            .clicked()
                        {
                            save = true;
                        }
                        if ui
                            .button(tr("Revert"))
                            .on_hover_text(tr("Back to what is in the file."))
                            .clicked()
                        {
                            revert = true;
                        }
                        ui.weak(tr("Picking another macro saves this one too."));
                    });
                }
            });
    }

    state.body_draft = body;
    state.reveal_body = reveal;
    state.capture_shortcut = capture;

    if let Some(m) = run {
        actions.push(MacroAction::Run(m));
    }
    if save && state.commit(groups) {
        actions.push(MacroAction::Save);
    }
    if revert {
        // Loading the draft again from the file is exactly what selecting the
        // macro afresh does.
        state.draft_of = None;
        state.select(Some((gi, mi)), groups);
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

    /// Selecting one macro and then another writes the first one's edits back,
    /// which is what makes moving down the list safe without a modal.
    #[test]
    fn moving_off_an_edited_macro_keeps_the_edit() {
        let mut groups = groups();
        let mut state = MacroManagerState::default();
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
        let mut state = MacroManagerState::default();
        state.select(Some((1, 0)), &mut groups);
        assert!(!state.select(Some((0, 0)), &mut groups));
    }

    /// The shared file is never written, whatever the draft says - the guard
    /// that matters most here, since these macros come from a share the whole
    /// team reads.
    #[test]
    fn an_organization_macro_is_never_committed() {
        let mut groups = groups();
        let mut state = MacroManagerState::default();
        state.select(Some((0, 0)), &mut groups);
        state.draft.as_mut().unwrap().name = "Tampered".into();

        assert!(!state.commit(&mut groups));
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
        let mut state = MacroManagerState::default();
        state.select(Some((1, 0)), &mut groups);
        state.body_draft = "  W 1\n\n W 2  ".into();

        assert!(state.commit(&mut groups));
        assert_eq!(groups[1].macros[0].body, vec!["W 1", "W 2"]);
    }
}
