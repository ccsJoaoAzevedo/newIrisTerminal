//! Side panels and dialogs: macros, natives, settings, and export.
//!
//! These are pure-ish view functions — they render, and report back what the
//! user asked for as an [`UiRequest`], which `app.rs` then carries out. Keeping
//! the "decide" and "do" halves apart is what lets a destructive macro be
//! routed through a confirmation step without the panel knowing anything about
//! sessions.

use egui::{Context, Ui};

use crate::config::{profile::LogMode, CursorStyle, Profile, Settings, Theme};
use crate::features::macros::{Macro, MacroGroup, Origin, Param};
use crate::features::natives::Native;
use crate::term::Encoding;
use crate::ui::shortcut;

/// Colour for "this is set, but it will not do what you expect". Not from the
/// theme: it has to stay legible as a warning in every one of them.
const WARNING: egui::Color32 = egui::Color32::from_rgb(220, 120, 60);

/// Shown in the font picker for "whatever egui ships with".
const BUILT_IN_FONT: &str = "Built-in monospace";

/// Something the user asked for. `app.rs` decides whether and how to honour it.
#[derive(Clone, Debug)]
pub enum UiRequest {
    /// Send a macro, after confirmation if it asks for one.
    RunMacro(Macro),
    /// Send a native-utility invocation, with one value per parameter.
    RunNative(Native, Vec<String>),
    /// Send literal lines to the active session.
    SendLines(Vec<String>),
    ExportText(crate::features::export::Range),
    ExportHtml(crate::features::export::Range),
    CopyRange(crate::features::export::Range),
    SettingsChanged,
    ReloadMacros,
    /// Write the personal macro file back to disk.
    SavePersonalMacros,
    /// Show a folder in the platform's file manager.
    OpenFolder(std::path::PathBuf),
}

/// State the panels own between frames.
#[derive(Default)]
pub struct PanelState {
    pub show_macros: bool,
    pub show_settings: bool,
    pub show_export: bool,
    pub macro_filter: String,
    /// A macro waiting on parameter values and/or confirmation.
    pub pending: Option<PendingMacro>,
    /// One value per parameter of the selected native helper.
    pub native_values: Vec<String>,
    pub selected_native: Option<Native>,
    /// Which helper `native_values` belongs to, so folding one away and opening
    /// it again keeps what was typed while switching to a different one starts
    /// from that one's defaults.
    native_values_of: Option<Native>,
    /// Macro shown in the details block, by (group, index).
    pub selected: Option<(usize, usize)>,
    /// The personal macro currently open in the editor, by (group, index).
    pub editing: Option<(usize, usize)>,
    /// Draft being edited, kept separate so Cancel is a real cancel.
    pub draft: Option<Macro>,
    /// Whether a hidden body is currently shown in the editor. Deliberately not
    /// persisted and cleared every time the editor closes, so opening a macro
    /// never starts by putting its password on screen.
    reveal_body: bool,
    /// Group name for a macro about to be created.
    pub new_group: String,
    /// Installed monospace families, listed once. Enumerating system fonts is
    /// slow enough that doing it per frame would be felt while the Settings
    /// window is open.
    font_families: Option<Vec<String>>,
}

impl PanelState {
    /// Shows one IRIS helper's fields, or `None` to fold the open one away.
    ///
    /// The fields are only reset when a *different* helper is opened; the
    /// compile flag starts at its default rather than blank, and re-opening the
    /// helper you just closed gives you back what you had typed.
    fn open_native(&mut self, native: Option<Native>) {
        self.selected_native = native;
        if let Some(native) = native {
            if self.native_values_of != Some(native) {
                self.native_values = native.default_values();
                self.native_values_of = Some(native);
            }
        }
    }

    /// Opens the editor on one macro, on a copy of it.
    fn open_editor(&mut self, at: (usize, usize), draft: Macro) {
        self.editing = Some(at);
        self.draft = Some(draft);
        self.reveal_body = false;
    }

    /// Forgets which macro is selected or being edited.
    ///
    /// Called when the macro list is replaced: both are indices into it, and
    /// after a reload they would address a different macro.
    pub fn forget_macro_selection(&mut self) {
        self.selected = None;
        self.editing = None;
        self.draft = None;
        self.reveal_body = false;
    }
}

#[derive(Clone, Debug)]
pub struct PendingMacro {
    pub source: Macro,
    pub values: Vec<(String, String)>,
    /// Set once parameters are filled and only the yes/no remains.
    pub confirming: bool,
}

impl PendingMacro {
    pub fn new(source: Macro) -> Self {
        let values = source.default_values();
        let confirming = values.is_empty() && source.confirm;
        PendingMacro {
            source,
            values,
            confirming,
        }
    }

    pub fn preview(&self) -> Vec<String> {
        self.source.expand(&self.values)
    }
}

/// The macro browser: pick a macro, see what it will do, then run it.
///
/// Selection rather than run-on-click. A list where clicking a name fires the
/// command underneath it has no room to show what that command is, and half
/// these macros write to a shared database — so the click selects, the details
/// below say what would happen, and Run is a separate, deliberate press.
/// Clicking the open macro again folds those details away.
///
/// Takes the groups mutably because personal macros are edited in place here;
/// organisation macros are shown with a badge and no edit affordance, since
/// their file is shared and never written from the app.
pub fn macros_panel(
    ui: &mut Ui,
    groups: &mut Vec<MacroGroup>,
    state: &mut PanelState,
) -> Option<UiRequest> {
    let mut request = None;

    ui.horizontal(|ui| {
        ui.label("Filter");
        ui.text_edit_singleline(&mut state.macro_filter);
        if ui.button("Reload").clicked() {
            request = Some(UiRequest::ReloadMacros);
        }
    });
    ui.separator();

    if groups.is_empty() {
        ui.label("No macros defined.");
        ui.small("Add them below, or configure the organization file in Settings.");
    }

    // An index into a list that has since been reloaded, or had a macro
    // deleted, would point at the wrong macro; a stale selection is dropped
    // rather than followed.
    if let Some((gi, mi)) = state.selected {
        if groups.get(gi).and_then(|g| g.macros.get(mi)).is_none() {
            state.selected = None;
        }
    }

    let filter = state.macro_filter.to_lowercase();
    // `Some(None)` is "fold the details away", which is what clicking the open
    // macro again means; `None` is "nothing was clicked".
    let mut to_select: Option<Option<(usize, usize)>> = None;
    let mut to_run: Option<Macro> = None;

    // No scroll area of its own: the whole panel scrolls, so a long macro list
    // is not squeezed into half the height with the details below it fighting
    // for the rest.
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

        let title = if group.name.is_empty() {
            "Macros"
        } else {
            &group.name
        };
        egui::CollapsingHeader::new(title)
            .id_source(("macro-group", gi))
            .default_open(true)
            .show(ui, |ui| {
                for (mi, m) in matching {
                    ui.horizontal(|ui| {
                        // Run sits on the row with the name, so firing
                        // a macro is one press from the list. It still
                        // goes through the same request path, so a
                        // macro that asks for parameters or a
                        // confirmation asks for them here too.
                        if ui
                            .small_button("Run")
                            .on_hover_text(if m.confirm {
                                "Runs after confirming."
                            } else {
                                "Sends this macro to the active session."
                            })
                            .clicked()
                        {
                            to_run = Some(m.clone());
                        }
                        let label = if m.confirm {
                            format!("{}  (confirms)", m.name)
                        } else {
                            m.name.clone()
                        };
                        let selected = state.selected == Some((gi, mi));
                        let button = ui.selectable_label(selected, label);
                        let button = if m.description.is_empty() {
                            button
                        } else {
                            button.on_hover_text(&m.description)
                        };
                        if button.clicked() {
                            // Clicking the open one again folds its
                            // details away, the same gesture the IRIS
                            // utilities below use.
                            to_select = Some((!selected).then_some((gi, mi)));
                        }

                        // Provenance at a glance: a shared macro
                        // behaving oddly is someone else's file, not
                        // something the user can have broken locally.
                        if m.origin == Origin::Organization {
                            ui.weak("org")
                                .on_hover_text("Provided by the organization; read-only here.");
                        }

                        // The shortcut belongs next to the name. A
                        // binding nobody can see is a binding nobody
                        // uses.
                        if let Some(key) = &m.key {
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.weak(key);
                                },
                            );
                        }
                    });
                }
            });
    }

    if let Some(at) = to_select {
        state.selected = at;
    }
    if let Some(m) = to_run {
        request = Some(UiRequest::RunMacro(m));
    }

    ui.separator();
    ui.horizontal(|ui| {
        ui.label("New in group");
        ui.add(egui::TextEdit::singleline(&mut state.new_group).desired_width(90.0));
        let named = !state.new_group.trim().is_empty();
        if ui.add_enabled(named, egui::Button::new("Add")).clicked() {
            let name = state.new_group.trim().to_string();
            let gi = match groups.iter().position(|g| g.name == name) {
                Some(i) => i,
                None => {
                    groups.push(MacroGroup {
                        name,
                        origin: Origin::Personal,
                        macros: Vec::new(),
                    });
                    groups.len() - 1
                }
            };
            groups[gi].macros.push(Macro {
                origin: Origin::Personal,
                name: "New macro".into(),
                ..Macro::default()
            });
            let mi = groups[gi].macros.len() - 1;
            state.selected = Some((gi, mi));
            state.open_editor((gi, mi), groups[gi].macros[mi].clone());
        }
    });

    if let Some(r) = macro_details(ui, groups, state) {
        request = Some(r);
    }
    request
}

/// What the selected macro is, what it would send, and the actions on it.
fn macro_details(
    ui: &mut Ui,
    groups: &mut Vec<MacroGroup>,
    state: &mut PanelState,
) -> Option<UiRequest> {
    let (gi, mi) = state.selected?;
    let m = groups.get(gi).and_then(|g| g.macros.get(mi))?.clone();

    let mut request = None;
    let mut delete = false;

    ui.separator();
    ui.heading(&m.name);
    if !m.description.is_empty() {
        ui.label(&m.description);
    }

    // No Run here: it lives next to the name in the list above, where it is
    // reachable without selecting the macro first.
    ui.horizontal(|ui| match m.origin {
        Origin::Personal => {
            if ui.button("Edit").clicked() {
                state.open_editor((gi, mi), m.clone());
            }
            if ui
                .button("Delete")
                .on_hover_text("Removes it from your personal macro file.")
                .clicked()
            {
                delete = true;
            }
        }
        Origin::Organization => {
            ui.weak("Provided by the organization; read-only here.");
        }
    });

    if let Some(key) = &m.key {
        ui.horizontal(|ui| {
            ui.label("Shortcut");
            ui.weak(key);
            if shortcut::parse(key).is_none() {
                ui.colored_label(WARNING, "not a shortcut this app understands");
            }
        });
    }

    if !m.params.is_empty() {
        ui.label("Parameters");
        for p in &m.params {
            let prompt = if p.prompt.is_empty() {
                &p.name
            } else {
                &p.prompt
            };
            ui.small(format!("{}  -  {prompt}", p.name));
        }
    }

    if m.hide_command {
        // The whole point of the flag: this body holds a credential, so the
        // panel does not put it on screen just because the macro is selected.
        ui.weak(hidden_body_note(&m));
    } else {
        for line in &m.body {
            ui.code(line);
        }
    }

    if delete {
        // Guarded by construction - Delete only appears on personal macros -
        // but checked anyway so a future refactor cannot destroy shared data.
        if groups[gi].macros[mi].origin.is_editable() {
            groups[gi].macros.remove(mi);
            if groups[gi].macros.is_empty() {
                groups.remove(gi);
            }
            state.selected = None;
            state.editing = None;
            state.draft = None;
            request = Some(UiRequest::SavePersonalMacros);
        }
    }

    request
}

/// How many body lines a hidden macro has, without saying what they are.
fn hidden_body_note(m: &Macro) -> String {
    match m.body.len() {
        1 => "1 command hidden".to_string(),
        n => format!("{n} commands hidden"),
    }
}

/// Editor for one personal macro, in a window of its own.
///
/// A window rather than another section stacked under the list: the sidebar is
/// for choosing and reading, and an editor wedged into the bottom of it left
/// neither enough room.
pub fn macro_editor_dialog(
    ctx: &Context,
    groups: &mut [MacroGroup],
    state: &mut PanelState,
) -> Option<UiRequest> {
    let (gi, mi) = state.editing?;
    if groups.get(gi).and_then(|g| g.macros.get(mi)).is_none() {
        state.editing = None;
        state.draft = None;
        return None;
    }

    let mut request = None;
    let mut close = false;
    let mut open = true;
    let mut reveal = state.reveal_body;

    egui::Window::new("Edit macro")
        .collapsible(false)
        .resizable(true)
        .default_width(480.0)
        .open(&mut open)
        .show(ctx, |ui| {
            let Some(draft) = state.draft.as_mut() else {
                close = true;
                return;
            };

            ui.horizontal(|ui| {
                ui.label("Name");
                ui.text_edit_singleline(&mut draft.name);
            });
            ui.horizontal(|ui| {
                ui.label("Description");
                ui.text_edit_singleline(&mut draft.description);
            });

            ui.horizontal(|ui| {
                ui.label("Shortcut");
                let mut key = draft.key.clone().unwrap_or_default();
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut key)
                            .hint_text("Ctrl+Shift+G")
                            .desired_width(140.0),
                    )
                    .changed()
                {
                    let key = key.trim().to_string();
                    draft.key = (!key.is_empty()).then_some(key);
                }
            });
            // Reported rather than rejected: this field is also edited by hand
            // in the shared XML, and a value we do not understand has to
            // survive a round trip through here instead of being erased.
            if let Some(key) = draft.key.as_deref() {
                match shortcut::parse(key) {
                    None => {
                        ui.colored_label(
                            WARNING,
                            "Not understood, so it will not fire. Needs a modifier, like Ctrl+Shift+G.",
                        );
                    }
                    Some((modifiers, parsed)) => {
                        if let Some(used_for) = shortcut::is_reserved(modifiers, parsed) {
                            ui.colored_label(
                                WARNING,
                                format!("The app already uses this for {used_for}; add Shift."),
                            );
                        }
                    }
                }
            }

            ui.checkbox(
                &mut draft.confirm,
                "Confirm before sending (use for anything that writes)",
            );
            if ui
                .checkbox(&mut draft.hide_command, "Hide command")
                .on_hover_text(
                    "For a body that carries a password. Keeps it out of the macro panel; IRIS still echoes what it is sent.",
                )
                .changed()
            {
                // Ticking the box hides the body again immediately, so the
                // secret is not left on screen by the act of protecting it.
                reveal = false;
            }

            ui.label("Body - one command per line, {{param}} is substituted");
            if draft.hide_command && !reveal {
                ui.horizontal(|ui| {
                    ui.weak(hidden_body_note(draft));
                    if ui.button("Reveal").clicked() {
                        reveal = true;
                    }
                });
            } else {
                let mut body = draft.body.join("\n");
                if ui
                    .add(egui::TextEdit::multiline(&mut body).desired_rows(4))
                    .changed()
                {
                    draft.body = body
                        .lines()
                        .map(str::trim)
                        .filter(|l| !l.is_empty())
                        .map(str::to_string)
                        .collect();
                }
            }

            ui.label("Parameters");
            let mut drop_param = None;
            for (pi, param) in draft.params.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut param.name).desired_width(60.0))
                        .on_hover_text("Name used as {{name}} in the body");
                    ui.add(egui::TextEdit::singleline(&mut param.prompt).desired_width(100.0))
                        .on_hover_text("Prompt shown when running");
                    ui.add(egui::TextEdit::singleline(&mut param.default).desired_width(70.0))
                        .on_hover_text("Default value");
                    if ui.small_button("x").clicked() {
                        drop_param = Some(pi);
                    }
                });
            }
            if let Some(pi) = drop_param {
                draft.params.remove(pi);
            }
            if ui.small_button("Add parameter").clicked() {
                draft.params.push(Param::default());
            }

            ui.separator();
            ui.horizontal(|ui| {
                if ui.button("Save").clicked() {
                    // Origin is never taken from the draft: an edited macro
                    // stays personal, so nothing can promote itself into the
                    // shared file.
                    let mut saved = draft.clone();
                    saved.origin = Origin::Personal;
                    groups[gi].macros[mi] = saved;
                    close = true;
                    request = Some(UiRequest::SavePersonalMacros);
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });

    state.reveal_body = reveal;

    if close || !open {
        state.editing = None;
        state.draft = None;
        state.reveal_body = false;
    }
    request
}

/// Parameter-fill and confirmation dialog for a pending macro.
///
/// The preview is not decoration: it is the last chance to notice that a
/// substituted value points at the wrong global before the text reaches a
/// shared database.
pub fn pending_macro_dialog(ctx: &Context, state: &mut PanelState) -> Option<UiRequest> {
    let pending = state.pending.as_mut()?;

    let mut request = None;
    let mut close = false;
    let mut open = true;

    let title = if pending.confirming {
        format!("Confirm: {}", pending.source.name)
    } else {
        pending.source.name.clone()
    };

    egui::Window::new(title)
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            if !pending.source.description.is_empty() {
                ui.label(&pending.source.description);
                ui.separator();
            }

            for (index, param) in pending.source.params.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(&param.prompt);
                    if let Some((_, value)) = pending.values.get_mut(index) {
                        ui.text_edit_singleline(value);
                    }
                });
            }

            ui.separator();
            ui.label("Will send:");
            let preview = pending.source.expand(&pending.values);
            for line in &preview {
                ui.code(line);
            }

            if pending.source.confirm {
                ui.separator();
                ui.colored_label(
                    WARNING,
                    "This macro is marked as modifying data. \
                     RDB* databases are shared with the team.",
                );
            }

            ui.separator();
            ui.horizontal(|ui| {
                let send_label = if pending.source.confirm {
                    "Yes, send it"
                } else {
                    "Send"
                };
                if ui.button(send_label).clicked() {
                    request = Some(UiRequest::SendLines(preview.clone()));
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });

    if close || !open {
        state.pending = None;
    }
    request
}

/// The native-utility helpers.
pub fn natives_panel(ui: &mut Ui, state: &mut PanelState) -> Option<UiRequest> {
    let mut request = None;

    ui.label("IRIS utilities");
    ui.separator();

    for native in Native::ALL {
        let selected = state.selected_native == Some(native);
        if ui.selectable_label(selected, native.label()).clicked() {
            // Clicking the open one again folds it away rather than resetting
            // its fields, which is what the same click used to do - and losing
            // a typed package name to a stray click is a poor trade for a
            // gesture that reads like "close this".
            state.open_native(if selected { None } else { Some(native) });
        }
    }

    if let Some(native) = state.selected_native {
        ui.separator();
        // A selection made before the last reload could be holding fewer
        // values than the helper now asks for.
        state
            .native_values
            .resize(native.params().len(), String::new());
        for (param, value) in native.params().iter().zip(state.native_values.iter_mut()) {
            ui.label(param.label);
            ui.text_edit_singleline(value);
        }

        let invocation = native.build(&state.native_values);
        ui.add_space(8.0);
        ui.label("Will send:");
        for line in &invocation.lines {
            ui.code(line);
        }
        ui.add_space(8.0);
        if ui.button("Run").clicked() {
            request = Some(UiRequest::RunNative(native, state.native_values.clone()));
        }
    }

    request
}

/// Export / copy actions.
pub fn export_dialog(ctx: &Context, state: &mut PanelState) -> Option<UiRequest> {
    use crate::features::export::Range;

    if !state.show_export {
        return None;
    }
    let mut request = None;
    let mut open = true;

    egui::Window::new("Export output")
        .collapsible(false)
        .resizable(false)
        .open(&mut open)
        .show(ctx, |ui| {
            ui.label("Save to a file");
            ui.horizontal(|ui| {
                if ui.button("Screen as text").clicked() {
                    request = Some(UiRequest::ExportText(Range::Screen));
                }
                if ui.button("Everything as text").clicked() {
                    request = Some(UiRequest::ExportText(Range::All));
                }
            });
            ui.horizontal(|ui| {
                if ui.button("Screen as HTML").clicked() {
                    request = Some(UiRequest::ExportHtml(Range::Screen));
                }
                if ui.button("Everything as HTML").clicked() {
                    request = Some(UiRequest::ExportHtml(Range::All));
                }
            });

            ui.separator();
            ui.label("Copy to the clipboard");
            ui.horizontal(|ui| {
                if ui.button("Screen").clicked() {
                    request = Some(UiRequest::CopyRange(Range::Screen));
                }
                if ui.button("Everything").clicked() {
                    request = Some(UiRequest::CopyRange(Range::All));
                }
            });
        });

    if !open {
        state.show_export = false;
    }
    if request.is_some() {
        state.show_export = false;
    }
    request
}

/// Settings: theme, font, scrollback, logging, and profiles.
pub fn settings_dialog(
    ctx: &Context,
    settings: &mut Settings,
    themes: &[Theme],
    state: &mut PanelState,
    instances: &[String],
) -> Option<UiRequest> {
    if !state.show_settings {
        return None;
    }
    let mut changed = false;
    let mut open = true;
    // Kept apart from `changed`: opening a folder is an action, not an edit,
    // and it must not make the app rewrite settings.toml.
    let mut action: Option<UiRequest> = None;

    egui::Window::new("Settings")
        .collapsible(false)
        .default_width(520.0)
        .open(&mut open)
        .show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                ui.heading("Appearance");
                ui.horizontal(|ui| {
                    ui.label("Theme");
                    egui::ComboBox::from_id_source("theme-picker")
                        .selected_text(settings.theme.clone())
                        .show_ui(ui, |ui| {
                            for theme in themes {
                                if ui
                                    .selectable_label(theme.name == settings.theme, &theme.name)
                                    .clicked()
                                {
                                    settings.theme = theme.name.clone();
                                    changed = true;
                                }
                            }
                        });
                    // A theme is a TOML file edited by hand, so the useful
                    // button next to the picker is the one that shows where
                    // they live.
                    if ui
                        .button("Open folder")
                        .on_hover_text(crate::config::themes_dir().display().to_string())
                        .clicked()
                    {
                        action = Some(UiRequest::OpenFolder(crate::config::themes_dir()));
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Font");
                    let families = state
                        .font_families
                        .get_or_insert_with(crate::ui::fonts::monospace_families);
                    let selected = if settings.font_family.is_empty() {
                        BUILT_IN_FONT
                    } else {
                        settings.font_family.as_str()
                    };
                    egui::ComboBox::from_id_source("font-picker")
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            if ui
                                .selectable_label(settings.font_family.is_empty(), BUILT_IN_FONT)
                                .clicked()
                            {
                                settings.font_family.clear();
                                changed = true;
                            }
                            for family in families.iter() {
                                if ui
                                    .selectable_label(&settings.font_family == family, family)
                                    .clicked()
                                {
                                    settings.font_family = family.clone();
                                    changed = true;
                                }
                            }
                        });
                });
                ui.small("Monospace families only: the terminal is a character grid, so a proportional font would not line up.");

                ui.horizontal(|ui| {
                    ui.label("Font size");
                    if ui
                        .add(egui::Slider::new(&mut settings.font_size, 8.0..=28.0))
                        .changed()
                    {
                        changed = true;
                    }
                });

                ui.horizontal(|ui| {
                    ui.label("Cursor");
                    for style in CursorStyle::ALL {
                        if ui
                            .selectable_label(settings.cursor_style == style, style.label())
                            .clicked()
                        {
                            settings.cursor_style = style;
                            changed = true;
                        }
                    }
                    if ui.checkbox(&mut settings.cursor_blink, "Blink").changed() {
                        changed = true;
                    }
                });

                if ui
                    .checkbox(
                        &mut settings.terminal_syntax_highlight,
                        "Syntax highlighting",
                    )
                    .on_hover_text(
                        "Colours globals, strings, numbers, commands, macros and class references. A guess about the text on screen; a colour IRIS sets itself always wins.",
                    )
                    .changed()
                {
                    changed = true;
                }
                if ui
                    .checkbox(&mut settings.show_scrollbars, "Show scrollbars")
                    .on_hover_text(
                        "Solid scrollbars instead of the thin ones that only appear on hover.",
                    )
                    .changed()
                {
                    changed = true;
                }
                ui.small("Theme files live in the themes folder; drop one in and restart.");
                ui.small(
                    "Each theme also carries the ObjectScript colours, named after the scopes the InterSystems VS Code extension uses - copy them straight across from an editor colour customisation.",
                );
                ui.small(
                    "A built-in theme is rewritten on every start; clear its `builtin` flag to keep your own edits.",
                );

                ui.separator();
                ui.heading("Session");
                ui.horizontal(|ui| {
                    ui.label("Scrollback lines");
                    if ui
                        .add(
                            egui::DragValue::new(&mut settings.scrollback_limit).range(0..=200_000),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                });
                if ui
                    .checkbox(&mut settings.wrap_lines, "Wrap long lines")
                    .on_hover_text(
                        "On: a long line continues on the next row, breaking at the window edge. Off: it runs off to the right, reached by scrolling sideways or widening the window.",
                    )
                    .changed()
                {
                    changed = true;
                }
                ui.small(
                    "Either way the whole line is kept: the terminal is reported wider than the window, because IRIS cuts a line at the terminal width instead of wrapping it.",
                );

                if ui
                    .checkbox(&mut settings.copy_on_select, "Copy on select")
                    .on_hover_text(
                        "Put a selection on the clipboard as soon as the mouse is released, without waiting for Ctrl+C.",
                    )
                    .changed()
                {
                    changed = true;
                }
                if ui
                    .checkbox(
                        &mut settings.save_command_history,
                        "Remember commands from earlier sessions",
                    )
                    .on_hover_text(
                        "Keeps the commands typed at an IRIS prompt in history.txt, so Up reaches back past the session that is open now. Off keeps recall working within the session and writes nothing to disk.",
                    )
                    .changed()
                {
                    changed = true;
                }

                if ui
                    .checkbox(
                        &mut settings.open_on_start,
                        "Open the default profile at startup",
                    )
                    .changed()
                {
                    changed = true;
                }
                if ui
                    .checkbox(
                        &mut settings.confirm_close_with_live_session,
                        "Ask before closing with a session still connected",
                    )
                    .changed()
                {
                    changed = true;
                }

                ui.separator();
                ui.heading("Window");
                if ui
                    .checkbox(
                        &mut settings.native_decorations,
                        "Use the system title bar",
                    )
                    .on_hover_text(
                        "Off by default: the app draws its own, which frees the row the system bar would take.",
                    )
                    .changed()
                {
                    changed = true;
                }
                ui.small("Takes effect the next time the app starts.");

                ui.separator();
                ui.heading("Macros");
                ui.label("Organization macro file (shared, read-only)");
                let mut org = settings.org_macros_path.display().to_string();
                if ui.text_edit_singleline(&mut org).changed() {
                    settings.org_macros_path = std::path::PathBuf::from(org.trim());
                    changed = true;
                }
                ui.small(
                    "A UNC share, mapped drive, or local copy. Leave empty for none.                      Your personal macros are edited in the Macros panel.",
                );
                if let Some(path) = settings.org_macros() {
                    if path.exists() {
                        ui.small("Found.");
                    } else {
                        ui.colored_label(
                            WARNING,
                            "Not reachable right now - personal macros will still load.",
                        );
                    }
                }

                ui.separator();
                ui.heading("Logging");
                ui.horizontal(|ui| {
                    ui.label("Default mode");
                    for (mode, label) in [
                        (LogMode::Off, "Off"),
                        (LogMode::Clean, "Clean text"),
                        (LogMode::Raw, "Raw bytes"),
                    ] {
                        if ui
                            .selectable_label(settings.default_log_mode == mode, label)
                            .clicked()
                        {
                            settings.default_log_mode = mode;
                            changed = true;
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Keep logs for (days, 0 = forever)");
                    if ui
                        .add(egui::DragValue::new(&mut settings.log_retention_days).range(0..=3650))
                        .changed()
                    {
                        changed = true;
                    }
                });
                ui.small(format!("Logs: {}", settings.log_dir.display()));

                ui.separator();
                ui.heading("Profiles");
                if profiles_editor(ui, settings, instances) {
                    changed = true;
                }
            });
        });

    if !open {
        state.show_settings = false;
    }
    action.or_else(|| changed.then_some(UiRequest::SettingsChanged))
}

/// Returns true when anything changed.
fn profiles_editor(ui: &mut Ui, settings: &mut Settings, instances: &[String]) -> bool {
    let mut changed = false;
    let mut remove = None;

    for (index, profile) in settings.profiles.iter_mut().enumerate() {
        egui::CollapsingHeader::new(if profile.name.is_empty() {
            format!("Profile {}", index + 1)
        } else {
            profile.name.clone()
        })
        .id_source(("profile", index))
        .show(ui, |ui| {
            changed |= labelled_edit(ui, "Name", &mut profile.name);

            ui.horizontal(|ui| {
                ui.label("Instance");
                if instances.is_empty() {
                    changed |= ui.text_edit_singleline(&mut profile.instance).changed();
                } else {
                    egui::ComboBox::from_id_source(("instance", index))
                        .selected_text(profile.instance.clone())
                        .show_ui(ui, |ui| {
                            for name in instances {
                                if ui
                                    .selectable_label(*name == profile.instance, name)
                                    .clicked()
                                {
                                    profile.instance = name.clone();
                                    changed = true;
                                }
                            }
                        });
                }
            });

            changed |= labelled_edit(ui, "Namespace", &mut profile.namespace);

            ui.horizontal(|ui| {
                ui.label("Encoding");
                egui::ComboBox::from_id_source(("encoding", index))
                    .selected_text(profile.encoding.label())
                    .show_ui(ui, |ui| {
                        for enc in Encoding::ALL {
                            if ui
                                .selectable_label(profile.encoding == enc, enc.label())
                                .clicked()
                            {
                                profile.encoding = enc;
                                changed = true;
                            }
                        }
                    });
            });
            ui.small("Leave as UTF-8 unless accented characters come out wrong.");

            ui.separator();
            changed |= ui
                .checkbox(&mut profile.autologon, "Log in automatically")
                .changed();
            if profile.autologon {
                changed |= labelled_edit(ui, "Username", &mut profile.username);
                password_editor(ui, profile, index);
                if profile.username.is_empty() {
                    ui.colored_label(WARNING, "Autologon needs a username.");
                }
            }

            ui.separator();
            ui.horizontal(|ui| {
                ui.label("Logging");
                for (mode, label) in [
                    (LogMode::Off, "Use default"),
                    (LogMode::Clean, "Clean"),
                    (LogMode::Raw, "Raw"),
                ] {
                    if ui
                        .selectable_label(profile.logging == mode, label)
                        .clicked()
                    {
                        profile.logging = mode;
                        changed = true;
                    }
                }
            });

            if ui.button("Delete this profile").clicked() {
                remove = Some(index);
            }
        });
    }

    if let Some(index) = remove {
        // Forget the stored secret too, or it outlives the profile that
        // explained what it was for.
        settings.profiles[index].clear_password();
        settings.profiles.remove(index);
        changed = true;
    }

    ui.horizontal(|ui| {
        if ui.button("Add profile").clicked() {
            settings.profiles.push(Profile {
                name: format!("Profile {}", settings.profiles.len() + 1),
                instance: instances.first().cloned().unwrap_or_default(),
                ..Profile::default()
            });
            changed = true;
        }
        if !settings.profiles.is_empty() {
            ui.label("Default");
            egui::ComboBox::from_id_source("default-profile")
                .selected_text(settings.default_profile.clone())
                .show_ui(ui, |ui| {
                    for profile in &settings.profiles {
                        if ui
                            .selectable_label(
                                profile.name == settings.default_profile,
                                &profile.name,
                            )
                            .clicked()
                        {
                            settings.default_profile = profile.name.clone();
                            changed = true;
                        }
                    }
                });
        }
    });

    changed
}

/// Password field. The value is never held in the settings struct — it goes
/// straight to the OS credential store — so this widget keeps its own buffer
/// in egui's temporary memory and clears it once saved.
fn password_editor(ui: &mut Ui, profile: &Profile, index: usize) {
    let id = egui::Id::new(("password-buffer", index));
    let mut buffer: String = ui.memory_mut(|m| m.data.get_temp(id).unwrap_or_default());

    let stored = profile.password().is_some();
    ui.horizontal(|ui| {
        ui.label("Password");
        ui.add(egui::TextEdit::singleline(&mut buffer).password(true));

        if ui.button("Save").clicked() {
            match profile.set_password(&buffer) {
                Ok(()) => buffer.clear(),
                Err(e) => log::error!("could not store the password: {e:#}"),
            }
        }
        if stored && ui.button("Forget").clicked() {
            profile.clear_password();
        }
    });
    ui.small(if stored {
        "A password is stored in the OS credential manager."
    } else {
        "No password stored; autologon will stop at the password prompt."
    });

    ui.memory_mut(|m| m.data.insert_temp(id, buffer));
}

fn labelled_edit(ui: &mut Ui, label: &str, value: &mut String) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.label(label);
        changed = ui.text_edit_singleline(value).changed();
    });
    changed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::features::macros::Param;

    fn macro_with_params() -> Macro {
        Macro {
            name: "Show global".into(),
            params: vec![Param {
                name: "g".into(),
                prompt: "Global".into(),
                default: "CSW1".into(),
            }],
            body: vec!["ZWRITE ^{{g}}".into()],
            ..Macro::default()
        }
    }

    #[test]
    fn a_pending_macro_starts_from_its_defaults() {
        let pending = PendingMacro::new(macro_with_params());
        assert_eq!(pending.values, vec![("g".to_string(), "CSW1".to_string())]);
        assert_eq!(pending.preview(), vec!["ZWRITE ^CSW1".to_string()]);
    }

    #[test]
    fn editing_a_value_changes_the_preview() {
        let mut pending = PendingMacro::new(macro_with_params());
        pending.values[0].1 = "OTHER".into();
        assert_eq!(pending.preview(), vec!["ZWRITE ^OTHER".to_string()]);
    }

    /// A parameterless destructive macro must still stop for a yes/no.
    #[test]
    fn a_confirming_macro_without_params_goes_straight_to_confirmation() {
        let pending = PendingMacro::new(Macro {
            name: "Kill".into(),
            confirm: true,
            body: vec!["KILL ^DATA".into()],
            ..Macro::default()
        });
        assert!(pending.confirming);
    }

    #[test]
    fn a_harmless_macro_does_not_ask_for_confirmation() {
        let pending = PendingMacro::new(Macro {
            body: vec!["Write 1".into()],
            ..Macro::default()
        });
        assert!(!pending.confirming);
        assert!(!pending.source.confirm);
    }
}
